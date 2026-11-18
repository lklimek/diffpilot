//! The stored judgement body (`dashboard_judgements.body`): a validated answer, and what the scorer and the item's
//! LLM text read from it.

use chrono::{DateTime, Utc};
use contracts::agent::judge_work::ValidAnswer;
use contracts::api::dashboard::{ItemKey, ItemText, ScoreComponent};
use serde::{Deserialize, Serialize};

use crate::dashboard::score::{EligibilityJudgement, Freshness, JudgedPart, Judgement};

/// A validated judge answer as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Body {
    /// Judged parts (defect severity as likelihood + impact).
    pub parts: Vec<BodyPart>,
    /// The eligibility decision; only when the scorer asked for it.
    pub eligibility: Option<BodyEligibility>,
    /// Suggested items this one covers.
    pub also_keys: Vec<ItemKey>,
    /// P0 alarm reason.
    pub alarm: Option<String>,
    /// Why it ranks where it does.
    pub why: String,
    /// Next action.
    pub next: String,
}

/// One judged part.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct BodyPart {
    pub component: ScoreComponent,
    pub points: i32,
    pub basis: String,
}

/// An eligibility decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct BodyEligibility {
    pub eligible: bool,
    pub reason: String,
}

impl Body {
    /// The stored form of a validated answer. An eligibility answer counts only when the scorer asked for it: an
    /// unasked flip could hide an item.
    pub fn of(answer: ValidAnswer, eligibility_asked: bool) -> Self {
        Self {
            parts: answer
                .parts
                .into_iter()
                .map(|part| BodyPart {
                    component: part.component,
                    points: part.points,
                    basis: part.basis,
                })
                .collect(),
            eligibility: answer.eligibility.filter(|_| eligibility_asked).map(|e| {
                BodyEligibility {
                    eligible: e.eligible,
                    reason: e.reason,
                }
            }),
            also_keys: answer.also_keys,
            alarm: answer.alarm,
            why: answer.why,
            next: answer.next,
        }
    }

    /// What the scorer merges into the item.
    pub fn judgement(&self, judged_at: DateTime<Utc>, freshness: Freshness) -> Judgement {
        Judgement {
            parts: self
                .parts
                .iter()
                .map(|part| JudgedPart {
                    component: part.component,
                    points: part.points,
                    basis: part.basis.clone(),
                })
                .collect(),
            eligibility: self.eligibility.as_ref().map(|e| EligibilityJudgement {
                eligible: e.eligible,
                reason: e.reason.clone(),
            }),
            alarm: self.alarm.is_some(),
            also: self.also_keys.clone(),
            judged_at,
            freshness,
        }
    }

    /// The item's LLM text.
    pub fn text(&self, judged_at: DateTime<Utc>, freshness: Freshness) -> ItemText {
        ItemText {
            why: self.why.clone(),
            next: self.next.clone(),
            alarm: self.alarm.clone(),
            judged_at,
            inherited: freshness == Freshness::Inherited,
            also_keys: self.also_keys.clone(),
        }
    }
}
