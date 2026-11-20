//! Whole-analysis tests of [`super::analysis`] over the fixture pull request `acme/node-client#482`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use contracts::agent::analyse_flow::FlowAnalysisOutput;
use contracts::agent::ask::AskOutput;
use contracts::agent::plan::PlanOutput;
use contracts::agent::pr_context::{FitQuestion, PrContextOutput, RelatedWork};
use contracts::common::{
    Classification, Evidence, FileRanges, FitTopic, LineRange, LineSide, RelatedWorkKind,
};
use contracts::ids::{CommitSha, FlowId, HunkId};
use serde::de::DeserializeOwned;

use super::*;
use crate::diff::{FileDiff, FileStatus, Hunk, LineSpan, diff};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482")
}

fn read<T: DeserializeOwned>(path: &str) -> T {
    let text = std::fs::read_to_string(fixture_dir().join(path)).expect(path);
    serde_json::from_str(&text).expect(path)
}

fn id(flow: &str) -> FlowId {
    FlowId(flow.to_owned())
}

/// The fixture repository, its diff split into listed and quiet hunks (`repo/hunks.json` `prepass`), and the
/// agent outputs of `agent/`.
struct Fixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    base: CommitSha,
    head: CommitSha,
    files: Vec<FileDiff>,
    listed: Vec<FileDiff>,
    quiet: Vec<FileRanges>,
    plan: PlanOutput,
    pr_context: PrContextOutput,
    flows: Vec<FlowAnalysisOutput>,
    aliases: BTreeMap<FlowId, Vec<String>>,
    moves: MovesReport,
    stored: bool,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let repo = dir.path().join("repo");
        let built = Command::new(fixture_dir().join("repo/build-repo.sh"))
            .arg(&repo)
            .output()
            .expect("run build-repo.sh");
        assert!(built.status.success(), "{built:?}");
        let commits: serde_json::Value = read("repo/commits.json");
        let sha = |key: &str| CommitSha(commits[key].as_str().expect("sha").to_owned());
        let (base, head) = (sha("base"), sha("head"));
        let files = diff(&repo, &base, &head).expect("diff");
        let prepass: Vec<serde_json::Value> = read("repo/hunks.json");
        let agent = |hunk: &Hunk| {
            prepass
                .iter()
                .any(|entry| entry["id"] == hunk.id.0.as_str() && entry["prepass"] == "agent")
        };
        let mut listed = files.clone();
        for file in &mut listed {
            file.hunks.retain(agent);
        }
        let mut quiet = Vec::new();
        for file in &files {
            for hunk in file.hunks.iter().filter(|hunk| !agent(hunk)) {
                for (side, lines) in [
                    (LineSide::Head, &hunk.added),
                    (LineSide::Base, &hunk.removed),
                ] {
                    if let (Some(path), false) = (file.path_on(side), lines.is_empty()) {
                        quiet.push(FileRanges {
                            file: path.to_owned(),
                            side,
                            ranges: crate::lines::ranges(lines),
                        });
                    }
                }
            }
        }
        let flows = (1..=5)
            .map(|n| read(&format!("agent/flow-{n}.json")))
            .collect();
        Self {
            _dir: dir,
            repo,
            base,
            head,
            files,
            listed,
            quiet,
            plan: read("agent/plan.json"),
            pr_context: read("agent/pr_context.json"),
            flows,
            aliases: BTreeMap::from([(id("f1"), vec!["t1".to_owned(), "t2".to_owned()])]),
            moves: MovesReport::default(),
            stored: false,
        }
    }

    /// Line facts of every file the outputs and the planned flows name, read from the fixture repository.
    fn facts(&self) -> LineFacts {
        let listed = DiffIndex::new(&self.listed).expect("listed");
        let mut wanted = pr_context_files(&self.pr_context);
        let flow_lines = match plan(&self.plan, &listed) {
            Ok(verified) => verified.flow_lines,
            Err(_) => BTreeMap::new(),
        };
        for output in &self.flows {
            let lines = flow_lines
                .get(&output.flow_id)
                .map_or(&[][..], Vec::as_slice);
            wanted.extend(flow_files(output, lines));
        }
        LineFacts::load(&self.repo, &self.base, &self.head, &wanted).expect("line facts")
    }

    fn verdict(&self, facts: &LineFacts) -> AnalysisVerdict {
        let diff = DiffIndex::new(&self.files).expect("diff");
        let listed = DiffIndex::new(&self.listed).expect("listed");
        analysis(AnalysisInput {
            diff: &diff,
            listed: &listed,
            quiet: &self.quiet,
            facts: &|_| Ok(facts.clone()),
            moves: &self.moves,
            plan: &self.plan,
            pr_context: &self.pr_context,
            flows: &self.flows,
            thread_aliases: &self.aliases,
            stored: self.stored,
        })
        .expect("verdict")
    }
}

fn file_ranges(file: &str, side: LineSide, ranges: &[(u32, u32)]) -> FileRanges {
    FileRanges {
        file: file.to_owned(),
        side,
        ranges: ranges
            .iter()
            .map(|&(start_line, end_line)| LineRange {
                start_line,
                end_line,
            })
            .collect(),
    }
}

fn flow_ids(flows: &[VerifiedFlow]) -> Vec<&str> {
    flows.iter().map(|flow| flow.flow_id.0.as_str()).collect()
}

