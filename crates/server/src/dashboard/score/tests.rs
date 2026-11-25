//! Contract tests: the skill's test vectors (`test_scoring.py`, `test_score.py`, `test_milestones.py`,
//! `test_todos.py`, `test_digest.py`) and the plan §I.2 acceptance tests of WP11-SCORE.

use contracts::api::dashboard::{
    DashboardSettings, ItemKey, ItemTarget, PartSource, PartStatus, PolicyRef, RepositoryRule,
    ScoreComponent as C, ScorePart, ScoreState, Tier,
};
use contracts::common::RepoRef;
use contracts::time::Timestamp;
use jiff::SignedDuration;
use uuid::Uuid;

use super::*;

fn now() -> Timestamp {
    "2026-11-18T08:00:00Z"
        .parse::<Timestamp>()
        .expect("valid time")
}

fn ago(days: f64) -> Timestamp {
    now() - SignedDuration::from_micros((days * 86_400e6) as i64)
}

fn repo(owner: &str, name: &str) -> RepoRef {
    RepoRef {
        owner: owner.into(),
        name: name.into(),
    }
}

fn rules() -> ScoreRules {
    let settings = DashboardSettings {
        linked_logins: vec!["alice-ci".into()],
        ..DashboardSettings::default()
    };
    ScoreRules::new("alice", &settings)
}

fn pr() -> PrFacts {
    PrFacts {
        repo: repo("acme", "wallet-ui"),
        number: 1,
        title: "fix: thing".into(),
        body: String::new(),
        author: Some("alice".into()),
        author_is_bot: false,
        relations: vec![Relation::Authored],
        is_draft: false,
        labels: vec![],
        updated_at: now(),
        additions: Some(0),
        deletions: Some(0),
        base_ref: Some("v1.0-dev".into()),
        default_branch: Some("main".into()),
        dependents: Some(vec![]),
        stacked_on: Some(vec![]),
        milestone: Milestone::Unassigned,
        milestone_context: MilestoneContext::default(),
        ci: CiEffective::Green,
        mergeable: Mergeable::Mergeable,
        merge_state: MergeState::Unknown,
        viewer_can_write: false,
        review_decision: None,
        unresolved_threads: 0,
        review_pending_for_us: false,
        review_requested_at: None,
        review_request_age_is_approx: false,
        changes_requested_by: vec![],
        changes_requested_by_bot: Some(false),
        changes_requested_by_human: false,
        changes_requested_human_at: None,
        reviews_truncated: false,
        awaiting_our_reply: false,
        last_comment_author: None,
        last_comment_at: None,
        owned_change: OwnedChange::NoPolicy,
        approved_by: vec![],
        approved_by_bots: vec![],
        review_requested_from_others: vec![],
        others_review_requested_at: None,
    }
}

fn issue() -> IssueFacts {
    IssueFacts {
        repo: repo("acme", "consensus"),
        number: 7,
        title: "crash on start".into(),
        body: String::new(),
        author: Some("someone".into()),
        relations: vec![Relation::RepoOpen],
        labels: vec![],
        updated_at: now(),
        milestone: Milestone::Unassigned,
        milestone_context: MilestoneContext::default(),
    }
}

fn scored(pr: &PrFacts) -> Scored {
    score_pr(&rules(), pr, None, now()).expect("scores")
}

fn part(item: &Scored, component: C) -> &ScorePart {
    item.score
        .parts
        .iter()
        .find(|p| p.component == component)
        .expect("every component")
}

fn tenths_of(item: &Scored, component: C) -> Option<i32> {
    part(item, component).points_tenths
}

fn computed(item: &Scored, component: C) -> (Option<i32>, PartStatus) {
    let p = part(item, component);
    (p.points_tenths, p.status)
}

fn judged(parts: &[(C, i32)]) -> Judgement {
    Judgement {
        parts: parts
            .iter()
            .map(|(c, p)| JudgedPart {
                component: *c,
                points: *p,
                basis: "evidence".into(),
            })
            .collect(),
        eligibility: None,
        alarm: false,
        also: vec![],
        judged_at: ago(1.0),
        freshness: Freshness::Current,
    }
}

const JUDGED: [(C, i32); 4] = [
    (C::DefectSeverity, 0),
    (C::UrgencyBonus, 0),
    (C::ChangeImpact, 5),
    (C::Completion, 5),
];

// --- formulas ---------------------------------------------------------------------------------------------

#[test]
fn recency_points_match_skill_examples() {
    for (days, expected) in [
        (0.0, 15.0),
        (1.0, 12.74),
        (3.0, 9.08),
        (7.0, 4.26),
        (15.0, 0.0),
        (30.0, -1.76),
    ] {
        assert!((recency_points(days) - expected).abs() < 0.01, "{days}");
    }
    assert!((recency_points(10_000.0) + 2.0).abs() < 1e-6 && recency_points(10_000.0) >= -2.0);
    assert!(
        (recency_points(-0.5) - 15.0).abs() < f64::EPSILON,
        "clock skew never exceeds the maximum"
    );
}

#[test]
fn change_size_points_match_skill_examples() {
    for (lines, expected) in [
        (0, 12.5),
        (1000, 10.0),
        (5000, 4.096),
        (10_000, 1.342_177_28),
    ] {
        assert!(
            (change_size_points(lines) - expected).abs() < 1e-9 * expected,
            "{lines}"
        );
    }
}

#[test]
fn waiting_and_unblocks_buckets() {
    for (age, expected) in [
        (0.0, 15.0),
        (2.0, 15.0),
        (2.01, 10.0),
        (7.0, 10.0),
        (7.01, 5.0),
        (90.0, 5.0),
    ] {
        assert!(
            (waiting_points(age) - expected).abs() < f64::EPSILON,
            "{age}"
        );
    }
    for (n, expected) in [(0, 0.0), (1, 5.0), (2, 10.0), (3, 15.0), (9, 15.0)] {
        assert!((unblocks_points(n) - expected).abs() < f64::EPSILON, "{n}");
    }
}

#[test]
fn tier_thresholds_at_full_precision() {
    for (points, expected) in [
        (157.5, Tier::P1),
        (100.0001, Tier::P1),
        (65.0, Tier::P1),
        (64.999, Tier::P2),
        (64.96, Tier::P2),
        (35.0, Tier::P2),
        (34.999, Tier::P3),
        (-22.0, Tier::P3),
    ] {
        assert_eq!(tier(points), expected, "{points}");
    }
    assert_eq!((P1_FROM_TENTHS, P2_FROM_TENTHS), (650, 350));
}

// --- parts ------------------------------------------------------------------------------------------------

#[test]
fn new_item_is_unjudged_with_named_estimates() {
    let item = scored(&pr());
    assert_eq!(item.score.state, ScoreState::Unjudged);
    let order: Vec<C> = item.score.parts.iter().map(|p| p.component).collect();
    assert_eq!(order, C::ALL);
    for component in [
        C::DefectSeverity,
        C::UrgencyBonus,
        C::ChangeImpact,
        C::Completion,
    ] {
        let p = part(&item, component);
        assert_eq!(
            (p.source, p.status),
            (PartSource::Llm, PartStatus::Estimated),
            "{component:?}"
        );
        assert!(
            p.basis.starts_with("estimate until judged: "),
            "{}",
            p.basis
        );
        assert!(p.points_tenths.is_some());
    }
    assert_eq!(
        item.asks,
        [
            C::DefectSeverity,
            C::ChangeImpact,
            C::Completion,
            C::UrgencyBonus
        ]
    );
    // K13 constants: `fix` type → defect 8, impact 10; CI green, mergeable, ready → completion 5; urgency 0.
    let fix: Vec<_> = [
        C::DefectSeverity,
        C::ChangeImpact,
        C::Completion,
        C::UrgencyBonus,
    ]
    .iter()
    .map(|c| tenths_of(&item, *c))
    .collect();
    assert_eq!(fix, [Some(80), Some(100), Some(50), Some(0)]);
    for (title, impact, defect) in [
        ("docs: readme", 0, 0),
        ("refactor(x): y", 50, 0),
        ("feat: z", 100, 0),
        ("Panics on start", 100, 80),
    ] {
        let item = scored(&PrFacts {
            title: title.into(),
            ci: CiEffective::Red,
            ..pr()
        });
        assert_eq!(tenths_of(&item, C::ChangeImpact), Some(impact), "{title}");
        assert_eq!(tenths_of(&item, C::DefectSeverity), Some(defect), "{title}");
        assert_eq!(tenths_of(&item, C::Completion), Some(0), "{title}");
    }
    let docs = scored(&PrFacts {
        title: "docs: readme".into(),
        ..pr()
    });
    assert_eq!(
        part(&docs, C::ChangeImpact).basis,
        "estimate until judged: conventional type `docs` → 0"
    );
}

#[test]
fn issue_parts_and_defaults() {
    let item = score_issue(&rules(), &issue(), None, now()).expect("scores");
    assert_eq!(
        computed(&item, C::ChangeSize),
        (Some(0), PartStatus::Computed)
    );
    assert_eq!(part(&item, C::ChangeSize).source, PartSource::NotApplicable);
    assert_eq!(
        part(&item, C::DefaultBranchBonus).source,
        PartSource::NotApplicable
    );
    assert_eq!(
        computed(&item, C::Recency),
        (Some(150), PartStatus::Computed)
    );
    assert_eq!(
        computed(&item, C::Advancement),
        (Some(0), PartStatus::Computed)
    );
    for component in [C::Unblocks, C::HumanWaiting] {
        let p = part(&item, component);
        assert_eq!(
            (p.points_tenths, p.source, p.status),
            (Some(0), PartSource::Computed, PartStatus::Estimated)
        );
    }
    assert!(item.asks.contains(&C::Unblocks) && item.asks.contains(&C::HumanWaiting));
}

#[test]
fn default_branch_bonus_uses_repository_metadata() {
    for (base, default, expected) in [
        ("main", Some("main"), Some(50)),
        ("v1.0-dev", Some("v1.0-dev"), Some(50)),
        ("feat/base", Some("main"), Some(0)),
        ("release/1", Some("main"), Some(0)),
        ("main", None, None),
    ] {
        let item = scored(&PrFacts {
            base_ref: Some(base.into()),
            default_branch: default.map(Into::into),
            ..pr()
        });
        let p = part(&item, C::DefaultBranchBonus);
        assert_eq!(p.points_tenths, expected, "{base}");
        let status = if expected.is_some() {
            PartStatus::Computed
        } else {
            PartStatus::Pending
        };
        assert_eq!(p.status, status, "{base}");
    }
}

