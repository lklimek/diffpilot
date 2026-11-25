//! The PR state badge (plan §3.9 B1–B5): first match wins; facts diffpilot read select it; with an input unknown
//! (CI not read, GitHub read failed, a plug-in that can hold the PR not current, My work has not scanned the head and
//! base) there is no "Ready to merge". CI and Ready to merge follow My work's rules (DR1, K17): the reader's Ignored
//! checks never fail CI or keep it running, and "Ready to merge" needs My work's Ready to merge at the same head and
//! base, while a draft or an unresolved thread in the last read vetoes it.

use contracts::api::dashboard::DashboardSettings;
use contracts::api::review_loop::{PrBadge, PrBadgeView};
use contracts::api::tour::{CiCounts, CiState, CiStatus};
use contracts::common::RepoRef;
use db::error::DbError;
use serde_json::Value;

use crate::dashboard::score::{self, ScoreRules};
use crate::dashboard::settings;
use crate::dashboard::snapshot::{self, StoredPr};
use crate::state::AppState;

/// What the reader's My work decides for the badge (DR1): its rules (Ignored checks per repository) and Ready to
/// merge (K17) of the pull requests its last scan read.
#[derive(Clone, Debug)]
pub struct MyWork {
    rules: ScoreRules,
    prs: Vec<ScannedPr>,
}

#[derive(Clone, Debug)]
struct ScannedPr {
    repo: RepoRef,
    number: u32,
    head: String,
    base: Option<String>,
    base_sha: Option<String>,
    ready: bool,
}

impl MyWork {
    /// The rules of `own_login` with `settings`, and Ready to merge of the scanned `prs`.
    pub fn new(own_login: &str, settings: &DashboardSettings, prs: &[StoredPr]) -> Self {
        let rules = ScoreRules::new(own_login, settings);
        let prs = prs
            .iter()
            .map(|pr| ScannedPr {
                repo: pr.facts.repo.clone(),
                number: pr.facts.number,
                head: pr.head_sha.clone(),
                base: pr.facts.base_ref.clone(),
                base_sha: pr.base_sha.clone(),
                ready: score::ready_to_merge(&rules, &pr.facts),
            })
            .collect();
        Self { rules, prs }
    }

    /// The check never fails CI in `repo` (K17 R4).
    pub fn is_ignored_check(&self, repo: &RepoRef, name: &str) -> bool {
        self.rules.is_ignored_check(repo, name)
    }

    /// Ready to merge (K17, before a plug-in's hold) of `repo#number` at `head` into `base` at commit `base_sha`;
    /// `None` when the last scan did not read it at that head, base and base commit.
    pub fn ready_to_merge(
        &self,
        repo: &RepoRef,
        number: u32,
        head: &str,
        base: &str,
        base_sha: &str,
    ) -> Option<bool> {
        self.prs
            .iter()
            .find(|pr| {
                pr.number == number
                    && pr.repo.owner.eq_ignore_ascii_case(&repo.owner)
                    && pr.repo.name.eq_ignore_ascii_case(&repo.name)
                    && pr.head.eq_ignore_ascii_case(head)
                    && pr.base.as_deref() == Some(base)
                    && pr
                        .base_sha
                        .as_deref()
                        .is_some_and(|scanned| scanned.eq_ignore_ascii_case(base_sha))
            })
            .map(|pr| pr.ready)
    }
}

/// The reader's [`MyWork`]: its last scan with the rules it used, else the current settings and nothing scanned.
pub(crate) async fn my_work(
    state: &AppState,
    tenant_id: &str,
    user_id: &str,
) -> Result<MyWork, DbError> {
    let scanned = state
        .db
        .dashboard()
        .latest_snapshot::<Value>(tenant_id, user_id)
        .await?
        .and_then(|(_, document)| snapshot::readable(document));
    Ok(match scanned {
        Some(scan) => MyWork::new(&scan.own_login, &scan.settings, &scan.pull_requests),
        None => MyWork::new(
            "",
            &settings::effective(&state.db, tenant_id, user_id).await?,
            &[],
        ),
    })
}

