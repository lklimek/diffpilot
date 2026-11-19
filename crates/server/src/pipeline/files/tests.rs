use contracts::agent::analyse_flow::FlowIntroduction;
use contracts::api::tour::UnexplainedReason;
use contracts::api::tour::{
    FlowAnalysisStatus, FlowFacts, FlowGraph, FlowGroup, FlowLlm, MoveFact, PlannedFlowDetails,
    StepFacts, StepLlm,
};
use contracts::common::{Classification, FileRange, FragmentKind, ProvenanceBadge};
use serde_json::json;

use super::*;
use analysis::diff::LineSpan;

const A: &str = "src/a.rs";

fn lines(first: u32, last: u32) -> LineRange {
    LineRange {
        start_line: first,
        end_line: last,
    }
}

/// A diff hunk `-base_start,base_len +head_start,head_len` (zero context).
fn hunk(id: &str, (base_start, base_len): (u32, u32), (head_start, head_len): (u32, u32)) -> Hunk {
    Hunk {
        id: HunkId(id.to_owned()),
        base: LineSpan {
            start: base_start,
            len: base_len,
        },
        head: LineSpan {
            start: head_start,
            len: head_len,
        },
        added: (head_start..head_start + head_len).collect(),
        removed: (base_start..base_start + base_len).collect(),
    }
}

fn file(path: &str, status: FileStatus, hunks: Vec<Hunk>) -> FileDiff {
    FileDiff {
        path: path.to_owned(),
        old_path: None,
        status,
        binary: false,
        hunks,
    }
}

/// A pre-pass with the given hunk dispositions (default agent), quiet moves, whitespace pairs of `A`.
fn prepass(
    not_analysed: &[(&str, &str)],
    quiet_moves: serde_json::Value,
    pairs: &[(u32, u32)],
) -> Prepass {
    let hunks: Vec<serde_json::Value> = not_analysed
        .iter()
        .map(|(id, category)| json!({"id": id, "disposition": {"kind": "not_analysed", "category": category}}))
        .collect();
    let whitespace = if pairs.is_empty() {
        json!([])
    } else {
        json!([{"base": A, "head": A, "pairs": pairs}])
    };
    serde_json::from_value(json!({
        "hunks": hunks, "agent_lines": [], "not_analysed": [],
        "moves": {"moved": [], "whitespace_only": [], "incomplete": false},
        "quiet_moves": quiet_moves, "whitespace": whitespace,
        "counters": {"counters": {"lines_removed_from_tests": 0, "dependencies_changed": [],
            "dependency_files_not_checked": []}, "removed_test_lines": [], "dependencies": [],
            "dependency_files_not_checked": []},
        "related_repos": [], "profile_file": null, "agent_instruction_files": [],
        "not_rendered": [{"file": "gen/big.rs", "side": "head", "reason": "generated"}]
    }))
    .expect("pre-pass")
}

fn step(id: &str, path: &str, side: LineSide, range: LineRange) -> Step {
    Step {
        id: StepId(id.to_owned()),
        number: 1,
        facts: StepFacts {
            file: path.to_owned(),
            side,
            range,
            github_url: String::new(),
            changed_lines: Vec::new(),
            moved: None,
            call_hierarchy: None,
            thread_ids: Vec::new(),
        },
        llm: StepLlm {
            title: id.to_owned(),
            kind: FragmentKind::Modified,
            symbol: None,
            called_by: String::new(),
            decides: String::new(),
            next: String::new(),
            why: String::new(),
            annotations: Vec::new(),
            assessment: None,
            moved_from: None,
            impact: Vec::new(),
        },
    }
}