#[test]
fn fixture_pr_482_output_is_accepted_and_ordered() {
    let fixture = Fixture::new();
    let verdict = fixture.verdict(&fixture.facts());
    assert_eq!(verdict.repairs, [], "the fixture output is consistent");
    assert_eq!(verdict.warnings, [], "the fixture wording is clean");
    let tour = verdict.tour.expect("tour");
    assert_eq!(flow_ids(&tour.flows), ["f1", "f2", "f3", "f4", "f5"]);
    assert!(tour.unanalysed.is_empty());
    assert_eq!(
        tour.coverage.unexplained,
        [
            file_ranges("src/rpc/client.rs", LineSide::Head, &[(112, 152)]),
            file_ranges("src/rpc/client.rs", LineSide::Base, &[(173, 214)]),
            file_ranges("src/rpc/metrics.rs", LineSide::Head, &[(40, 46)]),
        ],
        "the plan's unassigned hunks h19, h20 and h22 are Unexplained"
    );
    assert_eq!(tour.coverage.double_covered, []);
    let steps: usize = tour.flows.iter().map(|flow| flow.step_ids.len()).sum();
    assert_eq!(steps, 17);
    assert!(!verdict.moves_incomplete);
}

#[test]
fn stored_outputs_read_under_stricter_rules_leave_new_lines_unexplained() {
    let mut fixture = Fixture::new();
    let facts = fixture.facts();
    // Written under earlier rules: the plan left out h19/h20 (then quiet), flow f1 has no step on lines 35 and 42.
    fixture
        .plan
        .unassigned
        .retain(|hunk| !["h19", "h20"].contains(&hunk.hunk_id.0.as_str()));
    let f1 = &mut fixture.flows[0];
    f1.steps.retain(|step| step.id != "s9");
    f1.edges
        .retain(|edge| edge.from_step_id != "s9" && edge.to_step_id != "s9");

    let fresh = fixture.verdict(&facts);
    assert!(fresh.tour.is_none(), "a new plan stays strict");

    fixture.stored = true;
    let verdict = fixture.verdict(&facts);
    assert_eq!(
        verdict.repairs,
        [],
        "a stored, accepted output is not rejected for coverage"
    );
    let tour = verdict.tour.expect("tour");
    assert_eq!(flow_ids(&tour.flows), ["f1", "f2", "f3", "f4", "f5"]);
    assert!(tour.unanalysed.is_empty());
    assert_eq!(
        tour.coverage.unexplained,
        [
            file_ranges(
                "src/rpc/client.rs",
                LineSide::Head,
                &[(35, 35), (42, 42), (112, 152)]
            ),
            file_ranges("src/rpc/client.rs", LineSide::Base, &[(41, 41), (173, 214)]),
            file_ranges("src/rpc/metrics.rs", LineSide::Head, &[(40, 46)]),
        ],
        "h19/h20 and the lines step s9 covered (head 35, 42, removed base 41)"
    );
}

#[test]
fn incomplete_move_detection_is_passed_through_not_rejected() {
    let mut fixture = Fixture::new();
    fixture.moves.incomplete = true;
    let verdict = fixture.verdict(&fixture.facts());
    assert!(verdict.moves_incomplete, "the pipeline records it");
    assert_eq!(
        verdict.repairs,
        [],
        "an exhausted move budget is no rejection"
    );
    assert!(verdict.tour.is_some());
}

/// Real-Codex smoke finding 8: PR 482's output replayed against another pull request was accepted because
/// only shape and id existence were checked.
#[test]
fn foreign_pr_output_is_rejected() {
    let fixture = Fixture::new();
    let hunk = |n: u32| Hunk {
        id: HunkId(format!("h{n}")),
        base: LineSpan {
            start: n * 10,
            len: 1,
        },
        head: LineSpan {
            start: n * 10,
            len: 2,
        },
        added: vec![n * 10, n * 10 + 1],
        removed: vec![n * 10],
    };
    let foreign = |numbers: &[u32]| {
        vec![FileDiff {
            path: "app/server.go".to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            hunks: numbers.iter().copied().map(hunk).collect(),
        }]
    };
    let mut facts = LineFacts::default();
    for side in [LineSide::Head, LineSide::Base] {
        let file = FileFacts {
            line_count: 400,
            blank_lines: Vec::new(),
        };
        facts.insert(side, "app/server.go", file);
    }
    let run = |files: &[FileDiff]| {
        let diff = DiffIndex::new(files).expect("diff");
        analysis(AnalysisInput {
            diff: &diff,
            listed: &diff,
            quiet: &[],
            facts: &|_| Ok(facts.clone()),
            moves: &MovesReport::default(),
            plan: &fixture.plan,
            pr_context: &fixture.pr_context,
            flows: &fixture.flows,
            thread_aliases: &fixture.aliases,
            stored: false,
        })
        .expect("verdict")
    };

    // Another pull request with 30 hunks: every id of the plan exists, most hunks are in no flow.
    let verdict = run(&foreign(&(1..=30).collect::<Vec<_>>()));
    assert!(verdict.tour.is_none());
    assert_eq!(verdict.repairs[0].output, OutputRef::Plan);
    let unassigned = verdict.repairs[0].issues.iter();
    let unassigned = unassigned.filter(|issue| issue.kind == VerifyIssueKind::LineUnassigned);
    assert_eq!(unassigned.count(), 17, "{:#?}", verdict.repairs[0]);
    assert_eq!(
        verdict.repairs[1].output,
        OutputRef::PrContext,
        "its evidence is not in this tree"
    );

    // Worst case: exactly the plan's hunk ids. The plan fits, but no flow output does.
    let verdict = run(&foreign(&[
        11, 12, 13, 14, 15, 16, 18, 19, 20, 21, 22, 23, 24,
    ]));
    let tour = verdict
        .tour
        .expect("the plan's whole-hunk selections fit any hunk list");
    assert!(tour.flows.is_empty());
    assert_eq!(tour.unanalysed, ["f1", "f2", "f3", "f4", "f5"].map(id));
    let rejected: Vec<&OutputRef> = verdict
        .repairs
        .iter()
        .map(|repair| &repair.output)
        .collect();
    assert_eq!(
        rejected[1..],
        ["f1", "f2", "f3", "f4", "f5"]
            .map(|f| OutputRef::Flow(id(f)))
            .each_ref()
    );
    for repair in &verdict.repairs[1..] {
        let kinds: Vec<VerifyIssueKind> = repair.issues.iter().map(|issue| issue.kind).collect();
        assert!(kinds.contains(&VerifyIssueKind::UnknownFile), "{repair:#?}");
        assert!(
            kinds.contains(&VerifyIssueKind::ChangedLineUncovered),
            "{repair:#?}"
        );
    }
    let unexplained_lines: u32 = tour
        .coverage
        .unexplained
        .iter()
        .flat_map(|ranges| &ranges.ranges)
        .map(|range| range.end_line - range.start_line + 1)
        .sum();
    assert_eq!(
        unexplained_lines,
        13 * 3,
        "every changed line is Unexplained"
    );
}

