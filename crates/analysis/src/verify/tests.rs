//! Whole-analysis tests of [`super::analysis`] over the fixture pull request `acme/node-client#482`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use contracts::agent::analyse_flow::FlowAnalysisOutput;
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
        [file_ranges(
            "src/rpc/metrics.rs",
            LineSide::Head,
            &[(40, 46)]
        )],
        "the plan's unassigned hunk h22 is Unexplained"
    );
    assert_eq!(tour.coverage.double_covered, []);
    let steps: usize = tour.flows.iter().map(|flow| flow.step_ids.len()).sum();
    assert_eq!(steps, 17);
    assert!(!verdict.moves_incomplete);
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
        })
        .expect("verdict")
    };

    // Another pull request with 30 hunks: every id of the plan exists, most hunks are in no flow.
    let verdict = run(&foreign(&(1..=30).collect::<Vec<_>>()));
    assert!(verdict.tour.is_none());
    assert_eq!(verdict.repairs[0].output, OutputRef::Plan);
    let unassigned = verdict.repairs[0].issues.iter();
    let unassigned = unassigned.filter(|issue| issue.kind == VerifyIssueKind::LineUnassigned);
    assert_eq!(unassigned.count(), 19, "{:#?}", verdict.repairs[0]);
    assert_eq!(
        verdict.repairs[1].output,
        OutputRef::PrContext,
        "its evidence is not in this tree"
    );

    // Worst case: exactly the plan's hunk ids. The plan fits, but no flow output does.
    let verdict = run(&foreign(&[11, 12, 13, 14, 15, 16, 18, 21, 22, 23, 24]));
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
        11 * 3,
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
    // f1 (rank 1, rated HIGH) becomes LLM-classified tests, f4 an LLM-classified refactor.
    classify(&mut fixture.plan, "f1", Classification::Test);
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
    assert_eq!(flow_ids(&tour.flows), ["f3", "f5", "f4", "f1"]);
    let f1 = tour.flows.last().expect("f1");
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
                assessment: None,
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
            .filter(|(_, answered)| **answered)
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