fn flow(id: &str, state: FlowAnalysisState, steps: Vec<Step>, planned: Vec<FileRanges>) -> Flow {
    Flow {
        analysis_status: Some(FlowAnalysisStatus {
            state,
            started_at: None,
            message: None,
        }),
        planned: Some(PlannedFlowDetails {
            intent: String::new(),
            importance_rank: 1,
            ranges: planned,
        }),
        id: FlowId(id.to_owned()),
        position: 1,
        group: FlowGroup::Behaviour,
        facts: FlowFacts {
            files: Vec::new(),
            step_count: 0,
        },
        llm: FlowLlm {
            title: id.to_owned(),
            classification: Classification::Behavioural,
            risk: None,
            introduction: FlowIntroduction {
                problem_before: String::new(),
                change: String::new(),
                example_before_after: String::new(),
            },
            depends_on: Vec::new(),
            pattern_repetitions: Vec::new(),
        },
        graph: FlowGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
        },
        steps,
    }
}

fn on(path: &str, side: LineSide, ranges: &[LineRange]) -> FileRanges {
    FileRanges {
        file: path.to_owned(),
        side,
        ranges: ranges.to_vec(),
    }
}

fn part(side: LineSide, range: LineRange, owner: PartOwner, also_in: &[&str]) -> HunkPart {
    HunkPart {
        side,
        range,
        owner,
        also_in: also_in.iter().map(|id| StepId((*id).to_owned())).collect(),
    }
}

fn step_owner(flow: &str, step: &str) -> PartOwner {
    PartOwner::Step {
        flow_id: FlowId(flow.to_owned()),
        step_id: StepId(step.to_owned()),
    }
}

struct Case {
    files: Vec<FileDiff>,
    prepass: Prepass,
    covered: Vec<(FlowId, Vec<FileRanges>)>,
    flows: Vec<Flow>,
    unexplained: Vec<UnexplainedRanges>,
    not_analysed: Vec<NotAnalysedSection>,
}

impl Case {
    fn run(&self) -> Vec<PrFile> {
        let covered: Vec<(&FlowId, &[FileRanges])> = self
            .covered
            .iter()
            .map(|(flow, lines)| (flow, lines.as_slice()))
            .collect();
        project(&FilesInput {
            files: &self.files,
            prepass: &self.prepass,
            covered: &covered,
            flows: &self.flows,
            unexplained: &self.unexplained,
            not_analysed: &self.not_analysed,
        })
    }
}

