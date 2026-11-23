//! The effect function (plan §2.3, ADR-023 §6): a decision on an item maps to ordered effects with their executor.
//! Pure; validates the decision against its item first. The owner's comment never enters an effect: only the reply
//! text is posted.

use contracts::api::review_loop::{
    AuthorKind, CommentItemKind, DecideRequest, DecisionKind, DecisionTarget, EffectKind,
    EffectParams, Executor, Gate, Override, PlannedEffect,
};
use url::Url;

/// Longest reason, reply text, comment or title accepted.
pub const MAX_TEXT_CHARS: usize = 4000;
/// Reply of a follow-up item (D5, P1); the agent adds the pull request's link.
const FOLLOW_UP_REPLY: &str = "Per maintainer decision, this moves to a follow-up pull request.";
/// Default message of a stale-review dismissal (P3).
pub const DISMISS_MESSAGE: &str = "Per maintainer decision, every item of this review is decided.";

/// Which PR-level question an item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrQuestion {
    /// "Merge now, or another round?" (P1/P2).
    Merge,
    /// "Dismiss stale bot reviews?" (P3/P4).
    Dismiss,
}

/// The item a decision is about, as the effect function needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectItem {
    /// `C<no>`.
    pub no: u32,
    /// Source.
    pub kind: CommentItemKind,
    /// Author's login.
    pub author: String,
    /// Author kind.
    pub author_kind: AuthorKind,
    /// GitHub page (a body item: its review's).
    pub url: Option<String>,
    /// GitHub thread node id (threads).
    pub thread_id: Option<String>,
    /// The review a body item belongs to.
    pub review_id: Option<String>,
    /// Gates on it (assessment and owner).
    pub gates: Vec<Gate>,
    /// Which PR-level question, for `pr_question` items.
    pub question: Option<PrQuestion>,
}

impl EffectItem {
    /// `text` as posted on this item: a PR-level comment answering a review starts with a line naming the review
    /// (plan §2.3), a person by an @-mention, a bot without one (the mention would trigger it).
    fn on_review(&self, text: String) -> String {
        let url = match (&self.thread_id, &self.review_id, &self.url) {
            (None, Some(_), Some(url)) if self.kind == CommentItemKind::ReviewBody => url,
            _ => return text,
        };
        let line = match self.author_kind {
            AuthorKind::Human => format!("@{}, re [your review]({url}):", self.author),
            AuthorKind::Bot | AuthorKind::Diffpilot => {
                format!("Re [{}'s review]({url}):", self.author)
            }
        };
        format!("{line}\n{text}")
    }
}

/// What a PR-level decision acts on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrScope {
    /// The PR's number (D4 must name another one).
    pub number: u32,
    /// P1: the items still undecided, gate items left out.
    pub remaining: Vec<EffectItem>,
    /// P3: the stale bot reviews the question lists (node ids).
    pub stale_reviews: Vec<String>,
}

/// A decision does not fit its item.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecisionError {
    /// The decision kind is not offered for this item.
    #[error("this decision is not offered for this item")]
    KindNotOffered,
    /// A reject (and "Accept despite the gate") needs a reason.
    #[error("this decision needs a reason")]
    ReasonRequired,
    /// The decision needs its target (existing PR, issue link or "create", TODO title).
    #[error("this decision needs its target")]
    TargetRequired,
    /// The target belongs to another decision kind or is not valid.
    #[error("the target does not fit this decision")]
    TargetInvalid,
    /// The override does not fit (a gate override needs a reject on an item with that gate).
    #[error("the override does not fit this decision")]
    OverrideInvalid,
    /// "Resolve this thread" stays ticked for "Resolve" and "Accept despite the gate".
    #[error("this decision always resolves the thread")]
    ResolveLocked,
    /// A text is longer than allowed.
    #[error("a text is longer than {MAX_TEXT_CHARS} characters")]
    TooLong,
    /// A review the dismissal question lists has new undecided items or no longer requests changes.
    #[error(
        "the reviews this question lists changed since it was asked; keep the reviews, the next round asks again"
    )]
    ReviewsChanged,
    /// A bulk decision includes an item asked again; it is decided on its own (as "Decide the rest" leaves it out).
    #[error("it was asked again, so it is decided on its own, not in bulk")]
    AskedAgain,
    /// A bulk decision includes an item the latest complete read no longer produced.
    #[error("it is not in the latest read from GitHub, so it is decided on its own, not in bulk")]
    NotRead,
}

/// The planned effects and the reply text that gets posted (stored with the decision).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    /// Effects in `seq` order.
    pub effects: Vec<PlannedEffect>,
    /// The reply text that gets posted; `None` without a reply.
    pub reply_text: Option<String>,
}

/// Who executes `kind` in phases 1–2: always an external agent (diffpilot[bot] takes replies, PR-level comments,
/// resolves and dismissals in phase 3, RQ2).
fn executor(_kind: EffectKind) -> Executor {
    Executor::Agent
}

struct Builder {
    effects: Vec<PlannedEffect>,
}

