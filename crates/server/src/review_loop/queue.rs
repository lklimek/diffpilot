//! The question queue (plan §2.4): one question at a time in a deterministic order, groups of items with the same
//! recommendation, and the "Decide the rest per recommendation" selection. Pure. Gate items are never grouped and
//! never decided in bulk (RQ4, RQ5).

use std::collections::BTreeMap;

use contracts::api::review_loop::{
    DecideRequest, DecisionKind, ItemState, LeftOutItem, LeftOutReason, QueueEntry, QueueReason,
    Recommendation,
};

use super::effects::PrQuestion;

/// What the queue needs of one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueItem {
    /// `C<no>`.
    pub no: u32,
    /// State.
    pub state: ItemState,
    /// Revision.
    pub revision: u32,
    /// It has a gate (assessment or owner).
    pub gated: bool,
    /// The reviewer marked it Blocking (its claim).
    pub blocking: bool,
    /// Round first seen.
    pub first_round: Option<u32>,
    /// The LLM's recommendation, when assessed.
    pub recommendation: Option<Recommendation>,
    /// PR-level question, and the round it belongs to.
    pub question: Option<(PrQuestion, u32)>,
    /// Listed in a standing "merge now" decision's follow-up PR: handled.
    pub followed_up: bool,
    /// The reader skipped it at its current revision.
    pub skipped: bool,
}

/// States that still wait for the owner.
pub fn is_open(state: ItemState) -> bool {
    matches!(
        state,
        ItemState::Undecided | ItemState::Assessing | ItemState::AskedBack | ItemState::Reopened
    )
}

/// Items waiting for a decision: open, not handled by a follow-up PR, and of PR-level questions only the latest of
/// each kind.
pub fn waiting(items: &[QueueItem]) -> Vec<&QueueItem> {
    let mut latest: BTreeMap<u8, u32> = BTreeMap::new();
    for item in items.iter().filter(|item| is_open(item.state)) {
        if let Some((question, round)) = item.question {
            let slot = latest.entry(question as u8).or_insert(round);
            *slot = (*slot).max(round);
        }
    }
    items
        .iter()
        .filter(|item| is_open(item.state) && !item.followed_up)
        .filter(|item| match item.question {
            Some((question, round)) => latest.get(&(question as u8)) == Some(&round),
            None => true,
        })
        .collect()
}

/// The key two items share to form a group: the recommended kind and the target, else the reason class.
fn group_key(recommendation: &Recommendation) -> (String, String) {
    let detail = match &recommendation.target {
        Some(target) => serde_json::to_string(target).unwrap_or_default(),
        None => format!("class:{}", recommendation.reason_class),
    };
    (
        serde_json::to_string(&recommendation.kind).unwrap_or_default(),
        detail,
    )
}

/// The queue in plan §2.4 order; skipped entries last.
pub fn queue(items: &[QueueItem]) -> Vec<QueueEntry> {
    let waiting = waiting(items);
    let mut entries: Vec<(u8, (u32, u32), QueueEntry)> = Vec::new();
    let mut groups: BTreeMap<(String, String), Vec<&QueueItem>> = BTreeMap::new();
    for item in &waiting {
        let (rank, reason) = if item.question.is_some() {
            (1, QueueReason::PrQuestion)
        } else if item.state == ItemState::Reopened {
            (2, QueueReason::AskedAgain)
        } else if item.state == ItemState::AskedBack {
            (3, QueueReason::Answered)
        } else if item.gated {
            (4, QueueReason::Gate)
        } else if item.blocking {
            (5, QueueReason::Blocking)
        } else {
            if let Some(recommendation) = &item.recommendation
                && !item.skipped
            {
                groups
                    .entry(group_key(recommendation))
                    .or_default()
                    .push(item);
                continue;
            }
            (6, QueueReason::Single)
        };
        let order = if rank == 5 {
            (item.first_round.unwrap_or(u32::MAX), item.no)
        } else {
            (0, item.no)
        };
        entries.push((
            rank,
            order,
            QueueEntry {
                item_nos: vec![item.no],
                reason,
                skipped: item.skipped,
            },
        ));
    }
    for members in groups.into_values() {
        let (rank, reason) = if members.len() >= 2 {
            (7, QueueReason::Group)
        } else {
            (6, QueueReason::Single)
        };
        let first = members.iter().map(|item| item.no).min().unwrap_or(0);
        entries.push((
            rank,
            (0, first),
            QueueEntry {
                item_nos: members.iter().map(|item| item.no).collect(),
                reason,
                skipped: false,
            },
        ));
    }
    entries.sort_by_key(|(rank, order, entry)| (entry.skipped, *rank, *order));
    entries.into_iter().map(|(_, _, entry)| entry).collect()
}