/// Head lines 1–12 of `A` added in one hunk; each quiet kind, Unexplained (with reason), an accepted flow, a
/// waiting flow and a failed flow (its lines are Unexplained rows) own some of them.
#[test]
fn every_changed_line_gets_the_owner_the_tour_decided() {
    let head = |range| FileRange {
        file: A.to_owned(),
        side: LineSide::Head,
        range,
    };
    let moved_from = FileRange {
        file: "src/old.rs".to_owned(),
        side: LineSide::Base,
        range: lines(5, 6),
    };
    let quiet_moves =
        json!([{"from": moved_from, "to": head(lines(1, 2)), "badge": "text_identical"}]);
    let case = Case {
        files: vec![file(
            A,
            FileStatus::Modified,
            vec![hunk("h1", (0, 0), (1, 12))],
        )],
        // Line 2 is in the move and whitespace-only: the move wins (pre-pass priority).
        prepass: prepass(&[], quiet_moves, &[(2, 2), (3, 3)]),
        covered: vec![(
            FlowId("f1".to_owned()),
            vec![on(A, LineSide::Head, &[lines(7, 8)])],
        )],
        flows: vec![
            flow(
                "f1",
                FlowAnalysisState::Analysed,
                vec![step("f1.1", A, LineSide::Head, lines(7, 8))],
                vec![on(A, LineSide::Head, &[lines(7, 8)])],
            ),
            flow(
                "f2",
                FlowAnalysisState::Queued,
                Vec::new(),
                vec![on(A, LineSide::Head, &[lines(9, 10)])],
            ),
            flow(
                "f3",
                FlowAnalysisState::Failed,
                Vec::new(),
                vec![on(A, LineSide::Head, &[lines(11, 11)])],
            ),
        ],
        unexplained: vec![
            UnexplainedRanges {
                file: A.to_owned(),
                side: LineSide::Head,
                ranges: vec![lines(5, 6)],
                reason: Some(UnexplainedReason::TooLargeForPrompt),
            },
            UnexplainedRanges {
                file: A.to_owned(),
                side: LineSide::Head,
                ranges: vec![lines(11, 11)],
                reason: None,
            },
        ],
        not_analysed: vec![NotAnalysedSection {
            category: NotAnalysedCategory::DocsComments,
            file_count: 1,
            items: vec![NotAnalysedItem {
                renderable: Some(true),
                rule_kind: None,
                file: A.to_owned(),
                side: LineSide::Head,
                ranges: Some(vec![lines(3, 4)]),
                badge: ProvenanceBadge::ClassifiedByLineRule,
                rule: analysis::tech::LINE_COMMENT_RULE.to_owned(),
            }],
        }],
    };
    let files = case.run();
    let [file] = files.as_slice() else {
        panic!("one file: {files:?}")
    };
    assert_eq!(
        file.group,
        FileGroup::Code,
        "a line rule does not make a file not analysed"
    );
    assert_eq!((file.added, file.removed), (12, 0));
    let [hunk] = file.hunks.as_slice() else {
        panic!("one hunk")
    };
    assert_eq!(
        (hunk.base, hunk.head, hunk.after_line),
        (None, Some(lines(1, 12)), None)
    );
    let head = LineSide::Head;
    let rule = PartOwner::ContextRule {
        badge: ProvenanceBadge::ClassifiedByLineRule,
        rule: analysis::tech::LINE_COMMENT_RULE.to_owned(),
    };
    assert_eq!(
        hunk.parts,
        [
            part(
                head,
                lines(1, 2),
                PartOwner::IdenticalMove {
                    from: moved_from,
                    to: FileRange {
                        file: A.to_owned(),
                        side: head,
                        range: lines(1, 2)
                    }
                },
                &[]
            ),
            part(head, lines(3, 3), PartOwner::WhitespaceOnly, &[]),
            part(head, lines(4, 4), rule, &[]),
            part(
                head,
                lines(5, 6),
                PartOwner::Unexplained {
                    reason: Some(UnexplainedReason::TooLargeForPrompt)
                },
                &[]
            ),
            part(head, lines(7, 8), step_owner("f1", "f1.1"), &[]),
            part(
                head,
                lines(9, 10),
                PartOwner::Flow {
                    flow_id: FlowId("f2".to_owned())
                },
                &[]
            ),
            // The failed flow's line (a Tour row) and line 12 (owned by nothing: the fallback) read the same and
            // form one part.
            part(
                head,
                lines(11, 12),
                PartOwner::Unexplained { reason: None },
                &[]
            ),
        ]
    );
}

/// Q11: a step may show lines another flow explains; those steps are listed, and parts split where they start.
#[test]
fn steps_of_other_flows_on_the_lines_are_also_in() {
    let case = Case {
        files: vec![file(
            A,
            FileStatus::Modified,
            vec![hunk("h1", (10, 2), (10, 5))],
        )],
        prepass: prepass(&[], json!([]), &[]),
        covered: vec![(
            FlowId("f1".to_owned()),
            vec![
                on(A, LineSide::Head, &[lines(10, 14)]),
                on(A, LineSide::Base, &[lines(10, 11)]),
            ],
        )],
        flows: vec![
            flow(
                "f1",
                FlowAnalysisState::Analysed,
                vec![
                    step("f1.1", A, LineSide::Head, lines(10, 11)),
                    step("f1.2", A, LineSide::Head, lines(12, 14)),
                ],
                Vec::new(),
            ),
            flow(
                "f4",
                FlowAnalysisState::Analysed,
                vec![step("f4.2", A, LineSide::Head, lines(11, 20))],
                Vec::new(),
            ),
        ],
        unexplained: Vec::new(),
        not_analysed: Vec::new(),
    };
    let files = case.run();
    let hunk = &files[0].hunks[0];
    assert_eq!(
        (hunk.base, hunk.head),
        (Some(lines(10, 11)), Some(lines(10, 14)))
    );
    let head = LineSide::Head;
    assert_eq!(
        hunk.parts,
        [
            // Removed lines of a replacement belong to the head step at their deletion marker (after line 9).
            part(LineSide::Base, lines(10, 11), step_owner("f1", "f1.1"), &[]),
            part(head, lines(10, 10), step_owner("f1", "f1.1"), &[]),
            part(head, lines(11, 11), step_owner("f1", "f1.1"), &["f4.2"]),
            part(head, lines(12, 14), step_owner("f1", "f1.2"), &["f4.2"]),
        ]
    );
}

