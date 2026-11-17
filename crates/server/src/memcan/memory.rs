//! The MemCan memory of a feedback entry (text and metadata) and the lookup of the step or annotation a feedback
//! target names (server ids `f1.2` and `f1.2.a1`, see `pipeline::tour`).

use chrono::{DateTime, Utc};
use contracts::agent::analyse_flow::{AgentAnnotation, AgentStep, FlowAnalysisOutput};
use contracts::api::feedback::{FeedbackTarget, FeedbackValue};
use contracts::api::pr::PrSummary;
use contracts::common::{AnnotationType, LineSide, RiskBand};
use db::repo::feedback::PendingFeedback;
use serde_json::{Value, json};

/// Longest LLM, pull-request or path text copied into a memory, in characters.
const MAX_FIELD_CHARS: usize = 300;
/// Longest note copied into a memory, in characters (the API accepts no longer note).
const MAX_NOTE_CHARS: usize = contracts::api::feedback::FeedbackRequest::MAX_NOTE_CHARS;
/// Metadata `type` of a severity calibration example (`overrated` / `underrated`).
pub(crate) const CALIBRATION_TYPE: &str = "severity-calibration";
/// Metadata `type` of any other feedback.
const FEEDBACK_TYPE: &str = "review-feedback";
/// Metadata `source` of every memory the server writes.
pub(crate) const SOURCE: &str = "smart-review";

/// What a step or annotation feedback target names in the analysis.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Subject<'a> {
    /// A step.
    Step(&'a AgentStep),
    /// An annotation of a step.
    Annotation(&'a AgentStep, &'a AgentAnnotation),
}

/// The step or annotation `target` names; `None` for a line target or an id the analysis does not have.
pub(crate) fn subject<'a>(
    flows: &'a [FlowAnalysisOutput],
    target: &FeedbackTarget,
) -> Option<Subject<'a>> {
    match target {
        FeedbackTarget::Step { step_id } => step(flows, &step_id.0).map(Subject::Step),
        FeedbackTarget::Annotation { annotation_id } => {
            let (step_id, number) = annotation_id.0.rsplit_once(".a")?;
            let step = step(flows, step_id)?;
            let annotation = step.annotations.get(position(number)?)?;
            Some(Subject::Annotation(step, annotation))
        }
        FeedbackTarget::Line { .. } => None,
    }
}

/// Step `f<flow>.<n>`: the n-th step (1-based) of that flow's output.
// TODO(WP3-PIPE): share the step / annotation id rule with `pipeline::tour` (built inline there with `format!`).
fn step<'a>(flows: &'a [FlowAnalysisOutput], id: &str) -> Option<&'a AgentStep> {
    let (flow, number) = id.split_once('.')?;
    let flow = flows.iter().find(|output| output.flow_id.0 == flow)?;
    flow.steps.get(position(number)?)
}

/// Zero-based position of a 1-based number in the Tour's canonical form (decimal digits, no leading zero), so
/// every step or annotation has exactly one id.
fn position(number: &str) -> Option<usize> {
    if number.starts_with('0')
        || number.is_empty()
        || !number.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    number.parse::<usize>().ok()?.checked_sub(1)
}

/// One `_import_records` JSONL record: the memory stored under the feedback id in the analysed repository's
/// project. These are all fields that leave the server for MemCan.
pub(crate) fn import_record(
    entry: &PendingFeedback,
    pr: &PrSummary,
    flows: &[FlowAnalysisOutput],
    now: DateTime<Utc>,
) -> Value {
    let feedback = &entry.feedback;
    let project = &pr.repo.name;
    let kind = match feedback.value {
        FeedbackValue::Overrated | FeedbackValue::Underrated => CALIBRATION_TYPE,
        _ => FEEDBACK_TYPE,
    };
    json!({
        "_collection": "memories",
        "id": feedback.id.to_string(),
        "data": text(entry, pr, flows),
        "user_id": format!("project:{project}"),
        "project": project,
        "collection": "memories",
        "created_at": feedback.created_at.to_rfc3339(),
        "updated_at": now.to_rfc3339(),
        "type": kind,
        "source": SOURCE,
        "value": value_name(feedback.value),
        "feedback_id": feedback.id.to_string(),
        "revision": entry.revision,
        "repo": format!("{}/{}", pr.repo.owner, pr.repo.name),
        "pr": pr.number,
    })
}