#[test]
fn failed_flows_become_unexplained_and_quiet_rows_never_above_a_rated_flow() {
    let mut fixture = Fixture::new();
    let classify = |plan: &mut PlanOutput, flow: &str, classification| {
        if let Some(planned) = plan.flows.iter_mut().find(|planned| planned.id == id(flow)) {
            planned.classification = classification;
        }
    };
    // f1 and f4 become LLM-classified refactors; test-only flows are rejected separately.
    classify(&mut fixture.plan, "f1", Classification::RefactorPure);
    classify(&mut fixture.plan, "f4", Classification::RefactorPure);
    let facts = fixture.facts();
    // f2's output names a step that does not exist; outputs for an unplanned and a repeated flow.
    fixture.flows[1].edges[0].to_step_id = "s9".to_owned();
    let mut unplanned = fixture.flows[2].clone();
    unplanned.flow_id = id("f9");
    fixture.flows.push(unplanned);
    fixture.flows.push(fixture.flows[2].clone());

    let verdict = fixture.verdict(&facts);
    let rejected: Vec<(&OutputRef, Vec<VerifyIssueKind>)> = verdict
        .repairs
        .iter()
        .map(|repair| {
            (
                &repair.output,
                repair.issues.iter().map(|i| i.kind).collect(),
            )
        })
        .collect();
    assert_eq!(
        rejected,
        [
            (
                &OutputRef::Flow(id("f2")),
                vec![VerifyIssueKind::DanglingEdge]
            ),
            (
                &OutputRef::Flow(id("f9")),
                vec![VerifyIssueKind::UnknownReference]
            ),
            (
                &OutputRef::Flow(id("f3")),
                vec![VerifyIssueKind::DuplicateId]
            ),
        ]
    );
    let tour = verdict.tour.expect("tour");
    assert_eq!(flow_ids(&tour.flows), ["f3", "f5", "f1", "f4"]);
    let f1 = tour
        .flows
        .iter()
        .find(|f| f.flow_id == id("f1"))
        .expect("f1");
    assert!(
        f1.output.steps.iter().any(|step| step.assessment.is_some()),
        "a rated flow, still listed before every quiet row"
    );
    assert_eq!(tour.unanalysed, [id("f2")]);
    assert_eq!(
        tour.coverage.unexplained,
        [
            file_ranges("src/config.rs", LineSide::Head, &[(23, 23)]),
            file_ranges("src/config.rs", LineSide::Base, &[(23, 23)]),
            file_ranges("src/rpc/client.rs", LineSide::Head, &[(112, 152)]),
            file_ranges("src/rpc/client.rs", LineSide::Base, &[(173, 214)]),
            file_ranges("src/rpc/metrics.rs", LineSide::Head, &[(40, 46)]),
        ],
        "the lines of the failed flow f2 and the unassigned hunk"
    );
}

#[test]
fn pr_context_rejects_empty_texts_and_evidence_outside_the_tree() {
    let mut facts = LineFacts::default();
    let file = FileFacts {
        line_count: 10,
        blank_lines: Vec::new(),
    };
    facts.insert(LineSide::Head, "src/a.rs", file);
    let evidence = |file: &str, line| Evidence {
        file: file.to_owned(),
        side: LineSide::Head,
        line,
    };
    let output = PrContextOutput {
        problem: "Timeouts fail payments.".to_owned(),
        why_needed: " ".to_owned(),
        what_it_gives: "Retries.".to_owned(),
        related_work: vec![RelatedWork {
            reference: "acme/node-client#470".to_owned(),
            kind: RelatedWorkKind::CompetingSolution,
            overlap: String::new(),
        }],
        fit_questions: vec![FitQuestion {
            question: "Is the client the right layer?".to_owned(),
            topic: FitTopic::LayerOrRepository,
            evidence: vec![
                evidence("src/a.rs", 3),
                evidence("src/a.rs", 11),
                evidence("../x", 1),
            ],
        }],
    };
    let Err(VerifyError::Rejected(issues)) = pr_context(&output, &facts) else {
        panic!("rejected");
    };
    let kinds: Vec<VerifyIssueKind> = issues.iter().map(|issue| issue.kind).collect();
    assert_eq!(
        kinds,
        [
            VerifyIssueKind::EmptyText,
            VerifyIssueKind::EmptyText,
            VerifyIssueKind::LineOutOfRange,
            VerifyIssueKind::UnknownFile,
        ]
    );
    assert_eq!(
        pr_context_files(&output),
        [
            (LineSide::Head, "../x".to_owned()),
            (LineSide::Head, "src/a.rs".to_owned())
        ]
    );
}

fn ask_output(answer: &str, evidence: Vec<Evidence>) -> AskOutput {
    AskOutput {
        answer: answer.to_owned(),
        evidence,
    }
}

fn kinds_of(result: Result<(), VerifyError>) -> Vec<VerifyIssueKind> {
    match result {
        Ok(()) => Vec::new(),
        Err(VerifyError::Rejected(issues)) => issues.iter().map(|issue| issue.kind).collect(),
        Err(other) => panic!("verification did not run: {other}"),
    }
}