impl Builder {
    /// Appends an effect waiting for `after` (a `seq`); returns its `seq`.
    fn push(&mut self, kind: EffectKind, after: Option<u32>, params: EffectParams) -> u32 {
        let seq = u32::try_from(self.effects.len()).unwrap_or(u32::MAX - 1) + 1;
        self.effects.push(PlannedEffect {
            seq,
            after,
            kind,
            executor: executor(kind),
            params,
        });
        seq
    }

    /// The reply to `item` (thread reply, or a PR-level comment for items without a thread), then the resolve when
    /// `resolve` and the item has a thread; returns the last `seq`.
    fn reply_and_resolve(
        &mut self,
        item: &EffectItem,
        text: Option<String>,
        after: Option<u32>,
        resolve: bool,
    ) -> Option<u32> {
        let mut last = after;
        if let Some(text) = text {
            let kind = if item.thread_id.is_some() {
                EffectKind::Reply
            } else {
                EffectKind::PrComment
            };
            last = Some(self.push(
                kind,
                last,
                EffectParams {
                    text: Some(text),
                    thread_id: item.thread_id.clone(),
                    review_id: item.review_id.clone(),
                    item_nos: vec![item.no],
                    ..EffectParams::default()
                },
            ));
        }
        if resolve && item.thread_id.is_some() {
            last = Some(self.push(
                EffectKind::Resolve,
                last,
                EffectParams {
                    thread_id: item.thread_id.clone(),
                    item_nos: vec![item.no],
                    ..EffectParams::default()
                },
            ));
        }
        last
    }
}

fn too_long(text: Option<&str>) -> bool {
    text.is_some_and(|text| text.chars().count() > MAX_TEXT_CHARS)
}

fn non_empty(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|text| !text.is_empty())
}

fn gate_name(gate: Gate) -> &'static str {
    match gate {
        Gate::Funds => "G-FUNDS",
        Gate::Secret => "G-SECRET",
        Gate::Crypto => "G-CRYPTO",
        Gate::Data => "G-DATA",
    }
}

fn offered(item: &EffectItem, kind: DecisionKind) -> bool {
    use DecisionKind as K;
    match item.question {
        Some(PrQuestion::Merge) => matches!(kind, K::MergeWithFollowUps | K::AnotherRound),
        Some(PrQuestion::Dismiss) => matches!(kind, K::DismissStaleReviews | K::KeepReviews),
        None => !matches!(
            kind,
            K::MergeWithFollowUps | K::AnotherRound | K::DismissStaleReviews | K::KeepReviews
        ),
    }
}

