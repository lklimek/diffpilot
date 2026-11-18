//! The `judge_work` job (plan §B, ADR-017): up to [`MAX_ITEMS`] dashboard items with the facts the judge may see,
//! and its answer. The input travels in the prompt as one fenced JSON data block ([`JudgeWorkInput::data_block`]);
//! the answer is untrusted and re-validated against that input ([`JudgeWorkOutput::validate`]), so a wrong or
//! hostile answer can only leave an item provisional or move it within the closed value sets.

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent::strict::{self, StrictSchemaError};
use crate::api::dashboard::{ItemKey, ItemKind, PartStatus, ScoreComponent};

/// Items per job at most (plan §B.2).
pub const MAX_ITEMS: usize = 8;
/// Longest basis of a part or reason of an eligibility answer, in characters.
pub const BASIS_MAX_CHARS: usize = 200;
/// Longest `why`, in characters.
pub const WHY_MAX_CHARS: usize = 300;
/// Longest `next`, in characters.
pub const NEXT_MAX_CHARS: usize = 200;
/// Longest alarm reason, in characters.
pub const ALARM_MAX_CHARS: usize = 300;
/// Largest defect likelihood or impact; defect severity points = likelihood + impact (plan §B.1 J1).
pub const DEFECT_AXIS_MAX: u8 = 10;

/// First line of the input's data block in the prompt; the block ends with a line of three backticks.
pub const DATA_BLOCK_OPEN: &str = "```json judge-work-input";
const DATA_BLOCK_CLOSE: &str = "```";

/// Components a `parts` answer may name, with their closed point values (the scorer's `allowed_points`; a server
/// test keeps both equal). Defect severity is answered as `defect`; recency and change size are never judged.
pub const PART_VALUES: [(ScoreComponent, &[i32]); 9] = [
    (ScoreComponent::Advancement, &[0, 10, 15, 20, 30]),
    (ScoreComponent::Unblocks, &[0, 5, 10, 15]),
    (ScoreComponent::HumanWaiting, &[0, 5, 10, 15]),
    (ScoreComponent::ChangeImpact, &[0, 5, 10]),
    (ScoreComponent::Completion, &[0, 5, 10]),
    (ScoreComponent::UrgencyBonus, &[0, 30]),
    (ScoreComponent::BlockedPenalty, &[-20, 0]),
    (ScoreComponent::DefaultBranchBonus, &[0, 5]),
    (ScoreComponent::MilestoneBonus, &[-10, 0, 10, 20]),
];

/// The closed point values of a `parts` component; `None` for defect severity, recency and change size.
pub fn part_values(component: ScoreComponent) -> Option<&'static [i32]> {
    PART_VALUES
        .iter()
        .find(|(candidate, _)| *candidate == component)
        .map(|(_, values)| *values)
}

/// What one `judge_work` job judges. Every string that comes from GitHub or MemCan is untrusted data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgeWorkInput {
    /// 1..=[`MAX_ITEMS`] items with distinct keys.
    pub items: Vec<JudgeItem>,
}

/// One item to judge: the plan §B.4 judged inputs plus what the answer must cover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgeItem {
    /// Stable key; the answer names the item by it.
    pub key: ItemKey,
    /// Display id, e.g. `PLAT-1187`.
    pub id: String,
    /// Kind.
    pub kind: ItemKind,
    /// Scan facts.
    pub facts: JudgeFacts,
    /// Parts known without the judge (computed values and defaults, with their basis); never recency or change
    /// size, which rest on ages (not judged inputs, plan §B.4).
    pub known: Vec<KnownPart>,
    /// Components the answer must judge (the scorer's `Scored.asks`).
    pub asks: Vec<ScoreComponent>,
    /// The answer must decide whether the item is ranked (the scorer's `Scored.asks_eligibility`).
    pub asks_eligibility: bool,
    /// The computed eligibility the judge confirms or replaces.
    pub eligibility: Eligibility,
    /// The previous judgement when the item changed since it was judged; null otherwise.
    pub previous: Option<PreviousJudgement>,
    /// Items the server suggests merging into this one (explicit references); `also_keys` names only these.
    pub suggested_merges: Vec<ItemKey>,
}