/// A recommendation as the decision it proposes (bulk rows), or why it cannot be one.
pub fn proposed(
    recommendation: &Recommendation,
    revision: u32,
) -> Result<DecideRequest, LeftOutReason> {
    use DecisionKind as K;
    let reason = matches!(recommendation.kind, K::Reject)
        .then(|| recommendation.reason.trim().to_owned())
        .filter(|reason| !reason.is_empty());
    let incomplete = match recommendation.kind {
        K::Reject => reason.is_none(),
        K::HandledInPr | K::UpstreamIssue | K::Todo => recommendation.target.is_none(),
        K::MergeWithFollowUps | K::AnotherRound | K::DismissStaleReviews | K::KeepReviews => true,
        K::Resolve | K::FixHere | K::FollowUpPr => false,
    };
    if incomplete {
        return Err(LeftOutReason::Incomplete);
    }
    Ok(DecideRequest {
        kind: recommendation.kind,
        reason,
        target: recommendation.target.clone(),
        r#override: None,
        resolve_ticked: true,
        reply_text: None,
        comment: None,
        item_revision: revision,
    })
}

/// "Decide the rest per recommendation": the waiting items it decides (in `C<no>` order) and those it leaves out.
/// PR-level questions are neither.
pub fn decide_rest(items: &[QueueItem]) -> (Vec<(&QueueItem, DecideRequest)>, Vec<LeftOutItem>) {
    let mut rows = Vec::new();
    let mut left_out = Vec::new();
    let mut waiting = waiting(items);
    waiting.sort_by_key(|item| item.no);
    for item in waiting.into_iter().filter(|item| item.question.is_none()) {
        let outcome = if item.gated {
            Err(LeftOutReason::Gate)
        } else if item.state == ItemState::Reopened {
            Err(LeftOutReason::AskedAgain)
        } else {
            match &item.recommendation {
                None => Err(LeftOutReason::NotAssessed),
                Some(recommendation) => proposed(recommendation, item.revision),
            }
        };
        match outcome {
            Ok(request) => rows.push((item, request)),
            Err(reason) => left_out.push(LeftOutItem {
                item_no: item.no,
                reason,
            }),
        }
    }
    (rows, left_out)
}

#[cfg(test)]
mod tests {
    use contracts::api::review_loop::DecisionTarget;

    use super::*;

    fn item(no: u32) -> QueueItem {
        QueueItem {
            no,
            state: ItemState::Undecided,
            revision: 1,
            gated: false,
            blocking: false,
            first_round: Some(1),
            recommendation: None,
            question: None,
            followed_up: false,
            skipped: false,
        }
    }

    fn recommended(no: u32, kind: DecisionKind, class: &str) -> QueueItem {
        QueueItem {
            recommendation: Some(Recommendation {
                kind,
                reason: "Out of scope for this change.".into(),
                target: None,
                reason_class: class.into(),
            }),
            ..item(no)
        }
    }

