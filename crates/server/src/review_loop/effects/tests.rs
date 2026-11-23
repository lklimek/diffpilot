use contracts::api::review_loop::{
    AuthorKind, CommentItemKind, DecideRequest, DecisionKind, DecisionTarget, EffectKind, Executor,
    Gate, Override,
};

use super::*;

fn thread(author_kind: AuthorKind) -> EffectItem {
    EffectItem {
        no: 14,
        kind: CommentItemKind::Thread,
        author_kind,
        thread_id: Some("PRRT_1".into()),
        review_id: Some("PRR_1".into()),
        gates: Vec::new(),
        question: None,
    }
}

fn body_item() -> EffectItem {
    EffectItem {
        no: 9,
        kind: CommentItemKind::ReviewBody,
        author_kind: AuthorKind::Human,
        thread_id: None,
        review_id: Some("PRR_h1".into()),
        gates: Vec::new(),
        question: None,
    }
}

fn request(kind: DecisionKind) -> DecideRequest {
    DecideRequest {
        kind,
        reason: None,
        target: None,
        r#override: None,
        resolve_ticked: true,
        reply_text: None,
        comment: None,
        item_revision: 1,
        plan_digest: None,
    }
}

fn scope() -> PrScope {
    PrScope {
        number: 482,
        ..PrScope::default()
    }
}

/// (kind, after) of each effect.
fn shape(planned: &Planned) -> Vec<(EffectKind, Option<u32>)> {
    planned.effects.iter().map(|e| (e.kind, e.after)).collect()
}

fn planned(item: &EffectItem, request: &DecideRequest) -> Planned {
    plan(item, request, &scope()).expect("valid decision")
}

#[test]
fn effects_follow_matrix() {
    use EffectKind::{Fix, FollowUpPr, PrComment, Reply, Resolve, Todo, UpstreamIssue};
    let human = thread(AuthorKind::Human);
    let bot = thread(AuthorKind::Bot);
    let reject = DecideRequest {
        reason: Some("The lines are not part of this change.".into()),
        ..request(DecisionKind::Reject)
    };
    type Case = (
        &'static str,
        EffectItem,
        DecideRequest,
        Vec<(EffectKind, Option<u32>)>,
    );
    let cases: Vec<Case> = vec![
        (
            "D1 human: reply what was done, resolve",
            human.clone(),
            request(DecisionKind::Resolve),
            vec![(Reply, None), (Resolve, Some(1))],
        ),
        (
            "D1 bot: resolve only",
            bot.clone(),
            request(DecisionKind::Resolve),
            vec![(Resolve, None)],
        ),
        (
            "D2: fix, reply after the fix, resolve",
            bot.clone(),
            request(DecisionKind::FixHere),
            vec![(Fix, None), (Reply, Some(1)), (Resolve, Some(2))],
        ),
        (
            "D3: reply the reason, resolve",
            bot.clone(),
            reject.clone(),
            vec![(Reply, None), (Resolve, Some(1))],
        ),
        (
            "D4: reply with the PR, resolve",
            bot.clone(),
            DecideRequest {
                target: Some(DecisionTarget::ExistingPr { number: 490 }),
                ..request(DecisionKind::HandledInPr)
            },
            vec![(Reply, None), (Resolve, Some(1))],
        ),
        (
            "D5: create the follow-up PR, reply, resolve",
            bot.clone(),
            request(DecisionKind::FollowUpPr),
            vec![(FollowUpPr, None), (Reply, Some(1)), (Resolve, Some(2))],
        ),
        (
            "D6 create: issue, reply, resolve",
            bot.clone(),
            DecideRequest {
                target: Some(DecisionTarget::UpstreamIssue { url: None }),
                ..request(DecisionKind::UpstreamIssue)
            },
            vec![(UpstreamIssue, None), (Reply, Some(1)), (Resolve, Some(2))],
        ),
        (
            "D7: TODO, reply, resolve",
            bot.clone(),
            DecideRequest {
                target: Some(DecisionTarget::Todo {
                    title: "Cap the delay".into(),
                }),
                ..request(DecisionKind::Todo)
            },
            vec![(Todo, None), (Reply, Some(1)), (Resolve, Some(2))],
        ),
        (
            "D3 unticked: reply, no resolve",
            bot.clone(),
            DecideRequest {
                resolve_ticked: false,
                ..reject.clone()
            },
            vec![(Reply, None)],
        ),
        (
            "D2 on a body item: a PR-level comment, no resolve",
            body_item(),
            request(DecisionKind::FixHere),
            vec![(Fix, None), (PrComment, Some(1))],
        ),
    ];
    for (case, item, request, expected) in cases {
        let planned = planned(&item, &request);
        assert_eq!(shape(&planned), expected, "{case}");
        assert!(
            planned
                .effects
                .iter()
                .all(|e| e.executor == Executor::Agent),
            "{case}: phases 1-2 execute through agents"
        );
        for effect in &planned.effects {
            if let Some(text) = &effect.params.text {
                assert!(
                    text.starts_with("Per maintainer decision"),
                    "{case}: {text}"
                );
            }
            if matches!(effect.kind, Reply | Resolve) {
                assert_eq!(effect.params.thread_id.as_deref(), Some("PRRT_1"), "{case}");
            }
        }
    }
    let d7 = planned(
        &bot,
        &DecideRequest {
            target: Some(DecisionTarget::Todo {
                title: "Cap the delay".into(),
            }),
            ..request(DecisionKind::Todo)
        },
    );
    assert_eq!(
        d7.reply_text.as_deref(),
        Some("Per maintainer decision, out of scope for this PR."),
        "the D7 reply never mentions the TODO"
    );
    let d4 = planned(
        &bot,
        &DecideRequest {
            target: Some(DecisionTarget::ExistingPr { number: 490 }),
            ..request(DecisionKind::HandledInPr)
        },
    );
    assert_eq!(
        d4.reply_text.as_deref(),
        Some("Per maintainer decision, this is addressed in #490.")
    );
}

