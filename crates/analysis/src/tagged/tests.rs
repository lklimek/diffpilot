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
            text.replace(decides, "decides: <<END\nraw"),
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
fn explicit_connection_prose_keeps_external_callers_and_conditions() {
    let text = connections_fixture(
        "",
        "called_by: Worker::run outside this flow.\nnext: Leave only after committing.\n",
    );
    let doc = parse(&text, &Catalog::default()).unwrap();
    assert_eq!(doc.steps[0].called_by, "Worker::run outside this flow.");
    assert_eq!(doc.steps[0].next, "Leave only after committing.");
    assert_eq!(doc.steps[1].called_by, "Step 1");
    assert_eq!(
        parse(
            &encode(&doc, &Catalog::default()).unwrap(),
            &Catalog::default()
        )
        .unwrap(),
        doc
    );
}