/// Removed lines: a base-side step holds them; else a moved step's original; else the head step at the marker;
/// else (blank lines no step needs) the flow.
#[test]
fn removed_lines_belong_to_the_step_the_verification_assigns() {
    let mut moved = step("f1.3", A, LineSide::Head, lines(40, 45));
    moved.facts.moved = Some(MoveFact {
        from: FileRange {
            file: A.to_owned(),
            side: LineSide::Base,
            range: lines(30, 31),
        },
        badge: None,
        differing_lines: 1,
    });
    let covered = vec![
        on(
            A,
            LineSide::Base,
            &[lines(5, 6), lines(20, 21), lines(30, 31), lines(50, 50)],
        ),
        on(A, LineSide::Head, &[lines(40, 45)]),
    ];
    let case = Case {
        files: vec![file(
            A,
            FileStatus::Modified,
            vec![
                hunk("h1", (5, 2), (4, 0)),
                hunk("h2", (20, 2), (17, 0)),
                hunk("h3", (30, 2), (25, 0)),
                hunk("h4", (40, 0), (40, 6)),
                hunk("h5", (50, 1), (60, 0)),
            ],
        )],
        prepass: prepass(&[], json!([]), &[]),
        covered: vec![(FlowId("f1".to_owned()), covered)],
        flows: vec![flow(
            "f1",
            FlowAnalysisState::Analysed,
            vec![
                step("f1.1", A, LineSide::Base, lines(5, 6)),
                step("f1.2", A, LineSide::Head, lines(18, 19)),
                moved,
            ],
            Vec::new(),
        )],
        unexplained: Vec::new(),
        not_analysed: Vec::new(),
    };
    let files = case.run();
    let owners: Vec<(Option<u32>, &PartOwner)> = files[0]
        .hunks
        .iter()
        .map(|hunk| (hunk.after_line, &hunk.parts[0].owner))
        .collect();
    assert_eq!(
        owners,
        [
            (Some(4), &step_owner("f1", "f1.1")),
            (Some(17), &step_owner("f1", "f1.2")),
            (Some(25), &step_owner("f1", "f1.3")),
            (None, &step_owner("f1", "f1.3")),
            (
                Some(60),
                &PartOwner::Flow {
                    flow_id: FlowId("f1".to_owned())
                }
            ),
        ]
    );
}