/// The head's CI state with the failing and `pending` checks `ignored` names left out (K17 R4), and the names left
/// out. Failing or pending checks are kept when their names are not all known.
pub fn without_ignored(
    ci: &CiStatus,
    pending: &[String],
    ignored: impl Fn(&str) -> bool,
) -> (CiState, Vec<String>) {
    let sources = [ci.check_runs, ci.statuses];
    let total = |count: fn(&CiCounts) -> u32| -> u32 { sources.iter().flatten().map(count).sum() };
    let named = |names: usize, count: u32| usize::try_from(count).is_ok_and(|count| count == names);
    let failing_names: Vec<&str> = ci.failing.iter().map(|check| check.name.as_str()).collect();
    let pending_names: Vec<&str> = pending.iter().map(String::as_str).collect();
    let mut left_out = Vec::new();
    // Whether a check of this kind still counts; when not all are named, every one does.
    let mut counts = |names: &[&str], count: u32| -> bool {
        if !named(names.len(), count) {
            return count > 0;
        }
        let mut counted = false;
        for name in names {
            if ignored(name) {
                left_out.push((*name).to_owned());
            } else {
                counted = true;
            }
        }
        counted
    };
    let still_failing = counts(&failing_names, total(|c| c.failed));
    let still_running = counts(&pending_names, total(|c| c.pending));
    let state = if still_failing {
        CiState::Failing
    } else if still_running {
        CiState::Pending
    } else if !left_out.is_empty() {
        CiState::NoFailures
    } else {
        ci.state
    };
    (state, left_out)
}