/// Scan facts of one item; fields that do not apply to its kind are null, false or empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgeFacts {
    /// Title.
    pub title: String,
    /// Body excerpt (≤ 300 characters, whitespace-normalised) or TODO description (≤ 500 characters).
    pub text: String,
    /// Repository `owner/name`, or the MemCan project of a TODO.
    pub place: String,
    /// Relations to the scanned logins: `authored`, `assigned`, `review_requested`, `repo_open`, `mentioned`.
    pub relations: Vec<String>,
    /// Author login.
    pub author: Option<String>,
    /// Label names, sorted.
    pub labels: Vec<String>,
    /// Draft pull request.
    pub draft: bool,
    /// Effective CI state: `green`, `red`, `pending`, `unknown`, `none`.
    pub ci: Option<String>,
    /// Names of failing checks.
    pub failing_checks: Vec<String>,
    /// `mergeable`, `conflicting` or `unknown`.
    pub mergeable: Option<String>,
    /// `changes_requested`, `approved` or `review_required`.
    pub review_decision: Option<String>,
    /// A human outside the scanned logins requests changes.
    pub human_change_request: bool,
    /// Unresolved review threads as a class: `0`, `1-2`, `3+`.
    pub unresolved_threads: Option<String>,
    /// The last comment is a human's and awaits our reply.
    pub awaiting_our_reply: bool,
    /// Base branch.
    pub base_branch: Option<String>,
    /// The base is the repository's default branch.
    pub targets_default_branch: Option<bool>,
    /// The base is a release or long-lived branch.
    pub targets_release_branch: Option<bool>,
    /// Display ids of open pull requests based on this one.
    pub dependents: Vec<String>,
    /// Display ids of open pull requests this one is stacked on.
    pub stacked_on: Vec<String>,
    /// Milestone title.
    pub milestone: Option<String>,
    /// Released versions and milestone titles of the repository, when the milestone mapping is unknown.
    pub release_context: Option<String>,
    /// Linked issues.
    pub linked_issues: Vec<LinkedIssue>,
    /// Ownership-policy verdict: `no_policy`, `unknown`, `none`, `minor`, `significant`.
    pub owned_change: Option<String>,
    /// Changed lines as a class: `<50`, `<200`, `<1000`, `<5000`, `>=5000`.
    pub change_size: Option<String>,
    /// TODO facts; null for pull requests and issues.
    pub todo: Option<TodoJudgeFacts>,
}

/// An issue linked to a pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedIssue {
    /// `owner/repo#number`.
    pub reference: String,
    /// Title.
    pub title: String,
}

/// MemCan TODO facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoJudgeFacts {
    /// `pending`, `in_progress`, `blocked` or `postponed`.
    pub status: String,
    /// `high`, `medium` or `low`, as stored.
    pub priority: Option<String>,
    /// Owner, as stored.
    pub owner: Option<String>,
    /// Number of TODOs it is blocked by.
    pub blocked_by: u32,
}

/// A part known before judging.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownPart {
    /// Component.
    pub component: ScoreComponent,
    /// Points in tenths; null while pending.
    pub points_tenths: Option<i32>,
    /// How it was reached.
    pub status: PartStatus,
    /// Basis.
    pub basis: String,
}

/// Whether the item is ranked, with the reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Eligibility {
    /// Ranked (true) or left out (false).
    pub eligible: bool,
    /// Plain reason.
    pub reason: String,
}

/// The judgement made before the item changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviousJudgement {
    /// Judged parts.
    pub parts: Vec<KnownPart>,
    /// Previous `why`.
    pub why: String,
    /// Previous `next`.
    pub next: String,
    /// Previous alarm reason.
    pub alarm: Option<String>,
}

/// An input that cannot be sent to the judge.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    /// No items, or more than [`MAX_ITEMS`].
    #[error("a judge job takes 1 to {MAX_ITEMS} items, not {0}")]
    ItemCount(usize),
    /// Two items share a key.
    #[error("item {0} appears twice")]
    DuplicateItem(ItemKey),
    /// A known part rests on ages, which are not judged inputs.
    #[error("item {key} lists the {component:?} part, which is never shown to the judge")]
    AgePart {
        /// Item.
        key: ItemKey,
        /// Component.
        component: ScoreComponent,
    },
    /// An asked component the judge cannot answer.
    #[error("item {key} asks for the {component:?} part, which is never judged")]
    NotJudgeable {
        /// Item.
        key: ItemKey,
        /// Component.
        component: ScoreComponent,
    },
    /// An item is suggested as a merge into itself.
    #[error("item {0} is suggested as a merge into itself")]
    SelfMerge(ItemKey),
    /// The data block could not be rendered.
    #[error("the judge input could not be rendered as JSON")]
    Json(String),
}