#[test]
fn accept_despite_gate_requires_reason_and_resolves() {
    let mut gated = thread(AuthorKind::Bot);
    gated.gates = vec![Gate::Funds];
    let d8 = DecideRequest {
        r#override: Some(Override::BlockerGate { gate: Gate::Funds }),
        reason: Some("The amount is bounded by the caller.".into()),
        ..request(DecisionKind::Reject)
    };
    let planned = planned(&gated, &d8);
    assert_eq!(
        shape(&planned),
        vec![(EffectKind::Reply, None), (EffectKind::Resolve, Some(1))]
    );
    assert_eq!(
        planned.reply_text.as_deref(),
        Some(
            "Per maintainer decision, accepted despite G-FUNDS: The amount is bounded by the caller."
        )
    );
    assert_eq!(
        plan(
            &gated,
            &DecideRequest {
                reason: Some("  ".into()),
                ..d8.clone()
            },
            &scope()
        ),
        Err(DecisionError::ReasonRequired)
    );
    assert_eq!(
        plan(
            &gated,
            &DecideRequest {
                resolve_ticked: false,
                ..d8.clone()
            },
            &scope()
        ),
        Err(DecisionError::ResolveLocked),
        "D8 always resolves"
    );
    assert_eq!(
        plan(&thread(AuthorKind::Bot), &d8, &scope()),
        Err(DecisionError::OverrideInvalid),
        "only on an item with that gate"
    );
    assert_eq!(
        plan(
            &gated,
            &DecideRequest {
                kind: DecisionKind::FixHere,
                ..d8
            },
            &scope()
        ),
        Err(DecisionError::OverrideInvalid)
    );
    assert_eq!(
        plan(
            &thread(AuthorKind::Human),
            &DecideRequest {
                resolve_ticked: false,
                ..request(DecisionKind::Resolve)
            },
            &scope()
        ),
        Err(DecisionError::ResolveLocked),
        "D1 always resolves"
    );
}

#[test]
fn follow_up_pr_needs_no_link() {
    let item = thread(AuthorKind::Bot);
    for target in [
        None,
        Some(DecisionTarget::FollowUpPr { title: None }),
        Some(DecisionTarget::FollowUpPr {
            title: Some("Bound the retry time".into()),
        }),
    ] {
        let planned = planned(
            &item,
            &DecideRequest {
                target: target.clone(),
                ..request(DecisionKind::FollowUpPr)
            },
        );
        assert_eq!(
            planned.effects[0].kind,
            EffectKind::FollowUpPr,
            "{target:?}"
        );
        assert_eq!(planned.effects[0].params.url, None, "{target:?}");
    }
    assert_eq!(
        plan(
            &item,
            &DecideRequest {
                target: Some(DecisionTarget::ExistingPr { number: 490 }),
                ..request(DecisionKind::FollowUpPr)
            },
            &scope()
        ),
        Err(DecisionError::TargetInvalid)
    );
}