/// Validates `request` against `item` and plans its effects (plan §2.3 matrix D1–D8, P1–P4).
pub fn plan(
    item: &EffectItem,
    request: &DecideRequest,
    scope: &PrScope,
) -> Result<Planned, DecisionError> {
    use DecisionKind as K;
    if !offered(item, request.kind) {
        return Err(DecisionError::KindNotOffered);
    }
    if too_long(request.reason.as_deref())
        || too_long(request.reply_text.as_deref())
        || too_long(request.comment.as_deref())
    {
        return Err(DecisionError::TooLong);
    }
    let gate = match request.r#override {
        Some(Override::BlockerGate { gate }) => {
            if request.kind != K::Reject || !item.gates.contains(&gate) {
                return Err(DecisionError::OverrideInvalid);
            }
            Some(gate)
        }
        Some(Override::NecessityGate) if request.kind != K::FixHere => {
            return Err(DecisionError::OverrideInvalid);
        }
        Some(Override::NecessityGate) | None => None,
    };
    let target_fits = match (&request.target, request.kind) {
        (None, K::FollowUpPr) => true,
        (None, kind) => !matches!(kind, K::HandledInPr | K::UpstreamIssue | K::Todo),
        (Some(DecisionTarget::ExistingPr { number }), K::HandledInPr) => {
            if *number == 0 || *number == scope.number {
                return Err(DecisionError::TargetInvalid);
            }
            true
        }
        (Some(DecisionTarget::FollowUpPr { title }), K::FollowUpPr) => {
            if too_long(title.as_deref()) {
                return Err(DecisionError::TooLong);
            }
            true
        }
        (Some(DecisionTarget::UpstreamIssue { url }), K::UpstreamIssue) => {
            let valid = url.as_deref().is_none_or(|url| {
                Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
            });
            if !valid || too_long(url.as_deref()) {
                return Err(DecisionError::TargetInvalid);
            }
            true
        }
        (Some(DecisionTarget::Todo { title }), K::Todo) => {
            if non_empty(Some(title)).is_none() {
                return Err(DecisionError::TargetRequired);
            }
            if too_long(Some(title)) {
                return Err(DecisionError::TooLong);
            }
            true
        }
        (Some(_), _) => return Err(DecisionError::TargetInvalid),
    };
    if !target_fits {
        return Err(DecisionError::TargetRequired);
    }
    let reason = non_empty(request.reason.as_deref());
    if request.kind == K::Reject && reason.is_none() {
        return Err(DecisionError::ReasonRequired);
    }
    let always_resolves = request.kind == K::Resolve || gate.is_some();
    if always_resolves && item.thread_id.is_some() && !request.resolve_ticked {
        return Err(DecisionError::ResolveLocked);
    }
    let resolve = request.resolve_ticked;
    let edited = non_empty(request.reply_text.as_deref()).map(str::to_owned);
    // An edited reply text is posted as written: it starts from the planned one, line naming the review included.
    let reply = |default: String| Some(edited.clone().unwrap_or_else(|| item.on_review(default)));
    let mut out = Builder {
        effects: Vec::new(),
    };
    let reply_text = match request.kind {
        K::Resolve => {
            let text = (item.author_kind == AuthorKind::Human).then(|| {
                edited.clone().unwrap_or_else(|| {
                    item.on_review(
                        "Per maintainer decision, this is already addressed in this pull request."
                            .to_owned(),
                    )
                })
            });
            out.reply_and_resolve(item, text.clone(), None, true);
            text
        }
        K::FixHere => {
            let fix = out.push(
                EffectKind::Fix,
                None,
                EffectParams {
                    thread_id: item.thread_id.clone(),
                    item_nos: vec![item.no],
                    ..EffectParams::default()
                },
            );
            let text = reply("Per maintainer decision, this is fixed in this pull request.".into());
            out.reply_and_resolve(item, text.clone(), Some(fix), resolve);
            text
        }
        K::Reject => {
            let reason = reason.unwrap_or_default();
            let text = reply(match gate {
                Some(gate) => format!(
                    "Per maintainer decision, accepted despite {}: {reason}",
                    gate_name(gate)
                ),
                None => format!("Per maintainer decision, not changed: {reason}"),
            });
            out.reply_and_resolve(item, text.clone(), None, resolve || gate.is_some());
            text
        }
        K::HandledInPr => {
            let number = match &request.target {
                Some(DecisionTarget::ExistingPr { number }) => *number,
                _ => return Err(DecisionError::TargetRequired),
            };
            let text = reply(format!(
                "Per maintainer decision, this is addressed in #{number}."
            ));
            out.reply_and_resolve(item, text.clone(), None, resolve);
            text
        }
        K::FollowUpPr => {
            let title = match &request.target {
                Some(DecisionTarget::FollowUpPr { title }) => non_empty(title.as_deref()),
                _ => None,
            };
            let create = out.push(
                EffectKind::FollowUpPr,
                None,
                EffectParams {
                    title: title.map(str::to_owned),
                    item_nos: vec![item.no],
                    ..EffectParams::default()
                },
            );
            let text = reply(FOLLOW_UP_REPLY.into());
            out.reply_and_resolve(item, text.clone(), Some(create), resolve);
            text
        }
        K::UpstreamIssue => {
            let url = match &request.target {
                Some(DecisionTarget::UpstreamIssue { url }) => non_empty(url.as_deref()),
                _ => return Err(DecisionError::TargetRequired),
            };
            let issue = out.push(
                EffectKind::UpstreamIssue,
                None,
                EffectParams {
                    url: url.map(str::to_owned),
                    item_nos: vec![item.no],
                    ..EffectParams::default()
                },
            );
            let text = reply(match url {
                Some(url) => format!("Per maintainer decision, this is tracked upstream: {url}"),
                None => "Per maintainer decision, this is tracked in an upstream issue.".into(),
            });
            out.reply_and_resolve(item, text.clone(), Some(issue), resolve);
            text
        }
        K::Todo => {
            let title = match &request.target {
                Some(DecisionTarget::Todo { title }) => title.trim().to_owned(),
                _ => return Err(DecisionError::TargetRequired),
            };
            let todo = out.push(
                EffectKind::Todo,
                None,
                EffectParams {
                    title: Some(title),
                    item_nos: vec![item.no],
                    ..EffectParams::default()
                },
            );
            // The reply never mentions the TODO (plan §2.3 D7).
            let text = reply("Per maintainer decision, out of scope for this PR.".into());
            out.reply_and_resolve(item, text.clone(), Some(todo), resolve);
            text
        }
        K::MergeWithFollowUps => {
            if !scope.remaining.is_empty() {
                let create = out.push(
                    EffectKind::FollowUpPr,
                    None,
                    EffectParams {
                        item_nos: scope.remaining.iter().map(|item| item.no).collect(),
                        ..EffectParams::default()
                    },
                );
                for remaining in &scope.remaining {
                    out.reply_and_resolve(
                        remaining,
                        Some(remaining.on_review(FOLLOW_UP_REPLY.to_owned())),
                        Some(create),
                        true,
                    );
                }
            }
            None
        }
        K::DismissStaleReviews => {
            let message = reason.unwrap_or(DISMISS_MESSAGE).to_owned();
            for review in &scope.stale_reviews {
                out.push(
                    EffectKind::DismissReview,
                    None,
                    EffectParams {
                        text: Some(message.clone()),
                        review_id: Some(review.clone()),
                        ..EffectParams::default()
                    },
                );
            }
            Some(message)
        }
        K::AnotherRound | K::KeepReviews => None,
    };
    Ok(Planned {
        effects: out.effects,
        reply_text,
    })
}

#[cfg(test)]
mod tests;
