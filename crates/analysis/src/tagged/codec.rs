//! Small, strict section parser. Raw multiline values use a chosen heredoc terminator.

use std::collections::BTreeMap;

use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::common::{
    AnnotationType, Basis, Blocking, FragmentKind, Origin, RiskBand, ThreadLlmStatus,
};
use serde_json::{Map, Value, json};

use super::{Anchor, Catalog, MAX_BYTES, Problem};

#[derive(Debug)]
struct Section {
    depth: usize,
    words: Vec<String>,
    fields: BTreeMap<String, (usize, String)>,
    line: usize,
}

impl Section {
    fn field(&mut self, key: &str) -> Option<String> {
        self.fields.remove(key).map(|(_, value)| value)
    }
    fn required(&mut self, key: &str) -> Result<String, Problem> {
        self.field(key)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                Problem::new(
                    self.line,
                    format!("{} has no `{key}:` line", self.words.join(" ")),
                )
            })
    }
    fn end(self) -> Result<(), Problem> {
        if let Some((key, (line, _))) = self.fields.into_iter().next() {
            return Err(Problem::new(
                line,
                format!("unknown or misplaced field `{key}`"),
            ));
        }
        Ok(())
    }
    fn args(&self, count: usize) -> Result<(), Problem> {
        if self.words.len() != count {
            return Err(Problem::new(
                self.line,
                format!("wrong number of arguments for `{}`", self.words[0]),
            ));
        }
        Ok(())
    }
}

fn sections(text: &str) -> Result<Vec<Section>, Problem> {
    if text.len() > MAX_BYTES {
        return Err(Problem::new(1, "document exceeds 1 MiB"));
    }
    let mut out: Vec<Section> = Vec::new();
    let mut lines = text.split_terminator('\n').enumerate();
    while let Some((index, line)) = lines.next() {
        if line.is_empty() {
            continue;
        }
        let number = index + 1;
        if line.starts_with('#') {
            let depth = line.bytes().take_while(|b| *b == b'#').count();
            if depth > 3 || !line[depth..].starts_with(' ') {
                return Err(Problem::new(
                    number,
                    "expected # flow, ## section or ### subsection",
                ));
            }
            let words = line[depth + 1..]
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if words.is_empty() {
                return Err(Problem::new(number, "empty section heading"));
            }
            out.push(Section {
                depth,
                words,
                fields: BTreeMap::new(),
                line: number,
            });
            continue;
        }
        let (key, raw) = line
            .split_once(':')
            .ok_or_else(|| Problem::new(number, "expected `key: value`"))?;
        let value = raw.strip_prefix(' ').unwrap_or(raw);
        let value = if let Some(end) = value.strip_prefix("<<") {
            if end.is_empty() || !end.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err(Problem::new(
                    number,
                    "heredoc terminator must be an identifier",
                ));
            }
            let mut block = Vec::new();
            let mut closed = false;
            for (_, next) in lines.by_ref() {
                if next == end {
                    closed = true;
                    break;
                }
                block.push(next);
            }
            if !closed {
                return Err(Problem::new(
                    number,
                    format!("missing heredoc terminator `{end}`"),
                ));
            }
            block.join("\n")
        } else {
            value.to_owned()
        };
        let section = out
            .last_mut()
            .ok_or_else(|| Problem::new(number, "expected `# flow <id>` first"))?;
        if section.fields.insert(key.into(), (number, value)).is_some() {
            return Err(Problem::new(number, format!("duplicate field `{key}`")));
        }
    }
    Ok(out)
}

fn range(text: &str, line: usize) -> Result<(u32, u32), Problem> {
    let (a, b) = text.split_once('-').unwrap_or((text, text));
    match (a.parse::<u32>(), b.parse::<u32>()) {
        (Ok(a), Ok(b)) if a > 0 && b >= a => Ok((a, b)),
        _ => Err(Problem::new(
            line,
            format!("invalid inclusive range `{text}`"),
        )),
    }
}

fn path<'a>(
    id: &str,
    files: &'a BTreeMap<String, String>,
    line: usize,
) -> Result<&'a str, Problem> {
    files
        .get(id)
        .map(String::as_str)
        .ok_or_else(|| Problem::new(line, format!("unknown file id `{id}`")))
}

