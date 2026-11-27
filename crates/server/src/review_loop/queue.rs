//! The question queue (plan §2.4): one question at a time in a deterministic order, groups of items with the same
//! recommendation (resolved review-body items in one group), and the "Decide the rest per recommendation" selection.
//! Pure. Gate items are never grouped and never decided in bulk (RQ4, RQ5).

use std::collections::{BTreeMap, HashMap};

use contracts::api::review_loop::{
    AssessmentState, CommentItemKind, DecideRequest, GateMark, ImportedGithubDecision, ItemMark,
    ItemState, LeftOutItem, LeftOutReason, QueueEntry, QueueReason, Recommendation,
    ReviewDecisionKind,
};

use super::effects::PrQuestion;
use contracts::time::Timestamp;

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
    /// A review-body item: it has no thread.
    pub review_body: bool,
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

/// The facts used to classify an item in the Comments list.
#[cfg_attr(test, derive(serde::Deserialize))]
pub(super) struct StatusItem {
    pub no: u32,
    pub kind: CommentItemKind,
    pub state: ItemState,
    pub assessment_state: AssessmentState,
    #[cfg_attr(test, serde(default))]
    pub status_comment: bool,
    pub imported_decision: Option<ImportedGithubDecision>,
    pub not_a_review_point: Option<ItemMark>,
    pub duplicate_of: Option<u32>,
    #[cfg_attr(test, serde(default))]
    pub decision_follows_primary: bool,
    pub gone_at: Option<Timestamp>,
    pub gates: Vec<GateMark>,
}

/// The Comments list status, in N1 precedence order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(serde::Deserialize), serde(rename_all = "snake_case"))]
pub(super) enum CommentStatus {
    StatusComments,
    Decided,
    NotAReviewPoint,
    Duplicates,
    All,
    NeedsYou,
    Waiting,
    Assessing,
    NotAssessed,
}

pub(super) fn comment_status(item: &StatusItem) -> CommentStatus {
    let linked = item.duplicate_of.is_some_and(|no| no != item.no);
    let awaits = item.gone_at.is_none()
        && matches!(
            item.state,
            ItemState::Undecided | ItemState::Reopened | ItemState::AskedBack
        );
    if item.status_comment {
        CommentStatus::StatusComments
    } else if item.imported_decision.is_some() || item.state == ItemState::Outside {
        CommentStatus::Decided
    } else if item.not_a_review_point.is_some() {
        CommentStatus::NotAReviewPoint
    } else if linked && item.decision_follows_primary && item.gates.is_empty() {
        CommentStatus::Duplicates
    } else if matches!(
        item.state,
        ItemState::Decided | ItemState::Executing | ItemState::Done | ItemState::Repeat
    ) {
        CommentStatus::Decided
    } else if item.gone_at.is_some() {
        CommentStatus::All
    } else if linked && (awaits || (!item.gates.is_empty() && item.state == ItemState::Assessing)) {
        CommentStatus::NeedsYou
    } else if item.assessment_state == AssessmentState::Queued {
        CommentStatus::Waiting
    } else if item.assessment_state == AssessmentState::Running {
        CommentStatus::Assessing
    } else if item.kind != CommentItemKind::PrQuestion
        && matches!(item.assessment_state, AssessmentState::NotAssessed { .. })
    {
        CommentStatus::NotAssessed
    } else if awaits {
        CommentStatus::NeedsYou
    } else {
        CommentStatus::All
    }
}