#[test]
fn ask_rejects_blank_answers_and_evidence_outside_the_tree() {
    let mut facts = LineFacts::default();
    let file = FileFacts {
        line_count: 10,
        blank_lines: Vec::new(),
    };
    facts.insert(LineSide::Head, "src/a.rs", file);
    let evidence = |file: &str, line| Evidence {
        file: file.to_owned(),
        side: LineSide::Head,
        line,
    };
    let fine = ask_output("Can `send` run twice?", vec![evidence("src/a.rs", 3)]);
    assert_eq!(kinds_of(ask(&fine, &facts)), []);
    let output = ask_output(
        " \n",
        vec![
            evidence("src/a.rs", 3),
            evidence("src/a.rs", 11),
            evidence("../x", 1),
        ],
    );
    assert_eq!(
        kinds_of(ask(&output, &facts)),
        [
            VerifyIssueKind::EmptyText,
            VerifyIssueKind::LineOutOfRange,
            VerifyIssueKind::UnknownFile,
        ]
    );
    assert_eq!(
        ask_files(&output),
        [
            (LineSide::Head, "../x".to_owned()),
            (LineSide::Head, "src/a.rs".to_owned())
        ]
    );
}

#[test]
fn ask_rejects_markup_and_links_outside_github() {
    let facts = LineFacts::default();
    let rejected = [
        "See <img src=x onerror=alert(1)> here?",
        "Is <script>alert(1)</script> run?",
        "Bold <b>text</b>?",
        "Read https://evil.example/x first?",
        "Read http://github.com/acme/x?",
        "Read https://github.com.evil.example/x?",
        "Click javascript:alert(1)?",
        "Open data:text/html;base64,AAAA?",
        "Mail <a href=\"https://github.com/acme\">here</a>?",
        "A<br>B",
        "A <br/> B",
        "Hidden <!-- note --> text?",
        "Closing </p> tag?",
        "Is Option<A> unchanged?",
        "Open [x](//evil.example/path)?",
        "Open [x](JaVaScRiPt:alert(1))?",
        "Open [x](src/rpc/client.rs)?",
        "Open ![x](https://evil.example/a.png)?",
        "Open <https://evil.example/x>?",
        "Open <mailto:a@evil.example>?",
        "Open //evil.example/path?",
        "Read https://github.com@evil.example/x?",
        "Read https://user:pass@github.com/acme/x?",
        "Read https://github.com:8443/acme/x?",
        "Read HTTP://GITHUB.COM/acme/x?",
        "Click VBScript:msgbox(1)?",
        "Open DATA:text/html,x?",
        "In code `curl https://evil.example/x`?",
        "[x]: https://evil.example/x",
        "Unclosed fence?\n```\nA<br>B",
        "Fake close?\n```rust\nlet x = 1;\n``` <br>",
        "Fake close?\n```\nlet x = 1;\n```[x](//evil.example/a)",
        "Read https://github.com:443/acme/x?",
        "See [PR 470](https://github.com/acme/node-client/pull/470).",
        "See the [upstream definition](https://github.com/emilk/egui/blob/0.36.2/crates/epaint/src/text/text_layout_types.rs).",
        "[PR 470]: https://github.com/acme/node-client/pull/470",
        "**Important** detail",
        "# Heading",
        "> Quoted answer",
        "See [the answer][ref].",
        "See www.example.org/details.",
    ];
    for answer in rejected {
        assert_eq!(
            kinds_of(ask(&ask_output(answer, Vec::new()), &facts)),
            [VerifyIssueKind::MarkupOrLink],
            "{answer}"
        );
    }
    let allowed = [
        "Compare https://github.com/acme/node-client/pull/470 and acme/node-client#471?",
        "Does `Result<Vec<u8>, Error>` or `Option<A>` change?",
        "Is `a < b && c > d` still true?",
        "Does `HashMap<K, V>` keep order?",
        "Read HTTPS://GITHUB.COM/acme/x?",
        "Does this change?\n```rust\nfn first<T: Clone>(items: &[T]) -> Option<T> { None }\n```\nOr not?",
        "Does ``Vec<`u8`>`` change?",
        "Is std::fmt::Display used, or a // comment in `// keep <b>`?",
        "Is 3 < 4 and a <= b?",
        "The handle_light_block function returns the response.",
        "This code is literal: `- item` and `[text](url)`.",
        "Closed with spaces?\n```rust\nlet v: Vec<u8> = vec![];\n```   \nDone?",
        // Requested lists and scenario steps are plain lines (the answer is shown with its line breaks).
        "Potential TOCTOU race:\n1. Claim A reads index 3.\n2) Claim B reads index 3.\n- A then B submit.",
        "* Not checked: the node-side handling.",
    ];
    for answer in allowed {
        assert_eq!(
            kinds_of(ask(&ask_output(answer, Vec::new()), &facts)),
            [],
            "{answer}"
        );
    }
}

#[test]
fn issue_codes_are_stable_snake_case() {
    assert_eq!(
        VerifyIssueKind::ChangedLineUncovered.code(),
        "changed_line_uncovered"
    );
    assert_eq!(VerifyIssueKind::UnknownFile.code(), "unknown_file");
}

#[test]
fn facts_are_kept_per_side() {
    let mut facts = LineFacts::default();
    facts.insert(
        LineSide::Base,
        "src/rpc/old.rs",
        FileFacts {
            line_count: 40,
            blank_lines: vec![12],
        },
    );
    facts.insert(
        LineSide::Head,
        "src/rpc/old.rs",
        FileFacts {
            line_count: 0,
            blank_lines: vec![],
        },
    );
    assert_eq!(
        facts
            .get(LineSide::Base, "src/rpc/old.rs")
            .map(|f| f.line_count),
        Some(40)
    );
    assert_eq!(
        facts
            .get(LineSide::Head, "src/rpc/old.rs")
            .map(|f| f.line_count),
        Some(0),
        "an emptied file"
    );
    assert!(facts.get(LineSide::Head, "src/rpc/gone.rs").is_none());
}