fn text(entry: &PendingFeedback, pr: &PrSummary, flows: &[FlowAnalysisOutput]) -> String {
    let feedback = &entry.feedback;
    let verdict = match feedback.value {
        FeedbackValue::Overrated => {
            "severity calibration example: the reviewer judged the LLM's risk rating too high (overrated)"
        }
        FeedbackValue::Underrated => {
            "severity calibration example: the reviewer judged the LLM's risk rating too low (underrated)"
        }
        FeedbackValue::Correct => "the reviewer marked the LLM analysis as correct",
        FeedbackValue::Wrong => "the reviewer marked the LLM analysis as wrong",
        FeedbackValue::Irrelevant => "the reviewer marked the LLM analysis as irrelevant",
        FeedbackValue::MissedByLlm => {
            "the reviewer commented on a changed line the LLM did not annotate (missed by the LLM)"
        }
    };
    let subject = match subject(flows, &feedback.target) {
        Some(Subject::Step(step)) => {
            let mut text = format!("Step {}", place(step));
            if let Some(assessment) = &step.assessment {
                text.push_str(&format!(
                    "; LLM assessment: {} risk, \"{}\"",
                    risk_name(assessment.risk),
                    clip(&assessment.headline, MAX_FIELD_CHARS)
                ));
            }
            text
        }
        Some(Subject::Annotation(step, annotation)) => format!(
            "{} annotation at lines {}-{} of step {}: \"{}\"",
            annotation_name(annotation.kind),
            annotation.start_line,
            annotation.end_line,
            place(step),
            clip(
                annotation.text.as_deref().unwrap_or(&annotation.is),
                MAX_FIELD_CHARS
            )
        ),
        None => match &feedback.target {
            FeedbackTarget::Line { path, side, line } => format!(
                "Line {line} of {} ({} side)",
                clip(path, MAX_FIELD_CHARS),
                match side {
                    LineSide::Head => "new",
                    LineSide::Base => "old",
                }
            ),
            FeedbackTarget::Step { step_id } => {
                format!("Step {}", clip(&step_id.0, MAX_FIELD_CHARS))
            }
            FeedbackTarget::Annotation { annotation_id } => {
                format!("Annotation {}", clip(&annotation_id.0, MAX_FIELD_CHARS))
            }
        },
    };
    let mut text = format!(
        "smart-review feedback on the LLM review of {}/{} pull request #{} \"{}\": {verdict}. {subject}.",
        clip(&pr.repo.owner, MAX_FIELD_CHARS),
        clip(&pr.repo.name, MAX_FIELD_CHARS),
        pr.number,
        clip(&pr.title, MAX_FIELD_CHARS),
    );
    if let Some(note) = feedback
        .note
        .as_deref()
        .filter(|note| !note.trim().is_empty())
    {
        text.push_str(&format!(
            " Reviewer note: \"{}\"",
            clip(note, MAX_NOTE_CHARS)
        ));
    }
    text
}

/// `"<title>" at <file>:<start>-<end>`.
fn place(step: &AgentStep) -> String {
    format!(
        "\"{}\" at {}:{}-{}",
        clip(&step.title, MAX_FIELD_CHARS),
        clip(&step.file, MAX_FIELD_CHARS),
        step.start_line,
        step.end_line
    )
}

/// Single-line text of at most `max` characters: control characters and runs of whitespace become one space.
pub(crate) fn clip(text: &str, max: usize) -> String {
    let mut clipped = String::new();
    for word in text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
    {
        if !clipped.is_empty() {
            clipped.push(' ');
        }
        clipped.push_str(word);
    }
    match clipped.char_indices().nth(max) {
        Some((end, _)) => clipped[..end].to_owned(),
        None => clipped,
    }
}

/// The serde name of a feedback value.
pub(crate) fn value_name(value: FeedbackValue) -> &'static str {
    match value {
        FeedbackValue::Correct => "correct",
        FeedbackValue::Wrong => "wrong",
        FeedbackValue::Irrelevant => "irrelevant",
        FeedbackValue::Overrated => "overrated",
        FeedbackValue::Underrated => "underrated",
        FeedbackValue::MissedByLlm => "missed_by_llm",
    }
}

fn risk_name(risk: RiskBand) -> &'static str {
    match risk {
        RiskBand::Low => "low",
        RiskBand::Medium => "medium",
        RiskBand::High => "high",
    }
}