fn location(text: &str, files: &BTreeMap<String, String>, line: usize) -> Result<Value, Problem> {
    let parts: Vec<_> = text.split(':').collect();
    if parts.len() != 3 || !["head", "base"].contains(&parts[1]) {
        return Err(Problem::new(
            line,
            "location must be file-id:head|base:first-last",
        ));
    }
    let (start, end) = range(parts[2], line)?;
    Ok(json!({"file":path(parts[0],files,line)?,"side":parts[1],"start_line":start,"end_line":end}))
}

fn enum_value<T: serde::de::DeserializeOwned>(value: &str, line: usize) -> Result<(), Problem> {
    serde_json::from_value::<T>(Value::String(value.into()))
        .map(|_| ())
        .map_err(|error| Problem::new(line, error.to_string()))
}

fn strings(section: &mut Section, required: &[&str], optional: &[&str]) -> Result<Value, Problem> {
    let mut result = Map::new();
    for &key in required {
        let line = section
            .fields
            .get(key)
            .map_or(section.line, |(line, _)| *line);
        let value = section.required(key)?;
        match key {
            "risk" => enum_value::<RiskBand>(&value, line)?,
            "origin" => enum_value::<Origin>(&value, line)?,
            "blocking" => enum_value::<Blocking>(&value, line)?,
            "basis" => enum_value::<Basis>(&value, line)?,
            _ => {}
        }
        result.insert(key.into(), Value::String(value));
    }
    for &key in optional {
        if let Some(value) = section.field(key) {
            result.insert(key.into(), Value::String(value));
        }
    }
    Ok(Value::Object(result))
}

