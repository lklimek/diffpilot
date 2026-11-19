//! Fixture PR `acme/node-client#482` (`fixtures/pr-482/`, plan §7 WP1-B): agent outputs, API seed and states,
//! GitHub and Codex recordings and the example repository are valid, mutually consistent and follow the trust
//! wording rules of brief §1; so does the text of the dashboard fixtures (`fixtures/dashboard/`).
#![expect(clippy::expect_used, reason = "test helpers outside #[test] functions")]

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::agent::plan::{HunkSelection, PlanOutput};
use contracts::agent::pr_context::PrContextOutput;
use contracts::agent::{JobKind, output_schema};
use contracts::api::analyses::{AnalysisState, AnalysisSummary};
use contracts::api::auth::MeResponse;
use contracts::api::code::LinesResponse;
use contracts::api::comments::{CommentKind, PendingReview};
use contracts::api::feedback::{Feedback, FeedbackTarget, FeedbackValue};
use contracts::api::pr::ResolvePrResponse;
use contracts::api::questions::{AskQuestion, AskState};
use contracts::api::review::{BlockRef, ReviewState};
use contracts::api::settings::Settings;
use contracts::api::tour::{Annotation, FlowGroup, ProfileSource, Tour};
use contracts::common::{
    AnnotationType, Basis, FlagLevel, FragmentKind, LineRange, LineSide, NodeKind,
};
use contracts::ids::{AnnotationId, StepId};
use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

const STATES: [&str; 5] = ["wrong", "missed", "noq", "noprofile", "resume"];
const GITHUB: &str = "https://github.com/acme/node-client";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482")
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|error| panic!("{rel}: {error}"))
}

fn json(rel: &str) -> Value {
    serde_json::from_str(&read(rel)).unwrap_or_else(|error| panic!("{rel}: {error}"))
}

fn typed<T: DeserializeOwned>(rel: &str) -> T {
    serde_json::from_value(json(rel)).unwrap_or_else(|error| panic!("{rel}: {error}"))
}

/// Files below `dir`, relative to it, sorted.
fn files_below(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).expect("readable fixture directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path.strip_prefix(dir).expect("below dir").to_path_buf());
            }
        }
    }
    out.sort();
    out
}

#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
struct Span {
    start: u32,
    len: u32,
}

/// One `git diff -U0` hunk with the pre-pass outcome the fixture expects (`repo/hunks.json`).
#[derive(Debug, Deserialize, Clone)]
struct HunkRec {
    id: String,
    path: String,
    old_path: Option<String>,
    status: String,
    base: Span,
    head: Span,
    /// `agent`, `identical_move`, `not_analysed:<category>`.
    prepass: String,
}

impl HunkRec {
    fn added(&self) -> Vec<u32> {
        (self.head.start..self.head.start + self.head.len).collect()
    }
    fn removed(&self) -> Vec<u32> {
        (self.base.start..self.base.start + self.base.len).collect()
    }
    fn base_path(&self) -> &str {
        self.old_path.as_deref().unwrap_or(&self.path)
    }
    /// Head line after which removed lines are shown (`analysis::diff::DiffIndex::deletion_marker`).
    fn marker(&self) -> u32 {
        if self.head.len == 0 {
            self.head.start
        } else {
            self.head.start - 1
        }
    }
}

fn hunks() -> Vec<HunkRec> {
    typed("repo/hunks.json")
}

fn side_dir(side: LineSide) -> &'static str {
    match side {
        LineSide::Head => "head",
        LineSide::Base => "base",
    }
}

/// Where a repository file is stored: Rust sources carry `.txt` so the pre-commit rustfmt check
/// leaves their line layout (fixture data) alone; `build-repo.sh` restores the names.
fn stored_path(path: &str) -> String {
    if path.ends_with(".rs") {
        format!("{path}.txt")
    } else {
        path.to_owned()
    }
}

fn file_lines(side: LineSide, path: &str) -> Option<Vec<String>> {
    let stored = root()
        .join("repo")
        .join(side_dir(side))
        .join(stored_path(path));
    let text = fs::read_to_string(stored).ok()?;
    Some(text.lines().map(str::to_owned).collect())
}

fn is_blank(side: LineSide, path: &str, line: u32) -> bool {
    file_lines(side, path)
        .and_then(|lines| lines.get(line as usize - 1).map(|l| l.trim().is_empty()))
        .unwrap_or(false)
}

fn assert_lines_exist(side: LineSide, path: &str, start: u32, end: u32, what: &str) {
    let lines = file_lines(side, path).unwrap_or_else(|| panic!("{what}: no {side:?} file {path}"));
    assert!(
        1 <= start && start <= end && end as usize <= lines.len(),
        "{what}: {path}:{start}-{end} outside the {side:?} file ({} lines)",
        lines.len()
    );
}

fn plan() -> PlanOutput {
    typed("agent/plan.json")
}

fn flow_outputs() -> Vec<FlowAnalysisOutput> {
    (1..=5)
        .map(|n| typed(&format!("agent/flow-{n}.json")))
        .collect()
}

fn round_trip<T: Serialize + DeserializeOwned>(what: &str, value: &Value) {
    let parsed: T =
        serde_json::from_value(value.clone()).unwrap_or_else(|error| panic!("{what}: {error}"));
    let back = serde_json::to_value(parsed).expect("serialises");
    assert_eq!(
        &back, value,
        "{what}: re-serialised JSON differs (unknown or defaulted field)"
    );
}

#[test]
fn agent_fixtures_validate_and_round_trip() {
    let mut files = vec![(JobKind::Plan, "agent/plan.json".to_owned())];
    files.push((JobKind::PrContext, "agent/pr_context.json".to_owned()));
    files.extend((1..=5).map(|n| (JobKind::AnalyseFlow, format!("agent/flow-{n}.json"))));
    for (kind, file) in files {
        // Flow JSON is the stored canonical projection; the model writes tagged Markdown (ADR-014).
        let schema = match kind {
            JobKind::AnalyseFlow => serde_json::to_value(schemars::schema_for!(FlowAnalysisOutput))
                .expect("canonical flow schema"),
            _ => output_schema(kind).expect("strict schema"),
        };
        let validator = jsonschema::validator_for(&schema).expect("valid schema");
        let value = json(&file);
        let errors: Vec<String> = validator
            .iter_errors(&value)
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "{file}: {errors:?}");
        match kind {
            JobKind::Plan => round_trip::<PlanOutput>(&file, &value),
            JobKind::PrContext => round_trip::<PrContextOutput>(&file, &value),
            JobKind::AnalyseFlow => round_trip::<FlowAnalysisOutput>(&file, &value),
            JobKind::Ask | JobKind::JudgeWork => unreachable!("no ask or judge_work fixture"),
        }
    }
}

