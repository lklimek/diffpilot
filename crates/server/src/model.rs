//! Installation model selection, applied at submission through one plain Rust function.

use contracts::agent::JobKind;
use contracts::job::spec::{ModelChoice, ReasoningEffort};
use db::repo::settings::{DEFAULT_ASK_EFFORT, DEFAULT_JUDGE_EFFORT, StoredLlmSelection};

/// Codex model id of "Sol" (spike S1a).
const SOL: &str = "gpt-6.1-sol";

/// What model selection may look at besides the job kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelContext {
    /// Changed lines of the pull request (added plus removed).
    pub changed_lines: u32,
}

/// The saved installation choice (Sol at high effort before any save); Ask jobs use the saved Ask effort
/// instead ([`DEFAULT_ASK_EFFORT`] before any save) and dashboard ranking jobs the saved ranking effort
/// ([`DEFAULT_JUDGE_EFFORT`] before any save).
pub fn select_model(
    kind: JobKind,
    context: &ModelContext,
    saved: Option<&StoredLlmSelection>,
) -> ModelChoice {
    let _ = context;
    let choice = saved
        .map(|saved| &saved.choice)
        .filter(|choice| allowed(choice))
        .cloned()
        .unwrap_or_else(|| ModelChoice {
            model: SOL.to_owned(),
            reasoning_effort: ReasoningEffort::High,
        });
    match kind {
        JobKind::Ask => ModelChoice {
            reasoning_effort: saved.map_or(DEFAULT_ASK_EFFORT, |saved| saved.ask_effort),
            ..choice
        },
        JobKind::JudgeWork => ModelChoice {
            reasoning_effort: saved.map_or(DEFAULT_JUDGE_EFFORT, |saved| saved.judge_effort),
            ..choice
        },
        JobKind::Plan | JobKind::PrContext | JobKind::AnalyseFlow => choice,
    }
}

/// Supported catalog recorded by the deployment's Codex spike S1a.
pub const MODELS: &[(&str, &str)] = &[
    ("gpt-6.1-sol", "Sol 6.1"),
    ("gpt-6-sol", "Sol 6"),
    ("gpt-5.6-sol", "Sol 5.6"),
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
                .map_or(DEFAULT_JUDGE_EFFORT, |s| s.judge_effort),
        ),
        ask_effort: Some(saved.as_ref().map_or(DEFAULT_ASK_EFFORT, |s| s.ask_effort)),
        changed_by: saved.as_ref().map(|s| s.changed_by.clone()),
        changed_at: saved.map(|s| s.changed_at),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn saved(model: &str, effort: ReasoningEffort, ask: ReasoningEffort) -> StoredLlmSelection {
        StoredLlmSelection {
            choice: ModelChoice {
                model: model.into(),
                reasoning_effort: effort,
            },
            judge_effort: ReasoningEffort::Medium,
            ask_effort: ask,
            changed_by: "alice".into(),
            changed_at: Utc::now(),
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
    fn ask_uses_the_saved_ask_effort_and_others_the_installation_choice() {
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
            choice: ModelChoice {
                model: "unlisted".into(),
                reasoning_effort: ReasoningEffort::Low,
            },
            ..saved
        };
        assert_eq!(
            select_model(JobKind::Ask, &ModelContext::default(), Some(&unlisted)).model,
            "gpt-6.1-sol",
            "a model outside the catalog falls back to Sol"
        );
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
