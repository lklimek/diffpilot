//! Installation model selection, applied at submission through one plain Rust function.

use contracts::agent::JobKind;
use contracts::job::spec::{ModelChoice, ReasoningEffort};

/// Codex model id of "Sol" (spike S1a).
const SOL: &str = "gpt-6.1-sol";

/// What model selection may look at besides the job kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelContext {
    /// Changed lines of the pull request (added plus removed).
    pub changed_lines: u32,
}

/// Uses the saved installation choice, falling back to Sol at high effort.
pub fn select_model(
    kind: JobKind,
    context: &ModelContext,
    setting: Option<&ModelChoice>,
) -> ModelChoice {
    let _ = (kind, context);
    setting
        .filter(|choice| allowed(choice))
        .cloned()
        .unwrap_or_else(|| ModelChoice {
            model: SOL.to_owned(),
            reasoning_effort: ReasoningEffort::High,
        })
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
pub fn selection(
    saved: Option<db::repo::settings::StoredLlmSelection>,
) -> contracts::api::settings::LlmSelection {
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
        current: select_model(
            JobKind::Plan,
            &ModelContext::default(),
            saved.as_ref().map(|s| &s.choice),
        ),
        changed_by: saved.as_ref().map(|s| s.changed_by.clone()),
        changed_at: saved.map(|s| s.changed_at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_model_returns_sol_high() {
        for kind in JobKind::ALL {
            for changed_lines in [0, 1, 100_000] {
                assert_eq!(
                    select_model(kind, &ModelContext { changed_lines }, None),
                    ModelChoice {
                        model: "gpt-6.1-sol".to_owned(),
                        reasoning_effort: ReasoningEffort::High,
                    },
                    "{kind:?}"
                );
            }
        }
    }
    #[test]
    fn select_model_uses_the_installation_choice_for_every_kind() {
        let choice = ModelChoice {
            model: "gpt-6-astra".into(),
            reasoning_effort: ReasoningEffort::Low,
        };
        for kind in JobKind::ALL {
            assert_eq!(
                select_model(kind, &ModelContext::default(), Some(&choice)),
                choice
            );
        }
    }
}