/// Changed lines a selection covers: (hunk id, `+n` added head line or `-n` removed base line).
fn selected(selection: &HunkSelection, hunks: &BTreeMap<String, HunkRec>) -> Vec<(String, i64)> {
    let hunk = hunks
        .get(&selection.hunk_id.0)
        .unwrap_or_else(|| panic!("unknown hunk {}", selection.hunk_id));
    assert_eq!(
        hunk.prepass, "agent",
        "{} is not given to the agent",
        hunk.id
    );
    let (from, to) = match (selection.head_start, selection.head_end) {
        (None, None) => (0, u32::MAX),
        (Some(from), Some(to)) => {
            assert!(
                hunk.head.len > 0,
                "{}: pure deletion selected by sub-range",
                hunk.id
            );
            let last = hunk.head.start + hunk.head.len - 1;
            assert!(
                hunk.head.start <= from && from <= to && to <= last,
                "{}: sub-range outside",
                hunk.id
            );
            (from, to)
        }
        _ => panic!(
            "{}: head_start and head_end must both be null or both set",
            hunk.id
        ),
    };
    let mut out: Vec<(String, i64)> = hunk
        .added()
        .into_iter()
        .filter(|l| (from..=to).contains(l))
        .map(|l| (hunk.id.clone(), i64::from(l)))
        .collect();
    if (from..=to).contains(&hunk.marker()) || (from, to) == (0, u32::MAX) {
        out.extend(
            hunk.removed()
                .into_iter()
                .map(|l| (hunk.id.clone(), -i64::from(l))),
        );
    }
    out
}

#[test]
fn plan_partitions_the_agent_hunks() {
    let hunks: BTreeMap<String, HunkRec> = hunks().into_iter().map(|h| (h.id.clone(), h)).collect();
    let plan = plan();
    let mut seen: BTreeMap<(String, i64), u32> = BTreeMap::new();
    let mut selections: Vec<HunkSelection> = plan
        .flows
        .iter()
        .flat_map(|flow| flow.hunks.clone())
        .collect();
    for u in &plan.unassigned {
        assert!(
            !u.reason.trim().is_empty(),
            "unassigned {} needs a reason",
            u.hunk_id
        );
        selections.push(HunkSelection {
            hunk_id: u.hunk_id.clone(),
            head_start: u.head_start,
            head_end: u.head_end,
        });
    }
    for selection in &selections {
        for key in selected(selection, &hunks) {
            *seen.entry(key).or_default() += 1;
        }
    }
    for hunk in hunks.values().filter(|h| h.prepass == "agent") {
        let lines = hunk
            .added()
            .into_iter()
            .map(i64::from)
            .chain(hunk.removed().into_iter().map(|l| -i64::from(l)));
        for line in lines {
            assert_eq!(
                seen.get(&(hunk.id.clone(), line)),
                Some(&1),
                "{} line {line}: not exactly once",
                hunk.id
            );
        }
    }
    let ids: Vec<&str> = plan.flows.iter().map(|f| f.id.0.as_str()).collect();
    assert_eq!(ids, ["f1", "f2", "f3", "f4", "f5"]);
    let ranks: BTreeSet<u32> = plan.flows.iter().map(|f| f.importance_rank).collect();
    assert_eq!(ranks.len(), plan.flows.len(), "importance ranks are unique");
    for flow in &plan.flows {
        assert!(
            flow.depends_on
                .iter()
                .all(|d| ids.contains(&d.0.as_str()) && d != &flow.id)
        );
    }
}

