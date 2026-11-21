use super::*;
use contracts::agent::analyse_flow::FlowAnalysisOutput;

#[test]
fn fixture_round_trips_and_reports_bytes() {
    let mut before = 0;
    let mut after = 0;
    for index in 1..=5 {
        let json = std::fs::read_to_string(format!(
            "{}/../../fixtures/pr-482/agent/flow-{index}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("fixture");
        let document: FlowAnalysisOutput = serde_json::from_str(&json).expect("document");
        let text = encode(&document, &Catalog::default()).expect("encode");
        assert_eq!(parse(&text, &Catalog::default()).expect("parse"), document);
        assert!(!text.contains("\"start_line\""));
        before += json.len();
        after += text.len();
    }
    println!("flow fixtures: JSON {before} bytes; tagged {after} bytes");
    assert!(after < before);
}

fn fixture() -> (FlowAnalysisOutput, String) {
    let document: FlowAnalysisOutput = serde_json::from_str(include_str!(
        "../../../../fixtures/pr-482/agent/flow-2.json"
    ))
    .unwrap();
    let text = encode(&document, &Catalog::default()).unwrap();
    (document, text)
}

#[test]
fn grammar_rejects_malformed_unknown_duplicate_and_missing_fields_with_lines() {
    let (_, text) = fixture();
    let decides = text
        .lines()
        .find(|line| line.starts_with("decides:"))
        .unwrap();
    let risk = text.lines().find(|line| line.starts_with("risk:")).unwrap();
    let heading = text
        .lines()
        .find(|line| line.starts_with("## step "))
        .unwrap();
    let cases = [
        (text.replace(decides, ""), "has no `decides:` line"),
        (
            text.replace(decides, &format!("{decides}\n{decides}")),
            "duplicate field `decides`",
        ),
        (
            text.replace(decides, "typo: value"),
            "has no `decides:` line",
        ),
        (
            text.replace(decides, &format!("{decides}\ntypo: value")),
            "unknown or misplaced field `typo`",
        ),
        (
            text.replace(heading, "## step s1 missing_hunk"),
            "unknown planned hunk",
        ),
        (text.replace("p1:head:", "missing:head:"), "unknown file id"),
        (text.replace(risk, "risk: enormous"), "unknown variant"),
        (format!("{text}\n### bad\n"), "unknown or misplaced section"),
        (
            text.replace(decides, "decides: <<UNCLOSED\nraw"),
            "missing heredoc terminator",
        ),
        (
            text.replace(heading, "## step s1 p1:head:0-1 modified"),
            "invalid inclusive range",
        ),
        (
            format!("{text}\n# flow another\n"),
            "unknown or misplaced section",
        ),
    ];
    for (bad, message) in cases {
        let error = parse(&bad, &Catalog::default()).unwrap_err();
        assert!(error.message.contains(message), "{message}: {error}");
        assert!(error.line > 0);
    }
    let bad = text.replace(risk, "risk: enormous");
    let error = parse(&bad, &Catalog::default()).unwrap_err();
    assert_eq!(
        error.line,
        bad.lines()
            .position(|line| line == "risk: enormous")
            .unwrap()
            + 1
    );
}

#[test]
fn raw_code_newlines_fences_and_reserved_prefixes_round_trip() {
    let (mut doc, _) = fixture();
    doc.introduction.example_before_after = "<<not_a_directive\n```rust\nfn main() { println!(\"x\\n\"); }\n```\nEND\n## step spoof\n\n".into();
    let text = encode(&doc, &Catalog::default()).unwrap();
    assert_eq!(parse(&text, &Catalog::default()).unwrap(), doc);
}

#[test]
fn planned_anchor_removes_known_fields_and_cannot_name_an_unplanned_hunk() {
    let (doc, _) = fixture();
    let step = &doc.steps[0];
    let mut catalog = Catalog::default();
    catalog.hunks.insert(
        "h1".into(),
        Anchor {
            file: step.file.clone(),
            side: "head".into(),
            start: step.start_line,
            end: step.end_line,
            kind: "modified".into(),
        },
    );
    let text = encode(&doc, &catalog).unwrap();
    assert!(text.contains("## step s1 h1"));
    assert_eq!(parse(&text, &catalog).unwrap(), doc);
    assert!(
        parse(&text.replace("## step s1 h1", "## step s1 h2"), &catalog)
            .unwrap_err()
            .message
            .contains("unknown planned hunk")
    );
}

#[test]
fn remaining_grammar_boundaries_have_actionable_diagnostics() {
    let (_, text) = fixture();
    let heading = text
        .lines()
        .find(|line| line.starts_with("## step "))
        .unwrap();
    let annotation = text
        .lines()
        .find(|line| line.starts_with("### annotation "))
        .unwrap();
    let header = text.lines().next().unwrap();
    let file = text
        .lines()
        .find(|line| line.starts_with("file p1:"))
        .unwrap();
    let cases = [
        (String::new(), "expected `# flow <id>`"),
        ("title: misplaced\n".into(), "expected `# flow <id>` first"),
        ("#\n".into(), "expected # flow"),
        ("# \n".into(), "empty section heading"),
        ("# flow\n".into(), "wrong number of arguments"),
        ("# other f1\n".into(), "expected `# flow <id>`"),
        (
            text.replace(header, &format!("{header}\ntitle: misplaced")),
            "only `file <id>: <path>`",
        ),
        (
            text.replace(file, "file p1: ../outside"),
            "invalid file declaration",
        ),
        (
            text.replace(file, "file p1: /outside"),
            "invalid file declaration",
        ),
        (
            text.replace(file, "file p1: ./file"),
            "invalid file declaration",
        ),
        (
            text.replace(file, "file p-1: file.rs"),
            "invalid file declaration",
        ),
        (
            text.replace(file, &format!("{file}\n{}", file.replace("p1:", "x1:"))),
            "duplicate or already known file declaration",
        ),
        (
            format!("{text}\n## introduction\n"),
            "duplicate introduction",
        ),
        (text.replace(heading, "## step"), "expected `## step"),
        (
            text.replace(heading, "## step s1 p1:wrong:1-2 modified"),
            "location must be",
        ),
        (
            text.replace(heading, "## step s1 p1:head:2-1 modified"),
            "invalid inclusive range",
        ),
        (
            text.replace(heading, "## step s1 p1:head:4294967296 modified"),
            "invalid inclusive range",
        ),
        (
            text.replace(heading, "## step s1 p1:head:1-2 unknown"),
            "unknown variant",
        ),
        (
            text.replace(heading, &format!("{heading}\nmoved_from: p1:head:1-2")),
            "moved_from must refer to base",
        ),
        (
            text.replace(annotation, &format!("{annotation}\nevidence: p1:head:1-2")),
            "duplicate field `evidence`",
        ),
        (
            text.replace(annotation, "### annotation bad fact 1"),
            "unknown variant",
        ),
        (
            text.replace(annotation, "### annotation note bad 1"),
            "unknown variant",
        ),
        (
            format!("{text}\n## impact s1 p1:base:1\nreason: Example\nbasis: fact\n"),
            "impact must refer to head",
        ),
        (
            format!("{text}\n## thread t1 bad\nreason: Example\n"),
            "unknown variant",
        ),
        (
            "# flow f1\n### assessment\n".into(),
            "assessment must follow its step",
        ),
        (
            "# flow f1\n### annotation note fact 1\n".into(),
            "annotation must follow its step",
        ),
        (
            "# flow f1\n## introduction\nproblem_before: <<bad-end\n".into(),
            "heredoc terminator must be an identifier",
        ),
        ("# flow f1\nnot a field\n".into(), "expected `key: value`"),
        ("x".repeat(MAX_BYTES + 1), "document exceeds 1 MiB"),
    ];
    for (bad, expected) in cases {
        let error = parse(&bad, &Catalog::default()).unwrap_err();
        assert!(
            error.message.contains(expected),
            "expected {expected:?}, got {error}"
        );
        assert!(error.line >= 1 && error.line <= bad.lines().count().max(1));
    }
    let duplicate = text.replace("## step s2 ", "## step s1 ");
    assert!(
        parse(&duplicate, &Catalog::default())
            .unwrap_err()
            .message
            .contains("duplicate step id")
    );
    let duplicate = text.replacen("### assessment","### assessment\nheadline: Extra\nrisk: low\nreason: Example\nbasis_note: Reading\nnot_checked: Runtime\norigin: introduced\nblocking: no_action_proposed\n### assessment",1);
    assert!(
        parse(&duplicate, &Catalog::default())
            .unwrap_err()
            .message
            .contains("duplicate assessment")
    );
    let evidence = text
        .lines()
        .find(|line| line.starts_with("evidence:"))
        .unwrap();
    let bad = text.replacen(evidence, "evidence: p1:head:1-2", 1);
    assert!(
        parse(&bad, &Catalog::default())
            .unwrap_err()
            .message
            .contains("evidence refers to one line")
    );
}

#[test]
fn raw_multiline_values_preserve_carriage_returns_without_json_escaping() {
    let (mut doc, _) = fixture();
    for raw in [
        "first\r\nsecond\r\n",
        "trailing carriage return\r",
        "END\r\nEND_\nlast",
    ] {
        doc.introduction.example_before_after = raw.into();
        let text = encode(&doc, &Catalog::default()).unwrap();
        assert_eq!(parse(&text, &Catalog::default()).unwrap(), doc);
    }
}

#[test]
fn concise_low_assessment_round_trips_and_is_not_padded() {
    let (mut doc, _) = fixture();
    let assessment = doc.steps[0].assessment.as_mut().unwrap();
    assessment.risk = contracts::common::RiskBand::Low;
    assessment.headline = None;
    assessment.basis_note = None;
    assessment.not_checked = None;
    assessment.origin = None;
    assessment.blocking = None;
    assessment.smallest_correction = None;
    let text = encode(&doc, &Catalog::default()).unwrap();
    assert!(text.contains("risk: low\nreason: "));
    assert!(!text.contains("basis_note: \n"));
    assert_eq!(parse(&text, &Catalog::default()).unwrap(), doc);
}

#[test]
fn assessment_still_requires_risk_and_reason() {
    let (_, text) = fixture();
    let reason = text
        .lines()
        .find(|line| line.starts_with("reason:") && !line.contains("thread"))
        .unwrap();
    let error = parse(&text.replacen(reason, "", 1), &Catalog::default()).unwrap_err();
    assert!(error.message.contains("has no `reason:` line"), "{error}");
}

fn connections_fixture(edges: &str, extra: &str) -> String {
    format!(
        "# flow f1\nfile p1: src/lib.rs\n\n## introduction\nproblem_before: Before.\nchange: After.\nexample_before_after: Example.\n\n## step entry p1:head:1-2 modified\ntitle: Enter\ndecides: Read.\nwhy: Dispatch.\n{extra}\n### assessment\nrisk: low\nreason: Limited to dispatch.\n\n## step exit p1:head:3-4 modified\ntitle: Leave\ndecides: Return.\nwhy: Finish.\n\n### assessment\nrisk: low\nreason: Limited to return.\n{edges}"
    )
}

#[test]
fn omitted_connections_follow_reading_order_and_keep_boundaries_empty() {
    let text = connections_fixture("", "");
    let doc = parse(&text, &Catalog::default()).expect("connections are optional");
    assert_eq!(doc.edges.len(), 1);
    assert_eq!(doc.edges[0].from_step_id, "entry");
    assert_eq!(doc.edges[0].to_step_id, "exit");
    assert_eq!(doc.steps[0].called_by, "");
    assert_eq!(doc.steps[0].next, "Step 2");
    assert_eq!(doc.steps[1].called_by, "Step 1");
    assert_eq!(doc.steps[1].next, "");
    let encoded = encode(&doc, &Catalog::default()).unwrap();
    assert!(!encoded.contains("called_by:"));
    assert!(!encoded.contains("next:"));
    assert!(!encoded.contains("## edge"));
    assert_eq!(parse(&encoded, &Catalog::default()).unwrap(), doc);
}

#[test]
fn explicit_edges_derive_connections_including_labels_and_loops() {
    let text = connections_fixture(
        "\n## edge entry exit\nlabel: ready\n\n## edge exit entry\nlabel: retry\n",
        "",
    );
    let doc = parse(&text, &Catalog::default()).unwrap();
    assert_eq!(doc.edges.len(), 2);
    assert_eq!(doc.steps[0].called_by, "Step 2 (retry)");
    assert_eq!(doc.steps[0].next, "Step 2 (ready)");
    assert_eq!(doc.steps[1].called_by, "Step 1 (ready)");
    assert_eq!(doc.steps[1].next, "Step 1 (retry)");
    assert_eq!(
        parse(
            &encode(&doc, &Catalog::default()).unwrap(),
            &Catalog::default()
        )
        .unwrap(),
        doc
    );
}

#[test]
fn connection_prose_that_only_restates_the_derived_target_is_not_doubled() {
    let text = connections_fixture("", "next: Step 2\n");
    let doc = parse(&text, &Catalog::default()).unwrap();
    assert_eq!(doc.steps[0].next, "Step 2");
    let encoded = encode(&doc, &Catalog::default()).unwrap();
    assert_eq!(parse(&encoded, &Catalog::default()).unwrap(), doc);
}

#[test]
fn explicit_connection_prose_adds_to_the_derived_step_targets() {
    let text = connections_fixture(
        "",
        "called_by: Worker::run outside this flow.\nnext: Leave only after committing.\n",
    );
    let doc = parse(&text, &Catalog::default()).unwrap();
    assert_eq!(doc.steps[0].called_by, "Worker::run outside this flow.");
    assert_eq!(doc.steps[0].next, "Step 2; Leave only after committing.");
    assert_eq!(doc.steps[1].called_by, "Step 1");
    let encoded = encode(&doc, &Catalog::default()).unwrap();
    assert!(
        encoded.contains("\nnext: Leave only after committing.\n"),
        "{encoded}"
    );
    assert_eq!(parse(&encoded, &Catalog::default()).unwrap(), doc);
}

/// A catalogue with the planned hunk `h1` in `p1`.
fn hunk_catalog() -> Catalog {
    let mut catalog = Catalog::default();
    catalog.files.insert("p1".into(), "src/queue.rs".into());
    catalog.hunks.insert(
        "h1".into(),
        Anchor {
            file: "src/queue.rs".into(),
            side: "head".into(),
            start: 1,
            end: 1,
            kind: "modified".into(),
        },
    );
    catalog
}

/// A grammar-valid document: one step `id` on `h1` with `annotations` minimal annotations.
fn one_step_document(id: &str, annotations: usize) -> String {
    let mut text = format!(
        "# flow f1\n\n## introduction\nproblem_before: x\nchange: x\nexample_before_after: x\n\n\
         ## step {id} h1\ntitle: x\ncalled_by: x\ndecides: x\nnext: x\nwhy: x\n\n\
         ### assessment\nrisk: low\nreason: x\n"
    );
    for _ in 0..annotations {
        text.push_str("### annotation note inference 1-1\ntext: x\n");
    }
    text
}

#[test]
fn ids_heading_words_field_names_and_sections_are_bounded_with_line_numbers() {
    let catalog = hunk_catalog();
    let valid = one_step_document("s1", 1);
    parse(&valid, &catalog).expect("the baseline document is valid");
    parse(&one_step_document(&"s".repeat(64), 1), &catalog).expect("64 characters are an id");
    parse(&one_step_document("S_1-a", 1), &catalog).expect("letters, digits, `_` and `-`");
    let long = "s".repeat(65);
    let cases = [
        (
            valid.replace("## step s1 ", &format!("## step {long} ")),
            "## step ",
            "step id",
        ),
        (
            valid.replace("## step s1 ", "## step s.1 "),
            "## step ",
            "step id",
        ),
        (
            valid.replace("# flow f1", &format!("# flow {long}")),
            "# flow ",
            "flow id",
        ),
        (format!("{valid}## edge s1 {long}\n"), "## edge ", "step id"),
        (format!("{valid}## edge s/1 s1\n"), "## edge ", "step id"),
        (
            format!("{valid}## impact {long} p1:head:1-1\nreason: x\nbasis: fact\n"),
            "## impact ",
            "step id",
        ),
        (
            format!("{valid}## repetition {long}\ndescription: x\nsites: p1:head:1-1\n"),
            "## repetition ",
            "step id",
        ),
        (
            format!("{valid}## thread {long} stale\nreason: x\n"),
            "## thread ",
            "thread alias",
        ),
        (
            valid.replace(
                "# flow f1",
                &format!("# flow f1\nfile {long}: src/other.rs"),
            ),
            "file ",
            "invalid file declaration",
        ),
        (
            valid.replace("## step s1 h1", &format!("## step s1 {}", "h".repeat(129))),
            "## step ",
            "heading word",
        ),
        (
            valid.replace("title: x", &format!("{}: x", "k".repeat(81))),
            "kkkk",
            "field name",
        ),
    ];
    for (bad, at, message) in cases {
        let error = parse(&bad, &catalog).unwrap_err();
        assert!(error.message.contains(message), "{message}: {error}");
        assert!(
            error.message.len() < 200,
            "the message does not echo the value"
        );
        assert_eq!(
            error.line,
            bad.lines().position(|line| line.starts_with(at)).unwrap() + 1,
            "{message}"
        );
    }
    // Header, introduction, step and assessment are four sections: one annotation too many.
    let many = one_step_document("s1", 2_000 - 3);
    let error = parse(&many, &catalog).unwrap_err();
    assert!(error.message.contains("more than 2000 sections"), "{error}");
    assert_eq!(error.line, many.lines().count() - 1);
    parse(&one_step_document("s1", 2_000 - 4), &catalog).expect("2000 sections are allowed");
}

/// The line map built for verifier diagnostics repeats the step id in every key: its size must stay
/// proportional to the document, whatever the document holds.
#[test]
fn diagnostic_line_map_stays_linear_in_the_document_size() {
    let key_bytes = |text: &str| {
        codec::source_lines(text).map(|map| map.keys().map(String::len).sum::<usize>())
    };
    // One huge id repeated into the keys of every annotation and field (scaled down from the 1 MiB case).
    let hostile = one_step_document(&"s".repeat(100_000), 100);
    if let Ok(bytes) = key_bytes(&hostile) {
        assert!(bytes <= 16 * hostile.len(), "{bytes} key bytes");
    }
    // The largest accepted shape: longest id, section count at its cap.
    let largest = one_step_document(&"s".repeat(64), 2_000 - 4);
    parse(&largest, &hunk_catalog()).expect("valid");
    let bytes = key_bytes(&largest).expect("valid");
    assert!(bytes <= 16 * largest.len(), "{bytes} key bytes");
}

#[test]
fn flags_round_trip_and_need_a_red_or_yellow_level_a_scenario_and_what_was_not_checked() {
    let (doc, text) = fixture();
    assert!(
        doc.steps
            .iter()
            .flat_map(|s| &s.annotations)
            .any(|a| a.flag.is_some())
    );
    let heading = text
        .lines()
        .find(|line| line.starts_with("### annotation flag "))
        .unwrap();
    let block = |level: &str| {
        format!(
            "level: {level}\ntext: Potential stall:\nscenario: <<END\n1. A peer is slow.\n\n- The request times out.\nEND\nnot_checked: Not run.\n"
        )
    };
    let start = text.find(heading).unwrap() + heading.len() + 1;
    let end = text[start..].find("evidence:").unwrap() + start;
    let with = |fields: &str| format!("{}{fields}{}", &text[..start], &text[end..]);
    let parsed = parse(&with(&block("red")), &Catalog::default()).unwrap();
    let flag = parsed
        .steps
        .iter()
        .flat_map(|s| &s.annotations)
        .find_map(|a| {
            a.flag
                .as_ref()
                .filter(|_| a.text.as_deref() == Some("Potential stall:"))
        })
        .unwrap();
    // List markers and blank lines are layout, not content: the reader numbers the steps.
    assert_eq!(flag.scenario, ["A peer is slow.", "The request times out."]);
    assert_eq!(flag.not_checked, "Not run.");
    let cases = [
        (
            with(&block("unknown")),
            "a flag needs `level: red` or `level: yellow`",
        ),
        (
            with(&block("red").replace("level: red\n", "")),
            "a flag needs `level: red`",
        ),
        (
            with(&block("red").replace("not_checked: Not run.\n", "")),
            "has no `not_checked:` line",
        ),
        (
            text.replace(heading, &heading.replace(" flag ", " question ")),
            "`question` is now `flag`",
        ),
        (
            text.replacen("was: ", "level: red\nwas: ", 1),
            "`level` belongs only to a flag",
        ),
    ];
    for (bad, expected) in cases {
        let error = parse(&bad, &Catalog::default()).unwrap_err();
        assert!(
            error.message.contains(expected),
            "expected {expected:?}, got {error}"
        );
    }
}

#[test]
fn incidental_changes_round_trip_by_location_or_planned_hunk() {
    let (mut doc, _) = fixture();
    let step = doc.steps[0].clone();
    doc.incidentals = vec![contracts::agent::analyse_flow::AgentIncidental {
        file: step.file.clone(),
        side: step.side,
        start_line: step.start_line,
        end_line: step.start_line,
        text: "Makes `send` public for the new caller.".into(),
    }];
    let text = encode(&doc, &Catalog::default()).unwrap();
    let heading = text
        .lines()
        .find(|line| line.starts_with("## incidental "))
        .expect("incidental section");
    assert!(heading.ends_with(&format!(":head:{0}-{0}", step.start_line)));
    assert!(text.contains(&format!("{heading}\ntext: Makes `send` public")));
    assert_eq!(parse(&text, &Catalog::default()).unwrap(), doc);
    // A planned hunk names its whole planned span.
    let mut catalog = Catalog::default();
    catalog.hunks.insert(
        "h9".into(),
        Anchor {
            file: step.file.clone(),
            side: "head".into(),
            start: step.start_line,
            end: step.start_line,
            kind: "modified".into(),
        },
    );
    let by_hunk = text.replace(heading, "## incidental h9");
    assert_eq!(parse(&by_hunk, &catalog).unwrap(), doc);
}

#[test]
fn incidental_changes_carry_only_a_text_never_a_flag_or_assessment() {
    let (_, text) = fixture();
    let at = "\n## incidental p1:head:1-1\n";
    let cases = [
        (format!("{text}{at}"), "has no `text:` line"),
        (
            format!("{text}{at}text: x\nlevel: red\n"),
            "unknown or misplaced field `level`",
        ),
        (
            format!("{text}{at}text: x\n\n### annotation flag inference 1-1\nlevel: red\n"),
            "annotation must follow its step",
        ),
        (
            format!("{text}{at}text: x\n\n### assessment\nrisk: high\nreason: y\n"),
            "assessment must follow its step",
        ),
        (
            format!("{text}\n## incidental\ntext: x\n"),
            "wrong number of arguments",
        ),
        (
            format!("{text}\n## incidental h404\ntext: x\n"),
            "unknown planned hunk",
        ),
    ];
    for (bad, message) in cases {
        let error = parse(&bad, &Catalog::default()).unwrap_err();
        assert!(error.message.contains(message), "{message}: {error}");
    }
    assert_eq!(
        VERSION, 3,
        "the grammar gained a section: checkers of older images refuse"
    );
}
