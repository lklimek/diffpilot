//! Properties (plan §C tests): parts sum to the total and stay in bounds, tiers are monotonic in the score,
//! provisional is never final, scoring and ranking are deterministic, and no input panics.

use contracts::api::dashboard::{DashboardSettings, PartStatus, ScoreComponent as C, ScoreState};
use contracts::common::RepoRef;
use contracts::time::Timestamp;
use jiff::SignedDuration;
use proptest::prelude::*;

use super::rules::BranchPattern;
use super::*;

fn now() -> Timestamp {
    "2026-11-18T08:00:00Z"
        .parse::<Timestamp>()
        .expect("valid time")
}

fn ago(days: f64) -> Timestamp {
    now() - SignedDuration::from_secs((days * 86_400.0) as i64)
}

fn rules() -> ScoreRules {
    ScoreRules::new("alice", &DashboardSettings::default())
}

/// Bounds in tenths per component (skill `COMPONENTS`).
fn bounds(component: C) -> (i32, i32) {
    match component {
        C::Advancement | C::UrgencyBonus => (0, 300),
        C::DefectSeverity => (0, 200),
        C::Unblocks | C::HumanWaiting => (0, 150),
        C::Recency => (-20, 150),
        C::ChangeImpact | C::Completion => (0, 100),
        C::ChangeSize => (0, 125),
        C::BlockedPenalty => (-200, 0),
        C::DefaultBranchBonus => (0, 50),
        C::MilestoneBonus => (-100, 200),
    }
}

fn component() -> impl Strategy<Value = C> {
    prop::sample::select(C::ALL.to_vec())
}

fn login() -> impl Strategy<Value = Option<String>> {
    prop::option::of(prop_oneof![
        "[a-z][a-z-]{0,8}",
        Just("alice".to_owned()),
        Just("dependabot[bot]".to_owned())
    ])
}

fn milestone() -> impl Strategy<Value = (Milestone, MilestoneContext)> {
    let title = prop_oneof!["v?[0-9]{1,2}\\.[0-9]{1,2}(\\.[0-9x])?", "\\PC{0,8}"];
    let list = || {
        prop::option::of(prop::collection::vec(
            prop_oneof!["v?[0-9]\\.[0-9]{1,2}\\.[0-9]", "\\PC{0,6}"],
            0..4,
        ))
    };
    let assignment = prop_oneof![
        Just(Milestone::Unknown),
        Just(Milestone::Unassigned),
        title.prop_map(Milestone::Assigned)
    ];
    (assignment, list(), list()).prop_map(|(m, release_tags, milestone_titles)| {
        (
            m,
            MilestoneContext {
                release_tags,
                milestone_titles,
            },
        )
    })
}

prop_compose! {
    fn review()(
        pending in any::<bool>(),
        requested in prop::option::of(-1.0f64..60.0),
        approx in any::<bool>(),
        decision in prop::option::of(prop_oneof![
            Just(ReviewDecision::ChangesRequested), Just(ReviewDecision::Approved), Just(ReviewDecision::ReviewRequired)
        ]),
        human_cr in any::<bool>(),
        cr_age in prop::option::of(0.0f64..60.0),
        truncated in any::<bool>(),
        reply in any::<bool>(),
        comment in prop::option::of(0.0f64..60.0),
        others in prop::option::of(0.0f64..60.0),
    ) -> (bool, Option<f64>, bool, Option<ReviewDecision>, bool, Option<f64>, bool, bool, Option<f64>, Option<f64>) {
        (pending, requested, approx, decision, human_cr, cr_age, truncated, reply, comment, others)
    }
}