#[test]
fn flow_outputs_follow_the_agent_output_rules() {
    let hunks: BTreeMap<String, HunkRec> = hunks().into_iter().map(|h| (h.id.clone(), h)).collect();
    let plan = plan();
    let mut kinds = HashSet::new();
    for (flow, planned) in flow_outputs().iter().zip(&plan.flows) {
        let f = flow.flow_id.0.as_str();
        assert_eq!(flow.flow_id, planned.id);
        let ids: BTreeSet<&str> = flow.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), flow.steps.len(), "{f}: step ids unique");
        let known = |id: &str| assert!(ids.contains(id), "{f}: unknown step {id}");
        for edge in &flow.edges {
            known(&edge.from_step_id);
            known(&edge.to_step_id);
        }
        for step in &flow.steps {
            let what = format!("{f}/{}", step.id);
            kinds.insert((step.side, step.kind));
            assert_lines_exist(step.side, &step.file, step.start_line, step.end_line, &what);
            for text in [&step.title, &step.decides, &step.why] {
                assert!(!text.trim().is_empty(), "{what}: empty text");
            }
            let has_edge = |incoming: bool| {
                flow.edges.iter().any(|e| {
                    let end = if incoming {
                        &e.to_step_id
                    } else {
                        &e.from_step_id
                    };
                    *end == step.id
                })
            };
            for (incoming, text) in [(true, &step.called_by), (false, &step.next)] {
                assert!(
                    !text.trim().is_empty() || !has_edge(incoming),
                    "{what}: connection empty although an edge carries it"
                );
            }
            if step.side == LineSide::Base {
                assert_eq!(
                    step.kind,
                    FragmentKind::DeletedSite,
                    "{what}: base steps are deleted sites"
                );
                let removed: BTreeSet<u32> = hunks
                    .values()
                    .filter(|h| h.base_path() == step.file)
                    .flat_map(HunkRec::removed)
                    .collect();
                assert!(
                    (step.start_line..=step.end_line).all(|l| removed.contains(&l)),
                    "{what}: base lines not removed"
                );
            }
            if let Some(from) = &step.moved_from {
                assert_eq!(
                    step.kind,
                    FragmentKind::Moved,
                    "{what}: moved_from on a moved step"
                );
                assert_lines_exist(
                    LineSide::Base,
                    &from.file,
                    from.base_start,
                    from.base_end,
                    &what,
                );
            }
            for a in &step.annotations {
                assert!(
                    step.start_line <= a.start_line
                        && a.start_line <= a.end_line
                        && a.end_line <= step.end_line,
                    "{what}: annotation outside"
                );
                assert!(
                    if a.kind == AnnotationType::Behaviour {
                        !a.was.trim().is_empty() && !a.is.trim().is_empty()
                    } else {
                        a.was.is_empty() && a.is.is_empty()
                    },
                    "{what}: was/is belongs only to behaviour"
                );
                assert_eq!(
                    a.text.is_none(),
                    a.kind == AnnotationType::Behaviour,
                    "{what}: text null iff behaviour"
                );
                assert!(
                    a.basis == Basis::Inference || !a.evidence.is_empty(),
                    "{what}: fact without evidence"
                );
                assert_eq!(
                    a.flag.is_some(),
                    a.kind == AnnotationType::Flag,
                    "{what}: a flag carries its level and scenario, nothing else does"
                );
                if let Some(flag) = &a.flag {
                    assert!(
                        a.text
                            .as_deref()
                            .is_some_and(|t| t.starts_with("Potential ")),
                        "{what}: flag title names the risk class"
                    );
                    assert!(
                        flag.level != FlagLevel::Unknown
                            && (2..=7).contains(&flag.scenario.len())
                            && !flag.not_checked.trim().is_empty(),
                        "{what}: flag has a level, a scenario and what was not checked"
                    );
                }
                for e in &a.evidence {
                    assert_lines_exist(e.side, &e.file, e.line, e.line, &what);
                }
            }
        }
        for impact in &flow.impact_refs {
            known(&impact.step_id);
            assert_lines_exist(
                LineSide::Head,
                &impact.file,
                impact.start_line,
                impact.end_line,
                f,
            );
        }
        for pattern in &flow.pattern_repetitions {
            known(&pattern.explained_in_step_id);
            for site in &pattern.sites {
                assert_lines_exist(site.side, &site.file, site.start_line, site.end_line, f);
            }
        }
        for status in &flow.thread_statuses {
            assert!(
                status
                    .thread_alias
                    .strip_prefix('t')
                    .is_some_and(|n| n.parse::<u32>().is_ok()),
                "{f}: alias"
            );
        }
        // Coverage: every non-blank changed line of the flow lies in a step (removed lines: a base step or a
        // head step around their deletion marker).
        for selection in &planned.hunks {
            let hunk = &hunks[&selection.hunk_id.0];
            for (_, line) in selected(selection, &hunks) {
                let covered = if line > 0 {
                    let l = line as u32;
                    is_blank(LineSide::Head, &hunk.path, l)
                        || flow.steps.iter().any(|s| {
                            s.side == LineSide::Head
                                && s.file == hunk.path
                                && (s.start_line..=s.end_line).contains(&l)
                        })
                } else {
                    let l = (-line) as u32;
                    is_blank(LineSide::Base, hunk.base_path(), l)
                        || flow.steps.iter().any(|s| {
                            (s.side == LineSide::Base
                                && s.file == hunk.base_path()
                                && (s.start_line..=s.end_line).contains(&l))
                                || (s.side == LineSide::Head
                                    && s.file == hunk.path
                                    && s.start_line <= hunk.marker()
                                    && hunk.marker() <= s.end_line)
                        })
                };
                assert!(
                    covered,
                    "{f}: changed line {line} of {} ({}) outside every step",
                    hunk.id, hunk.path
                );
            }
        }
    }
    for wanted in [
        (LineSide::Base, FragmentKind::DeletedSite),
        (LineSide::Head, FragmentKind::Moved),
        (LineSide::Head, FragmentKind::Context),
    ] {
        assert!(kinds.contains(&wanted), "the example covers {wanted:?}");
    }
}

/// One file of a diff: head path, base path of a rename, status, and (base, head) spans.
type DiffFile = (String, Option<String>, String, Vec<(Span, Span)>);