/// A flow output whose steps are exactly the flow's planned ranges (head steps for head lines, base
/// `deleted_site` steps for removed lines).
/// Three flows, each in its own 6 MiB file: each output fits the per-output read budget, all three together do
/// not. The final check reads per output, so none is rejected for the others' files.
#[test]
fn each_output_is_verified_with_its_own_bounded_read() {
    const SIZE: u64 = 6 * 1024 * 1024;
    let files: Vec<FileDiff> = (1..=3)
        .map(|n| FileDiff {
            path: format!("src/big{n}.rs"),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![Hunk {
                id: HunkId(format!("h{n}")),
                base: LineSpan { start: 10, len: 1 },
                head: LineSpan { start: 10, len: 1 },
                added: vec![10],
                removed: vec![10],
            }],
        })
        .collect();
    let plan_output = PlanOutput {
        flows: (1..=3u32)
            .map(|n| contracts::agent::plan::PlannedFlow {
                id: id(&format!("f{n}")),
                title: "flow".to_owned(),
                classification: Classification::Behavioural,
                importance_rank: n,
                intent: "intent".to_owned(),
                hunks: vec![contracts::agent::plan::HunkSelection {
                    hunk_id: HunkId(format!("h{n}")),
                    head_start: None,
                    head_end: None,
                }],
                depends_on: Vec::new(),
            })
            .collect(),
        unassigned: Vec::new(),
        crate_notes: None,
    };
    let diff = DiffIndex::new(&files).expect("diff");
    let verified_plan = plan(&plan_output, &diff).expect("plan");
    let outputs: Vec<FlowAnalysisOutput> = verified_plan
        .flow_lines
        .iter()
        .map(|(flow, lines)| covering_output(flow, lines))
        .collect();
    // A loader with the real per-load budget: files past MAX_FACTS_BYTES are unread.
    let load = |wanted: &[(LineSide, String)]| {
        let mut facts = LineFacts::default();
        let mut used = 0u64;
        for (side, path) in wanted {
            used += SIZE;
            if used > MAX_FACTS_BYTES {
                facts.insert_unread(*side, path, SIZE);
            } else {
                facts.insert(
                    *side,
                    path,
                    FileFacts {
                        line_count: 1000,
                        blank_lines: Vec::new(),
                    },
                );
            }
        }
        Ok(facts)
    };
    let verdict = analysis(AnalysisInput {
        diff: &diff,
        listed: &diff,
        quiet: &[],
        facts: &load,
        moves: &MovesReport::default(),
        plan: &plan_output,
        pr_context: &PrContextOutput {
            problem: "p".to_owned(),
            why_needed: "w".to_owned(),
            what_it_gives: "g".to_owned(),
            related_work: Vec::new(),
            fit_questions: Vec::new(),
        },
        flows: &outputs,
        thread_aliases: &BTreeMap::new(),
        stored: false,
    })
    .expect("verdict");
    assert_eq!(verdict.repairs, Vec::<Repair>::new());
    assert_eq!(
        flow_ids(&verdict.tour.expect("tour").flows),
        ["f1", "f2", "f3"]
    );
}

fn covering_output(id: &FlowId, lines: &[FileRanges]) -> FlowAnalysisOutput {
    let text = || "text".to_owned();
    let mut steps = Vec::new();
    for ranges in lines {
        for range in &ranges.ranges {
            steps.push(contracts::agent::analyse_flow::AgentStep {
                id: format!("s{}", steps.len() + 1),
                title: text(),
                file: ranges.file.clone(),
                side: ranges.side,
                start_line: range.start_line,
                end_line: range.end_line,
                symbol: None,
                kind: match ranges.side {
                    LineSide::Head => contracts::common::FragmentKind::Modified,
                    LineSide::Base => contracts::common::FragmentKind::DeletedSite,
                },
                moved_from: None,
                called_by: text(),
                decides: text(),
                next: text(),
                why: text(),
                annotations: Vec::new(),
                assessment: Some(contracts::agent::analyse_flow::Assessment {
                    headline: Some(text()),
                    risk: contracts::common::RiskBand::Low,
                    reason: text(),
                    basis_note: Some(text()),
                    not_checked: Some(text()),
                    origin: Some(contracts::common::Origin::Introduced),
                    blocking: Some(contracts::common::Blocking::NoActionProposed),
                    smallest_correction: None,
                }),
            });
        }
    }
    FlowAnalysisOutput {
        flow_id: id.clone(),
        introduction: contracts::agent::analyse_flow::FlowIntroduction {
            problem_before: text(),
            change: text(),
            example_before_after: text(),
        },
        steps,
        edges: Vec::new(),
        impact_refs: Vec::new(),
        pattern_repetitions: Vec::new(),
        thread_statuses: Vec::new(),
    }
}

fn expand(list: &[FileRanges]) -> Vec<(LineSide, String, u32)> {
    let mut lines = Vec::new();
    for ranges in list {
        for range in &ranges.ranges {
            for line in range.start_line..=range.end_line {
                lines.push((ranges.side, ranges.file.clone(), line));
            }
        }
    }
    lines
}