prop_compose! {
    fn pr_facts()(
        (number, title, body) in (1u32..100_000, "\\PC{0,30}", "\\PC{0,40}"),
        author in login(),
        flags in any::<(bool, bool)>(),
        labels in prop::collection::vec(prop_oneof!["\\PC{0,10}", Just("ready-for-human".to_owned()), Just("blocked".to_owned())], 0..3),
        updated in -2.0f64..500.0,
        diff in prop::option::of((0u32..400_000, 0u32..400_000)),
        branches in (prop::option::of("[a-z0-9.-]{1,8}"), prop::option::of(prop_oneof!["main", "[a-z0-9.-]{1,8}"])),
        relations in (prop::option::of(prop::collection::vec(1u32..900, 0..5)), prop::option::of(prop::collection::vec(1u32..900, 0..3))),
        (milestone, milestone_context) in milestone(),
        state in (
            prop_oneof![Just(CiEffective::Green), Just(CiEffective::Red), Just(CiEffective::Pending), Just(CiEffective::None)],
            prop_oneof![Just(Mergeable::Mergeable), Just(Mergeable::Conflicting)],
            0u32..4,
            prop_oneof![Just(OwnedChange::NoPolicy), Just(OwnedChange::Unknown), Just(OwnedChange::Minor), Just(OwnedChange::Significant)],
            prop::sample::subsequence(vec![Relation::Authored, Relation::ReviewRequested, Relation::Assigned], 0..3),
        ),
        review in review(),
    ) -> PrFacts {
        let (is_draft, author_is_bot) = flags;
        let (ci, mergeable, unresolved_threads, owned_change, rels) = state;
        let (pending, requested, approx, decision, human_cr, cr_age, truncated, reply, comment, others) = review;
        PrFacts {
            repo: RepoRef { owner: "acme".into(), name: "wallet-ui".into() },
            number, title, body, author, author_is_bot, relations: rels, is_draft, labels,
            updated_at: ago(updated),
            additions: diff.map(|d| d.0), deletions: diff.map(|d| d.1),
            base_ref: branches.0, default_branch: branches.1,
            dependents: relations.0, stacked_on: relations.1,
            milestone, milestone_context, ci, mergeable,
            merge_state: MergeState::Unknown, viewer_can_write: false,
            review_decision: decision, unresolved_threads,
            review_pending_for_us: pending,
            review_requested_at: requested.map(ago),
            review_request_age_is_approx: approx,
            changes_requested_by: vec![],
            changes_requested_by_human: human_cr,
            changes_requested_human_at: cr_age.map(ago),
            reviews_truncated: truncated,
            awaiting_our_reply: reply,
            last_comment_author: Some("bob".into()),
            last_comment_at: comment.map(ago),
            owned_change,
            approved_by: vec![],
            approved_by_bots: vec![],
            review_requested_from_others: vec!["bob".into()],
            others_review_requested_at: others.map(ago),
        }
    }
}

fn judgement() -> impl Strategy<Value = Option<Judgement>> {
    let part = (component(), prop_oneof![-30i32..40, Just(0)], "\\PC{0,8}").prop_map(
        |(component, points, basis)| JudgedPart {
            component,
            points,
            basis,
        },
    );
    let eligibility = prop::option::of(
        (any::<bool>(), "\\PC{0,8}")
            .prop_map(|(eligible, reason)| EligibilityJudgement { eligible, reason }),
    );
    let freshness = prop_oneof![Just(Freshness::Current), Just(Freshness::Inherited)];
    prop::option::of(
        (
            prop::collection::vec(part, 0..8),
            eligibility,
            any::<bool>(),
            freshness,
        )
            .prop_map(|(parts, eligibility, alarm, freshness)| Judgement {
                parts,
                eligibility,
                alarm,
                also: vec![],
                judged_at: ago(1.0),
                freshness,
            }),
    )
}

fn tier_rank(points: f64) -> u8 {
    match tier(points) {
        contracts::api::dashboard::Tier::P0 => 0,
        contracts::api::dashboard::Tier::P1 => 1,
        contracts::api::dashboard::Tier::P2 => 2,
        contracts::api::dashboard::Tier::P3 => 3,
    }
}