#[test]
fn targets_reasons_and_kinds_are_checked() {
    let item = thread(AuthorKind::Bot);
    let check = |request: DecideRequest| plan(&item, &request, &scope());
    assert_eq!(
        check(request(DecisionKind::Reject)),
        Err(DecisionError::ReasonRequired)
    );
    assert_eq!(
        check(request(DecisionKind::HandledInPr)),
        Err(DecisionError::TargetRequired)
    );
    assert_eq!(
        check(DecideRequest {
            target: Some(DecisionTarget::ExistingPr { number: 482 }),
            ..request(DecisionKind::HandledInPr)
        }),
        Err(DecisionError::TargetInvalid),
        "the PR itself is not another PR"
    );
    assert_eq!(
        check(request(DecisionKind::Todo)),
        Err(DecisionError::TargetRequired)
    );
    assert_eq!(
        check(DecideRequest {
            target: Some(DecisionTarget::UpstreamIssue {
                url: Some("javascript:alert(1)".into())
            }),
            ..request(DecisionKind::UpstreamIssue)
        }),
        Err(DecisionError::TargetInvalid)
    );
    assert_eq!(
        check(request(DecisionKind::MergeWithFollowUps)),
        Err(DecisionError::KindNotOffered)
    );
    assert_eq!(
        check(DecideRequest {
            comment: Some("x".repeat(MAX_TEXT_CHARS + 1)),
            ..request(DecisionKind::FixHere)
        }),
        Err(DecisionError::TooLong)
    );
    let edited = planned(
        &item,
        &DecideRequest {
            reply_text: Some("Per maintainer decision, fixed by bounding the loop.".into()),
            ..request(DecisionKind::FixHere)
        },
    );
    assert_eq!(
        edited.effects[1].params.text.as_deref(),
        Some("Per maintainer decision, fixed by bounding the loop."),
        "the edited preview is what gets posted"
    );
}

#[test]
fn owner_comment_never_enters_an_effect() {
    let comment = "keep the test small";
    for kind in [
        DecisionKind::Resolve,
        DecisionKind::FixHere,
        DecisionKind::FollowUpPr,
    ] {
        let planned = planned(
            &thread(AuthorKind::Human),
            &DecideRequest {
                comment: Some(comment.into()),
                ..request(kind)
            },
        );
        let json = serde_json::to_string(&planned.effects).expect("json");
        assert!(!json.contains(comment), "{kind:?}: {json}");
        assert!(!planned.reply_text.unwrap_or_default().contains(comment));
    }
}

#[test]
fn pr_level_questions_plan_follow_ups_and_dismissals() {
    let merge = EffectItem {
        no: 20,
        kind: CommentItemKind::PrQuestion,
        author_kind: AuthorKind::Diffpilot,
        thread_id: None,
        review_id: None,
        gates: Vec::new(),
        question: Some(PrQuestion::Merge),
    };
    let scope = PrScope {
        number: 482,
        remaining: vec![thread(AuthorKind::Bot), body_item()],
        stale_reviews: vec!["PRR_r5".into()],
    };
    let p1 = plan(&merge, &request(DecisionKind::MergeWithFollowUps), &scope).expect("P1");
    assert_eq!(
        shape(&p1),
        vec![
            (EffectKind::FollowUpPr, None),
            (EffectKind::Reply, Some(1)),
            (EffectKind::Resolve, Some(2)),
            (EffectKind::PrComment, Some(1)),
        ]
    );
    assert_eq!(p1.effects[0].params.item_nos, vec![14, 9]);
    assert!(
        plan(&merge, &request(DecisionKind::AnotherRound), &scope)
            .expect("P2")
            .effects
            .is_empty()
    );
    assert_eq!(
        plan(&merge, &request(DecisionKind::DismissStaleReviews), &scope),
        Err(DecisionError::KindNotOffered)
    );
    let dismiss = EffectItem {
        question: Some(PrQuestion::Dismiss),
        ..merge
    };
    let p3 = plan(
        &dismiss,
        &request(DecisionKind::DismissStaleReviews),
        &scope,
    )
    .expect("P3");
    assert_eq!(shape(&p3), vec![(EffectKind::DismissReview, None)]);
    assert_eq!(p3.effects[0].params.review_id.as_deref(), Some("PRR_r5"));
    assert_eq!(p3.effects[0].params.text.as_deref(), Some(DISMISS_MESSAGE));
    assert!(
        plan(&dismiss, &request(DecisionKind::KeepReviews), &scope)
            .expect("P4")
            .effects
            .is_empty()
    );
}