/// Parses `git diff -U0 -M` into hunk records (ids in path order, then file order; pre-pass left empty).
fn parse_diff(diff: &str) -> Vec<HunkRec> {
    let span = |text: &str| {
        let (start, len) = text.split_once(',').unwrap_or((text, "1"));
        Span {
            start: start.parse().expect("start"),
            len: len.parse().expect("len"),
        }
    };
    let mut files: Vec<DiffFile> = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            let (_, b) = rest.split_once(" b/").expect("diff header");
            files.push((b.to_owned(), None, "modified".to_owned(), Vec::new()));
        } else if let Some(file) = files.last_mut() {
            if line.starts_with("new file mode") {
                file.2 = "added".to_owned();
            } else if line.starts_with("deleted file mode") {
                file.2 = "deleted".to_owned();
            } else if let Some(from) = line.strip_prefix("rename from ") {
                file.1 = Some(from.to_owned());
                file.2 = "renamed".to_owned();
            } else if let Some(header) = line.strip_prefix("@@ -") {
                let (spans, _) = header.split_once(" @@").expect("hunk header");
                let (base, head) = spans.split_once(" +").expect("two spans");
                file.3.push((span(base), span(head)));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Vec::new();
    for (path, old_path, status, spans) in files {
        for (base, head) in spans {
            out.push(HunkRec {
                id: format!("h{}", out.len() + 1),
                path: path.clone(),
                old_path: old_path.clone(),
                status: status.clone(),
                base,
                head,
                prepass: String::new(),
            });
        }
    }
    out
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8")
}

#[test]
fn example_repository_reproduces_commits_and_hunks() {
    let dest = Path::new(env!("CARGO_TARGET_TMPDIR")).join("pr-482-repo");
    let _ = fs::remove_dir_all(&dest);
    let status = Command::new("bash")
        .arg(root().join("repo/build-repo.sh"))
        .arg(&dest)
        .status()
        .expect("bash runs");
    assert!(status.success(), "build-repo.sh failed");
    let commits = json("repo/commits.json");
    let base = git(
        &dest,
        &[
            "rev-parse",
            commits["base_branch"].as_str().expect("base branch"),
        ],
    );
    let head = git(
        &dest,
        &["rev-parse", commits["pull_ref"].as_str().expect("pull ref")],
    );
    assert_eq!(base.trim(), commits["base"], "base commit is reproducible");
    assert_eq!(head.trim(), commits["head"], "head commit is reproducible");
    let diff = git(
        &dest,
        &["diff", "-U0", "-M", "--no-color", base.trim(), head.trim()],
    );
    let actual = parse_diff(&diff);
    let expected = hunks();
    assert_eq!(actual.len(), expected.len(), "hunk count");
    for (a, e) in actual.iter().zip(&expected) {
        assert_eq!(
            (&a.id, &a.path, &a.old_path, &a.status, a.base, a.head),
            (&e.id, &e.path, &e.old_path, &e.status, e.base, e.head)
        );
    }
}

#[test]
fn github_fixtures_describe_the_example_pr() {
    let commits = json("repo/commits.json");
    for file in files_below(&root().join("github")) {
        let rel = format!("github/{}", file.display());
        let value = json(&rel);
        for key in ["request", "status", "response"] {
            assert!(value.get(key).is_some(), "{rel}: envelope needs {key}");
        }
    }
    let pull = json("github/rest/pull.json")["response"].clone();
    assert_eq!(pull["number"], 482);
    assert_eq!(pull["head"]["sha"], commits["head"]);
    assert_eq!(pull["base"]["sha"], commits["base"]);
    let mut counts: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for hunk in hunks() {
        let entry = counts.entry(hunk.path.clone()).or_default();
        entry.0 += u64::from(hunk.head.len);
        entry.1 += u64::from(hunk.base.len);
    }
    let files = json("github/rest/pull-files.json")["response"].clone();
    let listed: BTreeMap<String, (u64, u64)> = files
        .as_array()
        .expect("file list")
        .iter()
        .map(|f| {
            let name = f["filename"].as_str().expect("filename").to_owned();
            (
                name,
                (
                    f["additions"].as_u64().expect("additions"),
                    f["deletions"].as_u64().expect("deletions"),
                ),
            )
        })
        .collect();
    assert_eq!(listed, counts, "PR files match the repository diff");
    for null_case in [
        "add-thread-null-line-outside-diff",
        "add-thread-null-file-not-in-pr",
    ] {
        let value = json(&format!("github/graphql/{null_case}.json"));
        assert!(
            value["response"]["data"]["addPullRequestReviewThread"]["thread"].is_null(),
            "{null_case}"
        );
    }
}

#[test]
fn codex_streams_are_well_formed() {
    let manifest = json("codex/manifest.json");
    let entries = manifest.as_array().expect("manifest list");
    let started: BTreeSet<&str> = entries
        .iter()
        .filter(|e| e["session"] == "started")
        .map(|e| e["thread_id"].as_str().expect("thread id"))
        .collect();
    let mut outcomes = BTreeSet::new();
    let mut items = BTreeSet::new();
    for entry in entries {
        let file = entry["file"].as_str().expect("file");
        let events: Vec<Value> = read(&format!("codex/{file}"))
            .lines()
            .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{file}: {e}")))
            .collect();
        assert!(
            events.iter().all(|e| e["type"].is_string()),
            "{file}: every event has a type"
        );
        assert_eq!(events[0]["type"], "thread.started", "{file}");
        assert_eq!(events[0]["thread_id"], entry["thread_id"], "{file}");
        if entry["session"] == "resumed" {
            assert!(
                started.contains(entry["thread_id"].as_str().expect("id")),
                "{file}: resumes a started session"
            );
        }
        items.extend(
            events
                .iter()
                .filter_map(|e| e["item"]["type"].as_str().map(str::to_owned)),
        );
        let last = &events[events.len() - 1];
        let outcome = entry["outcome"].as_str().expect("outcome");
        outcomes.insert(outcome.to_owned());
        match outcome {
            "succeeded" => {
                assert_eq!(last["type"], "turn.completed", "{file}");
                assert_eq!(entry["exit_code"], 0, "{file}");
                let message = events
                    .iter()
                    .rev()
                    .find(|e| e["item"]["type"] == "agent_message")
                    .and_then(|e| e["item"]["text"].as_str())
                    .expect("final agent message");
                if let Some(output) = entry["output"].as_str() {
                    let parsed: Value =
                        serde_json::from_str(message).expect("final message is JSON");
                    assert_eq!(parsed, json(output), "{file}: final output equals {output}");
                }
            }
            "failed" | "usage_limited" => {
                assert_eq!(last["type"], "turn.failed", "{file}");
                assert_ne!(entry["exit_code"], 0, "{file}");
                let message = last["error"]["message"].as_str().expect("message");
                assert_eq!(
                    outcome == "usage_limited",
                    message.starts_with("You’ve hit your usage limit"),
                    "{file}"
                );
            }
            "cancelled" => {
                assert_eq!(entry["exit_code"], 143, "{file}: SIGTERM");
                assert!(
                    events.iter().all(|e| !e["type"]
                        .as_str()
                        .is_some_and(|t| t.starts_with("turn.c") || t == "turn.failed")),
                    "{file}"
                );
            }
            other => panic!("{file}: unknown outcome {other}"),
        }
    }
    for wanted in ["succeeded", "failed", "cancelled", "usage_limited"] {
        assert!(outcomes.contains(wanted), "a {wanted} stream exists");
    }
    for wanted in [
        "agent_message",
        "command_execution",
        "mcp_tool_call",
        "reasoning",
    ] {
        assert!(items.contains(wanted), "a {wanted} item exists");
    }
    assert!(
        entries
            .iter()
            .any(|e| e["job_kind"] == "compaction" && e["session"] == "resumed")
    );
}

/// `turn.completed.usage` is cumulative per session (ADR-003), so no counter may shrink across the
/// jobs of one thread in manifest order.
#[test]
fn codex_usage_is_cumulative_per_session() {
    let manifest = json("codex/manifest.json");
    let mut last: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let mut checked = 0;
    for entry in manifest.as_array().expect("manifest list") {
        let file = entry["file"].as_str().expect("file");
        let thread = entry["thread_id"].as_str().expect("thread id").to_owned();
        for line in read(&format!("codex/{file}")).lines() {
            let event: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{file}: {e}"));
            if event["type"] != "turn.completed" {
                continue;
            }
            let usage = event["usage"].as_object().expect("usage object");
            let previous = last.entry(thread.clone()).or_default();
            for (counter, value) in usage {
                let value = value
                    .as_u64()
                    .unwrap_or_else(|| panic!("{file}: {counter} is a count"));
                let before = previous.insert(counter.clone(), value).unwrap_or(0);
                assert!(
                    value >= before,
                    "{file}: {counter} drops from {before} to {value} within session {thread}"
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 1, "several completed turns are checked");
}

/// A `repository_file` profile source needs `.claudius/project-profile.md` on the base ref.
#[test]
fn repository_profile_source_is_on_the_base_ref() {
    let sources = tour_of(&seed()).pr_context.facts.project_profile.sources;
    assert!(sources.contains(&ProfileSource::RepositoryFile));
    let profile = ".claudius/project-profile.md";
    let base = file_lines(LineSide::Base, profile).expect("profile file on the base ref");
    assert!(!base.is_empty(), "profile file has content");
    assert_eq!(
        file_lines(LineSide::Head, profile),
        Some(base),
        "unchanged by the PR"
    );
}

/// The seed as one document: `{ "<path below api-seed/default without .json>": response }`.
fn seed() -> BTreeMap<String, Value> {
    let dir = root().join("api-seed/default");
    files_below(&dir)
        .into_iter()
        .map(|file| {
            let key = file.to_string_lossy().trim_end_matches(".json").to_owned();
            (key, json(&format!("api-seed/default/{}", file.display())))
        })
        .collect()
}

/// RFC 7396 JSON merge patch.
fn merge(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(serde_json::Map::new());
    }
    let Value::Object(map) = target else {
        unreachable!("made an object above")
    };
    for (key, value) in patch {
        if value.is_null() {
            map.remove(key);
        } else {
            merge(map.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}

fn state(name: &str) -> BTreeMap<String, Value> {
    let mut doc = Value::Object(seed().into_iter().collect());
    let patch = json(&format!("api-states/{name}.json"));
    for key in patch.as_object().expect("patch object").keys() {
        assert!(
            doc.get(key).is_some(),
            "{name}: patches unknown seed entry {key}"
        );
    }
    merge(&mut doc, &patch);
    // RFC 7396 deletes nulls, so a patch creating an analysis cannot insert the nullable usage field.
    // Materialize the current API default only here; old snapshots may still omit it (tested in api.rs).
    if let Some(analysis) = doc
        .pointer_mut("/pr-resolve/analysis")
        .and_then(Value::as_object_mut)
    {
        analysis.entry("usage").or_insert(Value::Null);
    }
    doc.as_object()
        .expect("object")
        .clone()
        .into_iter()
        .collect()
}

fn check_entry(what: &str, key: &str, value: &Value) {
    match key {
        "me" => round_trip::<MeResponse>(what, value),
        "pr-resolve" => round_trip::<ResolvePrResponse>(what, value),
        "analysis" => round_trip::<AnalysisSummary>(what, value),
        "tour" => round_trip::<Tour>(what, value),
        "review-state" => round_trip::<ReviewState>(what, value),
        "questions" => round_trip::<Vec<AskQuestion>>(what, value),
        "feedback" => round_trip::<Vec<Feedback>>(what, value),
        "pending-review" => round_trip::<PendingReview>(what, value),
        "settings" => round_trip::<Settings>(what, value),
        lines if lines.starts_with("lines/") => round_trip::<LinesResponse>(what, value),
        other => panic!("{what}: no contract type for seed entry {other}"),
    }
}

#[test]
fn api_seed_and_states_round_trip() {
    for (key, value) in seed() {
        check_entry(&format!("default/{key}"), &key, &value);
    }
    for name in STATES {
        for (key, value) in state(name) {
            check_entry(&format!("{name}/{key}"), &key, &value);
        }
    }
}

fn tour_of(doc: &BTreeMap<String, Value>) -> Tour {
    serde_json::from_value(doc["tour"].clone()).expect("tour")
}

fn step_annotations(tour: &Tour) -> impl Iterator<Item = (&StepId, &Annotation)> {
    tour.flows
        .iter()
        .flat_map(|f| f.steps.iter())
        .flat_map(|s| s.llm.annotations.iter().map(move |a| (&s.id, a)))
}

#[test]
fn api_states_show_what_the_inventory_names() {
    let default = seed();
    let base_tour = tour_of(&default);
    let me: MeResponse = serde_json::from_value(default["me"].clone()).expect("me");
    assert!(
        me.user.is_some() && me.csrf_token.is_some(),
        "default: signed in"
    );
    let resolve: ResolvePrResponse =
        serde_json::from_value(default["pr-resolve"].clone()).expect("resolve");
    assert!(
        resolve.analysis.is_none() && !resolve.head_moved,
        "default: no analysis yet"
    );
    assert!(
        !base_tour
            .pr_context
            .facts
            .project_profile
            .sources
            .is_empty(),
        "default: profile found"
    );
    for key in ["questions", "feedback"] {
        assert_eq!(default[key], Value::Array(vec![]), "default: no {key}");
    }
    assert_eq!(
        default["review-state"],
        serde_json::json!({"ticked_steps": [], "last_viewed": null})
    );

    let f31 = StepId("f3.1".to_owned());
    let wrong = state("wrong");
    let wrong_tour = tour_of(&wrong);
    let step = wrong_tour.flows[2]
        .steps
        .iter()
        .find(|s| s.id == f31)
        .expect("f3.1");
    assert_eq!(
        step.llm.assessment.as_ref().map(|a| a.risk),
        Some(contracts::common::RiskBand::High)
    );
    let feedback: Vec<Feedback> =
        serde_json::from_value(wrong["feedback"].clone()).expect("feedback");
    let mut on_step: Vec<String> = feedback
        .iter()
        .filter(|f| {
            f.target
                == FeedbackTarget::Step {
                    step_id: f31.clone(),
                }
        })
        .map(|f| serde_json::to_string(&f.value).expect("serialises"))
        .collect();
    on_step.sort();
    assert_eq!(
        on_step,
        ["\"overrated\"", "\"wrong\""],
        "wrong: feedback on f3.1"
    );
    let strip = |tour: &mut Tour| {
        tour.flows[2].llm.risk = None;
        tour.flows[2].steps[0].llm.assessment = None;
    };
    let (mut a, mut b) = (base_tour.clone(), wrong_tour);
    strip(&mut a);
    strip(&mut b);
    assert_eq!(
        a, b,
        "wrong: only the f3.1 assessment and the flow 3 risk differ"
    );

    let missed = state("missed");
    let pending: PendingReview =
        serde_json::from_value(missed["pending-review"].clone()).expect("pending");
    let [comment] = pending.comments.as_slice() else {
        panic!("missed: one pending comment")
    };
    assert_eq!(
        (
            comment.kind,
            comment.path.as_str(),
            comment.side,
            comment.line
        ),
        (
            CommentKind::Line,
            "src/rpc/client.rs",
            Some(LineSide::Head),
            Some(96)
        )
    );
    let feedback: Vec<Feedback> =
        serde_json::from_value(missed["feedback"].clone()).expect("feedback");
    assert!(
        feedback
            .iter()
            .any(|f| f.value == FeedbackValue::MissedByLlm
                && f.target
                    == FeedbackTarget::Line {
                        path: "src/rpc/client.rs".to_owned(),
                        side: LineSide::Head,
                        line: 96
                    })
    );
    assert!(
        hunks()
            .iter()
            .any(|h| h.path == "src/rpc/client.rs" && h.added().contains(&96)),
        "line 96 is changed"
    );
    assert!(
        step_annotations(&base_tour).all(|(s, a)| !(s.0.starts_with("f1.")
            && (a.range.start_line..=a.range.end_line).contains(&96))),
        "line 96 has no annotation"
    );

    let mut noq = tour_of(&state("noq"));
    assert!(
        step_annotations(&noq).all(|(_, a)| a.kind != AnnotationType::Flag),
        "noq: no flags"
    );
    assert!(
        noq.flows
            .iter()
            .all(|f| f.llm.risk.is_none() && f.steps.iter().all(|s| s.llm.assessment.is_none())),
        "noq: no assessments"
    );
    let mut expected = base_tour.clone();
    for flow in &mut expected.flows {
        flow.llm.risk = None;
        for step in &mut flow.steps {
            step.llm.assessment = None;
            step.llm
                .annotations
                .retain(|a| a.kind != AnnotationType::Flag);
        }
    }
    renumber(&mut expected);
    renumber(&mut noq);
    assert_eq!(noq, expected, "noq: only questions and assessments removed");

    let noprofile = tour_of(&state("noprofile"));
    assert!(
        noprofile
            .pr_context
            .facts
            .project_profile
            .sources
            .is_empty()
    );

    let resume = state("resume");
    let resolve: ResolvePrResponse =
        serde_json::from_value(resume["pr-resolve"].clone()).expect("resolve");
    assert!(matches!(
        resolve.analysis.map(|a| a.state),
        Some(AnalysisState::Finished { .. })
    ));
    let review: ReviewState =
        serde_json::from_value(resume["review-state"].clone()).expect("review state");
    assert!(review.ticked_steps.contains(&StepId("f2.1".to_owned())));
    assert_eq!(
        review.last_viewed,
        Some(BlockRef::Step {
            step_id: StepId("f1.5".to_owned())
        })
    );
    let questions: Vec<AskQuestion> =
        serde_json::from_value(resume["questions"].clone()).expect("questions");
    let [question] = questions.as_slice() else {
        panic!("resume: one question")
    };
    assert!(
        question.step_id.0 == "f1.5"
            && question.state == AskState::Answered
            && question.answer.is_some()
    );
}

/// Annotation ids are positional (`<step>.a<n>`); comparing after a removal needs fresh numbers.
fn renumber(tour: &mut Tour) {
    for step in tour.flows.iter_mut().flat_map(|f| f.steps.iter_mut()) {
        for (n, a) in step.llm.annotations.iter_mut().enumerate() {
            a.id = AnnotationId(format!("{}.a{}", step.id, n + 1));
        }
    }
}

fn ranges(lines: impl IntoIterator<Item = u32>) -> Vec<LineRange> {
    let mut out: Vec<LineRange> = Vec::new();
    for line in lines {
        match out.last_mut() {
            Some(last) if last.end_line + 1 == line => last.end_line = line,
            _ => out.push(LineRange {
                start_line: line,
                end_line: line,
            }),
        }
    }
    out
}

#[test]
fn api_seed_projects_the_agent_output_and_the_repository() {
    let default = seed();
    let tour = tour_of(&default);
    let commits = json("repo/commits.json");
    let hunks = hunks();
    let plan = plan();
    assert_eq!(tour.analysis.pr.head_sha.0, commits["head"]);
    assert_eq!(tour.analysis.pr.base_sha.0, commits["base"]);
    assert_eq!(
        serde_json::to_value(&tour.pr_context.llm).expect("json"),
        json("agent/pr_context.json")
    );
    assert_eq!(tour.flows.len(), plan.flows.len());
    let mut steps_total = 0;
    for (n, (flow, agent)) in tour.flows.iter().zip(flow_outputs()).enumerate() {
        let planned = &plan.flows[n];
        assert_eq!(
            (flow.id.clone(), flow.position),
            (planned.id.clone(), n as u32 + 1)
        );
        assert_eq!(flow.group, FlowGroup::from(planned.classification));
        assert_eq!(
            (
                &flow.llm.title,
                flow.llm.classification,
                &flow.llm.depends_on
            ),
            (&planned.title, planned.classification, &planned.depends_on)
        );
        assert_eq!(flow.llm.introduction, agent.introduction);
        assert_eq!(
            flow.llm.risk,
            agent
                .steps
                .iter()
                .filter_map(|s| s.assessment.as_ref().map(|a| a.risk))
                .max()
        );
        assert_eq!(flow.steps.len(), agent.steps.len());
        assert_eq!(flow.facts.step_count as usize, agent.steps.len());
        steps_total += agent.steps.len();
        let ids: BTreeMap<&str, StepId> = agent
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.as_str(), StepId(format!("{}.{}", flow.id, i + 1))))
            .collect();
        for (i, (step, a)) in flow.steps.iter().zip(&agent.steps).enumerate() {
            let what = step.id.to_string();
            assert_eq!((&step.id, step.number), (&ids[a.id.as_str()], i as u32 + 1));
            assert_eq!(
                (&step.facts.file, step.facts.side),
                (&a.file, a.side),
                "{what}"
            );
            assert_eq!(
                step.facts.range,
                LineRange {
                    start_line: a.start_line,
                    end_line: a.end_line
                },
                "{what}"
            );
            let sha = if a.side == LineSide::Base {
                &commits["base"]
            } else {
                &commits["head"]
            };
            let url = format!(
                "{GITHUB}/blob/{}/{}#L{}-L{}",
                sha.as_str().expect("sha"),
                a.file,
                a.start_line,
                a.end_line
            );
            assert_eq!(step.facts.github_url, url, "{what}");
            let changed = hunks.iter().flat_map(|h| match a.side {
                LineSide::Head if h.path == a.file => h.added(),
                LineSide::Base if h.base_path() == a.file => h.removed(),
                _ => Vec::new(),
            });
            let mut changed: Vec<u32> = changed
                .filter(|l| (a.start_line..=a.end_line).contains(l))
                .filter(|l| !WHITESPACE_ONLY.contains(&(a.file.as_str(), a.side, *l)))
                .collect();
            changed.sort_unstable();
            assert_eq!(
                step.facts.changed_lines,
                ranges(changed),
                "{what}: changed lines"
            );
            assert!(
                step.facts.call_hierarchy.is_none(),
                "{what}: call hierarchy is n/a in v1"
            );
            let llm = &step.llm;
            assert_eq!(
                (
                    &llm.title,
                    llm.kind,
                    &llm.symbol,
                    &llm.called_by,
                    &llm.decides,
                    &llm.next,
                    &llm.why
                ),
                (
                    &a.title,
                    a.kind,
                    &a.symbol,
                    &a.called_by,
                    &a.decides,
                    &a.next,
                    &a.why
                ),
                "{what}"
            );
            assert_eq!(
                (&llm.assessment, &llm.moved_from),
                (&a.assessment, &a.moved_from),
                "{what}"
            );
            let annotations: Vec<Annotation> = a
                .annotations
                .iter()
                .enumerate()
                .map(|(k, x)| {
                    Annotation::from_agent(
                        AnnotationId(format!("{}.a{}", step.id, k + 1)),
                        x.clone(),
                    )
                })
                .collect();
            assert_eq!(llm.annotations, annotations, "{what}");
            let impact: Vec<(&str, u32, u32)> = agent
                .impact_refs
                .iter()
                .filter(|r| r.step_id == a.id)
                .map(|r| (r.file.as_str(), r.start_line, r.end_line))
                .collect();
            let shown: Vec<(&str, u32, u32)> = llm
                .impact
                .iter()
                .map(|r| (r.file.as_str(), r.range.start_line, r.range.end_line))
                .collect();
            assert_eq!(shown, impact, "{what}: impact refs");
            let node = &flow.graph.nodes[i];
            assert_eq!(
                (&node.step_id, node.number, node.kind, &node.label),
                (&step.id, step.number, NodeKind::from(a.kind), &a.title)
            );
        }
        let edges: Vec<_> = agent
            .edges
            .iter()
            .map(|e| {
                (
                    ids[e.from_step_id.as_str()].clone(),
                    ids[e.to_step_id.as_str()].clone(),
                    e.label.clone(),
                )
            })
            .collect();
        let shown: Vec<_> = flow
            .graph
            .edges
            .iter()
            .map(|e| (e.from.clone(), e.to.clone(), e.label.clone()))
            .collect();
        assert_eq!(shown, edges, "{}: edges", flow.id);
    }
    assert_eq!(tour.pr_context.facts.flow_count as usize, tour.flows.len());
    assert_eq!(tour.pr_context.facts.step_count as usize, steps_total);
    // The hand-written seed keeps the in-file move of `build_request` (h19, h20) as a quiet move row for the UI
    // components; the server lists such a reorder under Unexplained (fixtures/pr-482/api/default/tour.json).
    let in_seed_move = |hunk: &HunkRec| {
        tour.quiet.moves.iter().any(|moved| {
            let lines = |side: &[u32], range: &contracts::common::FileRange, path: &str| {
                range.file == path
                    && side
                        .iter()
                        .any(|l| (range.range.start_line..=range.range.end_line).contains(l))
            };
            lines(&hunk.added(), &moved.to, &hunk.path)
                || lines(&hunk.removed(), &moved.from, hunk.base_path())
        })
    };
    let unexplained: BTreeSet<&str> = plan
        .unassigned
        .iter()
        .map(|u| hunks.iter().find(|h| h.id == u.hunk_id.0).expect("hunk"))
        .filter(|hunk| !in_seed_move(hunk))
        .map(|hunk| hunk.path.as_str())
        .collect();
    let shown: BTreeSet<&str> = tour
        .quiet
        .unexplained
        .iter()
        .map(|r| r.file.as_str())
        .collect();
    assert_eq!(shown, unexplained, "Unexplained lists the unassigned hunks");
    assert!(!tour.quiet.moves.is_empty() && !tour.quiet.not_analysed.is_empty());

    for (key, value) in default.iter().filter(|(k, _)| k.starts_with("lines/")) {
        let lines: LinesResponse = serde_json::from_value(value.clone()).expect("lines");
        let file =
            file_lines(lines.side, &lines.path).unwrap_or_else(|| panic!("{key}: no such file"));
        assert_eq!(
            key,
            &format!("lines/{}/{}", side_dir(lines.side), lines.path)
        );
        assert_eq!(lines.total_lines as usize, file.len(), "{key}");
        assert_eq!(
            lines.lines,
            file[lines.from as usize - 1..lines.to as usize].to_vec(),
            "{key}"
        );
        let mut changed: Vec<u32> = hunks
            .iter()
            .flat_map(|h| match lines.side {
                LineSide::Head if h.path == lines.path => h.added(),
                LineSide::Base if h.base_path() == lines.path => h.removed(),
                _ => Vec::new(),
            })
            .filter(|l| (lines.from..=lines.to).contains(l))
            .filter(|l| !WHITESPACE_ONLY.contains(&(lines.path.as_str(), lines.side, *l)))
            .collect();
        changed.sort_unstable();
        assert_eq!(lines.changed, changed, "{key}: changed lines");
    }
}

/// Changed lines of the fixture whose only change is whitespace (`let http = …` re-indented into the new `loop`): the
/// lines route shows them as unchanged code (the server's lexer pairs them; `crates/server/tests/slice.rs` checks it).
const WHITESPACE_ONLY: [(&str, LineSide, u32); 2] = [
    ("src/rpc/client.rs", LineSide::Base, 101),
    ("src/rpc/client.rs", LineSide::Head, 95),
];

/// The percentage rule `wording_violation` implements: digits or the `{}` slot, one optional whitespace, `%`.
const PERCENT_RULE: &str = r"(?:\d+|\{\})\s?%";

/// The wording rules of brief §1 (`words`, `percent`) from `scripts/wording-rules.json`, the file
/// `scripts/check-wording.mjs` and the e2e checks read; panics if the file or a key is missing.
fn wording_rules() -> (String, String) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/wording-rules.json");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let rules: Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let rule = |key: &str| {
        rules[key]
            .as_str()
            .filter(|rule| !rule.is_empty())
            .unwrap_or_else(|| panic!("{}: no non-empty string `{key}`", path.display()))
            .to_owned()
    };
    (rule("words"), rule("percent"))
}

/// The banned phrases of the `words` rule (matched case-insensitively, as the `i` flag of the script):
/// alternatives of words separated by `\s+`, with optional `\b` at either end.
fn banned_phrases(words: &str) -> Vec<(Vec<String>, bool, bool)> {
    let phrases: Vec<_> = words
        .split('|')
        .map(|alt| {
            let start = alt.starts_with("\\b");
            let end = alt.ends_with("\\b");
            let core = alt.trim_start_matches("\\b").trim_end_matches("\\b");
            assert!(
                !core.is_empty()
                    && core
                        .chars()
                        .all(|c| c.is_ascii_alphabetic() || "\\s+".contains(c)),
                "unsupported regex part {alt}"
            );
            (core.split("\\s+").map(str::to_owned).collect(), start, end)
        })
        .collect();
    assert!(!phrases.is_empty(), "no banned phrases");
    phrases
}

fn word_char(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// Banned wording or a percentage in `text`.
fn wording_violation(text: &str, phrases: &[(Vec<String>, bool, bool)]) -> Option<String> {
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    // PERCENT_RULE: a digit or `{}` right before `%`, or before one whitespace and `%`.
    let number_ends_at = |end: usize| {
        lower[..end].last().is_some_and(char::is_ascii_digit) || lower[..end].ends_with(&['{', '}'])
    };
    for (i, c) in lower.iter().enumerate() {
        if *c == '%'
            && (number_ends_at(i)
                || (i >= 1 && lower[i - 1].is_whitespace() && number_ends_at(i - 1)))
        {
            return Some("percentage".to_owned());
        }
    }
    for (words, start, end) in phrases {
        for i in 0..lower.len() {
            if *start && word_char(i.checked_sub(1).map(|p| lower[p])) {
                continue;
            }
            let mut pos = i;
            let mut ok = true;
            for (k, word) in words.iter().enumerate() {
                if k > 0 {
                    let gap = lower[pos..]
                        .iter()
                        .take_while(|c| c.is_whitespace())
                        .count();
                    if gap == 0 {
                        ok = false;
                        break;
                    }
                    pos += gap;
                }
                let w: Vec<char> = word.chars().collect();
                if lower.get(pos..pos + w.len()) != Some(&w[..]) {
                    ok = false;
                    break;
                }
                pos += w.len();
            }
            if ok && !(*end && word_char(lower.get(pos).copied())) {
                return Some(words.join(" "));
            }
        }
    }
    None
}

/// String values of a JSON document (property names are code, as in `scripts/check-wording.mjs`).
fn strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => out.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| strings(item, out)),
        Value::Object(map) => map.values().for_each(|item| strings(item, out)),
        _ => {}
    }
}