/// Parses grammar and required fields. Semantic checks are in [`super::Context::check`].
pub fn parse(text: &str, catalog: &Catalog) -> Result<FlowAnalysisOutput, Problem> {
    let mut sections = sections(text)?.into_iter();
    let mut header = sections
        .next()
        .ok_or_else(|| Problem::new(1, "expected `# flow <id>`"))?;
    header.args(2)?;
    if header.depth != 1 || header.words[0] != "flow" {
        return Err(Problem::new(header.line, "expected `# flow <id>`"));
    }
    let mut files = catalog.files.clone();
    for (key, (line, value)) in std::mem::take(&mut header.fields) {
        let Some(id) = key.strip_prefix("file ") else {
            return Err(Problem::new(
                line,
                "only `file <id>: <path>` is allowed in the header",
            ));
        };
        if id.is_empty()
            || !id.bytes().all(|b| b.is_ascii_alphanumeric())
            || value.is_empty()
            || value.starts_with('/')
            || value.split('/').any(|p| p == ".." || p == ".")
        {
            return Err(Problem::new(line, "invalid file declaration"));
        }
        if files.contains_key(id) || files.values().any(|p| p == &value) {
            return Err(Problem::new(
                line,
                "duplicate or already known file declaration",
            ));
        }
        files.insert(id.into(), value);
    }
    let mut root = json!({"flow_id":header.words[1], "steps":[], "edges":[], "impact_refs":[], "pattern_repetitions":[], "thread_statuses":[]});
    let mut active_step: Option<usize> = None;
    let mut step_lines = Vec::new();
    for mut section in sections {
        let line = section.line;
        let tag = section.words[0].clone();
        if section.depth == 2 {
            active_step = None;
        }
        match (section.depth, tag.as_str()) {
            (2, "introduction") => {
                section.args(1)?;
                if root.get("introduction").is_some() {
                    return Err(Problem::new(line, "duplicate introduction"));
                }
                root["introduction"] = strings(
                    &mut section,
                    &["problem_before", "change", "example_before_after"],
                    &[],
                )?;
            }
            (2, "step") => {
                if section.words.len() != 3 && section.words.len() != 4 {
                    return Err(Problem::new(
                        line,
                        "expected `## step <id> <hunk-id>` or `## step <id> <file-id>:<side>:<range> <kind>`",
                    ));
                }
                let mut value = if section.words.len() == 3 {
                    let anchor = catalog.hunks.get(&section.words[2]).ok_or_else(|| {
                        Problem::new(line, format!("unknown planned hunk `{}`", section.words[2]))
                    })?;
                    json!({"file":anchor.file,"side":anchor.side,"start_line":anchor.start,"end_line":anchor.end,"kind":anchor.kind})
                } else {
                    let mut value = location(&section.words[2], &files, line)?;
                    enum_value::<FragmentKind>(&section.words[3], line)?;
                    value["kind"] = json!(section.words[3]);
                    value
                };
                let fields = strings(
                    &mut section,
                    &["title", "called_by", "decides", "next", "why"],
                    &["symbol"],
                )?;
                for (key, value_field) in fields.as_object().into_iter().flatten() {
                    value[key] = value_field.clone();
                }
                value["id"] = json!(section.words[1]);
                value["annotations"] = json!([]);
                if let Some(moved) = section.field("moved_from") {
                    let loc = location(&moved, &files, line)?;
                    if loc["side"] != "base" {
                        return Err(Problem::new(line, "moved_from must refer to base"));
                    }
                    value["moved_from"] = json!({"file":loc["file"],"base_start":loc["start_line"],"base_end":loc["end_line"]});
                }
                let steps = array_mut(&mut root, "steps")?;
                if steps.iter().any(|s| s["id"] == value["id"]) {
                    return Err(Problem::new(line, "duplicate step id"));
                }
                active_step = Some(steps.len());
                step_lines.push(line);
                steps.push(value);
            }
            (3, "assessment") => {
                section.args(1)?;
                let index = active_step
                    .ok_or_else(|| Problem::new(line, "assessment must follow its step"))?;
                let step = &mut root["steps"][index];
                if step.get("assessment").is_some() {
                    return Err(Problem::new(line, "duplicate assessment"));
                }
                step["assessment"] = strings(
                    &mut section,
                    &[
                        "headline",
                        "risk",
                        "reason",
                        "basis_note",
                        "not_checked",
                        "origin",
                        "blocking",
                    ],
                    &["smallest_correction"],
                )?;
            }
            (3, "annotation") => {
                section.args(4)?;
                let index = active_step
                    .ok_or_else(|| Problem::new(line, "annotation must follow its step"))?;
                enum_value::<AnnotationType>(&section.words[1], line)?;
                enum_value::<Basis>(&section.words[2], line)?;
                let (start, end) = range(&section.words[3], line)?;
                let mut value = strings(&mut section, &[], &["was", "is", "text"])?;
                for key in ["was", "is"] {
                    if value.get(key).is_none() {
                        value[key] = json!("");
                    }
                }
                value["type"] = json!(section.words[1]);
                value["basis"] = json!(section.words[2]);
                value["start_line"] = json!(start);
                value["end_line"] = json!(end);
                let mut evidence = Vec::new();
                for reference in section
                    .field("evidence")
                    .unwrap_or_default()
                    .split_whitespace()
                {
                    let loc = location(reference, &files, line)?;
                    if loc["start_line"] != loc["end_line"] {
                        return Err(Problem::new(line, "evidence refers to one line"));
                    }
                    evidence.push(
                        json!({"file":loc["file"],"side":loc["side"],"line":loc["start_line"]}),
                    );
                }
                value["evidence"] = json!(evidence);
                array_mut(&mut root["steps"][index], "annotations")?.push(value);
            }
            (2, "edge") => {
                section.args(3)?;
                let mut value = strings(&mut section, &[], &["label"])?;
                value["from_step_id"] = json!(section.words[1]);
                value["to_step_id"] = json!(section.words[2]);
                array_mut(&mut root, "edges")?.push(value);
            }
            (2, "impact") => {
                section.args(3)?;
                let mut value = location(&section.words[2], &files, line)?;
                if value["side"] != "head" {
                    return Err(Problem::new(line, "impact must refer to head"));
                }
                if let Value::Object(ref mut fields) = value {
                    fields.remove("side");
                }
                value["step_id"] = json!(section.words[1]);
                let fields = strings(&mut section, &["reason", "basis"], &["symbol"])?;
                for (key, field) in fields.as_object().into_iter().flatten() {
                    value[key] = field.clone();
                }
                array_mut(&mut root, "impact_refs")?.push(value);
            }
            (2, "repetition") => {
                section.args(2)?;
                let description = section.required("description")?;
                let mut sites = Vec::new();
                for site in section.required("sites")?.split_whitespace() {
                    sites.push(location(site, &files, line)?);
                }
                array_mut(&mut root,"pattern_repetitions")?.push(json!({"description":description,"explained_in_step_id":section.words[1],"sites":sites}));
            }
            (2, "thread") => {
                section.args(3)?;
                enum_value::<ThreadLlmStatus>(&section.words[2], line)?;
                let reason = section.required("reason")?;
                array_mut(&mut root,"thread_statuses")?.push(json!({"thread_alias":section.words[1],"status":section.words[2],"reason":reason}));
            }
            _ => {
                return Err(Problem::new(
                    line,
                    format!("unknown or misplaced section `{tag}`"),
                ));
            }
        }
        section.end()?;
    }
    for (index, step) in root["steps"].as_array().into_iter().flatten().enumerate() {
        if step.get("assessment").is_none() {
            return Err(Problem::new(
                step_lines[index],
                format!(
                    "step {} has no assessment",
                    step["id"].as_str().unwrap_or("?")
                ),
            ));
        }
    }
    // All numeric and enum fields pass through their canonical typed definitions.
    serde_json::from_value(root).map_err(|e| Problem::new(1, format!("invalid flow: {e}")))
}

