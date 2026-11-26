//! Payer model selection, applied at submission through one plain Rust function.

use contracts::agent::JobKind;
use contracts::ids::UserId;
use contracts::job::spec::{ModelChoice, ReasoningEffort};
use db::error::DbError;
use db::repo::Db;
use db::repo::settings::{DEFAULT_ASK_EFFORT, DEFAULT_JUDGE_EFFORT, StoredLlmSelection};

/// Codex model id of "Sol" (spike S1a).
const SOL: &str = "gpt-6.1-sol";

/// What model selection may look at besides the job kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelContext {
    /// Changed lines of the pull request (added plus removed).
    pub changed_lines: u32,
}

/// The payer's effective choice (Sol at high effort by default), also for `verify_comments` jobs; Ask jobs use
/// the saved Ask effort instead ([`DEFAULT_ASK_EFFORT`] before any save) and dashboard ranking jobs the saved ranking
/// effort ([`DEFAULT_JUDGE_EFFORT`] before any save).
pub fn select_model(
    kind: JobKind,
    context: &ModelContext,
    saved: Option<&StoredLlmSelection>,
) -> ModelChoice {
    let _ = context;
    let choice = saved
        .and_then(|saved| saved.choice.as_ref())
        .filter(|choice| allowed(choice))
        .cloned()
        .unwrap_or_else(|| ModelChoice {
            model: SOL.to_owned(),
            reasoning_effort: ReasoningEffort::High,
        });
    match kind {
        JobKind::Ask => ModelChoice {
            reasoning_effort: saved
                .and_then(|saved| saved.ask_effort)
                .unwrap_or(DEFAULT_ASK_EFFORT),
            ..choice
        },
        JobKind::JudgeWork => ModelChoice {
            reasoning_effort: saved
                .and_then(|saved| saved.judge_effort)
                .unwrap_or(DEFAULT_JUDGE_EFFORT),
            ..choice
        },
        // `verify_comments` uses the analysis effort (plan §4.2; no own setting); at medium effort it misread
        // pre-existing code (spike S13-3).
        JobKind::Plan | JobKind::PrContext | JobKind::AnalyseFlow | JobKind::VerifyComments => {
            choice
        }
    }
}

/// Resolves effective preferences for the user whose linked plan pays for this job.
pub async fn select_model_for_payer(
    db: &Db,
    payer: &UserId,
    kind: JobKind,
    context: &ModelContext,
) -> Result<ModelChoice, DbError> {
    let saved = db.settings().llm_selection(&payer.0).await?;
    Ok(select_model(kind, context, saved.as_ref()))
}

/// Supported catalog: the Codex spike S1a models plus the 6.2 models of Codex 0.171 (ADR-002).
pub const MODELS: &[(&str, &str)] = &[
    ("gpt-6.2-sol", "Sol 6.2"),
    ("gpt-6.1-sol", "Sol 6.1"),
    ("gpt-6-sol", "Sol 6"),
    ("gpt-5.6-sol", "Sol 5.6"),
    ("gpt-6.2-astra", "Astra 6.2"),
    ("gpt-6-astra", "Astra 6"),
];

/// Accepts only the fixed model catalog; efforts are constrained by the wire enum.
pub fn allowed(choice: &ModelChoice) -> bool {
    MODELS.iter().any(|(id, _)| *id == choice.model)
}

/// The Settings projection, with default choice when nothing was saved yet.
pub fn selection(saved: Option<StoredLlmSelection>) -> contracts::api::settings::LlmSelection {
    use contracts::api::settings::{LlmModelOption, LlmSelection};
    LlmSelection {
        models: MODELS
            .iter()
            .map(|(id, label)| LlmModelOption {
                id: (*id).into(),
                label: (*label).into(),
            })
            .collect(),
        efforts: vec![
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ],
        current: select_model(JobKind::Plan, &ModelContext::default(), saved.as_ref()),
        judge_effort: Some(
            saved
                .as_ref()
                .and_then(|s| s.judge_effort)
                .unwrap_or(DEFAULT_JUDGE_EFFORT),
        ),
        ask_effort: Some(
            saved
                .as_ref()
                .and_then(|s| s.ask_effort)
                .unwrap_or(DEFAULT_ASK_EFFORT),
        ),
        changed_by: saved.as_ref().and_then(|s| s.changed_by.clone()),
        changed_at: saved.and_then(|s| s.changed_at),
    }
}

#[cfg(test)]
mod tests {
    use contracts::time::Timestamp;

    use super::*;

    fn saved(model: &str, effort: ReasoningEffort, ask: ReasoningEffort) -> StoredLlmSelection {
        StoredLlmSelection {
            choice: Some(ModelChoice {
                model: model.into(),
                reasoning_effort: effort,
            }),
            judge_effort: Some(ReasoningEffort::Medium),
            ask_effort: Some(ask),
            changed_by: Some("alice".into()),
            changed_at: Some(Timestamp::now()),
        }
    }

