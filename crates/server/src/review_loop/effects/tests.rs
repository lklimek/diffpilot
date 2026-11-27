use contracts::api::review_loop::{
    AuthorKind, CommentItemKind, DecideRequest, DecisionKind, DecisionTarget, EffectKind, Executor,
    Gate, Override,
};

use super::*;

fn thread(author_kind: AuthorKind) -> EffectItem {
    EffectItem {
        quote: None,
        no: 14,
        kind: CommentItemKind::Thread,
        author: "octo".into(),
        author_kind,
        url: Some("https://github.com/acme/node-client/pull/482#discussion_r1".into()),
        thread_id: Some("PRRT_1".into()),
        review_id: Some("PRR_1".into()),
        gates: Vec::new(),
        question: None,
    }
}

/// The page of bob's review h1.
const H1_URL: &str = "https://github.com/acme/node-client/pull/482#pullrequestreview-4000000105";

fn body_item() -> EffectItem {
    EffectItem {
        quote: None,
        no: 9,
        kind: CommentItemKind::ReviewBody,
        author: "bob".into(),
        author_kind: AuthorKind::Human,
        url: Some(H1_URL.into()),
        thread_id: None,
        review_id: Some("PRR_h1".into()),
        gates: Vec::new(),
        question: None,
    }
}