#[test]
fn advancement_draft_open_and_ready_label() {
    assert_eq!(
        computed(
            &scored(&PrFacts {
                is_draft: true,
                ..pr()
            }),
            C::Advancement
        ),
        (Some(150), PartStatus::Computed)
    );
    assert_eq!(
        computed(&scored(&pr()), C::Advancement),
        (Some(200), PartStatus::Computed)
    );
    let ready = scored(&PrFacts {
        labels: vec!["Ready-For-Human".into()],
        ..pr()
    });
    let p = part(&ready, C::Advancement);
    assert_eq!(
        (p.points_tenths, p.source, p.status),
        (Some(300), PartSource::Computed, PartStatus::Estimated)
    );
    let draft_ready = scored(&PrFacts {
        is_draft: true,
        labels: vec!["ready-for-human".into()],
        ..pr()
    });
    assert_eq!(
        computed(&draft_ready, C::Advancement),
        (Some(150), PartStatus::Computed)
    );
    let custom = ScoreRules::new(
        "alice",
        &DashboardSettings {
            ready_label: "lgtm".into(),
            ..DashboardSettings::default()
        },
    );
    let item = score_pr(
        &custom,
        &PrFacts {
            labels: vec!["LGTM".into()],
            ..pr()
        },
        None,
        now(),
    )
    .expect("scores");
    assert_eq!(tenths_of(&item, C::Advancement), Some(300));
}

#[test]
fn unblocks_counts_dependents_and_lookup_failure_is_not_zero() {
    let two = scored(&PrFacts {
        dependents: Some(vec![5, 6]),
        ..pr()
    });
    assert_eq!(
        computed(&two, C::Unblocks),
        (Some(100), PartStatus::Computed)
    );
    let failed = scored(&PrFacts {
        dependents: None,
        ..pr()
    });
    assert_eq!(computed(&failed, C::Unblocks), (None, PartStatus::Pending));
    assert!(
        part(&scored(&pr()), C::Unblocks)
            .basis
            .contains("not checked")
    );
}

fn review_request(days: f64, approx: bool, author: &str, bot: bool) -> PrFacts {
    PrFacts {
        author: Some(author.into()),
        author_is_bot: bot,
        relations: vec![Relation::ReviewRequested],
        review_pending_for_us: true,
        review_requested_at: Some(ago(days)),
        review_request_age_is_approx: approx,
        ..pr()
    }
}

#[test]
fn human_waiting_review_request_from_a_human() {
    for (days, expected) in [(1.0, 150), (5.0, 100), (10.0, 50)] {
        let item = scored(&review_request(days, false, "bob", false));
        assert_eq!(
            computed(&item, C::HumanWaiting),
            (Some(expected), PartStatus::Computed),
            "{days}"
        );
    }
    let approx = scored(&review_request(1.0, true, "bob", false));
    let p = part(&approx, C::HumanWaiting);
    assert_eq!(
        (p.points_tenths, p.status),
        (Some(150), PartStatus::Estimated)
    );
    assert!(p.basis.contains("approximate"), "{}", p.basis);
}

#[test]
fn human_waiting_ignores_bot_and_own_authors() {
    for (author, bot) in [
        ("dependabot", true),
        ("coderabbitai", true),
        ("thepastaclaw", false),
        ("alice-ci", false),
        ("alice", false),
    ] {
        let item = scored(&review_request(1.0, false, author, bot));
        assert_eq!(
            computed(&item, C::HumanWaiting),
            (Some(0), PartStatus::Computed),
            "{author}"
        );
    }
}

/// A bot is GitHub's `Bot` type, a `[bot]` login or a login of the user's known bots (defaults: the review bots);
/// an excluded author is no bot.
#[test]
fn is_bot_uses_typename_suffix_and_known_bots_only() {
    let r = rules();
    for (login, typename_bot, expected) in [
        ("dependabot", true, true),
        ("renovate[bot]", false, true),
        ("thepastaclaw", false, true),
        ("DCG-Claude", false, true),
        ("claudius-maginificent", false, true),
        ("Dependabot", false, false),
        ("bob", false, false),
    ] {
        assert_eq!(r.is_bot(login, typename_bot), expected, "{login}");
    }
    let edited = ScoreRules::new(
        "alice",
        &DashboardSettings {
            known_bots: vec!["Carol".into()],
            ..DashboardSettings::default()
        },
    );
    assert!(
        !edited.is_bot("thepastaclaw", false),
        "removed from the known bots"
    );
    assert!(edited.is_bot("carol", false), "added to the known bots");
}

/// A pull request by `author` (a human by GitHub's type) whose review is requested from alice.
fn requested_by(author: &str, settings: &DashboardSettings) -> Scored {
    score_pr(
        &ScoreRules::new("alice", settings),
        &PrFacts {
            author: Some(author.into()),
            relations: vec![Relation::ReviewRequested],
            review_pending_for_us: true,
            review_requested_at: Some(now() - SignedDuration::from_hours(24)),
            ..pr()
        },
        None,
        now(),
    )
    .expect("scores")
}

/// The known bots decide who counts as a waiting human: a known bot's review request gives no human-waiting
/// points; removed from the list, the same login is a human waiting.
#[test]
fn known_bots_decide_the_waiting_human() {
    let defaults = DashboardSettings::default();
    assert_eq!(
        part(&requested_by("thepastaclaw", &defaults), C::HumanWaiting).points_tenths,
        Some(0)
    );
    let without = DashboardSettings {
        known_bots: vec![],
        ..DashboardSettings::default()
    };
    assert_eq!(
        part(&requested_by("thepastaclaw", &without), C::HumanWaiting).points_tenths,
        Some(150)
    );
    let with_carol = DashboardSettings {
        known_bots: vec!["carol".into()],
        ..DashboardSettings::default()
    };
    assert_eq!(
        part(&requested_by("carol", &with_carol), C::HumanWaiting).points_tenths,
        Some(0)
    );
}

/// An excluded author is banned, not a bot: everything they author is left out, also a review they request
/// from the user or an item assigned to the user; their login still counts as a human elsewhere.
#[test]
fn excluded_author_is_always_left_out_and_not_a_bot() {
    let settings = DashboardSettings {
        skip_authors: vec!["carol".into()],
        ..DashboardSettings::default()
    };
    let rules = ScoreRules::new("alice", &settings);
    for relations in [
        vec![Relation::ReviewRequested],
        vec![Relation::Assigned],
        vec![Relation::Mentioned],
    ] {
        let item = score_pr(
            &rules,
            &PrFacts {
                author: Some("carol".into()),
                relations: relations.clone(),
                review_pending_for_us: relations == [Relation::ReviewRequested],
                ..pr()
            },
            None,
            now(),
        )
        .expect("scores");
        assert_eq!(
            item.excluded.as_deref(),
            Some("author @carol is excluded"),
            "{relations:?}"
        );
    }
    assert!(!rules.is_bot("carol", false));
}

#[test]
fn human_waiting_change_requests_and_unanswered_comments() {
    let cr = |decision, by_human, age: Option<f64>| PrFacts {
        review_decision: decision,
        changes_requested_by_human: by_human,
        changes_requested_human_at: age.map(ago),
        ..pr()
    };
    let no_decision = scored(&cr(None, true, Some(1.0)));
    assert_eq!(
        computed(&no_decision, C::HumanWaiting),
        (Some(150), PartStatus::Computed)
    );
    let own = scored(&cr(Some(ReviewDecision::ChangesRequested), true, Some(4.0)));
    assert_eq!(
        computed(&own, C::HumanWaiting),
        (Some(100), PartStatus::Computed)
    );
    let invisible = scored(&cr(Some(ReviewDecision::ChangesRequested), false, None));
    assert_eq!(
        part(&invisible, C::HumanWaiting).status,
        PartStatus::Estimated
    );
    assert!(
        part(&invisible, C::HumanWaiting).basis.contains(
            "GitHub's review decision is changes requested, but the last 20 reviews hold no change request from \
             another person"
        ),
        "{}",
        part(&invisible, C::HumanWaiting).basis
    );
    let rerequested = scored(&cr(None, true, None));
    assert_eq!(
        computed(&rerequested, C::HumanWaiting),
        (Some(0), PartStatus::Computed)
    );
    assert!(
        part(&rerequested, C::HumanWaiting)
            .basis
            .contains("re-requested")
    );
    let bot_cr = |truncated| PrFacts {
        changes_requested_by: vec!["coderabbitai".into()],
        reviews_truncated: truncated,
        ..cr(Some(ReviewDecision::ChangesRequested), false, None)
    };
    assert_eq!(
        part(&scored(&bot_cr(true)), C::HumanWaiting).status,
        PartStatus::Estimated
    );
    assert_eq!(
        computed(&scored(&bot_cr(false)), C::HumanWaiting),
        (Some(0), PartStatus::Computed)
    );
    let team = scored(&PrFacts {
        author: Some("bob".into()),
        relations: vec![Relation::ReviewRequested],
        ..pr()
    });
    let p = part(&team, C::HumanWaiting);
    assert_eq!(
        (p.points_tenths, p.status),
        (Some(0), PartStatus::Estimated)
    );
    assert!(p.basis.contains("team"));
    assert_eq!(
        computed(&scored(&pr()), C::HumanWaiting),
        (Some(0), PartStatus::Computed)
    );
    let comment = |days| PrFacts {
        awaiting_our_reply: true,
        last_comment_author: Some("bob".into()),
        last_comment_at: Some(ago(days)),
        ..review_request(10.0, false, "bob", false)
    };
    let fresh = scored(&comment(1.0));
    let p = part(&fresh, C::HumanWaiting);
    assert_eq!(
        (p.points_tenths, p.status),
        (Some(50), PartStatus::Estimated)
    );
    assert!(p.basis.contains("@bob"));
    let old = scored(&PrFacts {
        review_requested_at: Some(ago(1.0)),
        ..comment(9.0)
    });
    assert_eq!(
        computed(&old, C::HumanWaiting),
        (Some(150), PartStatus::Computed)
    );
}

#[test]
fn recency_and_change_size_from_facts() {
    assert_eq!(
        computed(
            &scored(&PrFacts {
                updated_at: ago(15.0),
                ..pr()
            }),
            C::Recency
        ),
        (Some(0), PartStatus::Computed)
    );
    let future = scored(&PrFacts {
        updated_at: ago(-0.001),
        ..pr()
    });
    assert_eq!(tenths_of(&future, C::Recency), Some(150));
    assert!(!part(&future, C::Recency).basis.contains("-0"));
    let size = scored(&PrFacts {
        additions: Some(600),
        deletions: Some(400),
        ..pr()
    });
    assert_eq!(
        computed(&size, C::ChangeSize),
        (Some(100), PartStatus::Computed)
    );
    assert!(part(&size, C::ChangeSize).basis.contains("1000"));
    let missing = scored(&PrFacts {
        additions: None,
        ..pr()
    });
    assert_eq!(
        computed(&missing, C::ChangeSize),
        (None, PartStatus::Pending)
    );
}