/// A file a path rule recognises: its group, its not-analysed hunks, and the not-rendered reason.
#[test]
fn path_rule_files_are_grouped_and_their_hunks_not_analysed() {
    let test_file = "tests/retry.rs";
    let item = |side, ranges| NotAnalysedItem {
        renderable: Some(true),
        rule_kind: None,
        file: test_file.to_owned(),
        side,
        ranges,
        badge: ProvenanceBadge::ClassifiedByPath,
        rule: "tests/**".to_owned(),
    };
    let case = Case {
        files: vec![
            file(
                test_file,
                FileStatus::Modified,
                vec![hunk("h1", (3, 1), (3, 2))],
            ),
            file(
                "gen/big.rs",
                FileStatus::Added,
                vec![hunk("h2", (0, 0), (1, 3))],
            ),
        ],
        prepass: prepass(&[("h1", "tests")], json!([]), &[]),
        covered: Vec::new(),
        flows: Vec::new(),
        unexplained: vec![UnexplainedRanges {
            file: "gen/big.rs".to_owned(),
            side: LineSide::Head,
            ranges: vec![lines(1, 3)],
            reason: None,
        }],
        not_analysed: vec![NotAnalysedSection {
            category: NotAnalysedCategory::Tests,
            file_count: 1,
            items: vec![
                item(LineSide::Base, Some(vec![lines(3, 3)])),
                item(LineSide::Head, None),
            ],
        }],
    };
    let files = case.run();
    let owner = PartOwner::NotAnalysed {
        category: NotAnalysedCategory::Tests,
        badge: ProvenanceBadge::ClassifiedByPath,
        rule: "tests/**".to_owned(),
    };
    assert_eq!(
        files[0].group,
        FileGroup::NotAnalysed {
            category: NotAnalysedCategory::Tests,
            badge: ProvenanceBadge::ClassifiedByPath,
            rule: "tests/**".to_owned()
        }
    );
    assert_eq!(
        files[0].hunks[0].parts,
        [
            part(LineSide::Base, lines(3, 3), owner.clone(), &[]),
            part(LineSide::Head, lines(3, 4), owner, &[]),
        ]
    );
    assert_eq!(files[0].not_rendered, None);
    assert_eq!(files[1].group, FileGroup::Code);
    assert_eq!(files[1].not_rendered, Some(NotRenderedReason::Generated));
    assert_eq!(files[1].status, FileChangeStatus::Added);
    assert_eq!(files[1].viewed, None, "Viewed is read separately");
}

