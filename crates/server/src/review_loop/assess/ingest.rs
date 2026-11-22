//! Ingest of a `verify_comments` result: the output is untrusted and re-validated (schema, items sent with the
//! pass only, closed sets, plain text within limits, wording rules of brief §1); each fit item assessment is stored
//! with the fingerprint it was sent at, an unfit one is left out (the item stays "Not assessed").

use std::collections::HashSet;

use analysis::verify::{self, LineFacts, wording};
use contracts::agent::ask::AskOutput;
use contracts::agent::verify_comments::{
    ItemAssessment, RecommendedDecision, VerifyCommentsOutput,
};
use contracts::agent::{self as agent, JobKind};
use contracts::api::review_loop::{CommentAssessment, DecisionTarget, Necessity, Recommendation};
use contracts::ids::{CommitSha, JobId};
use contracts::job::result::{JobOutcome, JobResult};
use contracts::time::Timestamp;
use db::repo::questions::QuestionUpdate;
use db::repo::review_loop::PassRecord;
use db::repo::settings::CallFailure;
use serde_json::Value;

use super::{AssessError, VERIFY_PROMPT_VERSION, ask};
use crate::api::settings::plain_failure;
use crate::state::AppState;
use analysis::blob::is_tree_path;

/// Category slugs of claudius `check-pr-comments` §4 (plan §2.2).
pub(super) const CATEGORIES: [&str; 17] = [
    "funds",
    "security",
    "privacy",
    "data-loss",
    "correctness",
    "concurrency",
    "error-handling",
    "performance",
    "state-freshness",
    "migration",
    "api-contract",
    "ux",
    "maintainability",
    "tests",
    "docs",
    "build-ci",
    "other",
];
/// Longest short text (reasons, notes, purpose), in characters.
const SHORT_CHARS: usize = 600;
/// Longest paragraph (code description, fix sketch, reply draft), in characters.
const PARAGRAPH_CHARS: usize = 1_500;
/// Most evidence lines per item.
const MAX_EVIDENCE: usize = 20;
/// Most entries of `elsewhere` and `reply_drafts` per item.
const MAX_LIST: usize = 5;