#[test]
fn blocked_penalty_rules() {
    let stacked = scored(&PrFacts {
        base_ref: Some("feat/base".into()),
        stacked_on: Some(vec![42]),
        ..pr()
    });
    assert_eq!(
        computed(&stacked, C::BlockedPenalty),
        (Some(-200), PartStatus::Computed)
    );
    assert!(part(&stacked, C::BlockedPenalty).basis.contains("#42"));
    let labelled = scored(&PrFacts {
        labels: vec!["blocked".into()],
        ..pr()
    });
    assert_eq!(
        part(&labelled, C::BlockedPenalty).status,
        PartStatus::Pending
    );
    assert_eq!(part(&labelled, C::BlockedPenalty).source, PartSource::Llm);
    assert!(labelled.asks.contains(&C::BlockedPenalty));
    let failed = scored(&PrFacts {
        stacked_on: None,
        ..pr()
    });
    assert_eq!(
        computed(&failed, C::BlockedPenalty),
        (None, PartStatus::Pending)
    );
    let both = scored(&PrFacts {
        labels: vec!["blocked".into()],
        stacked_on: Some(vec![42]),
        ..pr()
    });
    assert_eq!(
        computed(&both, C::BlockedPenalty),
        (Some(-200), PartStatus::Computed)
    );
    let body = scored(&PrFacts {
        body: "Depends on acme/platform#5588".into(),
        ..pr()
    });
    assert_eq!(
        computed(&body, C::BlockedPenalty),
        (None, PartStatus::Pending)
    );
    assert_eq!(
        computed(&scored(&pr()), C::BlockedPenalty),
        (Some(0), PartStatus::Computed)
    );
    let issue_status = |labels: Vec<String>, body: &str| {
        let item = score_issue(
            &rules(),
            &IssueFacts {
                labels,
                body: body.into(),
                ..issue()
            },
            None,
            now(),
        );
        part(&item.expect("scores"), C::BlockedPenalty).status
    };
    assert_eq!(
        issue_status(vec!["Blocked".into()], ""),
        PartStatus::Pending
    );
    assert_eq!(
        issue_status(vec![], "This is blocked by #12."),
        PartStatus::Pending
    );
    assert_eq!(issue_status(vec![], "plain report"), PartStatus::Computed);
}

// --- CI ---------------------------------------------------------------------------------------------------

fn check(
    name: &str,
    conclusion: Option<&str>,
    status: &str,
    started: Option<&str>,
    workflow: &str,
) -> CheckContext {
    CheckContext::CheckRun {
        name: name.into(),
        workflow: Some(workflow.into()),
        status: Some(status.into()),
        conclusion: conclusion.map(Into::into),
        started_at: started.map(|s| s.parse().expect("time")),
    }
}

fn ok(name: &str) -> CheckContext {
    check(
        name,
        Some("SUCCESS"),
        "COMPLETED",
        Some("2026-10-01T00:00:00Z"),
        "CI",
    )
}

fn failed(name: &str, conclusion: &str) -> CheckContext {
    check(
        name,
        Some(conclusion),
        "COMPLETED",
        Some("2026-10-01T00:00:00Z"),
        "CI",
    )
}

fn status(context: &str, state: &str) -> CheckContext {
    CheckContext::StatusContext {
        context: context.into(),
        state: state.into(),
        created_at: Some("2026-10-01T00:00:00Z".parse().expect("time")),
    }
}

fn classify(contexts: Vec<CheckContext>, truncated: bool) -> CiSummary {
    classify_ci(
        Some(&CheckRollup {
            contexts,
            truncated,
        }),
        text::is_builtin_ignored_check,
    )
}

#[test]
fn ci_classification_matches_skill_vectors() {
    let policy = classify(
        vec![
            ok("Clippy"),
            status("PR Hygiene", "PENDING"),
            failed("policy / reconcile", "FAILURE"),
        ],
        false,
    );
    assert_eq!(policy.effective, CiEffective::Green);
    assert!(policy.red.is_empty());
    assert_eq!(
        policy.ignored_not_passing,
        ["PR Hygiene (PENDING)", "policy / reconcile (FAILURE)"]
    );

    let red = classify(
        vec![
            failed("Test Suite", "FAILURE"),
            failed("Clippy", "TIMED_OUT"),
            status("ci/legacy", "ERROR"),
            status("PR Hygiene", "FAILURE"),
        ],
        false,
    );
    assert_eq!(red.effective, CiEffective::Red);
    assert_eq!(red.red, ["Clippy", "Test Suite", "ci/legacy"]);
    assert_eq!(red.ignored_not_passing, ["PR Hygiene (FAILURE)"]);

    let running = classify(
        vec![check(
            "Build",
            None,
            "IN_PROGRESS",
            Some("2026-10-01T00:00:00Z"),
            "CI",
        )],
        false,
    );
    assert_eq!(
        (running.effective, running.pending.as_slice()),
        (CiEffective::Pending, ["Build".to_owned()].as_slice())
    );

    let rerun_passed = classify(
        vec![
            check(
                "Test Suite",
                Some("SUCCESS"),
                "COMPLETED",
                Some("2026-10-02T00:00:00Z"),
                "CI",
            ),
            check(
                "Test Suite",
                Some("FAILURE"),
                "COMPLETED",
                Some("2026-10-01T00:00:00Z"),
                "CI",
            ),
        ],
        false,
    );
    assert_eq!(rerun_passed.effective, CiEffective::Green);
    let queued = classify(
        vec![
            check(
                "Test Suite",
                Some("FAILURE"),
                "COMPLETED",
                Some("2026-10-01T00:00:00Z"),
                "CI",
            ),
            check("Test Suite", None, "QUEUED", None, "CI"),
        ],
        false,
    );
    assert_eq!(queued.effective, CiEffective::Pending);

    let twins = classify(
        vec![
            check(
                "build",
                Some("FAILURE"),
                "COMPLETED",
                Some("2026-10-01T00:00:00Z"),
                "A",
            ),
            check(
                "build",
                Some("SUCCESS"),
                "COMPLETED",
                Some("2026-10-02T00:00:00Z"),
                "B",
            ),
        ],
        false,
    );
    assert_eq!(
        (twins.effective, twins.red.as_slice()),
        (CiEffective::Red, ["A / build".to_owned()].as_slice())
    );

    assert_eq!(classify(vec![], false).effective, CiEffective::Pending);
    let neutral = classify(vec![failed("a", "SKIPPED"), failed("b", "NEUTRAL")], false);
    assert_eq!(neutral.effective, CiEffective::Green);
    assert_eq!(
        classify(vec![ok("a")], true).effective,
        CiEffective::Unknown
    );
    assert_eq!(
        classify(vec![failed("a", "FAILURE")], true).effective,
        CiEffective::Red
    );
    let none = classify_ci(None, text::is_builtin_ignored_check);
    assert_eq!((none.effective, none.red.len()), (CiEffective::None, 0));
}

/// K17 R4: a repository row's Ignored checks replace the built-ins (whole name, any case, `*` any text); an ignored
/// check never makes CI red or pending and is listed as information; rows without a list keep the built-ins.
#[test]
fn ignored_checks_follow_the_repository_row() {
    let settings = DashboardSettings {
        repositories: vec![RepositoryRule {
            repo: repo("acme", "platform"),
            alias: None,
            watch_issues: false,
            ownership_policy: None,
            branch_patterns: vec![],
            ignored_checks: vec!["*lint*".into(), "Docs X.Y".into()],
        }],
        ..DashboardSettings::default()
    };
    let r = ScoreRules::new("alice", &settings);
    let platform = repo("acme", "platform");
    for (name, ignored) in [
        ("Clippy lint", true),
        ("LINT", true),
        ("docs x.y", true),
        ("Docs 1.2", false),
        ("PR Hygiene", false),
        ("Test Suite", false),
    ] {
        assert_eq!(r.is_ignored_check(&platform, name), ignored, "{name}");
    }
    let other = repo("acme", "wallet-ui");
    for (name, ignored) in [
        ("PR Hygiene", true),
        ("PastaClaw review", true),
        ("Clippy lint", false),
    ] {
        assert_eq!(r.is_ignored_check(&other, name), ignored, "{name}");
    }
    let ci = classify_ci(
        Some(&CheckRollup {
            contexts: vec![
                ok("Build"),
                failed("Clippy lint", "FAILURE"),
                status("PR Hygiene", "PENDING"),
            ],
            truncated: false,
        }),
        |name| r.is_ignored_check(&platform, name),
    );
    assert_eq!(
        (
            ci.effective,
            ci.pending.as_slice(),
            ci.ignored_not_passing.as_slice()
        ),
        (
            CiEffective::Pending,
            ["PR Hygiene".to_owned()].as_slice(),
            ["Clippy lint (FAILURE)".to_owned()].as_slice()
        )
    );
}

/// F39: the display size mark of a pull request from the fixed table (tiny < 150, small < 500, medium < 3000,
/// large < 10 000, huge from 10 000 changed lines); unknown totals have none.
#[test]
fn size_mark_follows_the_fixed_table() {
    use contracts::api::dashboard::{ChangeSizeMark, SizeMark};
    let mark = |additions: Option<u32>, deletions: Option<u32>| {
        size_mark(&PrFacts {
            additions,
            deletions,
            ..pr()
        })
        .map(|m| m.size)
    };
    assert_eq!(mark(Some(0), Some(0)), Some(SizeMark::Tiny));
    assert_eq!(mark(Some(100), Some(49)), Some(SizeMark::Tiny));
    assert_eq!(mark(Some(100), Some(50)), Some(SizeMark::Small));
    assert_eq!(mark(Some(400), Some(99)), Some(SizeMark::Small));
    assert_eq!(mark(Some(400), Some(100)), Some(SizeMark::Medium));
    assert_eq!(mark(Some(2000), Some(999)), Some(SizeMark::Medium));
    assert_eq!(mark(Some(2000), Some(1000)), Some(SizeMark::Large));
    assert_eq!(mark(Some(9000), Some(999)), Some(SizeMark::Large));
    assert_eq!(mark(Some(9000), Some(1000)), Some(SizeMark::Huge));
    assert_eq!(mark(None, Some(5)), None);
    assert_eq!(mark(Some(5), None), None);
    assert_eq!(
        size_mark(&PrFacts {
            additions: Some(30),
            deletions: Some(5),
            ..pr()
        }),
        Some(ChangeSizeMark {
            size: SizeMark::Tiny,
            additions: 30,
            deletions: 5
        })
    );
}

// --- eligibility --------------------------------------------------------------------------------------------