/// One hunk of 200,000 changed lines crossed by 20,000 steps of two flows: segment work, not per-line work.
#[test]
fn a_huge_hunk_with_many_steps_is_not_quadratic() {
    const LINES: u32 = 200_000;
    let steps = |flow: &str, offset: u32| -> Vec<Step> {
        (0..10_000)
            .map(|n| {
                step(
                    &format!("{flow}.{n}"),
                    A,
                    LineSide::Head,
                    lines(n * 20 + offset, n * 20 + offset + 9),
                )
            })
            .collect()
    };
    let case = Case {
        files: vec![file(
            A,
            FileStatus::Added,
            vec![hunk("h1", (0, 0), (1, LINES))],
        )],
        prepass: prepass(&[], json!([]), &[]),
        covered: vec![(
            FlowId("f1".to_owned()),
            vec![on(A, LineSide::Head, &[lines(1, LINES)])],
        )],
        flows: vec![
            flow(
                "f1",
                FlowAnalysisState::Analysed,
                steps("f1", 1),
                Vec::new(),
            ),
            flow(
                "f2",
                FlowAnalysisState::Analysed,
                steps("f2", 6),
                Vec::new(),
            ),
        ],
        unexplained: Vec::new(),
        not_analysed: Vec::new(),
    };
    let started = std::time::Instant::now();
    let files = case.run();
    let parts = &files[0].hunks[0].parts;
    assert_eq!(parts.first().map(|p| p.range), Some(lines(1, 5)));
    assert!(parts.len() > 20_000, "{}", parts.len());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

/// Fixture PR 482 through the real verification: every line an accepted flow covers is owned by a step of a flow
/// covering it (the verifier's step rule, shared through `analysis::verify::spans`), or by the flow without a step
/// only where the verifier needs none (a blank line).
#[tokio::test]
async fn owners_of_covered_lines_follow_the_verification_on_fixture_pr_482() {
    use std::collections::HashMap;

    use crate::pipeline::context::StoredResults;
    use crate::pipeline::tour;
    use crate::test_support::github::pr482;
    use db::repo::documents::DocumentKind;

    let f = pr482().await;
    let fixture = |path: &str| -> serde_json::Value {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482");
        serde_json::from_str(&std::fs::read_to_string(dir.join(path)).expect("fixture"))
            .expect("JSON")
    };
    let documents = f.state.db.documents();
    let now = contracts::time::Timestamp::now();
    for (kind, path) in [
        (DocumentKind::Plan, "agent/plan.json"),
        (DocumentKind::PrContext, "agent/pr_context.json"),
    ] {
        documents
            .put(f.summary.id, kind, "", &fixture(path), now)
            .await
            .expect("document");
    }
    let golden = fixture("api/default/tour.json");
    let mut threads = golden["threads"].clone();
    for thread in threads.as_array_mut().expect("threads") {
        for key in ["step_id", "llm_status", "llm_reason"] {
            thread[key] = serde_json::Value::Null;
        }
    }
    let snapshot = json!({ "ci": golden["pr_context"]["facts"]["ci"], "threads": threads, "threads_status": "checked" });
    documents
        .put(
            f.summary.id,
            DocumentKind::GithubSnapshot,
            "",
            &snapshot,
            now,
        )
        .await
        .expect("snapshot");
    let results = StoredResults::load(&f.state.db, f.summary.id)
        .await
        .expect("results");
    let (stepless, checked) =
        tour::with_projection(&f.state, &f.summary, results, |projection, tour| {
            let listed = from_tour(projection, &tour);
            let mut owners: HashMap<(LineSide, String, u32), PartOwner> = HashMap::new();
            for (file, listed) in projection.files.iter().zip(&listed) {
                for part in listed.hunks.iter().flat_map(|hunk| &hunk.parts) {
                    let Some(path) = file.path_on(part.side) else {
                        continue;
                    };
                    for line in part.range.start_line..=part.range.end_line {
                        owners.insert((part.side, path.to_owned(), line), part.owner.clone());
                    }
                }
            }
            let covering = |side: LineSide, path: &str, line: u32| -> Vec<&FlowId> {
                projection
                    .skeleton
                    .flows
                    .iter()
                    .filter(|flow| {
                        flow.covered.iter().any(|lines| {
                            lines.side == side
                                && lines.file == path
                                && lines
                                    .ranges
                                    .iter()
                                    .any(|range| range.start_line <= line && line <= range.end_line)
                        })
                    })
                    .map(|flow| &flow.flow_id)
                    .collect()
            };
            let mut stepless = Vec::new();
            let mut checked = 0;
            for flow in &projection.skeleton.flows {
                for lines in &flow.covered {
                    for range in &lines.ranges {
                        for line in range.start_line..=range.end_line {
                            let key = (lines.side, lines.file.clone(), line);
                            let owner = owners.get(&key).expect("a covered line is a changed line");
                            checked += 1;
                            match owner {
                                PartOwner::Step { flow_id, .. } => assert!(
                                    covering(lines.side, &lines.file, line).contains(&flow_id),
                                    "{key:?}: owned by a step of {flow_id}, which does not cover it"
                                ),
                                PartOwner::Flow { flow_id } => {
                                    assert_eq!(flow_id, &flow.flow_id, "{key:?}");
                                    stepless.push(key);
                                }
                                // Quiet kinds and Unexplained rows come first by priority.
                                _ => {}
                            }
                        }
                    }
                }
            }
            (stepless, checked)
        })
        .await
        .expect("tour");
    assert!(checked > 50, "{checked} covered lines");
    let (mirror, _) = crate::pipeline::pr_diff(&f.state, &f.summary)
        .await
        .expect("diff");
    for (side, path, line) in stepless {
        let commit = match side {
            LineSide::Head => &f.summary.pr.head_sha,
            LineSide::Base => &f.summary.pr.base_sha,
        };
        let bytes = crate::github::mirror::read_blob(&mirror, commit, &path)
            .await
            .expect("read")
            .expect("file");
        let text = String::from_utf8_lossy(&bytes);
        let at = text.lines().nth(line as usize - 1).unwrap_or_default();
        assert!(
            at.trim().is_empty(),
            "{side:?} {path}:{line} has no step but is not blank: {at:?}"
        );
    }
}
