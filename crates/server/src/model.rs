//! Model selection: `select_model`, a plain function of job kind and context.

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

/// Model and reasoning effort for a job; v1: Sol at high effort for every job kind.
pub fn select_model(kind: JobKind, context: &ModelContext) -> ModelChoice {
    let _ = (kind, context);
    ModelChoice {
        model: SOL.to_owned(),
        reasoning_effort: ReasoningEffort::High,
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
                    select_model(kind, &ModelContext { changed_lines }),
                    ModelChoice {
                        model: "gpt-6.1-sol".to_owned(),
                        reasoning_effort: ReasoningEffort::High,
                    },
                    "{kind:?}"
                );
            }
        }
    }
}
