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
use crate::diff::{FileStatus, Hunk, LineSpan, diff};

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
            facts,
            moves: &MovesReport::default(),
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
            facts: &facts,
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