impl JudgeWorkInput {
    /// Checks the shape the answer is validated against: item count, distinct keys, judgeable asks, no age-based
    /// known parts, no self-merge.
    pub fn check(&self) -> Result<(), InputError> {
        if self.items.is_empty() || self.items.len() > MAX_ITEMS {
            return Err(InputError::ItemCount(self.items.len()));
        }
        let mut seen = HashSet::new();
        for item in &self.items {
            if !seen.insert(&item.key) {
                return Err(InputError::DuplicateItem(item.key.clone()));
            }
            if let Some(part) = item.known.iter().find(|part| is_age_part(part.component)) {
                return Err(InputError::AgePart {
                    key: item.key.clone(),
                    component: part.component,
                });
            }
            if let Some(component) = item.asks.iter().find(|c| !is_judgeable(**c)) {
                return Err(InputError::NotJudgeable {
                    key: item.key.clone(),
                    component: *component,
                });
            }
            if item.suggested_merges.contains(&item.key) {
                return Err(InputError::SelfMerge(item.key.clone()));
            }
        }
        Ok(())
    }

    /// The input as the prompt's data block: [`DATA_BLOCK_OPEN`], pretty JSON, a closing fence. JSON escapes every
    /// line break in a string, so no untrusted text can start a line or close the fence.
    pub fn data_block(&self) -> Result<String, InputError> {
        self.check()?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| InputError::Json(error.to_string()))?;
        Ok(format!("{DATA_BLOCK_OPEN}\n{json}\n{DATA_BLOCK_CLOSE}"))
    }

    /// The input of the first data block of `prompt`; `None` when there is none or it does not parse.
    pub fn from_prompt(prompt: &str) -> Option<Self> {
        let mut lines = prompt.lines();
        lines.by_ref().find(|line| *line == DATA_BLOCK_OPEN)?;
        let body: Vec<&str> = lines.take_while(|line| *line != DATA_BLOCK_CLOSE).collect();
        serde_json::from_str(&body.join("\n")).ok()
    }
}

fn is_age_part(component: ScoreComponent) -> bool {
    matches!(
        component,
        ScoreComponent::Recency | ScoreComponent::ChangeSize
    )
}

fn is_judgeable(component: ScoreComponent) -> bool {
    component == ScoreComponent::DefectSeverity || part_values(component).is_some()
}

/// The judge's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JudgeWorkOutput {
    /// One answer per input item.
    pub items: Vec<ItemAnswer>,
}

/// The judgement of one item. Every value needs its basis; never judge from the title alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ItemAnswer {
    /// The item's `key`, exactly as in the input.
    pub key: String,
    /// The pre-existing problem the item addresses: required when `defect_severity` is asked (0 and 0 when it
    /// addresses none), else null unless you replace a known value.
    pub defect: Option<DefectAnswer>,
    /// Every other asked component, plus any known part you replace with stated evidence.
    pub parts: Vec<PartAnswer>,
    /// Ranked or left out: required when `asks_eligibility` is true, else null unless you change the computed
    /// eligibility.
    pub eligibility: Option<EligibilityAnswer>,
    /// Keys from the item's `suggested_merges` that this item covers; usually empty.
    pub also_keys: Vec<String>,
    /// What needs the user's attention now (P0 alarm), or null; a high score alone is no alarm.
    pub alarm: Option<String>,
    /// Why the item ranks where it does, one or two plain sentences.
    pub why: String,
    /// The next concrete action, one plain sentence.
    pub next: String,
}

/// Likelihood and impact of the pre-existing problem, each 0–10.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DefectAnswer {
    /// How likely users meet the problem, 0–10.
    pub likelihood: u8,
    /// How much harm when they do, 0–10.
    pub impact: u8,
    /// Short evidence.
    pub basis: String,
}

/// One judged component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PartAnswer {
    /// Component.
    pub component: ScoreComponent,
    /// Points, one of the component's allowed values.
    pub points: i32,
    /// Short evidence.
    pub basis: String,
}

/// Whether the item is ranked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EligibilityAnswer {
    /// Ranked (true) or left out (false).
    pub eligible: bool,
    /// Plain reason.
    pub reason: String,
}

/// The output schema: [`JudgeWorkOutput`] with each part's points closed to its component's values and defect
/// axes capped at [`DEFECT_AXIS_MAX`].
pub(crate) fn output_schema() -> Result<Value, StrictSchemaError> {
    let mut schema = strict::output_schema_for::<JudgeWorkOutput>()?;
    close_values(&mut schema);
    strict::check_output(&schema)?;
    Ok(schema)
}