#[test]
fn a_grounded_point_starts_with_its_quote_under_the_review_heading() {
    let item = EffectItem {
        quote: Some("Bound the retry loop.\nAdditional context.".into()),
        ..body_item()
    };
    let text = planned_text(&item, &request(DecisionKind::FixHere));
    assert!(
        text.starts_with(&format!(
            "@bob, re [your review]({H1_URL}):\n> Bound the retry loop.\n"
        )),
        "{text}"
    );
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
                let text = text
                    .strip_prefix(&format!("@bob, re [your review]({H1_URL}):\n"))
                    .unwrap_or(text);
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
fn follow_up_issue_creates_one_issue_before_reply_and_optional_resolve() {
    let item = EffectItem {
        quote: Some("Bound the retry loop.".into()),
        ..thread(AuthorKind::Human)
    };
    for resolve_ticked in [true, false] {
        let result = planned(
            &item,
            &DecideRequest {
                target: Some(DecisionTarget::FollowUpIssue {
                    title: "  Retry limit  ".into(),
                }),
                resolve_ticked,
                ..request(DecisionKind::FollowUpIssue)
            },
        );
        let issue = &result.effects[0];
        assert_eq!(issue.kind, EffectKind::UpstreamIssue);
        assert_eq!(issue.params.title.as_deref(), Some("Retry limit"));
        assert!(issue.params.url.is_none());
        let body = issue.params.text.as_deref().expect("issue body");
        assert!(body.contains(item.url.as_deref().expect("source URL")));
        assert!(body.contains("Bound the retry loop."));
        assert_eq!(result.effects[1].kind, EffectKind::Reply);
        assert_eq!(result.effects[1].after, Some(issue.seq));
        assert_eq!(result.effects.len(), if resolve_ticked { 3 } else { 2 });
        if resolve_ticked {
            assert_eq!(result.effects[2].kind, EffectKind::Resolve);
            assert_eq!(result.effects[2].after, Some(result.effects[1].seq));
        }
    }
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
        quote: None,
        no: 20,
        kind: CommentItemKind::PrQuestion,
        author: "diffpilot".into(),
        author_kind: AuthorKind::Diffpilot,
        url: None,
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
        ..merge.clone()
    };
    for question in [&merge, &dismiss] {
        assert_eq!(
            plan(question, &request(DecisionKind::NotAReviewPoint), &scope),
            Err(DecisionError::KindNotOffered)
        );
    }
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

/// Plan §2.3 (owner, 2026-11-23): a PR-level comment answering a person's review starts with a line naming the review
/// and its author; the stored reply text is the posted one.
#[test]
fn a_pr_level_comment_on_a_persons_review_names_it() {
    let planned = planned(&body_item(), &request(DecisionKind::FixHere));
    let expected = format!(
        "@bob, re [your review]({H1_URL}):\nPer maintainer decision, this is fixed in this pull request."
    );
    assert_eq!(planned.effects[1].kind, EffectKind::PrComment);
    assert_eq!(
        planned.effects[1].params.text.as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(planned.reply_text.as_deref(), Some(expected.as_str()));
    let resolved = planned_text(&body_item(), &request(DecisionKind::Resolve));
    assert_eq!(
        resolved,
        format!(
            "@bob, re [your review]({H1_URL}):\n\
             Per maintainer decision, this is already addressed in this pull request."
        )
    );
}

/// The text of the reply or PR-level comment `request` plans on `item`.
fn planned_text(item: &EffectItem, request: &DecideRequest) -> String {
    planned(item, request)
        .effects
        .into_iter()
        .find_map(|effect| {
            matches!(effect.kind, EffectKind::Reply | EffectKind::PrComment)
                .then_some(effect.params.text)?
        })
        .expect("a reply")
}

/// Plan §2.3: a bot's review is named without an @-mention, which would trigger the bot.
#[test]
fn a_pr_level_comment_on_a_bots_review_names_it_without_a_mention() {
    let bot = EffectItem {
        author: "thepastaclaw".into(),
        author_kind: AuthorKind::Bot,
        url: Some(
            "https://github.com/acme/node-client/pull/482#pullrequestreview-4000000101".into(),
        ),
        review_id: Some("PRR_r1".into()),
        ..body_item()
    };
    let text = planned_text(
        &bot,
        &DecideRequest {
            reason: Some("The caller bounds it.".into()),
            ..request(DecisionKind::Reject)
        },
    );
    assert_eq!(
        text,
        "Re [thepastaclaw's review](https://github.com/acme/node-client/pull/482#pullrequestreview-4000000101):\n\
         Per maintainer decision, not changed: The caller bounds it."
    );
    assert!(!text.contains('@'), "{text}");
}

/// A PR-level comment identifies the source when it answers a comment or an edited response.
#[test]
fn pr_level_comments_name_the_source_even_when_edited() {
    let fix = request(DecisionKind::FixHere);
    let thread_reply = planned_text(&thread(AuthorKind::Human), &fix);
    assert_eq!(
        thread_reply,
        "Per maintainer decision, this is fixed in this pull request."
    );
    let comment = EffectItem {
        kind: CommentItemKind::PrComment,
        quote: Some("Bound the retry loop.".into()),
        url: Some("https://github.com/acme/node-client/pull/482#issuecomment-5000000103".into()),
        review_id: None,
        ..body_item()
    };
    assert_eq!(
        planned_text(&comment, &fix),
        "Re [the source comment](https://github.com/acme/node-client/pull/482#issuecomment-5000000103):\n\
         > Bound the retry loop.\nPer maintainer decision, this is fixed in this pull request."
    );
    let edited = DecideRequest {
        reply_text: Some("Thanks, fixed in the retry loop.".into()),
        ..fix
    };
    assert_eq!(
        planned_text(&body_item(), &edited),
        format!("@bob, re [your review]({H1_URL}):\nThanks, fixed in the retry loop.")
    );
}

/// Plan §2.3 P1: each remaining body item's PR-level comment names its own review.
#[test]
fn merge_follow_up_comments_name_their_reviews() {
    let merge = EffectItem {
        quote: None,
        no: 20,
        kind: CommentItemKind::PrQuestion,
        author: "diffpilot".into(),
        author_kind: AuthorKind::Diffpilot,
        url: None,
        thread_id: None,
        review_id: None,
        gates: Vec::new(),
        question: Some(PrQuestion::Merge),
    };
    let bot = EffectItem {
        no: 10,
        author: "coderabbitai".into(),
        author_kind: AuthorKind::Bot,
        url: Some(
            "https://github.com/acme/node-client/pull/482#pullrequestreview-4000000106".into(),
        ),
        review_id: Some("PRR_c1".into()),
        ..body_item()
    };
    let scope = PrScope {
        number: 482,
        remaining: vec![body_item(), bot],
        stale_reviews: Vec::new(),
    };
    let p1 = plan(&merge, &request(DecisionKind::MergeWithFollowUps), &scope).expect("P1");
    let texts: Vec<&str> = p1
        .effects
        .iter()
        .filter_map(|effect| effect.params.text.as_deref())
        .collect();
    assert_eq!(
        texts,
        [
            format!("@bob, re [your review]({H1_URL}):\n{FOLLOW_UP_REPLY}"),
            format!(
                "Re [coderabbitai's review]\
                 (https://github.com/acme/node-client/pull/482#pullrequestreview-4000000106):\n{FOLLOW_UP_REPLY}"
            ),
        ]
    );
}

/// Plan §2.5: a carried decision plans again with the stored reply text (`carry_over`), which already starts with the
/// line naming the review; the line is not added a second time.
#[test]
fn a_carried_decision_names_the_review_once() {
    for kind in [DecisionKind::FixHere, DecisionKind::Resolve] {
        let first = planned(&body_item(), &request(kind));
        let carried = DecideRequest {
            reply_text: first.reply_text.clone(),
            ..request(kind)
        };
        let again = planned(&body_item(), &carried);
        assert_eq!(again.reply_text, first.reply_text, "{kind:?}");
        assert_eq!(
            planned_text(&body_item(), &carried),
            planned_text(&body_item(), &request(kind)),
            "{kind:?}"
        );
        assert_eq!(
            again
                .reply_text
                .as_deref()
                .map(|t| t.matches("re [your review]").count()),
            Some(1),
            "{kind:?}"
        );
    }
}
