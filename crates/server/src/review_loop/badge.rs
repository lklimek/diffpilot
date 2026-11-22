//! The PR state badge (plan §3.9 B1–B5): first match wins; facts diffpilot read select it; with an input unknown
//! (CI not read, GitHub read failed, a plug-in that can hold the PR not current) there is no "Ready to merge".

use contracts::api::review_loop::{PrBadge, PrBadgeView};
use contracts::api::tour::CiState;

/// What the badge is chosen from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BadgeInputs {
    /// The PR is open (merged or closed PRs get no badge).
    pub open: bool,
    /// The last GitHub read failed, or there was none.
    pub read_failed: bool,
    /// CI of the head; `None` when not read.
    pub ci: Option<CiState>,
    /// Failed effects.
    pub failed_effects: u32,
    /// Undecided questions.
    pub undecided: u32,
    /// Decisions with open or claimed effects.
    pub executing: u32,
    /// thepastaclaw reviews or queues this PR (plug-in, or its gate comment).
    pub bot_round: bool,
    /// Authors whose latest review requests changes.
    pub changes_requested: Vec<String>,
    /// A plug-in holds the PR back, with its source name.
    pub held_by: Option<String>,
    /// Plug-ins that can hold the PR whose statements are not current (names).
    pub holders_not_current: Vec<String>,
}

fn plural(count: u32, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The badge and its facts.
pub fn badge(inputs: &BadgeInputs) -> PrBadgeView {
    let mut view = PrBadgeView {
        badge: None,
        facts: Vec::new(),
        missing: Vec::new(),
    };
    if !inputs.open {
        view.facts.push("The pull request is not open".to_owned());
        return view;
    }
    if inputs.read_failed {
        view.missing.push("GitHub could not be read".to_owned());
    }
    match inputs.ci {
        None | Some(CiState::NotChecked) => view.missing.push("CI was not read".to_owned()),
        Some(_) => {}
    }
    for name in &inputs.holders_not_current {
        view.missing
            .push(format!("{name} statements are not current"));
    }
    let ci_failed = inputs.ci == Some(CiState::Failing);
    if inputs.failed_effects > 0 || ci_failed {
        if inputs.failed_effects > 0 {
            view.facts.push(plural(
                inputs.failed_effects,
                "failed effect",
                "failed effects",
            ));
        }
        if ci_failed {
            view.facts.push("CI failed".to_owned());
        }
        view.badge = Some(PrBadge::BlockedFailing);
        return view;
    }
    if inputs.undecided > 0 {
        view.facts.push(plural(
            inputs.undecided,
            "undecided question",
            "undecided questions",
        ));
        view.badge = Some(PrBadge::QuestionsWaiting);
        return view;
    }
    if inputs.executing > 0 {
        view.facts.push(plural(
            inputs.executing,
            "decision with open effects",
            "decisions with open effects",
        ));
        view.badge = Some(PrBadge::Executing);
        return view;
    }
    if inputs.bot_round {
        view.facts
            .push("thepastaclaw reports this PR reviewing or queued".to_owned());
        view.badge = Some(PrBadge::WaitingForBotRound);
        return view;
    }
    let mut ready = view.missing.is_empty();
    match inputs.ci {
        Some(CiState::Passing | CiState::NoFailures) => view.facts.push("CI passed".to_owned()),
        Some(CiState::None) => view.facts.push("No CI checks reported".to_owned()),
        Some(CiState::Pending) => {
            view.facts.push("CI is running".to_owned());
            ready = false;
        }
        Some(CiState::Failing | CiState::NotChecked) | None => ready = false,
    }
    if !inputs.changes_requested.is_empty() {
        view.facts.push(format!(
            "Changes requested by {}",
            inputs.changes_requested.join(", ")
        ));
        ready = false;
    }
    if let Some(source) = &inputs.held_by {
        view.facts.push(format!("{source} holds it back"));
        ready = false;
    }
    if ready {
        view.badge = Some(PrBadge::ReadyToMerge);
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> BadgeInputs {
        BadgeInputs {
            open: true,
            ci: Some(CiState::Passing),
            ..BadgeInputs::default()
        }
    }

    #[test]
    fn state_badge_precedence_and_unknown_inputs() {
        assert_eq!(badge(&ready()).badge, Some(PrBadge::ReadyToMerge));
        let all = BadgeInputs {
            failed_effects: 1,
            undecided: 2,
            executing: 3,
            bot_round: true,
            ..ready()
        };
        assert_eq!(badge(&all).badge, Some(PrBadge::BlockedFailing));
        let b2 = BadgeInputs {
            failed_effects: 0,
            ..all.clone()
        };
        assert_eq!(badge(&b2).badge, Some(PrBadge::QuestionsWaiting));
        assert_eq!(badge(&b2).facts, ["2 undecided questions"]);
        let b3 = BadgeInputs {
            undecided: 0,
            ..b2.clone()
        };
        assert_eq!(badge(&b3).badge, Some(PrBadge::Executing));
        let b4 = BadgeInputs { executing: 0, ..b3 };
        assert_eq!(badge(&b4).badge, Some(PrBadge::WaitingForBotRound));
        let ci_failed = BadgeInputs {
            ci: Some(CiState::Failing),
            ..ready()
        };
        assert_eq!(badge(&ci_failed).badge, Some(PrBadge::BlockedFailing));
        assert_eq!(badge(&ci_failed).facts, ["CI failed"]);

        for (case, inputs, missing) in [
            (
                "CI not read",
                BadgeInputs {
                    ci: None,
                    ..ready()
                },
                "CI was not read",
            ),
            (
                "CI not checked",
                BadgeInputs {
                    ci: Some(CiState::NotChecked),
                    ..ready()
                },
                "CI was not read",
            ),
            (
                "read failed",
                BadgeInputs {
                    read_failed: true,
                    ..ready()
                },
                "GitHub could not be read",
            ),
            (
                "holder not current",
                BadgeInputs {
                    holders_not_current: vec!["PR Hygiene".into()],
                    ..ready()
                },
                "PR Hygiene statements are not current",
            ),
        ] {
            let view = badge(&inputs);
            assert_eq!(
                view.badge, None,
                "{case}: unknown inputs show no Ready to merge"
            );
            assert_eq!(view.missing, [missing], "{case}");
        }
        let unknown_but_blocked = BadgeInputs {
            ci: None,
            undecided: 1,
            ..ready()
        };
        assert_eq!(
            badge(&unknown_but_blocked).badge,
            Some(PrBadge::QuestionsWaiting),
            "facts still select B1-B4"
        );
        for not_ready in [
            BadgeInputs {
                changes_requested: vec!["bob".into()],
                ..ready()
            },
            BadgeInputs {
                held_by: Some("PR Hygiene".into()),
                ..ready()
            },
            BadgeInputs {
                ci: Some(CiState::Pending),
                ..ready()
            },
        ] {
            assert_eq!(badge(&not_ready).badge, None, "{not_ready:?}");
        }
        let closed = BadgeInputs {
            open: false,
            ..ready()
        };
        assert_eq!(badge(&closed).badge, None);
    }
}