fn annotation_name(kind: AnnotationType) -> &'static str {
    match kind {
        AnnotationType::Behaviour => "Behaviour",
        AnnotationType::Consequence => "Consequence",
        AnnotationType::Question => "Question",
        AnnotationType::Note => "Note",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use chrono::TimeZone;
    use contracts::api::feedback::{Feedback, FeedbackMemory};
    use contracts::common::RepoRef;
    use contracts::ids::{AnalysisId, AnnotationId, CommitSha, FeedbackId, StepId};

    use super::*;

    fn flows() -> Vec<FlowAnalysisOutput> {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pr-482/agent/flow-3.json");
        vec![serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("flow")]
    }

    fn pr() -> PrSummary {
        PrSummary {
            repo: RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned(),
            },
            number: 482,
            title: "Retry\nRPC calls".to_owned(),
            author: "alice".to_owned(),
            html_url: "https://github.com/acme/node-client/pull/482".to_owned(),
            base_sha: CommitSha("1".repeat(40)),
            head_sha: CommitSha("9".repeat(40)),
        }
    }

    fn entry(target: FeedbackTarget, value: FeedbackValue, note: Option<&str>) -> PendingFeedback {
        PendingFeedback {
            analysis_id: AnalysisId::new_random(),
            feedback: Feedback {
                id: FeedbackId::new_random(),
                target,
                value,
                note: note.map(str::to_owned),
                created_at: Utc
                    .with_ymd_and_hms(2026, 11, 17, 12, 0, 0)
                    .single()
                    .unwrap_or_default(),
                memory: FeedbackMemory::Pending,
            },
            revision: 3,
            removed: false,
            attempts: 0,
        }
    }

    fn step_target(id: &str) -> FeedbackTarget {
        FeedbackTarget::Step {
            step_id: StepId(id.to_owned()),
        }
    }

    fn annotation_target(id: &str) -> FeedbackTarget {
        FeedbackTarget::Annotation {
            annotation_id: AnnotationId(id.to_owned()),
        }
    }

    #[test]
    fn server_step_and_annotation_ids_resolve_like_the_tour() {
        let flows = flows();
        assert!(matches!(
            subject(&flows, &step_target("f3.1")),
            Some(Subject::Step(step)) if step.id == "s1"
        ));
        assert!(matches!(
            subject(&flows, &annotation_target("f3.1.a2")),
            Some(Subject::Annotation(step, annotation))
                if step.id == "s1" && annotation.kind == AnnotationType::Consequence
        ));
        for unknown in [
            "f3.0", "f3.9", "f9.1", "f3", "f3.+1", "f3.1.x", "f3.01", " f3.1",
        ] {
            assert!(
                subject(&flows, &step_target(unknown)).is_none(),
                "{unknown}"
            );
        }
        for unknown in [
            "f3.1.a0", "f3.1.a9", "f3.1", "f3.1.a", "f3.1.a02", "f3.01.a2",
        ] {
            assert!(
                subject(&flows, &annotation_target(unknown)).is_none(),
                "{unknown}"
            );
        }
    }

    #[test]
    fn the_record_holds_exactly_the_documented_fields() {
        let record = import_record(
            &entry(step_target("f3.1"), FeedbackValue::Underrated, Some("n")),
            &pr(),
            &flows(),
            Utc::now(),
        );
        let keys: BTreeSet<&str> = record
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                "_collection",
                "id",
                "data",
                "user_id",
                "project",
                "collection",
                "created_at",
                "updated_at",
                "type",
                "source",
                "value",
                "feedback_id",
                "revision",
                "repo",
                "pr",
            ])
        );
        assert_eq!(record["id"], record["feedback_id"]);
        assert_eq!(record["revision"], 3);
        assert_eq!(record["repo"], "acme/node-client");
        assert_eq!(record["type"], "severity-calibration");
    }

    #[test]
    fn memory_text_is_one_bounded_line() {
        let note = "line one\nline two\u{7}";
        let record = import_record(
            &entry(
                annotation_target("f3.1.a2"),
                FeedbackValue::Wrong,
                Some(note),
            ),
            &pr(),
            &flows(),
            Utc::now(),
        );
        let text = record["data"].as_str().expect("text");
        assert!(!text.chars().any(char::is_control), "{text}");
        assert!(text.contains("\"Retry RPC calls\""), "{text}");
        assert!(text.contains("Consequence annotation at lines"), "{text}");
        assert!(
            text.contains("Reviewer note: \"line one line two\""),
            "{text}"
        );
        assert_eq!(record["type"], "review-feedback");

        let missed = import_record(
            &entry(
                FeedbackTarget::Line {
                    path: format!("src/{}", "p".repeat(1000)),
                    side: LineSide::Head,
                    line: 96,
                },
                FeedbackValue::MissedByLlm,
                None,
            ),
            &pr(),
            &[],
            Utc::now(),
        );
        let text = missed["data"].as_str().expect("text");
        assert!(text.contains("missed by the LLM"), "{text}");
        assert!(text.contains("Line 96 of src/ppp"), "{text}");
        assert!(text.chars().count() < 1000, "path clipped: {}", text.len());
        assert!(!text.contains("Reviewer note"), "{text}");
    }

    #[test]
    fn clip_cuts_at_characters_not_bytes() {
        assert_eq!(clip("ąę ść", 3), "ąę ");
        assert_eq!(clip("  a \t b  ", 10), "a b");
    }
}