proptest::proptest! {
    /// Through the real entry: a generated diff, quiet hunks, a plan with whole and partial hunk selections
    /// (three flows and `unassigned`), outputs for some flows. Every changed line is in exactly one of: one
    /// accepted flow, the quiet lines, Unexplained; nothing is double-covered; Unexplained is exactly the lines
    /// of `unassigned` and of flows without an output.
    #[test]
    fn proptest_analysis_coverage_invariant(
        hunks in proptest::collection::vec((0usize..3, 1u32..4, 0u32..3, 0u32..4), 1..8),
        tape in proptest::collection::vec(proptest::prelude::any::<u8>(), 64),
        answered in proptest::collection::vec(proptest::prelude::any::<bool>(), 3),
    ) {
        let mut next = {
            let mut position = 0;
            move || {
                position += 1;
                tape[position % tape.len()]
            }
        };
        // The diff: hunks grouped by file, ids in path order then file order.
        let mut per_file: BTreeMap<usize, Vec<(u32, u32, u32)>> = BTreeMap::new();
        for &(file, gap, removed, added) in &hunks {
            per_file.entry(file).or_default().push((gap, removed, added.max(u32::from(removed == 0))));
        }
        let mut files = Vec::new();
        let mut number = 0;
        for (file, spans) in per_file {
            let (mut base, mut head) = (0, 0);
            let mut file_hunks = Vec::new();
            for (gap, removed, added) in spans {
                let (base_start, head_start) = (base + gap, head + gap);
                number += 1;
                file_hunks.push(Hunk {
                    id: HunkId(format!("h{number}")),
                    base: LineSpan { start: if removed == 0 { base_start - 1 } else { base_start }, len: removed },
                    head: LineSpan { start: if added == 0 { head_start - 1 } else { head_start }, len: added },
                    added: (head_start..head_start + added).collect(),
                    removed: (base_start..base_start + removed).collect(),
                });
                base = base_start + removed;
                head = head_start + added;
            }
            files.push(FileDiff {
                path: format!("src/f{file}.rs"),
                old_path: None,
                status: FileStatus::Modified,
                binary: false,
                hunks: file_hunks,
            });
        }
        // Quiet hunks, then a plan over the listed ones: segments of each hunk to flows f1-f3 or unassigned.
        let mut listed = files.clone();
        let mut quiet = Vec::new();
        let mut plan_output = PlanOutput {
            flows: (1..=3u32)
                .map(|n| contracts::agent::plan::PlannedFlow {
                    id: id(&format!("f{n}")),
                    title: "flow".to_owned(),
                    classification: Classification::Behavioural,
                    importance_rank: n,
                    intent: "intent".to_owned(),
                    hunks: Vec::new(),
                    depends_on: Vec::new(),
                })
                .collect(),
            unassigned: Vec::new(),
            crate_notes: None,
        };
        let mut assign = |owner: u8, hunk_id: &HunkId, range: Option<(u32, u32)>| {
            let (head_start, head_end) = (range.map(|r| r.0), range.map(|r| r.1));
            match usize::from(owner % 4) {
                3 => plan_output.unassigned.push(contracts::agent::plan::UnassignedHunk {
                    hunk_id: hunk_id.clone(),
                    head_start,
                    head_end,
                    reason: "unused".to_owned(),
                }),
                flow => plan_output.flows[flow].hunks.push(contracts::agent::plan::HunkSelection {
                    hunk_id: hunk_id.clone(),
                    head_start,
                    head_end,
                }),
            }
        };
        for file in &mut listed {
            let path = file.path.clone();
            file.hunks.retain(|hunk| {
                if next() % 5 != 0 {
                    return true;
                }
                for (side, lines) in [(LineSide::Head, &hunk.added), (LineSide::Base, &hunk.removed)] {
                    if !lines.is_empty() {
                        quiet.push(FileRanges { file: path.clone(), side, ranges: crate::lines::ranges(lines) });
                    }
                }
                false
            });
            for hunk in &file.hunks {
                let (Some(&first), Some(&last)) = (hunk.added.first(), hunk.added.last()) else {
                    assign(next(), &hunk.id, None);
                    continue;
                };
                let mut start = first;
                for line in first..=last {
                    if line == last || next() % 3 == 0 {
                        let whole = start == first && line == last && next() % 2 == 0;
                        assign(next(), &hunk.id, (!whole).then_some((start, line)));
                        start = line + 1;
                    }
                }
            }
        }
        let diff = DiffIndex::new(&files).expect("diff");
        let listed_index = DiffIndex::new(&listed).expect("listed");
        let verified_plan = match plan(&plan_output, &listed_index) {
            Ok(verified) => verified,
            Err(error) => return Err(proptest::test_runner::TestCaseError::fail(format!("{error:?}"))),
        };
        let outputs: Vec<FlowAnalysisOutput> = verified_plan
            .flow_lines
            .iter()
            .zip(&answered)
            .filter(|((_, lines), answered)| **answered && !lines.is_empty())
            .map(|((flow, lines), _)| covering_output(flow, lines))
            .collect();
        let mut facts = LineFacts::default();
        for file in &files {
            for side in [LineSide::Head, LineSide::Base] {
                facts.insert(side, &file.path, FileFacts { line_count: 1000, blank_lines: Vec::new() });
            }
        }
        let verdict = analysis(AnalysisInput {
            diff: &diff,
            listed: &listed_index,
            quiet: &quiet,
            facts: &|_| Ok(facts.clone()),
            moves: &MovesReport::default(),
            plan: &plan_output,
            pr_context: &PrContextOutput {
                problem: "p".to_owned(),
                why_needed: "w".to_owned(),
                what_it_gives: "g".to_owned(),
                related_work: Vec::new(),
                fit_questions: Vec::new(),
            },
            flows: &outputs,
            thread_aliases: &BTreeMap::new(),
            stored: false,
        })
        .expect("verdict");
        proptest::prop_assert_eq!(&verdict.repairs, &Vec::<Repair>::new());
        let tour = verdict.tour.expect("tour");
        proptest::prop_assert_eq!(&tour.coverage.double_covered, &Vec::<FileRanges>::new());

        let mut places: BTreeMap<(LineSide, String, u32), u32> = BTreeMap::new();
        let flows = tour.flows.iter().map(|flow| expand(&flow.covered));
        for lines in flows.chain([expand(&quiet), expand(&tour.coverage.unexplained)]) {
            for line in lines {
                *places.entry(line).or_default() += 1;
            }
        }
        let changed = expand(&crate::coverage::check(&files, &[], &[]).expect("all lines").unexplained);
        for line in &changed {
            proptest::prop_assert_eq!(places.get(line).copied(), Some(1), "{:?}", line);
        }
        proptest::prop_assert_eq!(places.len(), changed.len(), "only changed lines are placed");
        let mut expected: Vec<_> = expand(&verified_plan.unassigned);
        for id in &tour.unanalysed {
            expected.extend(expand(&verified_plan.flow_lines[id]));
        }
        expected.sort();
        let mut unexplained = expand(&tour.coverage.unexplained);
        unexplained.sort();
        proptest::prop_assert_eq!(unexplained, expected);
    }
}