    #[test]
    fn defaults_are_sol_high_medium_for_ask_and_low_for_ranking() {
        for kind in JobKind::ALL {
            let effort = match kind {
                JobKind::Ask => ReasoningEffort::Medium,
                JobKind::JudgeWork => ReasoningEffort::Low,
                _ => ReasoningEffort::High,
            };
            for changed_lines in [0, 1, 100_000] {
                assert_eq!(
                    select_model(kind, &ModelContext { changed_lines }, None),
                    ModelChoice {
                        model: "gpt-6.1-sol".to_owned(),
                        reasoning_effort: effort,
                    },
                    "{kind:?}"
                );
            }
        }
    }

    /// `verify_comments` uses the payer's analysis model and effort.
    #[test]
    fn verify_comments_uses_the_analysis_choice() {
        let saved = saved("gpt-6-astra", ReasoningEffort::Medium, ReasoningEffort::Low);
        assert_eq!(
            select_model(
                JobKind::VerifyComments,
                &ModelContext::default(),
                Some(&saved)
            ),
            saved.choice.unwrap()
        );
        assert_eq!(
            select_model(JobKind::VerifyComments, &ModelContext::default(), None).reasoning_effort,
            ReasoningEffort::High
        );
    }

    #[test]
    fn select_model_judge_work_uses_judge_effort() {
        let saved = saved("gpt-6-astra", ReasoningEffort::High, ReasoningEffort::Low);
        assert_eq!(
            select_model(JobKind::JudgeWork, &ModelContext::default(), Some(&saved)),
            ModelChoice {
                model: "gpt-6-astra".into(),
                reasoning_effort: ReasoningEffort::Medium,
            }
        );
        assert_eq!(
            select_model(JobKind::JudgeWork, &ModelContext::default(), None).reasoning_effort,
            ReasoningEffort::Low
        );
    }

    #[test]
    fn ask_uses_the_saved_ask_effort_and_others_the_analysis_choice() {
        let saved = saved("gpt-6-astra", ReasoningEffort::High, ReasoningEffort::Low);
        for kind in JobKind::ALL {
            let expected = match kind {
                JobKind::Ask => ReasoningEffort::Low,
                JobKind::JudgeWork => ReasoningEffort::Medium,
                _ => ReasoningEffort::High,
            };
            assert_eq!(
                select_model(kind, &ModelContext::default(), Some(&saved)),
                ModelChoice {
                    model: "gpt-6-astra".into(),
                    reasoning_effort: expected,
                },
                "{kind:?}"
            );
        }
        let unlisted = StoredLlmSelection {
            choice: Some(ModelChoice {
                model: "unlisted".into(),
                reasoning_effort: ReasoningEffort::Low,
            }),
            ..saved
        };
        assert_eq!(
            select_model(JobKind::Ask, &ModelContext::default(), Some(&unlisted)).model,
            "gpt-6.1-sol",
            "a model outside the catalog falls back to Sol"
        );
    }

    /// Sol 6.2 and Astra 6.2 (Codex 0.171) are selectable; the default stays Sol 6.1.
    #[test]
    fn sol_and_astra_6_2_are_selectable_and_the_default_stays_sol_6_1() {
        let shown = selection(None);
        assert_eq!(shown.current.model, "gpt-6.1-sol");
        for (id, label) in [("gpt-6.2-sol", "Sol 6.2"), ("gpt-6.2-astra", "Astra 6.2")] {
            let saved = saved(id, ReasoningEffort::Medium, ReasoningEffort::Low);
            assert_eq!(
                select_model(JobKind::Plan, &ModelContext::default(), Some(&saved)),
                saved.choice.unwrap(),
                "{id}"
            );
            assert!(
                shown
                    .models
                    .iter()
                    .any(|option| option.id == id && option.label == label),
                "{id} offered as {label}"
            );
        }
    }

    #[test]
    fn selection_reports_the_ask_and_ranking_efforts() {
        assert_eq!(selection(None).ask_effort, Some(ReasoningEffort::Medium));
        assert_eq!(selection(None).judge_effort, Some(ReasoningEffort::Low));
        let saved = saved("gpt-6-astra", ReasoningEffort::High, ReasoningEffort::Low);
        let shown = selection(Some(saved));
        assert_eq!(shown.ask_effort, Some(ReasoningEffort::Low));
        assert_eq!(shown.judge_effort, Some(ReasoningEffort::Medium));
        assert_eq!(shown.current.reasoning_effort, ReasoningEffort::High);
    }
}