/// D19: the ownership policy never drops a pull request directed at the user (authored by a scanned login,
/// assigned to one, or a review requested from one directly); a team-only request or a mention stays filtered,
/// and excluded authors still win.
#[test]
fn ownership_policy_keeps_pull_requests_directed_at_the_user() {
    let r = rules();
    let verdict = |facts: PrFacts| {
        let item = score_pr(&r, &facts, None, now()).expect("scores");
        (item.excluded, item.asks_eligibility)
    };
    let outside = |relations: Vec<Relation>, author: &str, direct: bool, owned| PrFacts {
        author: Some(author.into()),
        relations,
        review_pending_for_us: direct,
        owned_change: owned,
        ..pr()
    };
    for (facts, why) in [
        (
            outside(
                vec![Relation::ReviewRequested],
                "bob",
                true,
                OwnedChange::None,
            ),
            "review requested from alice",
        ),
        (
            outside(vec![Relation::Assigned], "bob", false, OwnedChange::Minor),
            "assigned to alice",
        ),
        (
            outside(
                vec![Relation::Mentioned],
                "alice-ci",
                false,
                OwnedChange::None,
            ),
            "authored by a linked login",
        ),
        (
            outside(
                vec![Relation::ReviewRequested],
                "bob",
                true,
                OwnedChange::Unknown,
            ),
            "unknown policy, directed",
        ),
    ] {
        assert_eq!(verdict(facts), (None, false), "{why}");
    }
    for (facts, reason) in [
        (
            outside(
                vec![Relation::ReviewRequested],
                "bob",
                false,
                OwnedChange::None,
            ),
            "no change in owned paths (ownership policy)",
        ),
        (
            outside(vec![Relation::Mentioned], "bob", false, OwnedChange::Minor),
            "only a minor change in owned paths (ownership policy)",
        ),
        (
            outside(
                vec![Relation::ReviewRequested],
                "dependabot",
                true,
                OwnedChange::Significant,
            ),
            "author @dependabot is excluded",
        ),
    ] {
        assert_eq!(verdict(facts), (Some(reason.to_owned()), false), "{reason}");
    }
}

/// A pull request of someone else that is not directed at the user (only mentioned).
fn undirected() -> PrFacts {
    PrFacts {
        author: Some("bob".into()),
        relations: vec![Relation::Mentioned],
        ..pr()
    }
}

#[test]
fn eligibility_ownership_policy_and_drafts() {
    let r = rules();
    for (verdict, eligible) in [
        (OwnedChange::Significant, true),
        (OwnedChange::Minor, false),
        (OwnedChange::None, false),
        (OwnedChange::NoPolicy, true),
    ] {
        let item = score_pr(
            &r,
            &PrFacts {
                owned_change: verdict,
                ..undirected()
            },
            None,
            now(),
        )
        .expect("scores");
        assert_eq!(item.excluded.is_none(), eligible, "{verdict:?}");
        assert!(!item.asks_eligibility);
    }
    let unknown = scored(&PrFacts {
        owned_change: OwnedChange::Unknown,
        ..undirected()
    });
    assert!(unknown.excluded.is_none() && unknown.asks_eligibility);
    let confirmed = score_pr(
        &r,
        &PrFacts {
            owned_change: OwnedChange::Unknown,
            ..undirected()
        },
        Some(&judged(&JUDGED)),
        now(),
    );
    assert_eq!(
        confirmed.expect("scores").score.state,
        ScoreState::Provisional,
        "eligibility still unconfirmed"
    );
    for (author, eligible) in [("alice", true), ("Alice-CI", true), ("bob", false)] {
        let item = scored(&PrFacts {
            is_draft: true,
            author: Some(author.into()),
            ..pr()
        });
        assert_eq!(item.excluded.is_none(), eligible, "{author}");
    }
    assert!(
        scored(&PrFacts {
            author: Some("bob".into()),
            ..pr()
        })
        .excluded
        .is_none()
    );
}

/// A dependabot pull request with `relations`.
fn dependabot(relations: Vec<Relation>) -> Scored {
    scored(&PrFacts {
        author: Some("dependabot[bot]".into()),
        author_is_bot: true,
        relations,
        ..pr()
    })
}

#[test]
fn excluded_author_items_are_left_out_whatever_their_relation() {
    for relations in [
        vec![Relation::ReviewRequested],
        vec![Relation::Assigned],
        vec![Relation::Mentioned],
        vec![Relation::RepoOpen],
        vec![],
    ] {
        assert_eq!(
            dependabot(relations.clone()).excluded.as_deref(),
            Some("author @dependabot[bot] is excluded"),
            "{relations:?}"
        );
    }
    let issue_by = |relations| {
        score_issue(
            &rules(),
            &IssueFacts {
                author: Some("Dependabot".into()),
                relations,
                ..issue()
            },
            None,
            now(),
        )
        .expect("scores")
        .excluded
    };
    assert_eq!(
        issue_by(vec![Relation::Assigned]).as_deref(),
        Some("author @Dependabot is excluded")
    );
    let renovate = scored(&PrFacts {
        author: Some("renovate[bot]".into()),
        author_is_bot: true,
        relations: vec![],
        ..pr()
    });
    assert!(
        renovate.excluded.is_none(),
        "only the configured authors are excluded"
    );
}

// --- milestones ---------------------------------------------------------------------------------------------

fn context(tags: Option<&[&str]>, titles: Option<&[&str]>) -> MilestoneContext {
    let own = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect();
    MilestoneContext {
        release_tags: tags.map(own),
        milestone_titles: titles.map(own),
    }
}

const TAGS: [&str; 4] = ["v1.7.0", "v1.8.0", "v1.8.2", "v1.6.9"];
const TITLES: [&str; 5] = ["v3.0", "v1.8", "v1.7", "v2.0", "v0.5.x"];

fn milestone_tenths(title: &str, ctx: MilestoneContext) -> (Option<i32>, Option<i32>) {
    let milestone = Milestone::Assigned(title.into());
    let pr_item = scored(&PrFacts {
        milestone: milestone.clone(),
        milestone_context: ctx.clone(),
        ..pr()
    });
    let issue_item = score_issue(
        &rules(),
        &IssueFacts {
            milestone,
            milestone_context: ctx,
            ..issue()
        },
        None,
        now(),
    );
    (
        tenths_of(&pr_item, C::MilestoneBonus),
        tenths_of(&issue_item.expect("scores"), C::MilestoneBonus),
    )
}

#[test]
fn milestone_bonus_from_shipped_versions() {
    for (title, points) in [
        ("v1.8", 200),
        ("v1.7", 200),
        ("v0.5.x", 200),
        ("v2.0", 100),
        ("v3.0", -100),
    ] {
        let got = milestone_tenths(title, context(Some(&TAGS), Some(&TITLES)));
        assert_eq!(got, (Some(points), Some(points)), "{title}");
    }
    let with_19 = [TITLES.as_slice(), &["v1.9"]].concat();
    assert_eq!(
        milestone_tenths("v2.0", context(Some(&TAGS), Some(with_19.as_slice()))).0,
        Some(-100)
    );
    fn s(list: &'static [&'static str]) -> Option<&'static [&'static str]> {
        Some(list)
    }
    for (title, tags, titles, expected) in [
        ("v1.8.2", s(&TAGS), s(&TITLES), Some(200)),
        ("v1.8.x", s(&TAGS), s(&TITLES), Some(200)),
        ("v1.8.X", s(&TAGS), s(&TITLES), Some(200)),
        ("v2.0.0", s(&TAGS), s(&TITLES), Some(100)),
        ("v1.8.3", s(&TAGS), s(&["v1.8.3", "v2.0"]), Some(100)),
        ("v2.0", s(&TAGS), s(&["v1.10", "v2.0"]), Some(-100)),
        ("v1.9", s(&["v1.10.0", "v1.8.2"]), s(&TITLES), Some(200)),
        ("v1.8", s(&TAGS), None, Some(200)),
        ("v2.0", s(&TAGS), None, None),
        ("v2.0", s(&[]), s(&TITLES), None),
        ("v2.0", None, s(&TITLES), None),
        ("v2.0", s(&["v2.0.0-rc.1"]), s(&TITLES), None),
        ("v2.0", s(&["v1.8.2", "release-2.0.0"]), s(&TITLES), None),
        ("v2.0", s(&TAGS), s(&["Release 1.9", "v2.0"]), None),
        ("Launch", s(&TAGS), s(&TITLES), None),
        ("v2.0", s(&TAGS), s(&["v3.0"]), None),
    ] {
        assert_eq!(
            milestone_tenths(title, context(tags, titles)).0,
            expected,
            "{title} {tags:?} {titles:?}"
        );
    }
    assert_eq!(tenths_of(&scored(&pr()), C::MilestoneBonus), Some(0));
    let unknown = scored(&PrFacts {
        milestone: Milestone::Unknown,
        ..pr()
    });
    assert_eq!(
        computed(&unknown, C::MilestoneBonus),
        (None, PartStatus::Pending)
    );
    assert!(unknown.asks.contains(&C::MilestoneBonus));
}

// --- judgement merge ------------------------------------------------------------------------------------

#[test]
fn complete_judgement_is_final_and_tiered() {
    let item = score_pr(&rules(), &pr(), Some(&judged(&JUDGED)), now()).expect("scores");
    assert_eq!(item.score.state, ScoreState::Final);
    // advancement 20 + recency 15 + size 12.5 + impact 5 + completion 5
    assert!((item.points - 57.5).abs() < 1e-9);
    assert_eq!((item.score.total_tenths, item.score.tier), (575, Tier::P2));
    let breakdown: Vec<_> = item
        .score
        .parts
        .iter()
        .map(|p| p.points_tenths.unwrap_or(i32::MIN))
        .collect();
    assert_eq!(breakdown, [200, 0, 0, 0, 150, 50, 50, 125, 0, 0, 0, 0]);
    assert_eq!(part(&item, C::ChangeImpact).status, PartStatus::Judged);
    assert_eq!(part(&item, C::ChangeImpact).source, PartSource::Llm);
}

#[test]
fn default_branch_and_milestone_move_the_tier() {
    let impact10 = [
        (C::DefectSeverity, 0),
        (C::UrgencyBonus, 0),
        (C::ChangeImpact, 10),
        (C::Completion, 5),
    ];
    for (base, total, expected) in [("main", 675, Tier::P1), ("feat/base", 625, Tier::P2)] {
        let item = score_pr(
            &rules(),
            &PrFacts {
                base_ref: Some(base.into()),
                ..pr()
            },
            Some(&judged(&impact10)),
            now(),
        );
        let item = item.expect("scores");
        assert_eq!(
            (item.score.total_tenths, item.score.tier),
            (total, expected),
            "{base}"
        );
    }
    for (title, total, expected) in [
        ("v1.8", 775, Tier::P1),
        ("v2.0", 675, Tier::P1),
        ("v3.0", 475, Tier::P2),
    ] {
        let facts = PrFacts {
            milestone: Milestone::Assigned(title.into()),
            milestone_context: context(Some(&["v1.8.2"]), Some(&["v1.8", "v2.0", "v3.0"])),
            ..pr()
        };
        let item = score_pr(&rules(), &facts, Some(&judged(&JUDGED)), now()).expect("scores");
        assert_eq!(
            (item.score.total_tenths, item.score.tier),
            (total, expected),
            "{title}"
        );
    }
    let distant = PrFacts {
        base_ref: Some("main".into()),
        milestone: Milestone::Assigned("v3.0".into()),
        milestone_context: context(Some(&["v1.8.2"]), Some(&["v2.0", "v3.0"])),
        ..pr()
    };
    let item = score_pr(&rules(), &distant, Some(&judged(&impact10)), now()).expect("scores");
    assert_eq!((item.score.total_tenths, item.score.tier), (575, Tier::P2));
}