#[test]
fn reviewer_fields_reject_capability_excuses_but_accept_project_failures() {
    let facts = LineFacts::default();
    for text in [
        "My tool was unavailable.",
        "I could not load the severity skill.",
        "As an AI, I cannot review it.",
    ] {
        let output = AskOutput {
            answer: text.to_owned(),
            evidence: vec![],
        };
        assert!(ask(&output, &facts).is_err(), "{text}");
    }
    for text in [
        "The server could not load its configuration file; the error propagates to the caller.",
        "The export tool was unavailable, so the CLI returned an error.",
        "The parser could not read the instructions section.",
    ] {
        let output = AskOutput {
            answer: text.to_owned(),
            evidence: vec![],
        };
        assert!(ask(&output, &facts).is_ok(), "{text}");
    }
}

#[test]
fn pr_context_does_not_invent_stacked_relationships() {
    let mut output = PrContextOutput {
        problem: "Retries can stall.".to_owned(),
        why_needed: "Requests must finish.".to_owned(),
        what_it_gives: "Nonblocking replies.".to_owned(),
        related_work: vec![contracts::agent::pr_context::RelatedWork {
            reference: "owner/repo#1550".to_owned(),
            kind: contracts::common::RelatedWorkKind::StackedPr,
            overlap: "Already merged beneath the supplied base.".to_owned(),
        }],
        fit_questions: vec![],
    };
    assert!(pr_context(&output, &LineFacts::default()).is_err());
    output.related_work[0].kind = contracts::common::RelatedWorkKind::CompetingSolution;
    assert!(pr_context(&output, &LineFacts::default()).is_ok());
}

#[test]
fn tagged_fixture_checker_matches_server_verifier_and_measures_bytes() {
    let fixture = Fixture::new();
    let mut before = 0;
    let mut after = 0;
    for (index, output) in fixture.flows.iter().enumerate() {
        let context = crate::tagged::Context {
            version: crate::tagged::VERSION,
            base: fixture.base.clone(),
            head: fixture.head.clone(),
            files: fixture.files.clone(),
            listed: fixture.listed.clone(),
            plan: fixture.plan.clone(),
            flow_id: output.flow_id.clone(),
            moves: fixture.moves.clone(),
            thread_aliases: fixture
                .aliases
                .get(&output.flow_id)
                .cloned()
                .unwrap_or_default(),
        };
        let text = crate::tagged::encode(output, &context.catalog()).unwrap();
        assert_eq!(context.check(&text, &fixture.repo).unwrap(), *output);
        let path = fixture_dir().join(format!("agent/flow-{}.md", index + 1));
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &text).unwrap();
        }
        assert_eq!(std::fs::read_to_string(path).unwrap(), text);
        before += std::fs::read(fixture_dir().join(format!("agent/flow-{}.json", index + 1)))
            .unwrap()
            .len();
        after += text.len();
        let mut broken = output.clone();
        broken.steps[0].assessment = None;
        let missing = crate::tagged::encode(&broken, &context.catalog()).unwrap();
        let errors = context.check(&missing, &fixture.repo).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("has no assessment"))
        );
        broken = output.clone();
        broken.steps[0].end_line = 99999;
        let bad = crate::tagged::encode(&broken, &context.catalog()).unwrap();
        assert!(
            context
                .check(&bad, &fixture.repo)
                .unwrap_err()
                .iter()
                .any(|e| e.kind == VerifyIssueKind::LineOutOfRange)
        );
    }
    println!(
        "Canonical fixture files: {before} bytes; tagged flow files: {after} bytes; saved {}%",
        100 * (before - after) / before
    );
}

#[test]
fn tagged_checker_rejects_the_same_semantic_violations_as_canonical_flow_verification() {
    let fixture = Fixture::new();
    let original = &fixture.flows[1];
    let context = crate::tagged::Context {
        version: crate::tagged::VERSION,
        base: fixture.base.clone(),
        head: fixture.head.clone(),
        files: fixture.files.clone(),
        listed: fixture.listed.clone(),
        plan: fixture.plan.clone(),
        flow_id: original.flow_id.clone(),
        moves: fixture.moves.clone(),
        thread_aliases: vec![],
    };
    type Mutation = fn(&mut FlowAnalysisOutput);
    let cases: Vec<(VerifyIssueKind, Mutation)> = vec![
        (VerifyIssueKind::ChangedLineUncovered, |out| {
            out.steps.remove(0);
        }),
        (VerifyIssueKind::DanglingEdge, |out| {
            out.edges[0].to_step_id = "missing".into()
        }),
        (VerifyIssueKind::FactWithoutEvidence, |out| {
            out.steps[0].annotations[0].evidence.clear()
        }),
        (VerifyIssueKind::BehaviourMissing, |out| {
            out.steps[0].annotations[0].was.clear()
        }),
        (VerifyIssueKind::RemarkMismatch, |out| {
            out.steps[0].annotations[0].text = Some("Extra remark".into())
        }),
        (VerifyIssueKind::LineOutOfRange, |out| {
            out.steps[0].annotations[0].evidence[0].line = 999999
        }),
        (VerifyIssueKind::MoveClaimMismatch, |out| {
            out.steps[0].kind = contracts::common::FragmentKind::Moved
        }),
        (VerifyIssueKind::BaseSideMisused, |out| {
            out.steps[0].side = LineSide::Base
        }),
        (VerifyIssueKind::AgentLimitation, |out| {
            out.steps[0].why = "The required skill was unavailable.".into()
        }),
        (VerifyIssueKind::UnknownThread, |out| {
            out.thread_statuses
                .push(contracts::agent::analyse_flow::AgentThreadStatus {
                    thread_alias: "t999".into(),
                    status: contracts::common::ThreadLlmStatus::AlreadyAddressed,
                    reason: "The branch changed.".into(),
                })
        }),
    ];
    for (kind, mutate) in cases {
        let mut output = original.clone();
        mutate(&mut output);
        let text = crate::tagged::encode(&output, &context.catalog()).unwrap();
        let errors = context.check(&text, &fixture.repo).unwrap_err();
        assert!(
            errors.iter().any(|error| error.kind == kind),
            "{kind:?}: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .all(|error| error.to_string().starts_with("line "))
        );
    }
}

