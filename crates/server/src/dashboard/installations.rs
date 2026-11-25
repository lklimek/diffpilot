//! Synchronizes and projects coverage of tracked repositories (ADR-026).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use contracts::api::settings::RepositoryInstallation;
use contracts::common::RepoRef;
use contracts::time::Timestamp;
use db::error::DbError;
use futures::{StreamExt, stream};
use jiff::{SignedDuration, civil::Date, tz::TimeZone};

use crate::github::app_token::{AppTokenError, InstallationLookup};
use crate::state::AppState;

const LOOKUPS_PER_TICK: usize = 100;
const CONCURRENT_LOOKUPS: usize = 4;
const LOOKUP_BACKOFF: SignedDuration = SignedDuration::from_mins(5);

/// Case-insensitive identity used by the installation map.
pub(crate) fn key(repo: &RepoRef) -> RepoRef {
    RepoRef {
        owner: repo.owner.to_ascii_lowercase(),
        name: repo.name.to_ascii_lowercase(),
    }
}

/// The tenant's tracked repositories, with the deployment's webhook configuration applied.
pub(crate) async fn for_tenant(
    state: &AppState,
    tenant: &str,
) -> Result<Vec<RepositoryInstallation>, DbError> {
    let mut repositories = state
        .db
        .github()
        .repository_installations(Some(tenant))
        .await?;
    if state.config.github_webhook_secret.is_none() {
        for repository in &mut repositories {
            repository.live = false;
        }
    }
    Ok(repositories)
}

/// Case-insensitive live repository keys for a tenant.
pub(crate) async fn live(state: &AppState, tenant: &str) -> Result<HashSet<RepoRef>, DbError> {
    if state.config.github_webhook_secret.is_none() {
        return Ok(HashSet::new());
    }
    Ok(state
        .db
        .github()
        .repository_installations(Some(tenant))
        .await?
        .into_iter()
        .filter(|row| row.live)
        .map(|row| key(&row.repo))
        .collect())
}

#[derive(Default)]
struct Sync {
    day: Option<Date>,
    checked: HashSet<RepoRef>,
    retry_after: HashMap<RepoRef, Timestamp>,
}

impl Sync {
    async fn tick(&mut self, state: &AppState, now: Timestamp) -> Result<(), DbError> {
        let github = state.db.github();
        let today = now.to_zoned(TimeZone::UTC).date();
        if self.day != Some(today) {
            github.prune_deliveries(now).await?;
            github.prune_installations().await?;
            self.checked.clear();
            self.day = Some(today);
        }
        let tracked = github.repository_installations(None).await?;
        let present = tracked
            .iter()
            .map(|row| key(&row.repo))
            .collect::<HashSet<_>>();
        self.checked.retain(|repo| present.contains(repo));
        self.retry_after
            .retain(|repo, retry| present.contains(repo) && *retry > now);
        let pending: Vec<_> = tracked
            .into_iter()
            .map(|row| row.repo)
            .filter(|repo| {
                let key = key(repo);
                !self.checked.contains(&key) && !self.retry_after.contains_key(&key)
            })
            .take(LOOKUPS_PER_TICK)
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        let lookup = match InstallationLookup::new(&state.config.github, now).await {
            Ok(lookup) => lookup,
            Err(error) => {
                self.retry_after
                    .extend(pending.iter().map(|repo| (key(repo), now + LOOKUP_BACKOFF)));
                tracing::warn!(%error, "GitHub App repository installation sync could not start");
                return Ok(());
            }
        };
        let results = stream::iter(pending)
            .map(|repo| {
                let lookup = &lookup;
                async move {
                    let started = Timestamp::now();
                    let result = lookup.get(&repo).await.and_then(|found| {
                        found
                            .map(|row| {
                                let id =
                                    i64::try_from(row.id).map_err(|_| AppTokenError::Malformed)?;
                                Ok((id, row.suspended()))
                            })
                            .transpose()
                    });
                    (repo, started, result)
                }
            })
            .buffer_unordered(CONCURRENT_LOOKUPS);
        tokio::pin!(results);
        let mut failed = 0;
        let mut last_error = None;
        while let Some((repo, started, result)) = results.next().await {
            match result {
                Ok(found) => {
                    github
                        .record_installation_lookup(&repo, found, started)
                        .await?;
                    self.checked.insert(key(&repo));
                }
                Err(error) => {
                    self.retry_after.insert(key(&repo), now + LOOKUP_BACKOFF);
                    failed += 1;
                    last_error = Some(error);
                }
            }
        }
        if let Some(error) = last_error {
            tracing::warn!(failed, %error, "some GitHub App repository installation lookups failed");
        }
        Ok(())
    }
}