#[test]
fn unanswered_or_unconfirmed_parts_keep_the_score_provisional() {
    let partial = score_pr(&rules(), &pr(), Some(&judged(&JUDGED[..3])), now()).expect("scores");
    assert_eq!(partial.score.state, ScoreState::Provisional);
    assert_eq!(part(&partial, C::Completion).status, PartStatus::Estimated);
    let ready = PrFacts {
        labels: vec!["ready-for-human".into()],
        ..pr()
    };
    let kept = score_pr(&rules(), &ready, Some(&judged(&JUDGED)), now()).expect("scores");
    assert_eq!(
        (kept.score.state, kept.score.total_tenths),
        (ScoreState::Provisional, 675)
    );
    let confirmed = [JUDGED.as_slice(), &[(C::Advancement, 20)]].concat();
    let item = score_pr(&rules(), &ready, Some(&judged(&confirmed)), now()).expect("scores");
    assert_eq!(item.score.state, ScoreState::Final);
    let p = part(&item, C::Advancement);
    assert_eq!(
        (p.points_tenths, p.status, p.basis.as_str()),
        (Some(200), PartStatus::Judged, "evidence")
    );
}

#[test]
fn overrides_need_a_basis_and_values_are_closed() {
    let mut override_unblocks = judged(&[JUDGED.as_slice(), &[(C::Unblocks, 5)]].concat());
    let item = score_pr(&rules(), &pr(), Some(&override_unblocks), now()).expect("scores");
    let p = part(&item, C::Unblocks);
    assert_eq!(
        (p.points_tenths, p.status, p.source),
        (Some(50), PartStatus::Overridden, PartSource::Llm)
    );
    if let Some(last) = override_unblocks.parts.last_mut() {
        last.basis = " ".into();
    }
    assert_eq!(
        score_pr(&rules(), &pr(), Some(&override_unblocks), now()),
        Err(ScoreError::OverrideWithoutBasis(C::Unblocks))
    );
    let same_value = judged(&[JUDGED.as_slice(), &[(C::Unblocks, 0)]].concat());
    let item = score_pr(&rules(), &pr(), Some(&same_value), now()).expect("scores");
    assert_eq!(part(&item, C::Unblocks).status, PartStatus::Computed);
    for (component, points) in [
        (C::ChangeImpact, 7),
        (C::DefectSeverity, 21),
        (C::BlockedPenalty, -10),
    ] {
        let bad = judged(&[(component, points)]);
        assert_eq!(
            score_pr(&rules(), &pr(), Some(&bad), now()),
            Err(ScoreError::OutOfRange { component, points })
        );
    }
    for component in [C::Recency, C::ChangeSize] {
        let bad = judged(&[(component, 0)]);
        assert_eq!(
            score_pr(&rules(), &pr(), Some(&bad), now()),
            Err(ScoreError::NotJudgeable(component))
        );
    }
    assert_eq!(
        JudgedPart::defect(4, 7, "x".into()).map(|p| p.points),
        Ok(11)
    );
    assert!(JudgedPart::defect(11, 0, "x".into()).is_err());
}

#[test]
fn eligibility_flip_needs_a_reason() {
    let asked = PrFacts {
        owned_change: OwnedChange::Unknown,
        ..undirected()
    };
    let mut j = judged(&JUDGED);
    j.eligibility = Some(EligibilityJudgement {
        eligible: false,
        reason: String::new(),
    });
    assert_eq!(
        score_pr(&rules(), &asked, Some(&j), now()),
        Err(ScoreError::EligibilityWithoutReason)
    );
    j.eligibility = Some(EligibilityJudgement {
        eligible: false,
        reason: "only test renames".into(),
    });
    let item = score_pr(&rules(), &asked, Some(&j), now()).expect("scores");
    assert_eq!(item.excluded.as_deref(), Some("only test renames"));
    // Not asked (eligibility settled by the facts): the answer is ignored.
    let item = score_pr(&rules(), &pr(), Some(&j), now()).expect("scores");
    assert_eq!(item.excluded, None);
}

#[test]
fn changed_item_keeps_judged_parts_as_inherited() {
    let ready = PrFacts {
        labels: vec!["ready-for-human".into()],
        ..pr()
    };
    let previous = Judgement {
        freshness: Freshness::Inherited,
        alarm: true,
        ..judged(&[
            (C::DefectSeverity, 12),
            (C::Advancement, 20),
            (C::Unblocks, 5),
            (C::Completion, 5),
        ])
    };
    let item = score_pr(&rules(), &ready, Some(&previous), now()).expect("scores");
    assert_eq!(item.score.state, ScoreState::Provisional);
    assert_eq!(
        item.score.tier,
        Tier::P0,
        "a carried alarm stays, provisional"
    );
    let defect = part(&item, C::DefectSeverity);
    assert_eq!(
        (defect.points_tenths, defect.status),
        (Some(120), PartStatus::Inherited)
    );
    assert!(
        defect
            .basis
            .starts_with("[judged 17 Nov, item changed since] "),
        "{}",
        defect.basis
    );
    // Computed parts and computed defaults follow the live scan.
    assert_eq!(
        computed(&item, C::Unblocks),
        (Some(0), PartStatus::Computed)
    );
    assert_eq!(
        computed(&item, C::Advancement),
        (Some(300), PartStatus::Estimated)
    );
    // Parts the old judgement left open stay estimates.
    assert_eq!(part(&item, C::ChangeImpact).status, PartStatus::Estimated);
    let current = Judgement {
        freshness: Freshness::Current,
        ..previous
    };
    let same = score_pr(&rules(), &ready, Some(&current), now()).expect("scores");
    assert_eq!(part(&same, C::DefectSeverity).status, PartStatus::Judged);
}

// --- ranking ----------------------------------------------------------------------------------------------

fn tie(id: &str, number: u32, adv: i32, defect: i32, unblocks: i32) -> Scored {
    let mut item = scored(&PrFacts { number, ..pr() });
    item.id = id.into();
    item.points = 45.0;
    for p in &mut item.score.parts {
        p.points_tenths = Some(match p.component {
            C::Advancement => adv,
            C::DefectSeverity => defect,
            C::Unblocks => unblocks,
            _ => 0,
        });
    }
    item
}

#[test]
fn tie_break_order_then_natural_id() {
    let items = vec![
        tie("DET-1000", 1, 150, 0, 50),
        tie("DET-999", 2, 150, 0, 50),
        tie("A-1", 3, 100, 50, 50),
        tie("B-1", 4, 150, 50, 0),
        tie("C-1", 5, 200, 0, 0),
    ];
    let ids: Vec<String> = rank(items).ranked.into_iter().map(|i| i.id).collect();
    assert_eq!(ids, ["C-1", "B-1", "DET-999", "DET-1000", "A-1"]);
    let mut fallback = vec![tie("node#12", 6, 0, 0, 0), tie("node#9", 7, 0, 0, 0)];
    fallback.reverse();
    let ids: Vec<String> = rank(fallback).ranked.into_iter().map(|i| i.id).collect();
    assert_eq!(ids, ["node#9", "node#12"]);
}

#[test]
fn alarms_rank_first_and_merges_leave_items_out() {
    let low = score_pr(
        &rules(),
        &PrFacts { number: 2, ..pr() },
        Some(&Judgement {
            alarm: true,
            ..judged(&JUDGED)
        }),
        now(),
    );
    let high = scored(&PrFacts { number: 3, ..pr() });
    let todo_id = Uuid::from_u128(7);
    let todo_key = ItemKey::new(&ItemTarget::Todo(todo_id)).expect("key");
    let gone = ItemKey::new(&ItemTarget::Todo(Uuid::from_u128(8))).expect("key");
    let todo = TodoFacts {
        id: todo_id,
        title: "t".into(),
        status: TodoStatus::Pending,
        priority: None,
        changed_at: now(),
    };
    let mut owner_j = judged(&JUDGED);
    owner_j.also = vec![todo_key.clone(), gone, high.key.clone()];
    let owner = score_pr(
        &rules(),
        &PrFacts { number: 4, ..pr() },
        Some(&owner_j),
        now(),
    )
    .expect("scores");
    let todo = score_todo(&todo, "TODO-00000000".into(), None, now()).expect("scores");
    let ranking = rank(vec![todo, high, low.expect("scores"), owner.clone()]);
    assert_eq!(ranking.ranked.first().map(|i| i.score.tier), Some(Tier::P0));
    let ranked: Vec<&str> = ranking.ranked.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ranked, ["wallet-ui#2", "wallet-ui#4"]);
    let survivor = ranking
        .ranked
        .iter()
        .find(|i| i.key == owner.key)
        .expect("owner ranked");
    assert_eq!(
        survivor.also.len(),
        2,
        "a target that no longer exists is dropped"
    );
    let reasons: Vec<&str> = ranking.excluded.iter().map(|e| e.reason.as_str()).collect();
    assert_eq!(
        reasons,
        ["merged into wallet-ui#4", "merged into wallet-ui#4"]
    );
}

/// B4: every left-out item names the rule that left it out (one per rule) and, when merged, the item it went into.
#[test]
fn every_left_out_item_names_its_rule() {
    use contracts::api::dashboard::ExclusionReason as R;
    let r = rules();
    let score = |facts: PrFacts, judgement: Option<&Judgement>| {
        score_pr(&r, &facts, judgement, now()).expect("scores")
    };
    let mut not_eligible = judged(&JUDGED);
    not_eligible.eligibility = Some(EligibilityJudgement {
        eligible: false,
        reason: "Generated code only.".into(),
    });
    let survivor = score(PrFacts { number: 6, ..pr() }, None);
    let merged = score(PrFacts { number: 5, ..pr() }, None);
    let items = vec![
        score(
            PrFacts {
                number: 1,
                author: Some("dependabot".into()),
                ..pr()
            },
            None,
        ),
        score(
            PrFacts {
                number: 2,
                owned_change: OwnedChange::Minor,
                ..undirected()
            },
            None,
        ),
        score(
            PrFacts {
                number: 3,
                is_draft: true,
                ..undirected()
            },
            None,
        ),
        score(
            PrFacts {
                number: 4,
                owned_change: OwnedChange::Unknown,
                ..undirected()
            },
            Some(&not_eligible),
        ),
        merged.clone(),
        Scored {
            also: vec![merged.key.clone()],
            ..survivor
        },
    ];
    let ranking = rank(items);
    let left_out: Vec<_> = ranking
        .excluded
        .iter()
        .map(|e| (e.id.as_str(), e.rule, e.merged_into.as_deref()))
        .collect();
    assert_eq!(
        left_out,
        [
            ("wallet-ui#1", R::ExcludedAuthor, None),
            ("wallet-ui#2", R::OutsideOwnedAreas, None),
            ("wallet-ui#3", R::Draft, None),
            ("wallet-ui#4", R::JudgedNotEligible, None),
            ("wallet-ui#5", R::MergedIntoAnother, Some("wallet-ui#6")),
        ]
    );
    let ranked: Vec<_> = ranking.ranked.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ranked, ["wallet-ui#6"]);
}