/// Why an item assessment is not stored; logged by kind only (the output is untrusted text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Unfit {
    /// A text is blank, over its limit, or not plain text.
    Text(&'static str),
    /// A text breaks the wording rules (a verdict, a percentage).
    Wording(&'static str),
    /// A list is over its limit.
    TooMany(&'static str),
    /// The category is not a catalog slug.
    Category,
    /// A path is not a repository path, or a line range is not valid.
    Location,
    /// The recommendation lacks the target its decision needs, or names a link outside GitHub.
    Target,
}

/// Ingests a pass's job result: stores the fit assessments and the PR purpose, answers or fails the pass's question,
/// and ends the pass. `true` when every sent item got an assessment.
pub(super) async fn result(
    state: &AppState,
    pass: &PassRecord,
    result: JobResult,
) -> Result<bool, AssessError> {
    let db = &state.db;
    let failure = match result.outcome {
        JobOutcome::Succeeded { output } => match parse(output)? {
            Some(output) => {
                let made = Made {
                    job: result.job_id,
                    skills_commit: result.skills_commit,
                };
                let complete = store(state, pass, &made, &output).await?;
                db.review_loop()
                    .finish_pass(pass.id, true, Timestamp::now())
                    .await?;
                tracing::info!(pass = %pass.id, complete, "comment assessment pass ingested");
                return Ok(complete);
            }
            None => {
                tracing::warn!(pass = %pass.id, "the verify_comments output does not fit its schema; nothing is stored");
                ask::UNFIT_ANSWER.to_owned()
            }
        },
        JobOutcome::Cancelled => ask::CANCELLED.to_owned(),
        JobOutcome::Failed { kind, .. } => {
            tracing::warn!(pass = %pass.id, ?kind, "the verify_comments job failed");
            format!(
                "{} Ask again.",
                plain_failure(Some(CallFailure::Failed(kind)))
            )
        }
        JobOutcome::UsageLimited { .. } => {
            format!(
                "{} Ask again.",
                plain_failure(Some(CallFailure::UsageLimited))
            )
        }
    };
    db.review_loop()
        .finish_pass(pass.id, false, Timestamp::now())
        .await?;
    ask::fail_pass_question(state, pass, &failure).await?;
    Ok(false)
}

/// The output if it fits the schema.
fn parse(output: Value) -> Result<Option<VerifyCommentsOutput>, AssessError> {
    let schema = agent::output_schema(JobKind::VerifyComments)?;
    let fits =
        jsonschema::validator_for(&schema).is_ok_and(|validator| validator.is_valid(&output));
    Ok(fits.then(|| serde_json::from_value(output).ok()).flatten())
}

/// What made a pass's assessments: its job and the claudius commit whose skills it read.
pub(super) struct Made {
    pub(super) job: JobId,
    pub(super) skills_commit: Option<String>,
}

/// Stores the fit assessments of sent items (the first entry per item counts), the purpose and the answer.
async fn store(
    state: &AppState,
    pass: &PassRecord,
    made: &Made,
    output: &VerifyCommentsOutput,
) -> Result<bool, AssessError> {
    let db = &state.db;
    let tenant = db
        .pull_requests()
        .get(pass.pull_request_id)
        .await?
        .tenant_id;
    let now = Timestamp::now();
    let mut seen = HashSet::new();
    let mut stored = HashSet::new();
    let mut ignored = 0_usize;
    for entry in &output.items {
        let Some(sent) = pass
            .sent
            .iter()
            .find(|sent| format!("C{}", sent.no) == entry.item)
        else {
            ignored += 1;
            continue;
        };
        if !seen.insert(sent.item) {
            ignored += 1;
            continue;
        }
        match assessment(entry, made, &pass.head, now) {
            Ok(assessment) => {
                db.review_loop()
                    .add_assessment(&tenant, sent.item, &assessment, &sent.fingerprint)
                    .await?;
                stored.insert(sent.item);
            }
            Err(unfit) => {
                tracing::warn!(pass = %pass.id, item = %sent.item, ?unfit,
                    "an item assessment did not pass the checks; the item stays not assessed");
            }
        }
    }
    if ignored > 0 {
        tracing::warn!(pass = %pass.id, ignored, "assessment entries of items not sent or repeated were ignored");
    }
    if fit_text(&output.purpose, SHORT_CHARS).is_ok() {
        db.review_loop()
            .set_purpose(pass.pull_request_id, &output.purpose)
            .await?;
    }
    if let Some(question) = pass.question {
        let update = match output.answer.as_deref().map(answer) {
            Some(Some(answer)) => QuestionUpdate::Answered(answer),
            _ => {
                tracing::warn!(pass = %pass.id, "the answer of a question pass did not pass the checks");
                QuestionUpdate::Failed(ask::UNFIT_ANSWER.to_owned())
            }
        };
        db.questions()
            .update(question, &update, Timestamp::now())
            .await?;
    }
    Ok(pass.sent.iter().all(|sent| stored.contains(&sent.item)))
}

/// A question pass's answer as an Ask answer, if it is plain text within the Ask limits and the wording rules.
fn answer(text: &str) -> Option<AskOutput> {
    let output = AskOutput {
        answer: text.to_owned(),
        evidence: Vec::new(),
    };
    (output.answer.chars().count() <= crate::ask::MAX_ANSWER_CHARS
        && wording::ask(&output).is_empty()
        && verify::ask(&output, &LineFacts::default()).is_ok())
    .then_some(output)
}

/// Plain, non-blank text within `max` characters and the wording rules.
fn fit_text(text: &str, max: usize) -> Result<(), Unfit> {
    let output = AskOutput {
        answer: text.to_owned(),
        evidence: Vec::new(),
    };
    if text.chars().count() > max || verify::ask(&output, &LineFacts::default()).is_err() {
        return Err(Unfit::Text("text"));
    }
    if !wording::warnings_in(text).is_empty() {
        return Err(Unfit::Wording("text"));
    }
    Ok(())
}

/// `fit_text` naming the field on failure.
fn checked(text: &str, max: usize, field: &'static str) -> Result<(), Unfit> {
    fit_text(text, max).map_err(|unfit| match unfit {
        Unfit::Wording(_) => Unfit::Wording(field),
        _ => Unfit::Text(field),
    })
}

/// The stored assessment of one fit entry.
pub(super) fn assessment(
    entry: &ItemAssessment,
    made: &Made,
    head: &CommitSha,
    now: Timestamp,
) -> Result<CommentAssessment, Unfit> {
    if !CATEGORIES.contains(&entry.category.as_str()) {
        return Err(Unfit::Category);
    }
    checked(&entry.necessity_reason, SHORT_CHARS, "necessity_reason")?;
    if let Some(better) = &entry.better_pr {
        checked(better, SHORT_CHARS, "better_pr")?;
    }
    if entry.evidence.len() > MAX_EVIDENCE {
        return Err(Unfit::TooMany("evidence"));
    }
    for evidence in &entry.evidence {
        if !is_tree_path(&evidence.path) || evidence.line == 0 {
            return Err(Unfit::Location);
        }
        checked(&evidence.note, SHORT_CHARS, "evidence.note")?;
    }
    if entry.elsewhere.len() > MAX_LIST || entry.reply_drafts.len() > MAX_LIST {
        return Err(Unfit::TooMany("elsewhere or reply_drafts"));
    }
    for elsewhere in &entry.elsewhere {
        checked(&elsewhere.reference, SHORT_CHARS, "elsewhere.reference")?;
        checked(&elsewhere.note, SHORT_CHARS, "elsewhere.note")?;
    }
    for draft in &entry.reply_drafts {
        checked(draft, PARAGRAPH_CHARS, "reply_drafts")?;
    }
    if let Some(sketch) = &entry.fix_sketch {
        checked(sketch, PARAGRAPH_CHARS, "fix_sketch")?;
    }
    if let Some(code) = &entry.code {
        let range = code.enclosing;
        if !is_tree_path(&code.path) || range.start_line == 0 || range.start_line > range.end_line {
            return Err(Unfit::Location);
        }
        checked(&code.description, PARAGRAPH_CHARS, "code.description")?;
    }
    let recommended = &entry.recommendation;
    checked(&recommended.reason, SHORT_CHARS, "recommendation.reason")?;
    checked(
        &recommended.reason_class,
        SHORT_CHARS,
        "recommendation.reason_class",
    )?;
    let title = |title: &Option<String>| -> Result<Option<String>, Unfit> {
        match title {
            Some(title) => {
                checked(title, SHORT_CHARS, "recommendation.title").map(|()| Some(title.clone()))
            }
            None => Ok(None),
        }
    };
    let target = match recommended.kind {
        RecommendedDecision::Resolve
        | RecommendedDecision::FixHere
        | RecommendedDecision::Reject => None,
        RecommendedDecision::HandledInPr => Some(DecisionTarget::ExistingPr {
            number: recommended
                .target_pr
                .filter(|number| *number > 0)
                .ok_or(Unfit::Target)?,
        }),
        RecommendedDecision::FollowUpPr => Some(DecisionTarget::FollowUpPr {
            title: title(&recommended.title)?,
        }),
        RecommendedDecision::UpstreamIssue => {
            // Links are shown only to GitHub (threat model T24).
            if recommended.target_url.as_ref().is_some_and(|url| {
                !url.starts_with("https://github.com/") || url.contains(char::is_whitespace)
            }) {
                return Err(Unfit::Target);
            }
            Some(DecisionTarget::UpstreamIssue {
                url: recommended.target_url.clone(),
            })
        }
        RecommendedDecision::Todo => Some(DecisionTarget::Todo {
            title: title(&recommended.title)?.ok_or(Unfit::Target)?,
        }),
    };
    Ok(CommentAssessment {
        job: made.job,
        head: head.clone(),
        assessed_at: now,
        reused_since: None,
        validity: entry.validity,
        evidence: entry.evidence.clone(),
        pre_existing: entry.pre_existing,
        category: entry.category.clone(),
        necessity: Necessity {
            verdict: entry.necessity,
            reason: entry.necessity_reason.clone(),
            better_pr: entry.better_pr.clone(),
        },
        gates: entry.gates.clone(),
        elsewhere: entry.elsewhere.clone(),
        fix_sketch: entry.fix_sketch.clone(),
        fix_size: entry.fix_size,
        reply_drafts: entry.reply_drafts.clone(),
        recommendation: Recommendation {
            kind: recommended.kind.into(),
            reason: recommended.reason.clone(),
            target,
            reason_class: recommended.reason_class.clone(),
        },
        code: entry.code.clone(),
        prompt_version: VERIFY_PROMPT_VERSION,
        skills_commit: made.skills_commit.clone().filter(|commit| {
            (7..=64).contains(&commit.len()) && commit.chars().all(|c| c.is_ascii_hexdigit())
        }),
    })
}