#[test]
fn fixture_text_follows_the_wording_rules_and_holds_no_secrets() {
    let (words, percent) = wording_rules();
    assert_eq!(
        percent, PERCENT_RULE,
        "the percentage rule changed; update wording_violation"
    );
    let phrases = banned_phrases(&words);
    for caught in [
        "No  risk here",
        "Nothing to review.",
        "This was Verified.",
        "up to 50 % more",
        "up to 50% more",
        "{}% of lines",
    ] {
        assert!(wording_violation(caught, &phrases).is_some(), "{caught}");
    }
    assert_eq!(
        wording_violation("noisy, unverified retries", &phrases),
        None
    );
    assert!(
        wording_violation("noisy, unverified, no risks", &phrases).as_deref() == Some("no risk")
    );
    // The dashboard fixtures (`fixtures/dashboard/`, v1.1) follow the same rules.
    let dashboard = root().join("../dashboard");
    for (dir, file) in files_below(&root())
        .into_iter()
        .map(|file| (root(), file))
        .chain(
            files_below(&dashboard)
                .into_iter()
                .map(|file| (dashboard.clone(), file)),
        )
    {
        let text = fs::read_to_string(dir.join(&file))
            .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let name = file.display().to_string();
        let mut texts = Vec::new();
        if name.ends_with(".json") {
            strings(
                &serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}")),
                &mut texts,
            );
        } else if name.ends_with(".jsonl") {
            for line in text.lines() {
                strings(
                    &serde_json::from_str(line).unwrap_or_else(|e| panic!("{name}: {e}")),
                    &mut texts,
                );
            }
        } else {
            texts.push(text.clone());
        }
        if let Some(found) = texts.iter().find_map(|t| wording_violation(t, &phrases)) {
            panic!("{name}: banned wording ({found})");
        }
        for (n, line) in text.lines().enumerate() {
            let at = format!("{name}:{}", n + 1);
            for secret in [
                "ghp_",
                "gho_",
                "ghu_",
                "ghs_",
                "ghr_",
                "github_pat_",
                "eyJ",
                "BEGIN RSA",
                "BEGIN PRIVATE",
            ] {
                assert!(
                    !line.contains(secret),
                    "{at}: looks like a credential ({secret})"
                );
            }
            for (i, _) in line.match_indices('@') {
                let before = line[..i].chars().last();
                let domain: String = line[i + 1..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '-')
                    .collect();
                let email = word_char(before) && domain.contains('.') && !domain.starts_with('.');
                assert!(
                    !email || domain == "example.com" || domain.ends_with(".example.com"),
                    "{at}: e-mail address outside example.com"
                );
            }
        }
    }
}