#[test]
fn merge_claims_never_form_cycles() {
    let a = scored(&PrFacts { number: 1, ..pr() });
    let b = scored(&PrFacts { number: 2, ..pr() });
    let claim = |item: &Scored, target: &Scored| Scored {
        also: vec![target.key.clone(), item.key.clone()],
        ..item.clone()
    };
    let ranking = rank(vec![claim(&b, &a), claim(&a, &b)]);
    assert_eq!(ranking.ranked.len(), 1);
    assert_eq!(ranking.excluded.len(), 1);
}

// --- TODOs ------------------------------------------------------------------------------------------------

#[test]
fn todo_rows_from_status_priority_and_title() {
    let todo = |status, title: &str, priority: &str| TodoFacts {
        id: Uuid::from_u128(1),
        title: title.into(),
        status,
        priority: Some(priority.into()),
        changed_at: ago(3.0),
    };
    let started = score_todo(
        &todo(TodoStatus::InProgress, "finish acme/platform#5150", "high"),
        "TODO-x".into(),
        None,
        now(),
    );
    let started = started.expect("scores");
    assert_eq!(
        computed(&started, C::Advancement),
        (Some(100), PartStatus::Computed)
    );
    assert_eq!(
        computed(&started, C::ChangeImpact),
        (Some(100), PartStatus::Estimated)
    );
    assert_eq!(started.score.state, ScoreState::Unjudged);
    assert_eq!(tenths_of(&started, C::Recency), Some(91));
    let blocked = score_todo(
        &todo(TodoStatus::Blocked, "fix panic", "medium"),
        "TODO-y".into(),
        None,
        now(),
    );
    let blocked = blocked.expect("scores");
    let p = part(&blocked, C::BlockedPenalty);
    assert_eq!(
        (p.points_tenths, p.source, p.status),
        (Some(-200), PartSource::Computed, PartStatus::Estimated)
    );
    assert_eq!(tenths_of(&blocked, C::DefectSeverity), Some(80));
    assert_eq!(tenths_of(&blocked, C::ChangeImpact), Some(50));
    let high = score_todo(
        &todo(TodoStatus::Pending, "[HIGH] review", "low"),
        "TODO-z".into(),
        None,
        now(),
    );
    assert_eq!(
        tenths_of(&high.expect("scores"), C::DefectSeverity),
        Some(80)
    );
}

#[test]
fn todo_display_ids_extend_on_collision() {
    let parse = |s: &str| Uuid::parse_str(s).expect("uuid");
    let ids = [
        parse("abcdef12-0000-4000-8000-000000000000"),
        parse("abcdef12-1111-4000-8000-000000000000"),
        parse("99999999-0000-4000-8000-000000000000"),
    ];
    let shown = todo_display_ids(&ids);
    let got: Vec<&str> = ids.iter().map(|id| shown[id].as_str()).collect();
    assert_eq!(got, ["TODO-abcdef12-0", "TODO-abcdef12-1", "TODO-99999999"]);
}

// --- settings-driven rules --------------------------------------------------------------------------------

fn row(name: &str, alias: Option<&str>, patterns: &[&str]) -> RepositoryRule {
    RepositoryRule {
        repo: repo("acme", name),
        alias: alias.map(Into::into),
        watch_issues: false,
        ownership_policy: Some(PolicyRef {
            repo: repo("acme", "policies"),
            path: "p.json".into(),
        }),
        branch_patterns: patterns.iter().map(|p| (*p).to_owned()).collect(),
        ignored_checks: Vec::new(),
    }
}

#[test]
fn repository_alias_falls_back_to_bare_name() {
    let settings = DashboardSettings {
        repositories: vec![
            row("platform", Some("PLAT"), &[]),
            row("node-client", None, &[]),
        ],
        ..DashboardSettings::default()
    };
    let r = ScoreRules::new("alice", &settings);
    assert_eq!(r.display_id(&repo("ACME", "Platform"), 1187), "PLAT-1187");
    assert_eq!(
        r.display_id(&repo("acme", "node-client"), 482),
        "node-client#482"
    );
    assert_eq!(
        r.display_id(&repo("other", "rust-dashcore"), 1234),
        "rust-dashcore#1234"
    );
}

#[test]
fn release_branch_patterns_fall_back_to_builtins() {
    let settings = DashboardSettings {
        repositories: vec![
            row("platform", None, &["stable-*", "X.Y"]),
            row("wallet-ui", None, &[]),
        ],
        ..DashboardSettings::default()
    };
    let r = ScoreRules::new("alice", &settings);
    let builtin = repo("acme", "wallet-ui");
    let unlisted = repo("other", "tool");
    for branch in [
        "main",
        "master",
        "release/2.0",
        "hotfix/x",
        "v1.8",
        "v1.8.2",
        "v1.0-dev",
        "v10.20-dev",
    ] {
        assert!(r.is_long_lived(&builtin, branch, None), "{branch}");
        assert!(r.is_long_lived(&unlisted, branch, None), "{branch}");
    }
    for branch in [
        "feat/x",
        "v1",
        "v1.",
        "1.8",
        "va.b",
        "v1.8-devx",
        "release",
        "mainline",
    ] {
        assert!(!r.is_long_lived(&builtin, branch, None), "{branch}");
    }
    assert!(
        r.is_long_lived(&builtin, "trunk", Some("trunk")),
        "the default branch is long-lived"
    );
    let custom = repo("acme", "platform");
    assert!(r.is_long_lived(&custom, "stable-3", None));
    assert!(r.is_long_lived(&custom, "1.8", None));
    assert!(
        !r.is_long_lived(&custom, "main", None),
        "row patterns replace the built-ins"
    );
    assert!(r.targets_release_branch(&builtin, "v1.8", Some("main")));
    assert!(!r.targets_release_branch(&builtin, "main", Some("main")));
    assert!(!r.targets_release_branch(&builtin, "feat/x", Some("main")));
    assert_eq!(
        BUILTIN_BRANCH_PATTERNS,
        [
            "main",
            "master",
            "release/*",
            "hotfix/*",
            "vX.Y",
            "vX.Y-dev"
        ]
    );
}

/// K15 by GitHub's signals alone: the read path's rule without a plug-in statement.
fn waits_on_others(r: &ScoreRules, pr: &PrFacts) -> bool {
    crate::dashboard::snapshot::waiting(r, pr, &crate::dashboard::plugin::Overlay::default())
        .is_some()
}

/// The PRs [`waits_on_others`], longest wait first (K15 W5); a PR without a known request time waits since it was
/// `opened`.
fn awaiting_others<'a>(r: &ScoreRules, prs: &'a [PrFacts], opened: Timestamp) -> Vec<&'a PrFacts> {
    let mut rows: Vec<(&PrFacts, Timestamp)> = prs
        .iter()
        .filter(|pr| waits_on_others(r, pr))
        .map(|pr| (pr, pr.others_review_requested_at.unwrap_or(opened)))
        .collect();
    by_longest_wait(r, &mut rows);
    rows.into_iter().map(|(pr, _)| pr).collect()
}

#[test]
fn awaiting_others_lists_waiting_prs_longest_wait_first() {
    let waiting = |number, author: &str, wait: Option<f64>| PrFacts {
        number,
        author: Some(author.into()),
        labels: vec!["ready-for-human".into()],
        review_requested_from_others: vec!["bob".into()],
        others_review_requested_at: wait.map(ago),
        ..pr()
    };
    let prs = vec![
        waiting(1, "alice", Some(1.2)),
        PrFacts {
            labels: vec![],
            review_requested_from_others: vec!["team:core".into(), "carol".into()],
            ..waiting(2, "alice-ci", Some(6.0))
        },
        PrFacts {
            mergeable: Mergeable::Conflicting,
            ..waiting(12, "alice", Some(9.0))
        },
        waiting(3, "alice", None),
        PrFacts {
            is_draft: true,
            ..waiting(4, "alice", Some(9.0))
        },
        waiting(5, "bob", Some(9.0)),
        PrFacts {
            labels: vec![],
            review_requested_from_others: vec![],
            ..waiting(6, "alice", Some(9.0))
        },
        PrFacts {
            ci: CiEffective::Red,
            ..waiting(7, "alice", Some(9.0))
        },
        PrFacts {
            ci: CiEffective::None,
            ..waiting(8, "alice", Some(9.0))
        },
        PrFacts {
            unresolved_threads: 1,
            ..waiting(9, "alice", Some(9.0))
        },
        PrFacts {
            labels: vec![],
            review_decision: Some(ReviewDecision::Approved),
            ..waiting(10, "alice", Some(9.0))
        },
        PrFacts {
            review_requested_from_others: vec![],
            ..waiting(11, "alice-ci", Some(7.0))
        },
    ];
    let numbers = |opened| -> Vec<u32> {
        awaiting_others(&rules(), &prs, opened)
            .iter()
            .map(|p| p.number)
            .collect()
    };
    assert_eq!(numbers(ago(1.0)), [11, 2, 1, 3]);
    assert_eq!(
        numbers(ago(30.0)),
        [3, 11, 2, 1],
        "no known request: waits since its opening"
    );
}