fn array_mut<'a>(value: &'a mut Value, key: &str) -> Result<&'a mut Vec<Value>, Problem> {
    value
        .get_mut(key)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| Problem::new(1, format!("invalid collection `{key}`")))
}

fn field(out: &mut String, key: &str, value: &str) {
    if value.contains('\n') || value.starts_with("<<") || value.ends_with('\r') {
        let mut end = "END".to_owned();
        while value.lines().any(|line| line == end) {
            end.push('_');
        }
        out.push_str(&format!("{key}: <<{end}\n{value}\n{end}\n"));
    } else {
        out.push_str(&format!("{key}: {value}\n"));
    }
}

fn emit_fields(out: &mut String, value: &Value, keys: &[&str]) {
    for &key in keys {
        if let Some(text) = value[key].as_str()
            && !text.is_empty()
        {
            field(out, key, text);
        }
    }
}

fn alias(file: &str, files: &mut BTreeMap<String, String>) -> String {
    if let Some((id, _)) = files.iter().find(|(_, path)| path.as_str() == file) {
        return id.clone();
    }
    let mut index = files.len() + 1;
    while files.contains_key(&format!("p{index}")) {
        index += 1;
    }
    let id = format!("p{index}");
    files.insert(id.clone(), file.into());
    id
}

fn loc(value: &Value, files: &mut BTreeMap<String, String>) -> String {
    format!(
        "{}:{}:{}-{}",
        alias(value["file"].as_str().unwrap_or(""), files),
        value["side"].as_str().unwrap_or("head"),
        value["start_line"],
        value["end_line"]
    )
}