proptest! {
    #[test]
    fn parts_sum_to_the_total_and_stay_in_bounds(pr in pr_facts(), judgement in judgement()) {
        let Ok(item) = score_pr(&rules(), &pr, judgement.as_ref(), now()) else { return Ok(()) };
        let parts = &item.score.parts;
        prop_assert_eq!(parts.iter().map(|p| p.component).collect::<Vec<_>>(), C::ALL.to_vec());
        prop_assert_eq!(item.score.total_tenths, parts.iter().filter_map(|p| p.points_tenths).sum::<i32>());
        prop_assert!((f64::from(item.score.total_tenths) / 10.0 - item.points).abs() <= 0.05 * 12.0);
        for p in parts {
            let (lo, hi) = bounds(p.component);
            if let Some(t) = p.points_tenths {
                prop_assert!((lo..=hi).contains(&t), "{:?} = {}", p.component, t);
            }
            prop_assert_eq!(p.points_tenths.is_none(), p.status == PartStatus::Pending);
            if p.status == PartStatus::Estimated {
                prop_assert!(p.basis.starts_with(ESTIMATE), "{}", p.basis);
            }
        }
    }

    #[test]
    fn provisional_is_never_final(pr in pr_facts(), judgement in judgement()) {
        let Ok(item) = score_pr(&rules(), &pr, judgement.as_ref(), now()) else { return Ok(()) };
        let open = item.score.parts.iter().any(|p| {
            matches!(p.status, PartStatus::Estimated | PartStatus::Pending | PartStatus::Inherited)
        });
        let unconfirmed = item.asks_eligibility && judgement.as_ref().is_none_or(|j| j.eligibility.is_none());
        if open || unconfirmed {
            prop_assert_ne!(item.score.state, ScoreState::Final);
        }
        if judgement.is_none() {
            prop_assert_eq!(item.score.state, ScoreState::Unjudged);
        }
        if item.score.state == ScoreState::Final {
            prop_assert_eq!(judgement.map(|j| j.freshness), Some(Freshness::Current));
        }
    }

    #[test]
    fn scoring_and_ranking_are_deterministic(prs in prop::collection::vec(pr_facts(), 1..12), seed in any::<u64>()) {
        let score = |list: &[PrFacts]| -> Vec<Scored> {
            list.iter().filter_map(|pr| score_pr(&rules(), pr, None, now()).ok()).collect()
        };
        let first = score(&prs);
        prop_assert_eq!(&first, &score(&prs));
        let mut shuffled = first.clone();
        let len = shuffled.len();
        shuffled.rotate_left((seed as usize) % len.max(1));
        let order = |r: Ranking| r.ranked.into_iter().map(|i| i.key).collect::<Vec<_>>();
        prop_assert_eq!(order(rank(first)), order(rank(shuffled)));
    }

    #[test]
    fn tier_is_monotonic_in_the_score(a in -50.0f64..200.0, b in -50.0f64..200.0) {
        let (low, high) = if a <= b { (a, b) } else { (b, a) };
        prop_assert!(tier_rank(high) <= tier_rank(low));
    }

    #[test]
    fn formulas_are_monotonic_and_bounded(d1 in -5.0f64..1000.0, d2 in -5.0f64..1000.0, l1 in 0u64..10_000_000, l2 in 0u64..10_000_000, n in 0usize..50) {
        let (d_lo, d_hi) = if d1 <= d2 { (d1, d2) } else { (d2, d1) };
        prop_assert!(recency_points(d_hi) <= recency_points(d_lo));
        prop_assert!((-2.0..=15.0).contains(&recency_points(d1)));
        prop_assert!(waiting_points(d_hi) <= waiting_points(d_lo));
        let (l_lo, l_hi) = if l1 <= l2 { (l1, l2) } else { (l2, l1) };
        prop_assert!(change_size_points(l_hi) <= change_size_points(l_lo));
        prop_assert!((0.0..=12.5).contains(&change_size_points(l1)));
        prop_assert!(unblocks_points(n) <= unblocks_points(n + 1) && unblocks_points(n) <= 15.0);
    }

    #[test]
    fn text_rules_never_panic(a in "\\PC{0,40}", b in "\\PC{0,40}", pattern in "[a-zX.Y*/-]{0,12}", branch in "[a-z0-9.*/-]{0,30}") {
        let _ = text::conventional_type(&a);
        let _ = (text::has_fix_wording(&a), text::has_blocker_wording(&a), text::has_word(&a, "blocked"));
        let _ = (text::version(&a, true), text::version(&a, false), text::is_builtin_ignored_check(&a));
        prop_assert_eq!(text::compare_ids(&a, &b), text::compare_ids(&b, &a).reverse());
        let _ = BranchPattern::new(&pattern).matches(&branch);
        prop_assert!(BranchPattern::new("*").matches(&branch));
    }
}