/// K15: the user's PR waits on others (Waiting on others, not Needs you) when nothing is the user's move (W2: no
/// conflicts, CI green with ignored checks left out, no change request (people; a bot's until it reviews a newer
/// head), no unresolved thread, no comment awaiting the reply, no review requested from the user; W1: the user's, not
/// a draft) and it waits on people (W3): the ready label even when approved or mergeable, or a review requested from
/// others while not approved and GitHub does not let the user merge it, also without the label or any approval.
#[test]
fn waits_on_others_only_when_nothing_is_the_users_move() {
    use crate::dashboard::score::ReviewDecision as D;
    let r = rules();
    let requested = PrFacts {
        review_decision: Some(D::ReviewRequired),
        review_requested_from_others: vec!["ana".into(), "team:core".into()],
        others_review_requested_at: Some(ago(1.0)),
        merge_state: MergeState::Blocked,
        viewer_can_write: true,
        ..pr()
    };
    let labelled = PrFacts {
        labels: vec!["Ready-For-Human".into()],
        ..requested.clone()
    };
    let cases = [
        (
            "a review requested, no label, no approval",
            requested.clone(),
            true,
        ),
        (
            "only a team is requested",
            PrFacts {
                review_requested_from_others: vec!["team:core".into()],
                ..requested.clone()
            },
            true,
        ),
        (
            "no review decision (no required review), merge blocked",
            PrFacts {
                review_decision: None,
                ..requested.clone()
            },
            true,
        ),
        (
            "approved: merging is the user's move",
            PrFacts {
                review_decision: Some(D::Approved),
                ..requested.clone()
            },
            false,
        ),
        (
            "GitHub lets the user merge it",
            PrFacts {
                merge_state: MergeState::Unstable,
                review_decision: None,
                ..requested.clone()
            },
            false,
        ),
        (
            "nobody requested, no label",
            PrFacts {
                review_requested_from_others: vec![],
                ..requested.clone()
            },
            false,
        ),
        ("ready label", labelled.clone(), true),
        (
            "ready label, approved and mergeable",
            PrFacts {
                review_decision: Some(D::Approved),
                merge_state: MergeState::Clean,
                ..labelled.clone()
            },
            true,
        ),
        (
            "ready label, nobody requested",
            PrFacts {
                review_requested_from_others: vec![],
                ..labelled.clone()
            },
            true,
        ),
        (
            "a review is pending for us",
            PrFacts {
                review_pending_for_us: true,
                ..labelled.clone()
            },
            false,
        ),
        (
            "changes requested",
            PrFacts {
                review_decision: Some(D::ChangesRequested),
                ..labelled.clone()
            },
            false,
        ),
        (
            "a bot requests changes",
            PrFacts {
                changes_requested_by: vec!["coderabbitai".into()],
                ..labelled.clone()
            },
            false,
        ),
        (
            "an unresolved thread",
            PrFacts {
                unresolved_threads: 1,
                ..labelled.clone()
            },
            false,
        ),
        (
            "CI running",
            PrFacts {
                ci: CiEffective::Pending,
                ..labelled.clone()
            },
            false,
        ),
        (
            "a comment awaits our reply",
            PrFacts {
                awaiting_our_reply: true,
                ..labelled.clone()
            },
            false,
        ),
        (
            "conflicts",
            PrFacts {
                mergeable: Mergeable::Conflicting,
                ..labelled.clone()
            },
            false,
        ),
        (
            "draft",
            PrFacts {
                is_draft: true,
                ..labelled.clone()
            },
            false,
        ),
        (
            "someone else's",
            PrFacts {
                author: Some("bob".into()),
                ..labelled.clone()
            },
            false,
        ),
    ];
    for (case, facts, waits) in cases {
        assert_eq!(waits_on_others(&r, &facts), waits, "{case}");
    }
}

/// The owner's example, dashpay/platform#5949: the user's PR into the release branch v5.2-dev (default v5.1-dev),
/// approved by a human and a bot, CI green except PR Hygiene pending (ignored), merge state unstable, the ready
/// label and five requested reviewers: it waits on others (the label), it is not ready to merge.
#[test]
fn a_labelled_pr_into_a_release_branch_waits_on_others_not_ready_to_merge() {
    let r = rules();
    let ci = classify_ci(
        Some(&CheckRollup {
            contexts: vec![
                ok("Rust tests"),
                ok("JS tests"),
                status("PR Hygiene", "PENDING"),
            ],
            truncated: false,
        }),
        |name| r.is_ignored_check(&repo("dashpay", "platform"), name),
    );
    let reviewers = [
        "QuantumExplorer",
        "shumkov",
        "pauldelucia",
        "ktechmidas",
        "team:platform",
    ];
    let platform = PrFacts {
        repo: repo("dashpay", "platform"),
        number: 5949,
        author: Some("alice".into()),
        base_ref: Some("v5.2-dev".into()),
        default_branch: Some("v5.1-dev".into()),
        review_decision: Some(ReviewDecision::Approved),
        approved_by: vec!["llbartekll".into(), "thepastaclaw".into()],
        approved_by_bots: vec!["thepastaclaw".into()],
        ci: ci.effective,
        merge_state: MergeState::Unstable,
        viewer_can_write: true,
        labels: vec!["ready-for-human".into()],
        review_requested_from_others: reviewers.map(Into::into).to_vec(),
        others_review_requested_at: Some(ago(4.0)),
        ..pr()
    };
    assert_eq!(platform.ci, CiEffective::Green, "PR Hygiene is ignored");
    assert!(!ready_to_merge(&r, &platform), "the label needs a human");
    assert!(waits_on_others(&r, &platform));
    assert_eq!(ready_label(&r, &platform), Some("ready-for-human"));
    assert!(
        ready_to_merge(
            &r,
            &PrFacts {
                labels: vec![],
                ..platform.clone()
            }
        ),
        "without the label: ready to merge (release branch, unstable only through an ignored check)"
    );
}

/// A linked login listed as an excluded author: its pull requests are not shown as waiting on others either.
#[test]
fn awaiting_others_leaves_out_excluded_authors() {
    let settings = DashboardSettings {
        linked_logins: vec!["alice-ci".into()],
        skip_authors: vec!["alice-ci".into()],
        ..DashboardSettings::default()
    };
    let ready = |number, author: &str| PrFacts {
        number,
        author: Some(author.into()),
        labels: vec!["ready-for-human".into()],
        review_requested_from_others: vec!["bob".into()],
        others_review_requested_at: Some(ago(1.0)),
        ..pr()
    };
    let prs = vec![ready(1, "alice"), ready(2, "alice-ci")];
    let numbers: Vec<u32> = awaiting_others(&ScoreRules::new("alice", &settings), &prs, ago(2.0))
        .iter()
        .map(|p| p.number)
        .collect();
    assert_eq!(numbers, [1]);
}

/// Ready to merge (K17): the user's own PR into the default branch or a release branch, not a draft, write access,
/// no conflicts, the merge state counts as clean (clean, has hooks, or unstable while only ignored checks did not
/// pass), CI not red, nothing left to answer and no ready label; a stored snapshot without these facts lists none.
#[test]
fn ready_to_merge_when_github_lets_the_user_merge() {
    let r = rules();
    let ready = PrFacts {
        merge_state: MergeState::Clean,
        viewer_can_write: true,
        base_ref: Some("main".into()),
        ..pr()
    };
    let cases = [
        ("clean", ready.clone(), true),
        (
            "into a release branch (built-in vX.Y-dev)",
            PrFacts {
                base_ref: Some("v1.0-dev".into()),
                ..ready.clone()
            },
            true,
        ),
        (
            "into a release branch, default branch not known",
            PrFacts {
                base_ref: Some("release/2.1".into()),
                default_branch: None,
                ..ready.clone()
            },
            true,
        ),
        (
            "into a feature branch",
            PrFacts {
                base_ref: Some("feat/x".into()),
                ..ready.clone()
            },
            false,
        ),
        (
            "base branch not known",
            PrFacts {
                base_ref: None,
                ..ready.clone()
            },
            false,
        ),
        (
            "pre-receive hooks",
            PrFacts {
                merge_state: MergeState::HasHooks,
                ..ready.clone()
            },
            true,
        ),
        (
            "unstable only through ignored, skipped or neutral checks",
            PrFacts {
                merge_state: MergeState::Unstable,
                ..ready.clone()
            },
            true,
        ),
        (
            "unstable: a check that is not required fails",
            PrFacts {
                merge_state: MergeState::Unstable,
                ci: CiEffective::Red,
                ..ready.clone()
            },
            false,
        ),
        (
            "unstable: a check that is not required is running",
            PrFacts {
                merge_state: MergeState::Unstable,
                ci: CiEffective::Pending,
                ..ready.clone()
            },
            false,
        ),
        (
            "red CI",
            PrFacts {
                ci: CiEffective::Red,
                ..ready.clone()
            },
            false,
        ),
        (
            "conflicts",
            PrFacts {
                mergeable: Mergeable::Conflicting,
                ..ready.clone()
            },
            false,
        ),
        (
            "an unresolved review thread",
            PrFacts {
                unresolved_threads: 1,
                ..ready.clone()
            },
            false,
        ),
        (
            "a bot requests changes",
            PrFacts {
                changes_requested_by: vec!["thepastaclaw".into()],
                ..ready.clone()
            },
            false,
        ),
        (
            "a comment awaits the user's reply",
            PrFacts {
                awaiting_our_reply: true,
                ..ready.clone()
            },
            false,
        ),
        (
            "more reviews than were fetched (an older change request may be hidden)",
            PrFacts {
                reviews_truncated: true,
                ..ready.clone()
            },
            false,
        ),
        (
            "a linked login's PR",
            PrFacts {
                author: Some("alice-ci".into()),
                ..ready.clone()
            },
            true,
        ),
        (
            "a review requested from others (approval is GitHub's merge state)",
            PrFacts {
                review_requested_from_others: vec!["bob".into()],
                ..ready.clone()
            },
            true,
        ),
        (
            "the ready label: it needs a human",
            PrFacts {
                labels: vec!["ready-for-human".into()],
                ..ready.clone()
            },
            false,
        ),
        (
            "blocked",
            PrFacts {
                merge_state: MergeState::Blocked,
                ..ready.clone()
            },
            false,
        ),
        (
            "behind",
            PrFacts {
                merge_state: MergeState::Behind,
                ..ready.clone()
            },
            false,
        ),
        (
            "dirty",
            PrFacts {
                merge_state: MergeState::Dirty,
                ..ready.clone()
            },
            false,
        ),
        (
            "not determined",
            PrFacts {
                merge_state: MergeState::Unknown,
                ..ready.clone()
            },
            false,
        ),
        (
            "draft",
            PrFacts {
                is_draft: true,
                ..ready.clone()
            },
            false,
        ),
        (
            "no write access",
            PrFacts {
                viewer_can_write: false,
                ..ready.clone()
            },
            false,
        ),
        (
            "someone else's PR",
            PrFacts {
                author: Some("bob".into()),
                ..ready.clone()
            },
            false,
        ),
        (
            "a deleted author",
            PrFacts {
                author: None,
                ..ready.clone()
            },
            false,
        ),
    ];
    for (case, facts, expected) in cases {
        assert_eq!(ready_to_merge(&r, &facts), expected, "{case}");
    }
    let mut stored = serde_json::to_value(&ready).expect("json");
    let fields = stored.as_object_mut().expect("object");
    fields.remove("merge_state");
    fields.remove("viewer_can_write");
    let old: PrFacts = serde_json::from_value(stored).expect("an old stored pull request loads");
    assert!(!ready_to_merge(&r, &old));
}