/// Checks newly tracked repositories on each tick and rechecks existing coverage each night.
pub(super) async fn run(state: AppState, tick: Duration) {
    let mut stopping = state.stopping.subscribe();
    let mut interval = tokio::time::interval(tick);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut sync = Sync::default();
    loop {
        tokio::select! {
            biased;
            _ = stopping.wait_for(|stop| *stop) => return,
            _ = async {
                interval.tick().await;
                if let Err(error) = sync.tick(&state, Timestamp::now()).await {
                    tracing::warn!(%error, "GitHub App installation maintenance failed; the next tick retries it");
                }
            } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use contracts::api::dashboard::{DashboardSettings, RepositoryRule};
    use db::repo::github::{Effect, NewDelivery};
    use db::repo::{DEFAULT_TENANT, Db, users::GithubProfile};
    use jiff::SignedDuration;
    use serde_json::json;
    use uuid::Uuid;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::config::tests::test_config;
    use crate::github::app_token::tests::{github_config, write_key};

    async fn sync_world(server: &MockServer, key_dir: &std::path::Path) -> AppState {
        let mut config = test_config();
        config.github = github_config(server, write_key(key_dir));
        let state = AppState::with_queue(config, Db::in_memory().await.expect("db"));
        state
            .db
            .users()
            .upsert_member(
                DEFAULT_TENANT,
                "u1",
                &GithubProfile {
                    github_id: 1,
                    login: "tester".to_owned(),
                    name: None,
                    avatar_url: None,
                },
                Timestamp::now(),
            )
            .await
            .expect("member");
        state
    }

    #[tokio::test]
    async fn failed_installation_lookup_retries_after_backoff() {
        let server = MockServer::start().await;
        let tmp = tempfile::tempdir().unwrap();
        let state = sync_world(&server, tmp.path()).await;
        let repo = RepoRef {
            owner: "acme".to_owned(),
            name: "retry".to_owned(),
        };
        let now = "2026-11-25T12:00:00Z".parse::<Timestamp>().unwrap();
        let settings = DashboardSettings {
            repositories: vec![RepositoryRule {
                repo: repo.clone(),
                alias: None,
                watch_issues: false,
                ownership_policy: None,
                branch_patterns: vec![],
                ignored_checks: vec![],
            }],
            ..DashboardSettings::default()
        };
        state
            .db
            .dashboard()
            .save_settings(DEFAULT_TENANT, "u1", &settings, now)
            .await
            .unwrap();
        let endpoint = "/api/repos/acme/retry/installation";
        Mock::given(method("GET"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let mut sync = Sync::default();
        sync.tick(&state, now).await.unwrap();
        sync.tick(&state, now + SignedDuration::from_mins(1))
            .await
            .unwrap();
        server.verify().await;
        server.reset().await;
        Mock::given(method("GET"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 42})))
            .expect(1)
            .mount(&server)
            .await;
        sync.tick(&state, now + SignedDuration::from_mins(5))
            .await
            .unwrap();
        assert!(
            state
                .db
                .github()
                .repository_installations(Some(DEFAULT_TENANT))
                .await
                .unwrap()
                .iter()
                .any(|row| row.repo == repo && row.live)
        );
        sync.tick(&state, now + SignedDuration::from_mins(10))
            .await
            .unwrap();
        server.verify().await;
    }

    #[tokio::test]
    async fn installation_sync_checks_new_repos_and_reconciles_existing_ones_nightly() {
        let server = MockServer::start().await;
        let tmp = tempfile::tempdir().expect("fixture");
        let state = sync_world(&server, tmp.path()).await;
        let db = state.db.github();
        let repo = |name: &str| RepoRef {
            owner: "acme".to_owned(),
            name: name.to_owned(),
        };
        let rule = |name: &str| RepositoryRule {
            repo: repo(name),
            alias: None,
            watch_issues: false,
            ownership_policy: None,
            branch_patterns: vec![],
            ignored_checks: vec![],
        };
        let now = Timestamp::now();
        let mut settings = DashboardSettings {
            repositories: vec![rule("node-client")],
            ..DashboardSettings::default()
        };
        state
            .db
            .dashboard()
            .save_settings(DEFAULT_TENANT, "u1", &settings, now)
            .await
            .expect("settings");

        db.record(
            &NewDelivery {
                id: Uuid::new_v4(),
                hash: &"a".repeat(64),
                event: "ping",
                action: None,
                installation_id: None,
                effect: Effect::Ignore,
            },
            now - SignedDuration::from_hours(15 * 24),
        )
        .await
        .expect("receipt");
        assert!(db.last_delivery_at().await.unwrap().is_some());

        let mut sync = Sync::default();
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/node-client/installation"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"id": 9, "suspended_at": null})),
            )
            .expect(1)
            .mount(&server)
            .await;
        sync.tick(&state, now).await.expect("first sync");
        assert_eq!(db.last_delivery_at().await.unwrap(), None);
        assert!(
            db.repository_installations(Some(DEFAULT_TENANT))
                .await
                .unwrap()
                .iter()
                .any(|row| row.repo == repo("node-client") && row.live)
        );

        settings.repositories.push(rule("new"));
        state
            .db
            .dashboard()
            .save_settings(DEFAULT_TENANT, "u1", &settings, now)
            .await
            .expect("new rule");
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/new/installation"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id": 9, "suspended_at": "2026-11-20"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        sync.tick(&state, now).await.expect("sync new rule");
        assert!(
            db.repository_installations(Some(DEFAULT_TENANT))
                .await
                .unwrap()
                .iter()
                .any(|row| row.repo == repo("new") && !row.live)
        );
        server.verify().await;
        server.reset().await;

        Mock::given(method("GET"))
            .and(path("/api/repos/acme/node-client/installation"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/new/installation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 15})))
            .expect(1)
            .mount(&server)
            .await;
        sync.tick(&state, now + SignedDuration::from_hours(24))
            .await
            .expect("nightly sync");
        let rows = db
            .repository_installations(Some(DEFAULT_TENANT))
            .await
            .unwrap();
        assert!(
            rows.iter()
                .any(|row| row.repo == repo("node-client") && !row.live)
        );
        assert!(rows.iter().any(|row| row.repo == repo("new") && row.live));
    }
}