#[test]
fn omitted_boundary_connections_pass_the_full_check() {
    let fixture = Fixture::new();
    let context = |output: &FlowAnalysisOutput| crate::tagged::Context {
        version: crate::tagged::VERSION,
        base: fixture.base.clone(),
        head: fixture.head.clone(),
        files: fixture.files.clone(),
        listed: fixture.listed.clone(),
        plan: fixture.plan.clone(),
        flow_id: output.flow_id.clone(),
        moves: fixture.moves.clone(),
        thread_aliases: vec![],
    };
    let blank = |mut output: FlowAnalysisOutput| {
        for step in &mut output.steps {
            step.called_by.clear();
            step.next.clear();
        }
        output
    };
    let single = blank(fixture.flows[3].clone());
    let mut walk = blank(fixture.flows[1].clone());
    walk.edges.clear();
    let mut isolated = walk.clone();
    isolated.edges = vec![contracts::agent::analyse_flow::AgentEdge {
        from_step_id: "s1".into(),
        to_step_id: "s1".into(),
        label: Some("retry".into()),
    }];
    // Per step: (called_by empty, next empty) after the check.
    let cases = [
        ("single step", single, vec![(true, true)]),
        (
            "entry and terminal",
            walk,
            vec![(true, false), (false, true)],
        ),
        (
            "isolated step",
            isolated,
            vec![(false, false), (true, true)],
        ),
    ];
    for (name, output, expected) in cases {
        let context = context(&output);
        let text = crate::tagged::encode(&output, &context.catalog()).unwrap();
        assert!(
            !text.contains("\ncalled_by:") && !text.contains("\nnext:"),
            "{name}"
        );
        let checked = match context.check(&text, &fixture.repo) {
            Ok(checked) => checked,
            Err(errors) => panic!("{name}: {errors:?}"),
        };
        let empty: Vec<_> = checked
            .steps
            .iter()
            .map(|step| (step.called_by.is_empty(), step.next.is_empty()))
            .collect();
        assert_eq!(empty, expected, "{name}");
    }
    let mut present = fixture.flows[3].clone();
    present.steps[0].next = "The required skill was unavailable.".into();
    let context = context(&present);
    let text = crate::tagged::encode(&present, &context.catalog()).unwrap();
    let errors = context.check(&text, &fixture.repo).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.kind == VerifyIssueKind::AgentLimitation),
        "{errors:?}"
    );
}

#[test]
fn plan_rejects_flow_with_only_test_locations() {
    let files = vec![FileDiff {
        path: "tests/retry.rs".into(),
        old_path: None,
        status: FileStatus::Added,
        binary: false,
        hunks: vec![Hunk {
            id: HunkId("h1".into()),
            base: LineSpan { start: 0, len: 0 },
            head: LineSpan { start: 1, len: 1 },
            added: vec![1],
            removed: vec![],
        }],
    }];
    let mut output: PlanOutput = read("agent/plan.json");
    output.flows.truncate(1);
    output.flows[0].depends_on.clear();
    output.flows[0].hunks.truncate(1);
    output.flows[0].hunks[0].hunk_id = HunkId("h1".into());
    output.flows[0].hunks[0].head_start = Some(1);
    output.flows[0].hunks[0].head_end = Some(1);
    output.unassigned.clear();
    assert!(plan(&output, &DiffIndex::new(&files).expect("diff")).is_err());
}

#[test]
fn a_flow_may_claim_context_only_lines_without_owing_them() {
    let mut fixture = Fixture::new();
    // h17 changes one comment line per side: listed as context (its lines quiet, nothing left to cover).
    let h17 = fixture
        .files
        .iter()
        .flat_map(|file| file.hunks.iter().map(move |hunk| (file, hunk)))
        .find(|(_, hunk)| hunk.id.0 == "h17")
        .map(|(file, hunk)| (file.path.clone(), hunk.clone()))
        .expect("h17");
    let listed = fixture
        .listed
        .iter_mut()
        .find(|file| file.path == h17.0)
        .expect("listed file");
    listed.hunks.push(Hunk {
        added: Vec::new(),
        removed: Vec::new(),
        ..h17.1
    });
    listed.hunks.sort_by_key(|hunk| hunk.head.start);
    fixture.plan.flows[0]
        .hunks
        .push(contracts::agent::plan::HunkSelection {
            hunk_id: HunkId("h17".to_owned()),
            head_start: None,
            head_end: None,
        });
    let verdict = fixture.verdict(&fixture.facts());
    assert_eq!(verdict.repairs, [], "claiming a context line is allowed");
    let tour = verdict.tour.expect("tour");
    assert!(tour.flows.iter().any(|flow| flow.flow_id == id("f1")));
    assert_eq!(tour.coverage.double_covered, []);
}