/// K17 R2: a repository row's Release branches decide which bases Ready to merge accepts besides the default branch.
#[test]
fn ready_to_merge_follows_the_rows_release_branches() {
    let settings = DashboardSettings {
        repositories: vec![RepositoryRule {
            repo: repo("acme", "wallet-ui"),
            alias: None,
            watch_issues: false,
            ownership_policy: None,
            branch_patterns: vec!["stable/*".into()],
            ignored_checks: vec![],
        }],
        ..DashboardSettings::default()
    };
    let r = ScoreRules::new("alice", &settings);
    let into = |base: &str| PrFacts {
        merge_state: MergeState::Clean,
        viewer_can_write: true,
        base_ref: Some(base.into()),
        ..pr()
    };
    assert!(ready_to_merge(&r, &into("main")), "the default branch");
    assert!(ready_to_merge(&r, &into("stable/3")));
    assert!(
        !ready_to_merge(&r, &into("v1.0-dev")),
        "the row replaces the built-ins"
    );
}

/// A merge state GitHub had not computed is pending only for a PR that would otherwise be checked for Ready to
/// merge.
#[test]
fn merge_state_pending_only_for_the_users_mergeable_prs() {
    let r = rules();
    let unknown = PrFacts {
        merge_state: MergeState::Unknown,
        viewer_can_write: true,
        base_ref: Some("main".into()),
        ..pr()
    };
    assert!(merge_state_pending(&r, &unknown));
    assert!(merge_state_pending_for_ready(&r, &unknown));
    assert!(!ready_to_merge(&r, &unknown));
    for (case, facts) in [
        (
            "computed",
            PrFacts {
                merge_state: MergeState::Blocked,
                ..unknown.clone()
            },
        ),
        (
            "draft",
            PrFacts {
                is_draft: true,
                ..unknown.clone()
            },
        ),
        (
            "no write access",
            PrFacts {
                viewer_can_write: false,
                ..unknown.clone()
            },
        ),
        (
            "someone else's",
            PrFacts {
                author: Some("bob".into()),
                ..unknown.clone()
            },
        ),
    ] {
        assert!(!merge_state_pending(&r, &facts), "{case}");
    }
    for (case, facts) in [
        (
            "into a feature branch",
            PrFacts {
                base_ref: Some("feat/x".into()),
                ..unknown.clone()
            },
        ),
        (
            "the ready label",
            PrFacts {
                labels: vec!["ready-for-human".into()],
                ..unknown.clone()
            },
        ),
    ] {
        assert!(merge_state_pending(&r, &facts), "{case}: read again");
        assert!(
            !merge_state_pending_for_ready(&r, &facts),
            "{case}: not counted"
        );
    }
}

/// Ready to merge lists by display id, leaves out excluded authors and keeps the PRs in the ranking.
#[test]
fn merge_ready_by_display_id_without_excluded_authors() {
    let settings = DashboardSettings {
        linked_logins: vec!["alice-ci".into()],
        skip_authors: vec!["alice-ci".into()],
        ..DashboardSettings::default()
    };
    let r = ScoreRules::new("alice", &settings);
    let ready = |name: &str, number, author: &str| PrFacts {
        repo: repo("acme", name),
        number,
        author: Some(author.into()),
        merge_state: MergeState::Clean,
        viewer_can_write: true,
        base_ref: Some("main".into()),
        ..pr()
    };
    let prs = vec![
        ready("wallet-ui", 12, "alice"),
        ready("wallet-ui", 9, "alice"),
        ready("consensus", 40, "alice"),
        ready("consensus", 41, "alice-ci"),
    ];
    let ids: Vec<String> = merge_ready(&r, &prs)
        .iter()
        .map(|p| r.display_id(&p.repo, p.number))
        .collect();
    assert_eq!(ids, ["consensus#40", "wallet-ui#9", "wallet-ui#12"]);
    assert!(scored(&prs[0]).excluded.is_none(), "still ranked");
}

/// K15 W4: Ready to merge and Waiting on others are disjoint; a PR ready to merge stays ranked, also when a review
/// is still requested from someone else.
#[test]
fn a_pr_ready_to_merge_does_not_wait_on_others() {
    let r = rules();
    let facts = PrFacts {
        review_requested_from_others: vec!["bob".into()],
        others_review_requested_at: Some(ago(2.0)),
        review_decision: None,
        merge_state: MergeState::Clean,
        viewer_can_write: true,
        base_ref: Some("main".into()),
        ..pr()
    };
    assert!(
        ready_to_merge(&r, &facts),
        "precondition: GitHub lets the user merge it"
    );
    assert!(!nothing_is_users_move(&r, &facts));
    assert!(!waits_on_others(&r, &facts));
}

/// K15 W3c keeps GitHub's merge button as "a merge the user can make": a PR into a feature branch, or with a failed
/// check that is not required, is the user's move although Ready to merge does not list it.
#[test]
fn a_pr_github_lets_the_user_merge_does_not_wait_on_a_requested_review() {
    let r = rules();
    let mergeable = PrFacts {
        review_requested_from_others: vec!["bob".into()],
        others_review_requested_at: Some(ago(2.0)),
        review_decision: None,
        merge_state: MergeState::Clean,
        viewer_can_write: true,
        base_ref: Some("main".into()),
        ..pr()
    };
    for (case, facts) in [
        (
            "into a feature branch",
            PrFacts {
                base_ref: Some("feat/x".into()),
                ..mergeable.clone()
            },
        ),
        (
            "a check that is not required failed (CI green: the check is ignored)",
            PrFacts {
                merge_state: MergeState::Unstable,
                base_ref: Some("feat/x".into()),
                ..mergeable.clone()
            },
        ),
    ] {
        assert!(!ready_to_merge(&r, &facts), "{case}: not ready to merge");
        assert!(!waits_on_others(&r, &facts), "{case}: the user's move");
    }
}

// --- text rules -------------------------------------------------------------------------------------------

#[test]
fn text_rules_match_the_skill_patterns() {
    assert_eq!(
        text::conventional_type("fix(sync)!: x"),
        Some(("fix".into(), true))
    );
    assert_eq!(
        text::conventional_type("  Docs : z"),
        Some(("docs".into(), false))
    );
    assert_eq!(text::conventional_type("fix(unclosed: x"), None);
    assert_eq!(text::conventional_type("no type here"), None);
    assert!(text::has_fix_wording("Fixes the crash") && text::has_fix_wording("deadlock"));
    assert!(!text::has_fix_wording("hotfix branch") && !text::has_fix_wording("prefix"));
    for (body, expected) in [
        ("blocked by #1", true),
        ("Blocked\n on infra", true),
        ("this depends on x", true),
        ("blocked-by x", false),
        ("unblocked by x", false),
        ("blocked byte", false),
        ("blocked", false),
    ] {
        assert_eq!(text::has_blocker_wording(body), expected, "{body}");
    }
    assert!(text::has_word("Blocked", "blocked") && text::has_word("not-blocked", "blocked"));
    assert!(!text::has_word("blocked_by", "blocked") && !text::has_word("unblocked", "blocked"));
    for (name, policy) in [
        ("PR Hygiene", true),
        ("policy / reconcile", true),
        ("policyx", false),
        ("PR title", true),
        ("semantic-pull-request", true),
        ("Milestone check", true),
        ("CodeRabbit", true),
        ("review", true),
        ("review-bot", false),
        ("PastaClaw review", true),
        ("pastaclaw reviews", false),
        ("Clippy", false),
    ] {
        assert_eq!(text::is_builtin_ignored_check(name), policy, "{name}");
    }
    assert_eq!(text::version(" v1.8.2+build.5 ", false), Some((1, 8, 2)));
    assert_eq!(text::version("1.8.x", false), None);
    assert_eq!(text::version("1.8.x", true), Some((1, 8, 0)));
    assert_eq!(text::version("99999999999999999999.1", true), None);
}

#[test]
fn scoring_has_no_io_imports() {
    for (file, source) in [
        ("mod.rs", include_str!("mod.rs")),
        ("ci.rs", include_str!("ci.rs")),
        ("facts.rs", include_str!("facts.rs")),
        ("parts.rs", include_str!("parts.rs")),
        ("rules.rs", include_str!("rules.rs")),
        ("text.rs", include_str!("text.rs")),
    ] {
        for banned in [
            "crate::db",
            "db::",
            "crate::github",
            "crate::memcan",
            "tokio",
            "Timestamp::now",
            "Zoned::now",
            "SystemTime",
        ] {
            assert!(!source.contains(banned), "{file} uses {banned}");
        }
    }
}

#[test]
fn fingerprint_signals_change_only_when_a_bucket_does() {
    let signals = |pr: &PrFacts| serde_json::to_string(&scored(pr).signals).expect("serialises");
    let asked = |days| review_request(days, false, "bob", false);
    assert_eq!(
        signals(&asked(1.0)),
        signals(&asked(1.5)),
        "same waiting bucket"
    );
    assert_ne!(signals(&asked(1.0)), signals(&asked(3.0)), "≤2 d to ≤7 d");
    assert_eq!(signals(&asked(3.0)), signals(&asked(6.0)));
    assert_ne!(signals(&asked(6.0)), signals(&asked(8.0)), "≤7 d to older");
    let reply = |days| PrFacts {
        awaiting_our_reply: true,
        last_comment_author: Some("bob".into()),
        last_comment_at: Some(ago(days)),
        ..asked(10.0)
    };
    assert_ne!(
        signals(&asked(10.0)),
        signals(&reply(1.0)),
        "an unanswered comment opens the part"
    );
    assert_eq!(signals(&reply(1.0)), signals(&reply(1.5)));
    let milestone = |tags: Option<&[&str]>| PrFacts {
        milestone: Milestone::Assigned("v2.0".into()),
        milestone_context: context(tags, Some(&["v1.8", "v2.0", "v3.0"])),
        ..pr()
    };
    assert_ne!(
        signals(&milestone(None)),
        signals(&milestone(Some(&["v1.8.2"]))),
        "unknown to computed"
    );
    assert_ne!(
        signals(&milestone(Some(&["v1.8.2"]))),
        signals(&milestone(Some(&["v2.0.0"]))),
        "next to past"
    );
    assert_eq!(
        signals(&milestone(Some(&["v1.8.2"]))),
        signals(&milestone(Some(&["v1.8.3", "v1.8.0"])))
    );
    assert_ne!(
        signals(&pr()),
        signals(&PrFacts {
            body: "blocked on infra".into(),
            ..pr()
        })
    );
    assert_ne!(
        signals(&pr()),
        signals(&PrFacts {
            labels: vec!["Blocked".into()],
            ..pr()
        })
    );
    assert_eq!(
        signals(&pr()),
        signals(&PrFacts {
            updated_at: ago(3.0),
            additions: Some(900),
            ..pr()
        })
    );
}