    #[test]
    fn queue_follows_the_plan_order() {
        let items = vec![
            item(1),
            QueueItem {
                blocking: true,
                first_round: Some(2),
                ..item(2)
            },
            QueueItem {
                blocking: true,
                first_round: Some(1),
                ..item(3)
            },
            QueueItem {
                gated: true,
                ..item(4)
            },
            QueueItem {
                state: ItemState::AskedBack,
                ..item(5)
            },
            QueueItem {
                state: ItemState::Reopened,
                ..item(6)
            },
            QueueItem {
                question: Some((PrQuestion::Merge, 3)),
                ..item(7)
            },
            recommended(8, DecisionKind::Reject, "pre-existing"),
            recommended(9, DecisionKind::Reject, "pre-existing"),
            QueueItem {
                state: ItemState::Decided,
                ..item(10)
            },
            QueueItem {
                skipped: true,
                ..item(11)
            },
        ];
        let order: Vec<(Vec<u32>, QueueReason)> = queue(&items)
            .into_iter()
            .map(|entry| (entry.item_nos, entry.reason))
            .collect();
        assert_eq!(
            order,
            vec![
                (vec![7], QueueReason::PrQuestion),
                (vec![6], QueueReason::AskedAgain),
                (vec![5], QueueReason::Answered),
                (vec![4], QueueReason::Gate),
                (vec![3], QueueReason::Blocking),
                (vec![2], QueueReason::Blocking),
                (vec![1], QueueReason::Single),
                (vec![8, 9], QueueReason::Group),
                (vec![11], QueueReason::Single),
            ]
        );
    }

    #[test]
    fn gate_items_never_grouped_or_bulk() {
        let gated = |no| QueueItem {
            gated: true,
            ..recommended(no, DecisionKind::Resolve, "fixed")
        };
        let items = vec![
            gated(1),
            gated(2),
            recommended(3, DecisionKind::Resolve, "fixed"),
            recommended(4, DecisionKind::Resolve, "fixed"),
            item(5),
            QueueItem {
                state: ItemState::Reopened,
                ..recommended(6, DecisionKind::Resolve, "fixed")
            },
        ];
        let entries = queue(&items);
        for entry in &entries {
            if entry.reason == QueueReason::Group {
                assert!(
                    entry.item_nos.iter().all(|no| ![1, 2].contains(no)),
                    "{entry:?}"
                );
            }
        }
        assert!(entries.iter().any(|e| e.item_nos == vec![3, 4]));
        let (rows, left_out) = decide_rest(&items);
        assert_eq!(
            rows.iter().map(|(item, _)| item.no).collect::<Vec<_>>(),
            [3, 4]
        );
        assert_eq!(
            left_out,
            vec![
                LeftOutItem {
                    item_no: 1,
                    reason: LeftOutReason::Gate
                },
                LeftOutItem {
                    item_no: 2,
                    reason: LeftOutReason::Gate
                },
                LeftOutItem {
                    item_no: 5,
                    reason: LeftOutReason::NotAssessed
                },
                LeftOutItem {
                    item_no: 6,
                    reason: LeftOutReason::AskedAgain
                },
            ]
        );
        assert!(rows.iter().all(|(_, request)| request.resolve_ticked));
    }

    #[test]
    fn groups_need_the_same_kind_and_class_or_target() {
        let mut handled = recommended(3, DecisionKind::HandledInPr, "elsewhere");
        if let Some(r) = &mut handled.recommendation {
            r.target = Some(DecisionTarget::ExistingPr { number: 490 });
        }
        let items = vec![
            recommended(1, DecisionKind::Reject, "style"),
            recommended(2, DecisionKind::Reject, "pre-existing"),
            handled,
        ];
        assert!(
            queue(&items)
                .iter()
                .all(|entry| entry.reason == QueueReason::Single)
        );
        let (rows, _) = decide_rest(&items);
        assert_eq!(rows.len(), 3);
        let reject = &rows[0].1;
        assert_eq!(
            reject.reason.as_deref(),
            Some("Out of scope for this change.")
        );
        assert_eq!(
            proposed(
                &Recommendation {
                    kind: DecisionKind::Todo,
                    reason: "x".into(),
                    target: None,
                    reason_class: "c".into()
                },
                1
            ),
            Err(LeftOutReason::Incomplete)
        );
    }

    #[test]
    fn only_the_latest_pr_question_of_a_kind_waits() {
        let items = vec![
            QueueItem {
                question: Some((PrQuestion::Merge, 3)),
                ..item(1)
            },
            QueueItem {
                question: Some((PrQuestion::Merge, 4)),
                ..item(2)
            },
            QueueItem {
                question: Some((PrQuestion::Dismiss, 3)),
                ..item(3)
            },
            QueueItem {
                followed_up: true,
                ..item(4)
            },
        ];
        let nos: Vec<u32> = waiting(&items).iter().map(|item| item.no).collect();
        assert_eq!(nos, [2, 3]);
    }
}