fn close_values(node: &mut Value) {
    let Some(map) = node.as_object_mut() else {
        return;
    };
    let names = |map: &serde_json::Map<String, Value>| {
        map.get("properties")
            .and_then(Value::as_object)
            .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let properties = names(map);
    if properties == ["component", "points", "basis"] {
        let description = map.get("description").cloned();
        let variants: Vec<Value> = PART_VALUES
            .iter()
            .map(|(component, values)| {
                json!({
                    "type": "object", "additionalProperties": false,
                    "properties": {
                        "component": {"type": "string", "enum": [component]},
                        "points": {"type": "integer", "enum": values},
                        "basis": {"type": "string", "description": "Short evidence."}
                    },
                    "required": ["component", "points", "basis"]
                })
            })
            .collect();
        *node = json!({"anyOf": variants});
        if let (Some(description), Some(map)) = (description, node.as_object_mut()) {
            map.insert("description".to_owned(), description);
        }
        return;
    }
    if properties == ["likelihood", "impact", "basis"]
        && let Some(axes) = map.get_mut("properties").and_then(Value::as_object_mut)
    {
        for axis in ["likelihood", "impact"] {
            if let Some(axis) = axes.get_mut(axis).and_then(Value::as_object_mut) {
                axis.insert("maximum".to_owned(), json!(DEFECT_AXIS_MAX));
            }
        }
    }
    for value in map.values_mut() {
        match value {
            Value::Array(items) => items.iter_mut().for_each(close_values),
            other => close_values(other),
        }
    }
}

/// A judged component after validation; defect severity carries likelihood + impact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JudgedValue {
    /// Component.
    pub component: ScoreComponent,
    /// Points, within the component's closed set.
    pub points: i32,
    /// Basis, trimmed plain text.
    pub basis: String,
}

/// A validated answer of one input item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidAnswer {
    /// Judged components, every asked one included.
    pub parts: Vec<JudgedValue>,
    /// Eligibility decision; present when asked.
    pub eligibility: Option<EligibilityAnswer>,
    /// Suggested items this one covers, deduplicated.
    pub also_keys: Vec<ItemKey>,
    /// P0 alarm reason.
    pub alarm: Option<String>,
    /// Why it ranks where it does.
    pub why: String,
    /// Next action.
    pub next: String,
}

/// Why one item's answer is not used; the item stays provisional and the server logs the reason.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AnswerRejection {
    /// The answer has no entry for the item.
    #[error("the answer has no entry for the item")]
    Missing,
    /// The item is answered more than once.
    #[error("the item is answered more than once")]
    AnsweredTwice,
    /// An asked component is missing.
    #[error("the asked {0:?} part is not answered")]
    NotAnswered(ScoreComponent),
    /// A component is answered more than once.
    #[error("the {0:?} part is answered more than once")]
    PartTwice(ScoreComponent),
    /// A component that is never judged in `parts` (defect severity, recency, change size).
    #[error("the {0:?} part cannot be answered in parts")]
    NotJudgeable(ScoreComponent),
    /// A value outside the component's closed set.
    #[error("{points} points is not an allowed value of the {component:?} part")]
    OutOfRange {
        /// Component.
        component: ScoreComponent,
        /// Value given.
        points: i32,
    },
    /// Defect likelihood or impact above [`DEFECT_AXIS_MAX`].
    #[error("defect likelihood {likelihood} and impact {impact} must each be 0–10")]
    DefectOutOfRange {
        /// Likelihood given.
        likelihood: u8,
        /// Impact given.
        impact: u8,
    },
    /// Eligibility is asked but not answered.
    #[error("eligibility is asked but not answered")]
    EligibilityNotAnswered,
    /// A merge outside the item's suggestions.
    #[error("an also-key is not one of the item's suggested merges")]
    UnsuggestedMerge,
    /// A text field is empty, too long or not plain text.
    #[error("{field} {problem}")]
    Text {
        /// Field name, e.g. `why` or `parts.completion.basis`.
        field: String,
        /// What is wrong.
        problem: TextProblem,
    },
}

/// What is wrong with a text field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TextProblem {
    /// Empty after trimming.
    #[error("is empty")]
    Empty,
    /// Over its character cap.
    #[error("is longer than {0} characters")]
    TooLong(usize),
    /// Contains a control or bidirectional formatting character (single-line plain text only).
    #[error("contains control or bidirectional formatting characters")]
    NotPlain,
}

/// The answer checked against its input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validated {
    /// Per input item, in input order: its valid answer or why it is not used.
    pub items: Vec<(ItemKey, Result<ValidAnswer, AnswerRejection>)>,
    /// Answers whose key names no input item (dropped; their text is never used).
    pub unknown_answers: usize,
}