/// Counts the Needs you items in the queue and those among them with a gate.
pub(super) fn needs_you(entries: &[QueueEntry], items: &HashMap<u32, StatusItem>) -> (u32, u32) {
    let mut total = 0u32;
    let mut gates = 0u32;
    for item in entries
        .iter()
        .filter(|entry| !entry.skipped)
        .flat_map(|entry| &entry.item_nos)
        .filter_map(|no| items.get(no))
    {
        if comment_status(item) == CommentStatus::NeedsYou {
            total = total.saturating_add(1);
            gates = gates.saturating_add(u32::from(!item.gates.is_empty()));
        }
    }
    (total, gates)
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

/// A review-body item that joins the one group of resolved review-body items (plan §2.4): recommended "Resolve", not
/// a PR-level question, gate item, repeat asked again, answer or skipped; Blocking or not, it has no thread to resolve.
fn resolved_body(item: &QueueItem) -> bool {
    item.review_body
        && item.question.is_none()
        && !item.gated
        && !item.skipped
        && !matches!(item.state, ItemState::Reopened | ItemState::AskedBack)
        && item
            .recommendation
            .as_ref()
            .is_some_and(|recommendation| recommendation.kind == ReviewDecisionKind::Resolve)
}

/// The queue in plan §2.4 order; skipped entries last.
pub fn queue(items: &[QueueItem]) -> Vec<QueueEntry> {
    let waiting = waiting(items);
    let resolved_bodies: Vec<u32> = waiting
        .iter()
        .filter(|item| resolved_body(item))
        .map(|item| item.no)
        .collect();
    let bodies_grouped = resolved_bodies.len() >= 2;
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
        } else if bodies_grouped && resolved_body(item) {
            continue;
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
    if bodies_grouped {
        entries.push((
            7,
            (0, resolved_bodies.iter().copied().min().unwrap_or(0)),
            QueueEntry {
                item_nos: resolved_bodies,
                reason: QueueReason::Group,
                skipped: false,
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
) -> Result<DecideRequest<ReviewDecisionKind>, LeftOutReason> {
    use ReviewDecisionKind as K;
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
        plan_digest: None,
    })
}

/// "Decide the rest per recommendation": the waiting items it decides (in `C<no>` order) and those it leaves out.
/// PR-level questions are neither.
pub fn decide_rest(
    items: &[QueueItem],
) -> (
    Vec<(&QueueItem, DecideRequest<ReviewDecisionKind>)>,
    Vec<LeftOutItem>,
) {
    let mut rows = Vec::new();
    let mut left_out = Vec::new();
    let mut waiting = waiting(items);
    waiting.sort_by_key(|item| item.no);
    for item in waiting
        .into_iter()
        .filter(|item| item.question.is_none() && !item.skipped)
    {
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

    #[test]
    fn needs_you_matches_fixture() {
        check_needs_you_fixture(false);
    }

    #[test]
    fn needs_you_fixture_mark_cases() {
        check_needs_you_fixture(true);
    }

    fn check_needs_you_fixture(marked: bool) {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/contracts/fixtures/needs-you-cases.json"
        ))
        .unwrap();
        let defaults = fixture["defaults"].as_object().unwrap();
        let items: Vec<_> = fixture["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|overrides| {
                let mut data = defaults.clone();
                data.extend(overrides.as_object().unwrap().clone());
                serde_json::from_value::<StatusItem>(data.into()).unwrap()
            })
            .collect();
        let expected: Vec<CommentStatus> =
            serde_json::from_value(fixture["expected_statuses"].clone()).unwrap();
        assert_eq!(items.len(), expected.len());
        for (item, status) in items.iter().zip(expected) {
            if item.not_a_review_point.is_some() == marked {
                assert_eq!(comment_status(item), status, "C{}", item.no);
            }
        }
        let entries: Vec<QueueEntry> = serde_json::from_value(fixture["queue"].clone()).unwrap();
        let items = items.into_iter().map(|item| (item.no, item)).collect();
        let (total, gates) = needs_you(&entries, &items);
        assert_eq!(fixture["expected_needs_you"], total);
        assert_eq!(fixture["expected_needs_you_gates"], gates);
    }

    #[test]
    fn rust_dashcore_1336_regression() {
        use contracts::api::review_loop::NotAssessedReason;
        use serde::Deserialize;

        #[derive(Deserialize)]
        struct Item {
            no: u32,
            kind: CommentItemKind,
            #[serde(default)]
            assessed: bool,
            duplicate_of: Option<u32>,
            comment: Option<crate::github::review_loop::PrComment>,
        }

        let fixture: Vec<Item> = serde_json::from_value(
            serde_json::from_str::<serde_json::Value>(include_str!(
                "../../../../fixtures/review-loop/regressions/rust-dashcore-1336.json"
            ))
            .unwrap()["items"]
                .clone(),
        )
        .unwrap();
        let bots =
            super::super::observe::BotRule::new(&contracts::api::dashboard::default_known_bots());
        let by_no: HashMap<_, _> = fixture
            .iter()
            .map(|item| {
                let status_comment = item
                    .comment
                    .as_ref()
                    .is_some_and(|comment| bots.pr_comment(comment).1);
                (
                    item.no,
                    StatusItem {
                        no: item.no,
                        kind: item.kind,
                        state: ItemState::Undecided,
                        assessment_state: if item.assessed {
                            AssessmentState::Assessed
                        } else {
                            AssessmentState::NotAssessed {
                                reason: NotAssessedReason::Missing,
                            }
                        },
                        status_comment,
                        imported_decision: None,
                        not_a_review_point: None,
                        duplicate_of: item.duplicate_of,
                        decision_follows_primary: false,
                        gone_at: None,
                        gates: vec![],
                    },
                )
            })
            .collect();
        let item_nos: Vec<_> = fixture.iter().map(|item| item.no).collect();
        let status: Vec<_> = item_nos
            .iter()
            .copied()
            .filter(|no| by_no[no].status_comment)
            .collect();
        assert_eq!(status, (29..=49).collect::<Vec<_>>());
        let actual: Vec<_> = item_nos
            .iter()
            .copied()
            .filter(|no| comment_status(&by_no[no]) == CommentStatus::NeedsYou)
            .collect();
        assert_eq!(actual, [21, 23, 24, 25, 28, 50, 51, 52]);
        let entries = [QueueEntry {
            item_nos,
            reason: QueueReason::Group,
            skipped: false,
        }];
        assert_eq!(needs_you(&entries, &by_no), (8, 0));
    }

    fn item(no: u32) -> QueueItem {
        QueueItem {
            no,
            state: ItemState::Undecided,
            revision: 1,
            gated: false,
            blocking: false,
            review_body: false,
            first_round: Some(1),
            recommendation: None,
            question: None,
            followed_up: false,
            skipped: false,
        }
    }

    fn recommended(no: u32, kind: ReviewDecisionKind, class: &str) -> QueueItem {
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
            recommended(8, ReviewDecisionKind::Reject, "pre-existing"),
            recommended(9, ReviewDecisionKind::Reject, "pre-existing"),
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
            ..recommended(no, ReviewDecisionKind::Resolve, "fixed")
        };
        let items = vec![
            gated(1),
            gated(2),
            recommended(3, ReviewDecisionKind::Resolve, "fixed"),
            recommended(4, ReviewDecisionKind::Resolve, "fixed"),
            item(5),
            QueueItem {
                state: ItemState::Reopened,
                ..recommended(6, ReviewDecisionKind::Resolve, "fixed")
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
        let mut handled = recommended(3, ReviewDecisionKind::HandledInPr, "elsewhere");
        if let Some(r) = &mut handled.recommendation {
            r.target = Some(DecisionTarget::ExistingPr { number: 490 });
        }
        let items = vec![
            recommended(1, ReviewDecisionKind::Reject, "style"),
            recommended(2, ReviewDecisionKind::Reject, "pre-existing"),
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
                    kind: ReviewDecisionKind::Todo,
                    reason: "x".into(),
                    target: None,
                    reason_class: "c".into()
                },
                1
            ),
            Err(LeftOutReason::Incomplete)
        );
    }

    /// Plan §2.4 (owner, 2026-11-23): review-body items recommended "Resolve" have no thread and form one group,
    /// Blocking ones and any reason class too; gate items, repeats asked again, skipped items and threads do not join.
    #[test]
    fn resolved_body_items_form_one_group_even_when_blocking() {
        let body = |no, blocking, class: &str| QueueItem {
            review_body: true,
            blocking,
            ..recommended(no, ReviewDecisionKind::Resolve, class)
        };
        let items = vec![
            body(1, true, "fixed_since"),
            body(2, true, "fixed_elsewhere"),
            body(3, false, "fixed_since"),
            QueueItem {
                blocking: true,
                ..recommended(4, ReviewDecisionKind::Resolve, "fixed_since")
            },
            QueueItem {
                gated: true,
                ..body(5, true, "fixed_since")
            },
            QueueItem {
                state: ItemState::Reopened,
                ..body(6, true, "fixed_since")
            },
            QueueItem {
                skipped: true,
                ..body(7, false, "fixed_since")
            },
            QueueItem {
                review_body: true,
                blocking: true,
                ..recommended(8, ReviewDecisionKind::FixHere, "bug")
            },
        ];
        let order: Vec<(Vec<u32>, QueueReason, bool)> = queue(&items)
            .into_iter()
            .map(|entry| (entry.item_nos, entry.reason, entry.skipped))
            .collect();
        assert_eq!(
            order,
            vec![
                (vec![6], QueueReason::AskedAgain, false),
                (vec![5], QueueReason::Gate, false),
                (vec![4], QueueReason::Blocking, false),
                (vec![8], QueueReason::Blocking, false),
                (vec![1, 2, 3], QueueReason::Group, false),
                (vec![7], QueueReason::Single, true),
            ]
        );
        let alone = vec![body(1, true, "fixed_since"), item(2)];
        assert_eq!(
            queue(&alone)
                .into_iter()
                .map(|entry| (entry.item_nos, entry.reason))
                .collect::<Vec<_>>(),
            vec![
                (vec![1], QueueReason::Blocking),
                (vec![2], QueueReason::Single)
            ],
            "a single one keeps its Blocking place"
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