/// What the badge is chosen from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BadgeInputs {
    /// The PR is open (merged or closed PRs get no badge).
    pub open: bool,
    /// The last GitHub read failed, or there was none.
    pub read_failed: bool,
    /// CI of the head with the reader's Ignored checks left out ([`without_ignored`]); `None` when not read.
    pub ci: Option<CiState>,
    /// Failing or pending checks left out as Ignored checks.
    pub ignored_not_passing: Vec<String>,
    /// My work's Ready to merge at this head, base and base commit ([`MyWork::ready_to_merge`]); `None` when its last
    /// scan did not read them.
    pub ready_to_merge: Option<bool>,
    /// The last GitHub read shows a draft (K17 R1, whatever My work's scan saw).
    pub draft: bool,
    /// Unresolved review threads in the last GitHub read (K17, whatever My work's scan saw).
    pub unresolved_threads: u32,
    /// Failed effects.
    pub failed_effects: u32,
    /// Undecided questions.
    pub needs_you: u32,
    /// Open non-bot review points without a stored assessment.
    pub pending_human_review: u32,
    /// Open non-bot review points with a stored assessment that a running pass includes.
    pub reassessing_human_review: u32,
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
    if inputs.ready_to_merge.is_none() {
        view.missing
            .push("My work's last scan did not read this head and base".to_owned());
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
    if inputs.needs_you > 0 {
        view.facts.push(plural(
            inputs.needs_you,
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
    if inputs.pending_human_review > 0 {
        view.facts.push(plural(
            inputs.pending_human_review,
            "review item not assessed yet",
            "review items not assessed yet",
        ));
        ready = false;
    }
    if inputs.reassessing_human_review > 0 {
        view.facts.push(plural(
            inputs.reassessing_human_review,
            "review item being assessed again",
            "review items being assessed again",
        ));
        ready = false;
    }
    match inputs.ci {
        Some(CiState::Passing | CiState::NoFailures) => view.facts.push("CI passed".to_owned()),
        Some(CiState::None) => view.facts.push("No CI checks reported".to_owned()),
        Some(CiState::Pending) => {
            view.facts.push("CI is running".to_owned());
            ready = false;
        }
        Some(CiState::Failing | CiState::NotChecked) | None => ready = false,
    }
    if !inputs.ignored_not_passing.is_empty() {
        view.facts.push(format!(
            "Ignored checks not passing: {}",
            inputs.ignored_not_passing.join(", ")
        ));
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
    if inputs.draft {
        view.facts.push("Draft".to_owned());
        ready = false;
    }
    if inputs.unresolved_threads > 0 {
        view.facts.push(plural(
            inputs.unresolved_threads,
            "unresolved review thread",
            "unresolved review threads",
        ));
        ready = false;
    }
    if inputs.ready_to_merge == Some(false) {
        view.facts
            .push("My work does not list it as Ready to merge".to_owned());
        ready = false;
    }
    if ready {
        view.badge = Some(PrBadge::ReadyToMerge);
    }
    view
}

#[cfg(test)]
mod tests {
    use contracts::api::dashboard::RepositoryRule;
    use contracts::api::tour::{CiCheck, CiCounts};
    use contracts::time::Timestamp;

    use super::*;
    use crate::dashboard::score::{
        CiEffective, MergeState, Mergeable, Milestone, MilestoneContext, OwnedChange, PrFacts,
        Relation,
    };

    use contracts::api::pr::PrSummary;
    use contracts::api::review_loop::{
        AuthorKind, CommentItemKind, ItemGithubFacts, ItemState, WatchState,
    };
    use contracts::common::PrRef;
    use contracts::ids::CommitSha;
    use db::repo::DEFAULT_TENANT;
    use db::repo::review_loop::{ObservedItem, ReadResult};
    use db::repo::users::GithubProfile;
    use serde_json::json;

    fn platform() -> RepoRef {
        RepoRef {
            owner: "dashpay".into(),
            name: "platform".into(),
        }
    }

    /// The user's PR into a release branch, unstable only through an ignored check: My work lists it as Ready to
    /// merge (K17).
    fn mergeable_pr() -> StoredPr {
        let at: Timestamp = "2026-11-22T09:00:00Z".parse().expect("time");
        StoredPr {
            facts: PrFacts {
                repo: platform(),
                number: 5949,
                title: "feat(sdk): thing".into(),
                body: String::new(),
                author: Some("alice".into()),
                author_is_bot: false,
                relations: vec![Relation::Authored],
                is_draft: false,
                labels: vec![],
                updated_at: at,
                additions: Some(1),
                deletions: Some(1),
                base_ref: Some("v3.1-dev".into()),
                default_branch: Some("master".into()),
                dependents: Some(vec![]),
                stacked_on: Some(vec![]),
                milestone: Milestone::Unassigned,
                milestone_context: MilestoneContext::default(),
                ci: CiEffective::Green,
                mergeable: Mergeable::Mergeable,
                merge_state: MergeState::Unstable,
                viewer_can_write: true,
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
            },
            private: false,
            head_sha: "abc123".into(),
            head_repository: None,
            base_sha: Some("b45e00".into()),
            created_at: at,
            ci_red: vec![],
            ci_pending: vec![],
            ci_ignored: vec!["PR Hygiene (FAILURE)".into()],
            linked_issues: vec![],
        }
    }

    /// Ready to merge is My work's (K17) at the head its last scan read: a draft, a conflict, a base outside the
    /// release line, no write access or the ready label make it not ready; another head or PR is not known.
    #[test]
    fn ready_to_merge_is_my_works_at_the_scanned_head() {
        let settings = DashboardSettings::default();
        let ready = |change: &dyn Fn(&mut PrFacts)| {
            let mut pr = mergeable_pr();
            change(&mut pr.facts);
            let base = pr.facts.base_ref.clone().expect("base");
            MyWork::new("alice", &settings, &[pr]).ready_to_merge(
                &platform(),
                5949,
                "abc123",
                &base,
                "b45e00",
            )
        };
        assert_eq!(ready(&|_| {}), Some(true));
        for (case, change) in [
            (
                "draft",
                &(|f: &mut PrFacts| f.is_draft = true) as &dyn Fn(&mut PrFacts),
            ),
            ("conflicts", &|f| f.mergeable = Mergeable::Conflicting),
            ("stacked base", &|f| f.base_ref = Some("feat/base".into())),
            ("no write access", &|f| f.viewer_can_write = false),
            ("ready label", &|f| {
                f.labels = vec!["ready-for-human".into()]
            }),
            ("CI red", &|f| f.ci = CiEffective::Red),
        ] {
            assert_eq!(ready(change), Some(false), "{case}");
        }
        let work = MyWork::new("alice", &settings, &[mergeable_pr()]);
        assert_eq!(
            work.ready_to_merge(&platform(), 5949, "def456", "v3.1-dev", "b45e00"),
            None,
            "another head"
        );
        assert_eq!(
            work.ready_to_merge(&platform(), 5949, "abc123", "v3.2-dev", "b45e00"),
            None,
            "retargeted at the same head"
        );
        assert_eq!(
            work.ready_to_merge(&platform(), 1, "abc123", "v3.1-dev", "b45e00"),
            None,
            "not scanned"
        );
        let upper = RepoRef {
            owner: "DashPay".into(),
            name: "Platform".into(),
        };
        assert_eq!(
            work.ready_to_merge(&upper, 5949, "ABC123", "v3.1-dev", "B45E00"),
            Some(true)
        );
    }

    /// Mergeability depends on the base commit as well as the head: My work's verdict counts only at the base commit
    /// its last scan read, and not at all from a scan that kept none.
    #[test]
    fn ready_to_merge_counts_only_at_the_scanned_base_commit() {
        let settings = DashboardSettings::default();
        let work = MyWork::new("alice", &settings, &[mergeable_pr()]);
        assert_eq!(
            work.ready_to_merge(&platform(), 5949, "abc123", "v3.1-dev", "b45e00"),
            Some(true)
        );
        assert_eq!(
            work.ready_to_merge(&platform(), 5949, "abc123", "v3.1-dev", "9f8e7d"),
            None,
            "the base branch advanced after the scan"
        );
        let mut unkept = mergeable_pr();
        unkept.base_sha = None;
        assert_eq!(
            MyWork::new("alice", &settings, &[unkept]).ready_to_merge(
                &platform(),
                5949,
                "abc123",
                "v3.1-dev",
                "b45e00"
            ),
            None,
            "a scan stored before the base commit was kept"
        );
    }

    /// CI of the head with `failed` failing and `pending` pending checks, the failing ones named.
    fn ci(failed: &[&str], pending: u32) -> CiStatus {
        CiStatus {
            state: if failed.is_empty() {
                CiState::Pending
            } else {
                CiState::Failing
            },
            total: 5,
            failing: failed
                .iter()
                .map(|name| CiCheck {
                    name: (*name).to_owned(),
                    url: None,
                    failing_tests: Vec::new(),
                })
                .collect(),
            check_runs: Some(CiCounts {
                passed: 3,
                pending,
                ..CiCounts::default()
            }),
            statuses: Some(CiCounts {
                failed: u32::try_from(failed.len()).expect("count"),
                ..CiCounts::default()
            }),
        }
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    /// CI leaves out the reader's Ignored checks of the repository (K17 R4), failing or pending: the built-ins
    /// without a row, the row's patterns with one. A failing or pending check that is not ignored, or checks not all
    /// named, keep CI failed or running.
    #[test]
    fn ci_leaves_out_the_readers_ignored_checks() {
        let builtins = MyWork::new("alice", &DashboardSettings::default(), &[]);
        let ignored = |name: &str| builtins.is_ignored_check(&platform(), name);
        let hygiene = names(&["PR Hygiene"]);
        assert_eq!(
            without_ignored(&ci(&["PR Hygiene"], 0), &[], ignored),
            (CiState::NoFailures, hygiene.clone())
        );
        assert_eq!(
            without_ignored(&ci(&[], 1), &hygiene, ignored),
            (CiState::NoFailures, hygiene.clone()),
            "a pending ignored check: not running"
        );
        assert_eq!(
            without_ignored(&ci(&["PR Hygiene"], 1), &names(&["tests"]), ignored),
            (CiState::Pending, hygiene.clone())
        );
        assert_eq!(
            without_ignored(&ci(&["PR Hygiene", "tests"], 0), &[], ignored),
            (CiState::Failing, hygiene.clone())
        );
        assert_eq!(
            without_ignored(&ci(&[], 1), &[], ignored),
            (CiState::Pending, vec![]),
            "pending checks not named (read before they were kept)"
        );
        let mut unnamed = ci(&["PR Hygiene"], 0);
        unnamed.statuses = Some(CiCounts {
            failed: 2,
            ..CiCounts::default()
        });
        assert_eq!(
            without_ignored(&unnamed, &[], ignored),
            (CiState::Failing, vec![])
        );
        let passing = CiStatus {
            state: CiState::Passing,
            ..ci(&[], 0)
        };
        assert_eq!(
            without_ignored(&passing, &[], ignored),
            (CiState::Passing, vec![])
        );

        let settings = DashboardSettings {
            repositories: vec![RepositoryRule {
                repo: platform(),
                alias: None,
                watch_issues: false,
                ownership_policy: None,
                branch_patterns: vec![],
                ignored_checks: vec!["docs*".into()],
            }],
            ..DashboardSettings::default()
        };
        let row = MyWork::new("alice", &settings, &[]);
        let ignored = |name: &str| row.is_ignored_check(&platform(), name);
        assert_eq!(
            without_ignored(&ci(&["PR Hygiene"], 0), &[], ignored).0,
            CiState::Failing,
            "a row's list replaces the built-ins"
        );
        assert_eq!(
            without_ignored(&ci(&[], 1), &hygiene, ignored).0,
            CiState::Pending
        );
        assert_eq!(
            without_ignored(&ci(&["docs-build"], 1), &names(&["docs-lint"]), ignored).0,
            CiState::NoFailures
        );
    }

    /// Facts the last GitHub read shows veto "Ready to merge" whatever My work's scan at the same head saw (a PR
    /// turned into a draft, a thread opened since): K17 needs neither.
    #[test]
    fn the_last_read_vetoes_my_works_ready_to_merge() {
        let draft = badge(&BadgeInputs {
            draft: true,
            ..ready()
        });
        assert_eq!(draft.badge, None);
        assert_eq!(draft.facts, ["CI passed", "Draft"]);
        let threads = badge(&BadgeInputs {
            unresolved_threads: 2,
            ..ready()
        });
        assert_eq!(threads.badge, None);
        assert_eq!(threads.facts, ["CI passed", "2 unresolved review threads"]);
        assert_eq!(
            badge(&BadgeInputs {
                draft: true,
                needs_you: 1,
                ..ready()
            })
            .badge,
            Some(PrBadge::QuestionsWaiting),
            "B1-B4 still apply"
        );
    }

    /// An Ignored check that fails or runs never selects "Blocked / failing" nor keeps "CI is running"; it is named
    /// among the facts.
    #[test]
    fn an_ignored_check_not_passing_does_not_block() {
        let view = badge(&BadgeInputs {
            ci: Some(CiState::NoFailures),
            ignored_not_passing: vec!["PR Hygiene".into()],
            ..ready()
        });
        assert_eq!(view.badge, Some(PrBadge::ReadyToMerge));
        assert_eq!(
            view.facts,
            ["CI passed", "Ignored checks not passing: PR Hygiene"]
        );
    }

    async fn ready_pr_fixture() -> (AppState, super::super::PrHandle, MyWork) {
        let scanned = mergeable_pr();
        let work = MyWork::new(
            "alice",
            &DashboardSettings::default(),
            std::slice::from_ref(&scanned),
        );
        let state = AppState::with_queue(
            crate::config::tests::test_config(),
            db::repo::Db::in_memory().await.unwrap(),
        );
        let now = Timestamp::now();
        state
            .db
            .users()
            .upsert_member(
                DEFAULT_TENANT,
                "u1",
                &GithubProfile {
                    github_id: 1,
                    login: "alice".into(),
                    name: None,
                    avatar_url: None,
                },
                now,
            )
            .await
            .unwrap();
        let repo = state
            .db
            .repos()
            .upsert(DEFAULT_TENANT, &scanned.facts.repo, None)
            .await
            .unwrap();
        let head = CommitSha(scanned.head_sha.clone());
        let base = scanned.base_sha.unwrap();
        let number = scanned.facts.number;
        let pr = state
            .db
            .pull_requests()
            .upsert(
                DEFAULT_TENANT,
                repo.id,
                &PrSummary {
                    repo: repo.repo.clone(),
                    number,
                    title: "Fixture".into(),
                    author: "alice".into(),
                    html_url: "https://github.com/dashpay/platform/pull/5949".into(),
                    base_sha: CommitSha(base.clone()),
                    head_sha: head.clone(),
                },
                now,
            )
            .await
            .unwrap();
        let rl = state.db.review_loop();
        rl.ensure_pr(DEFAULT_TENANT, pr, now).await.unwrap();
        rl.record_read(pr, &ReadResult::Read {
            snapshot: json!({
                "title": "Fixture", "url": "", "state": "OPEN", "draft": false,
                "base": "v3.1-dev", "head": head.0, "base_sha": base,
                "rounds": [], "bot_change_requests": [], "changes_requested_by": [],
                "ci": {"state": "passing", "total": 0, "check_runs": null, "statuses": null, "failing": []},
                "counts": [0, 0, 1], "complete": true, "unresolved_threads": 0,
                "thread_ids": {}, "review_ids": {}
            }),
            round_count: 0,
        }, now).await.unwrap();
        let handle = super::super::PrHandle {
            id: pr,
            pr: PrRef {
                repo: repo.repo,
                number,
            },
        };
        (state, handle, work)
    }

    fn observed_comment() -> ObservedItem {
        ObservedItem {
            kind: CommentItemKind::PrComment,
            stable_key: "comment:1:1".into(),
            author: "reviewer".into(),
            author_kind: AuthorKind::Human,
            status_comment: false,
            url: None,
            location: None,
            title: "Check the edge case".into(),
            body: "Please check the empty input.".into(),
            body_hash: "body".into(),
            head: CommitSha("abc123".into()),
            round: None,
            bot_severity: None,
            github: ItemGithubFacts {
                resolved: None,
                resolved_by: None,
                outdated: None,
                bot_status: None,
                edited_after_decision: false,
                replies: 0,
                marker: None,
            },
            outside: None,
        }
    }

    fn merge_question(round: u32) -> ObservedItem {
        ObservedItem {
            kind: CommentItemKind::PrQuestion,
            stable_key: format!("pr:{round}:merge"),
            author: "diffpilot".into(),
            author_kind: AuthorKind::Diffpilot,
            round: Some(round),
            ..observed_comment()
        }
    }

    #[tokio::test]
    async fn watched_pr_at_daily_limit_with_unassessed_human_comment_is_not_ready() {
        let (state, handle, work) = ready_pr_fixture().await;
        let now = Timestamp::now();
        let rl = state.db.review_loop();
        rl.watch(DEFAULT_TENANT, "u1", handle.id, 20, now)
            .await
            .unwrap();
        rl.set_watch_state("u1", handle.id, WatchState::LimitReached, now)
            .await
            .unwrap();
        let view = || super::super::views::badge(&state, handle.clone(), &work);
        assert_eq!(view().await.unwrap().badge, Some(PrBadge::ReadyToMerge));
        rl.upsert_item(
            DEFAULT_TENANT,
            handle.id,
            &observed_comment(),
            ItemState::Undecided,
            now,
        )
        .await
        .unwrap();
        assert_eq!(view().await.unwrap().badge, None);
    }

    #[tokio::test]
    async fn old_round_pr_questions_do_not_veto_ready_to_merge() {
        let (state, handle, work) = ready_pr_fixture().await;
        let rl = state.db.review_loop();
        for (round, status) in [(3, ItemState::Undecided), (4, ItemState::Assessing)] {
            rl.upsert_item(
                DEFAULT_TENANT,
                handle.id,
                &merge_question(round),
                status,
                Timestamp::now(),
            )
            .await
            .unwrap();
        }
        let view = super::super::views::badge(&state, handle, &work)
            .await
            .unwrap();
        assert_eq!(view.badge, Some(PrBadge::ReadyToMerge), "{view:?}");
    }

    #[tokio::test]
    async fn already_assessed_comment_in_running_pass_vetoes_ready_to_merge() {
        use contracts::api::review_loop::CommentAssessment;
        use contracts::ids::JobId;
        use db::repo::review_loop::{NewPass, PassCause, PassStart, SentItem, UpsertOutcome};

        let (state, handle, work) = ready_pr_fixture().await;
        let rl = state.db.review_loop();
        let now = Timestamp::now();
        let observed = observed_comment();
        let UpsertOutcome::New(item) = rl
            .upsert_item(
                DEFAULT_TENANT,
                handle.id,
                &observed,
                ItemState::Assessing,
                now,
            )
            .await
            .unwrap()
        else {
            panic!("new review item")
        };
        let job = JobId::new_random();
        let assessment: CommentAssessment = serde_json::from_value(json!({
            "job": job, "head": observed.head, "assessed_at": now, "validity": "valid",
            "evidence": [], "category": "correctness", "necessity": {"verdict": "in", "reason": "Check input"},
            "gates": [], "elsewhere": [], "reply_drafts": [],
            "recommendation": {"kind": "fix_here", "reason": "Check input", "reason_class": "correctness"},
            "prompt_version": 1
        })).unwrap();
        rl.add_assessment(DEFAULT_TENANT, item.id, &assessment, "old")
            .await
            .unwrap();
        let PassStart::Started(pass) = rl
            .start_pass(
                DEFAULT_TENANT,
                &NewPass {
                    pull_request_id: handle.id,
                    user_id: Some("u1".into()),
                    cause: PassCause::AssessAgain,
                    head: observed.head,
                    sent: vec![SentItem {
                        item: item.id,
                        no: item.no,
                        fingerprint: "new".into(),
                    }],
                    question: None,
                },
                now,
            )
            .await
            .unwrap()
        else {
            panic!("new pass")
        };
        let view = || super::super::views::badge(&state, handle.clone(), &work);
        let reassessing = "1 review item being assessed again".to_owned();
        let queued = view().await.unwrap();
        assert_eq!(queued.badge, Some(PrBadge::ReadyToMerge));
        assert!(!queued.facts.contains(&reassessing));
        assert!(
            !queued
                .facts
                .contains(&"1 review item not assessed yet".to_owned())
        );
        rl.pass_running(pass, job).await.unwrap();
        let during = view().await.unwrap();
        assert_eq!(during.badge, None);
        assert!(during.facts.contains(&reassessing), "{during:?}");
        assert!(
            !during
                .facts
                .contains(&"1 review item not assessed yet".to_owned())
        );
        rl.finish_pass(pass, true, Timestamp::now()).await.unwrap();
        let finished = view().await.unwrap();
        assert_eq!(finished.badge, Some(PrBadge::ReadyToMerge));
        assert!(!finished.facts.contains(&reassessing));
    }

    #[tokio::test]
    async fn comment_handed_to_follow_up_does_not_veto_ready_to_merge() {
        use contracts::api::access_tokens::TokenScope;
        use contracts::api::review_loop::{
            DecisionKind, DecisionVia, EffectKind, EffectParams, Executor, ReportOutcome,
        };
        use db::repo::review_loop::{
            ClaimOutcome, DecideOutcome, NewDecision, NewEffect, ReportOutcomeRecord,
            ReportRequest, UpsertOutcome,
        };
        use jiff::SignedDuration;

        let (state, handle, work) = ready_pr_fixture().await;
        let rl = state.db.review_loop();
        let now = Timestamp::now();
        let observed = observed_comment();
        let UpsertOutcome::New(comment) = rl
            .upsert_item(
                DEFAULT_TENANT,
                handle.id,
                &observed,
                ItemState::Undecided,
                now,
            )
            .await
            .unwrap()
        else {
            panic!("new review item")
        };
        let UpsertOutcome::New(question) = rl
            .upsert_item(
                DEFAULT_TENANT,
                handle.id,
                &merge_question(3),
                ItemState::Undecided,
                now,
            )
            .await
            .unwrap()
        else {
            panic!("new merge question")
        };
        let DecideOutcome::Stored(decisions) = rl
            .decide(
                DEFAULT_TENANT,
                &[NewDecision {
                    item: question.id,
                    item_revision: question.revision,
                    user_id: "u1".into(),
                    kind: DecisionKind::MergeWithFollowUps,
                    reason: None,
                    target: None,
                    r#override: None,
                    resolve_ticked: false,
                    reply_text: None,
                    comment: None,
                    recommendation: None,
                    via: DecisionVia::Question,
                    carried_from: None,
                    head: observed.head,
                    effects: vec![NewEffect {
                        seq: 1,
                        after: None,
                        kind: EffectKind::FollowUpPr,
                        executor: Executor::Agent,
                        params: EffectParams {
                            item_nos: vec![comment.no],
                            ..EffectParams::default()
                        },
                    }],
                }],
                now,
            )
            .await
            .unwrap()
        else {
            panic!("new merge decision")
        };
        let decision = &decisions[0];
        let (_, agent) = crate::auth::access_tokens::stored_token(
            &state.db,
            "u1",
            vec![TokenScope::ReviewLoopExecute],
            now + SignedDuration::from_hours(1),
        )
        .await;
        let lease = SignedDuration::from_mins(5);
        assert!(matches!(
            rl.claim(
                decision.id,
                &agent.token_id,
                &[EffectKind::FollowUpPr],
                lease,
                now
            )
            .await
            .unwrap(),
            ClaimOutcome::Claimed(_)
        ));
        assert_eq!(
            rl.report(
                DEFAULT_TENANT,
                decision.effects[0].id,
                &agent.token_id,
                &ReportRequest {
                    outcome: ReportOutcome::Done,
                    reference: Some("https://github.com/dashpay/platform/pull/5950".into()),
                    message: None,
                    idempotency_key: "follow-up".into(),
                },
                lease,
                now
            )
            .await
            .unwrap(),
            ReportOutcomeRecord::Recorded
        );
        let view = super::super::views::badge(&state, handle, &work)
            .await
            .unwrap();
        assert_eq!(view.badge, Some(PrBadge::ReadyToMerge), "{view:?}");
    }

    fn ready() -> BadgeInputs {
        BadgeInputs {
            open: true,
            ci: Some(CiState::Passing),
            ready_to_merge: Some(true),
            ..BadgeInputs::default()
        }
    }

    #[test]
    fn badge_counts_first_assessments_and_reassessments_separately() {
        let view = badge(&BadgeInputs {
            pending_human_review: 1,
            reassessing_human_review: 2,
            ..ready()
        });
        assert_eq!(view.badge, None);
        assert!(
            view.facts
                .iter()
                .any(|fact| fact == "1 review item not assessed yet")
        );
        assert!(
            view.facts
                .iter()
                .any(|fact| fact == "2 review items being assessed again")
        );
    }

    #[test]
    fn state_badge_precedence_and_unknown_inputs() {
        assert_eq!(badge(&ready()).badge, Some(PrBadge::ReadyToMerge));
        let all = BadgeInputs {
            failed_effects: 1,
            needs_you: 2,
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
            needs_you: 0,
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
            (
                "head not scanned by My work",
                BadgeInputs {
                    ready_to_merge: None,
                    ..ready()
                },
                "My work's last scan did not read this head and base",
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
            needs_you: 1,
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
            BadgeInputs {
                ready_to_merge: Some(false),
                ..ready()
            },
        ] {
            assert_eq!(badge(&not_ready).badge, None, "{not_ready:?}");
        }
        assert_eq!(
            badge(&BadgeInputs {
                ready_to_merge: Some(false),
                ..ready()
            })
            .facts,
            ["CI passed", "My work does not list it as Ready to merge"]
        );
        let closed = BadgeInputs {
            open: false,
            ..ready()
        };
        assert_eq!(badge(&closed).badge, None);
    }
}