impl JudgeWorkOutput {
    /// Validates the answer against `input`: keys of the batch only, every asked part and eligibility answered,
    /// closed value sets, a basis for every value, merges only from the suggestions, capped plain-text fields.
    pub fn validate(&self, input: &JudgeWorkInput) -> Validated {
        let known: HashSet<&str> = input.items.iter().map(|item| item.key.as_str()).collect();
        let unknown_answers = self
            .items
            .iter()
            .filter(|answer| !known.contains(answer.key.as_str()))
            .count();
        let items = input
            .items
            .iter()
            .map(|item| {
                let mut answers = self
                    .items
                    .iter()
                    .filter(|answer| answer.key == item.key.as_str());
                let outcome = match (answers.next(), answers.next()) {
                    (None, _) => Err(AnswerRejection::Missing),
                    (Some(_), Some(_)) => Err(AnswerRejection::AnsweredTwice),
                    (Some(answer), None) => validate_item(item, answer),
                };
                (item.key.clone(), outcome)
            })
            .collect();
        Validated {
            items,
            unknown_answers,
        }
    }
}

fn validate_item(item: &JudgeItem, answer: &ItemAnswer) -> Result<ValidAnswer, AnswerRejection> {
    let mut parts = Vec::with_capacity(answer.parts.len() + 1);
    if let Some(defect) = &answer.defect {
        if defect.likelihood > DEFECT_AXIS_MAX || defect.impact > DEFECT_AXIS_MAX {
            return Err(AnswerRejection::DefectOutOfRange {
                likelihood: defect.likelihood,
                impact: defect.impact,
            });
        }
        let basis = plain("defect.basis", &defect.basis, BASIS_MAX_CHARS)?;
        parts.push(JudgedValue {
            component: ScoreComponent::DefectSeverity,
            points: i32::from(defect.likelihood + defect.impact),
            basis: format!(
                "likelihood {}/10, impact {}/10: {basis}",
                defect.likelihood, defect.impact
            ),
        });
    }
    for part in &answer.parts {
        let values =
            part_values(part.component).ok_or(AnswerRejection::NotJudgeable(part.component))?;
        if !values.contains(&part.points) {
            return Err(AnswerRejection::OutOfRange {
                component: part.component,
                points: part.points,
            });
        }
        if parts
            .iter()
            .any(|p: &JudgedValue| p.component == part.component)
        {
            return Err(AnswerRejection::PartTwice(part.component));
        }
        let field = format!("parts.{}.basis", component_name(part.component));
        parts.push(JudgedValue {
            component: part.component,
            points: part.points,
            basis: plain(&field, &part.basis, BASIS_MAX_CHARS)?,
        });
    }
    if let Some(missing) = item
        .asks
        .iter()
        .find(|asked| !parts.iter().any(|p| p.component == **asked))
    {
        return Err(AnswerRejection::NotAnswered(*missing));
    }
    let eligibility = match &answer.eligibility {
        Some(decision) => Some(EligibilityAnswer {
            eligible: decision.eligible,
            reason: plain("eligibility.reason", &decision.reason, BASIS_MAX_CHARS)?,
        }),
        None if item.asks_eligibility => return Err(AnswerRejection::EligibilityNotAnswered),
        None => None,
    };
    let mut also_keys: Vec<ItemKey> = Vec::new();
    for key in &answer.also_keys {
        let suggested = item
            .suggested_merges
            .iter()
            .find(|candidate| candidate.as_str() == key)
            .ok_or(AnswerRejection::UnsuggestedMerge)?;
        if !also_keys.contains(suggested) {
            also_keys.push(suggested.clone());
        }
    }
    let alarm = answer
        .alarm
        .as_deref()
        .map(|alarm| plain("alarm", alarm, ALARM_MAX_CHARS))
        .transpose()?;
    Ok(ValidAnswer {
        parts,
        eligibility,
        also_keys,
        alarm,
        why: plain("why", &answer.why, WHY_MAX_CHARS)?,
        next: plain("next", &answer.next, NEXT_MAX_CHARS)?,
    })
}

fn component_name(component: ScoreComponent) -> String {
    serde_json::to_value(component)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Trimmed single-line plain text of at most `max` characters, without control or bidirectional formatting
/// characters (which could disguise text, CWE-451).
fn plain(field: &str, text: &str, max: usize) -> Result<String, AnswerRejection> {
    let problem = |problem| AnswerRejection::Text {
        field: field.to_owned(),
        problem,
    };
    let text = text.trim();
    if text.is_empty() {
        return Err(problem(TextProblem::Empty));
    }
    if text.chars().count() > max {
        return Err(problem(TextProblem::TooLong(max)));
    }
    if text.chars().any(|c| c.is_control() || is_bidi_format(c)) {
        return Err(problem(TextProblem::NotPlain));
    }
    Ok(text.to_owned())
}

fn is_bidi_format(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

#[cfg(test)]
mod tests;