/// Encodes an existing canonical document, used by fixtures and the byte measurement helper.
pub fn encode(document: &FlowAnalysisOutput, catalog: &Catalog) -> Result<String, Problem> {
    let value = serde_json::to_value(document).map_err(|e| Problem::new(1, e.to_string()))?;
    let mut files = catalog.files.clone();
    let mut body = "\n## introduction\n".to_owned();
    emit_fields(
        &mut body,
        &value["introduction"],
        &["problem_before", "change", "example_before_after"],
    );
    for step in value["steps"].as_array().into_iter().flatten() {
        let anchor = Anchor {
            file: step["file"].as_str().unwrap_or("").into(),
            side: step["side"].as_str().unwrap_or("").into(),
            start: step["start_line"].as_u64().unwrap_or(0) as u32,
            end: step["end_line"].as_u64().unwrap_or(0) as u32,
            kind: step["kind"].as_str().unwrap_or("").into(),
        };
        let at = match catalog.hunks.iter().find(|(_, a)| **a == anchor) {
            Some((id, _)) => id.clone(),
            None => format!("{} {}", loc(step, &mut files), anchor.kind),
        };
        body.push_str(&format!(
            "\n## step {} {at}\n",
            step["id"].as_str().unwrap_or("")
        ));
        emit_fields(
            &mut body,
            step,
            &["title", "symbol", "called_by", "decides", "next", "why"],
        );
        if !step["moved_from"].is_null() {
            let moved = &step["moved_from"];
            field(
                &mut body,
                "moved_from",
                &format!(
                    "{}:base:{}-{}",
                    alias(moved["file"].as_str().unwrap_or(""), &mut files),
                    moved["base_start"],
                    moved["base_end"]
                ),
            );
        }
        for annotation in step["annotations"].as_array().into_iter().flatten() {
            body.push_str(&format!(
                "\n### annotation {} {} {}-{}\n",
                annotation["type"].as_str().unwrap_or(""),
                annotation["basis"].as_str().unwrap_or(""),
                annotation["start_line"],
                annotation["end_line"]
            ));
            emit_fields(&mut body, annotation, &["was", "is", "text"]);
            let refs = annotation["evidence"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| {
                    format!(
                        "{}:{}:{}",
                        alias(e["file"].as_str().unwrap_or(""), &mut files),
                        e["side"].as_str().unwrap_or(""),
                        e["line"]
                    )
                })
                .collect::<Vec<_>>();
            if !refs.is_empty() {
                field(&mut body, "evidence", &refs.join(" "));
            }
        }
        if !step["assessment"].is_null() {
            body.push_str("\n### assessment\n");
            emit_fields(
                &mut body,
                &step["assessment"],
                &[
                    "headline",
                    "risk",
                    "reason",
                    "basis_note",
                    "not_checked",
                    "origin",
                    "blocking",
                    "smallest_correction",
                ],
            );
        }
    }
    for edge in &document.edges {
        body.push_str(&format!(
            "\n## edge {} {}\n",
            edge.from_step_id, edge.to_step_id
        ));
        if let Some(label) = &edge.label {
            field(&mut body, "label", label);
        }
    }
    for impact in value["impact_refs"].as_array().into_iter().flatten() {
        body.push_str(&format!(
            "\n## impact {} {}\n",
            impact["step_id"].as_str().unwrap_or(""),
            loc(impact, &mut files)
        ));
        emit_fields(&mut body, impact, &["symbol", "reason", "basis"]);
    }
    for repetition in value["pattern_repetitions"]
        .as_array()
        .into_iter()
        .flatten()
    {
        body.push_str(&format!(
            "\n## repetition {}\n",
            repetition["explained_in_step_id"].as_str().unwrap_or("")
        ));
        emit_fields(&mut body, repetition, &["description"]);
        let sites = repetition["sites"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| loc(s, &mut files))
            .collect::<Vec<_>>();
        field(&mut body, "sites", &sites.join(" "));
    }
    for thread in &document.thread_statuses {
        let status =
            serde_json::to_value(thread.status).map_err(|e| Problem::new(1, e.to_string()))?;
        body.push_str(&format!(
            "\n## thread {} {}\n",
            thread.thread_alias,
            status.as_str().unwrap_or("")
        ));
        field(&mut body, "reason", &thread.reason);
    }
    let mut out = format!("# flow {}\n", document.flow_id.0);
    for (id, path) in files {
        if !catalog.files.contains_key(&id) {
            field(&mut out, &format!("file {id}"), &path);
        }
    }
    out.push_str(&body);
    Ok(out)
}

/// Canonical verifier field paths -> source lines, built from parsed sections, never raw prose headings.
pub(super) fn source_lines(text: &str) -> Result<BTreeMap<String, usize>, Problem> {
    let mut map = BTreeMap::new();
    let mut counts = BTreeMap::<String, usize>::new();
    let mut step = String::new();
    let mut annotations = 0;
    for section in sections(text)? {
        let path = match (section.depth, section.words[0].as_str()) {
            (2, "introduction") => "introduction".to_owned(),
            (2, "step") => {
                let index = counts.entry("steps".into()).or_default();
                step = format!("steps[{index}] \"{}\"", section.words[1]);
                *index += 1;
                annotations = 0;
                step.clone()
            }
            (3, "assessment") => format!("{step}.assessment"),
            (3, "annotation") => {
                let path = format!("{step}.annotations[{annotations}]");
                annotations += 1;
                path
            }
            (2, tag) => {
                let key = match tag {
                    "edge" => "edges",
                    "impact" => "impact_refs",
                    "repetition" => "pattern_repetitions",
                    "thread" => "thread_statuses",
                    _ => continue,
                };
                let index = counts.entry(key.into()).or_default();
                let path = format!("{key}[{index}]");
                *index += 1;
                path
            }
            _ => continue,
        };
        map.insert(path.clone(), section.line);
        for (key, (line, _)) in section.fields {
            map.insert(format!("{path}.{key}"), line);
        }
    }
    Ok(map)
}
