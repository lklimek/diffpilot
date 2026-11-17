//! Job claiming for runners: job tokens, lease, credentials and session drops (`runner-protocol.md` Claim).

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, TimeDelta, Utc};
use contracts::ids::JobId;
use contracts::job::access::{AgentAccess, InstallationToken, Secret};
use contracts::job::protocol::{Claim, ClaimRequest, ClaimResponse, LeaseTiming};
use db::error::DbError;
use db::repo::jobs::{ClaimParams, JobRecord};
use db::repo::{DEFAULT_TENANT, Db};
use secrecy::ExposeSecret;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::config::Config;
use crate::github::app_token::{self, AppTokenError};
use crate::jobs::queue::expire_leases;
use crate::secrets::{Aad, SecretError, TokenCipher};

/// Random bytes of a job token.
const TOKEN_BYTES: usize = 32;

/// How often pending installation-token revocations are retried (server start included).
const REVOKE_INTERVAL: Duration = Duration::from_secs(60);

/// A claim could not be served.
#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    /// The job queue's storage failed.
    #[error("the job queue is not available")]
    Db(#[from] DbError),
    /// The operating system's random source failed.
    #[error("could not generate a job token")]
    Random(#[source] getrandom::Error),
    /// GitHub could not mint the job's installation token; the job is retried after its lease.
    #[error("the job's GitHub installation token could not be minted")]
    GithubAccess(#[source] AppTokenError),
    /// The minted installation token could not be encrypted for revocation; it was revoked.
    #[error("the job's GitHub installation token could not be stored")]
    StoreToken(#[source] SecretError),
}

/// SHA-256 of a bearer value; only this hash of a job token is stored.
pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Compares two token hashes in constant time.
pub fn hashes_match(stored: &[u8], presented: &[u8]) -> bool {
    stored.ct_eq(presented).into()
}

/// A new random job token and its hash.
fn mint_token() -> Result<(Secret, Vec<u8>), ClaimError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(ClaimError::Random)?;
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let hash = token_hash(&token);
    Ok((Secret::new(token), hash))
}

/// The server-side lease end for a request received at `now` ([`LeaseTiming::V1`]).
pub fn lease_until(now: DateTime<Utc>) -> DateTime<Utc> {
    now + TimeDelta::seconds(i64::from(LeaseTiming::V1.lease_secs))
}

/// Serves one runner claim: housekeeping, credential registration, then the next runnable job, if any, with
/// its read-only installation token (`runner-protocol.md` Claim).
pub async fn claim(
    db: &Db,
    config: &Config,
    request: &ClaimRequest,
    now: DateTime<Utc>,
) -> Result<ClaimResponse, ClaimError> {
    if !expire_leases(db, now).await?.is_empty() {
        revoke_github_tokens(db, config, now).await?;
    }
    let resumed = db.jobs().resume_paused(now).await?;
    if !resumed.is_empty() {
        tracing::info!(
            jobs = resumed.len(),
            "paused jobs queued again after the usage limit reset"
        );
    }
    db.analyses()
        .ack_session_drops(&request.dropped_sessions)
        .await?;
    let mut usable = Vec::with_capacity(request.credentials.len());
    for status in &request.credentials {
        db.llm_credentials()
            .register(DEFAULT_TENANT, &status.credential, status.usable, now)
            .await?;
        if status.usable {
            usable.push(status.credential.id.clone());
        }
    }
    let drop_sessions = db.analyses().pending_session_drops().await?;
    let (token, hash) = mint_token()?;
    let lease_expires_at = lease_until(now);
    let claimed = db
        .jobs()
        .claim(ClaimParams {
            credential_ids: &usable,
            job_token_hash: &hash,
            lease_until: lease_expires_at,
            now,
        })
        .await?;
    let claim = match claimed {
        Some(job) => {
            tracing::info!(
                job = %job.id,
                attempt = job.attempt,
                runner = %request.runner_id,
                "runner claimed a job"
            );
            let github = github_access(db, config, &job, now).await?;
            let mut spec = job.spec;
            // TODO(WP3-RUN): fill `access.memcan` from `Config::memcan`; the stage-2 slice runs without MemCan.
            spec.access = AgentAccess {
                github,
                memcan: None,
            };
            Some(Claim {
                job_token: token,
                lease_expires_at,
                spec,
            })
        }
        None => None,
    };
    Ok(ClaimResponse {
        claim,
        drop_sessions,
    })
}

/// Where the encrypted installation token of a job attempt is stored (AES-GCM associated data).
fn token_aad(row_id: &str) -> Aad<'_> {
    Aad {
        table: "installation_tokens",
        column: "ciphertext",
        row_id,
    }
}

fn token_row_id(job: JobId, attempt: u32) -> String {
    format!("{job}:{attempt}")
}

/// Mints the claimed attempt's installation token for the PR repository and stores it encrypted for revocation.
/// No token when the App is not installed there or not usable as configured (ADR-005); other GitHub failures
/// fail the claim, so the job is retried after its lease instead of running without the access its prompt
/// expects.
async fn github_access(
    db: &Db,
    config: &Config,
    job: &JobRecord,
    now: DateTime<Utc>,
) -> Result<Option<InstallationToken>, ClaimError> {
    let repo = db.analyses().summary(job.analysis_id).await?.pr.repo;
    let runtime = TimeDelta::seconds(i64::from(job.spec.limits.max_runtime_secs));
    let token = match app_token::mint_for_job(&config.github, &repo, now, runtime).await {
        Ok(Some(token)) => token,
        Ok(None) => {
            tracing::info!(
                job = %job.id,
                owner = %repo.owner,
                repo = %repo.name,
                "GitHub App not installed on the PR repository; the job runs without GitHub access"
            );
            return Ok(None);
        }
        Err(error) if error.is_configuration() => {
            tracing::error!(
                job = %job.id,
                %error,
                "GitHub App unusable; the job runs without GitHub access"
            );
            return Ok(None);
        }
        Err(error) => return Err(ClaimError::GithubAccess(error)),
    };
    let row_id = token_row_id(job.id, job.attempt);
    let stored = match TokenCipher::new(&config.secret_key)
        .encrypt(token_aad(&row_id), token.token.expose())
    {
        Ok(ciphertext) => db
            .jobs()
            .record_github_token(job.id, job.attempt, &ciphertext, token.expires_at)
            .await
            .map_err(ClaimError::Db),
        Err(error) => Err(ClaimError::StoreToken(error)),
    };
    if let Err(error) = stored {
        if let Err(revoke) = app_token::revoke(&config.github, token.token.expose()).await {
            tracing::error!(
                job = %job.id,
                error = %revoke,
                "could not revoke an unstored installation token; it expires within an hour"
            );
        }
        return Err(error);
    }
    tracing::info!(
        job = %job.id,
        attempt = job.attempt,
        expires_at = %token.expires_at,
        "read-only installation token minted for the job"
    );
    Ok(Some(token))
}

/// Revokes the installation tokens of attempts that are no longer claimed or running and confirms each
/// revocation; an expired token is only confirmed. Failures are logged and retried by the next sweep.
pub async fn revoke_github_tokens(
    db: &Db,
    config: &Config,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let pending = db.jobs().github_tokens_to_revoke().await?;
    if pending.is_empty() {
        return Ok(());
    }
    let cipher = TokenCipher::new(&config.secret_key);
    for stale in pending {
        if stale.expires_at > now {
            let row_id = token_row_id(stale.job, stale.attempt);
            let token = match cipher.decrypt(token_aad(&row_id), &stale.ciphertext) {
                Ok(token) => token,
                Err(error) => {
                    tracing::error!(
                        job = %stale.job,
                        attempt = stale.attempt,
                        %error,
                        "stored installation token unreadable; it stays listed until it expires"
                    );
                    continue;
                }
            };
            if let Err(error) = app_token::revoke(&config.github, token.expose_secret()).await {
                tracing::warn!(
                    job = %stale.job,
                    attempt = stale.attempt,
                    %error,
                    "installation token revocation failed; retried by the next sweep"
                );
                continue;
            }
        }
        db.jobs()
            .confirm_github_token_revoked(stale.job, stale.attempt)
            .await?;
        tracing::info!(
            job = %stale.job,
            attempt = stale.attempt,
            "installation token of an ended job attempt revoked"
        );
    }
    Ok(())
}

/// Runs [`revoke_github_tokens`] at start and every minute, so tokens of leases a follower expired and failed
/// revocations are not left to expire.
pub async fn revoke_github_tokens_periodically(db: Db, config: Arc<Config>) {
    let mut ticks = tokio::time::interval(REVOKE_INTERVAL);
    loop {
        ticks.tick().await;
        if let Err(error) = revoke_github_tokens(&db, &config, Utc::now()).await {
            tracing::warn!(%error, "installation token sweep could not read the queue; retried in a minute");
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use contracts::agent::JobKind;
    use contracts::common::RepoRef;
    use contracts::ids::FlowId;
    use contracts::job::protocol::CredentialStatus;
    use contracts::job::result::{JobFailureKind, JobOutcome, JobResult};
    use contracts::job::spec::{CredentialKind, JobPriority, LlmCredentialRef};
    use contracts::routes::RUNNER_RESULT;
    use db::repo::analyses::testing::{CREDENTIAL, job_spec, seed_analysis};
    use db::repo::jobs::{JobStage, NewJob};
    use serde_json::json;
    use tempfile::TempDir;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::config::tests::test_config;
    use crate::github::app_token::tests::{
        MINTED_TOKEN, github_config, minted, mount_installation, mount_mint, mount_revoke,
        write_key,
    };
    use crate::jobs::queue::QueueExecutor;
    use crate::state::AppState;
    use crate::test_support::logs::LogCapture;

    struct Setup {
        db: Db,
        config: Config,
        github: MockServer,
        _dir: TempDir,
    }

    async fn setup() -> Setup {
        let dir = tempfile::tempdir().expect("temp dir");
        let github = MockServer::start().await;
        let mut config = test_config();
        config.github = github_config(&github, write_key(dir.path()));
        Setup {
            db: Db::in_memory().await.expect("db"),
            config,
            github,
            _dir: dir,
        }
    }

    async fn enqueue(db: &Db) -> JobId {
        let analysis = seed_analysis(db).await.expect("seed");
        db.jobs()
            .enqueue(
                NewJob {
                    spec: job_spec(analysis, JobKind::AnalyseFlow),
                    priority: JobPriority::Batch,
                    stage: JobStage::Flow(FlowId("f1".to_owned())),
                    generation: 0,
                },
                Utc::now(),
            )
            .await
            .expect("enqueue")
            .id
    }

    fn request() -> ClaimRequest {
        ClaimRequest {
            runner_id: "runner-1".to_owned(),
            credentials: vec![CredentialStatus {
                credential: LlmCredentialRef {
                    id: CREDENTIAL.to_owned(),
                    kind: CredentialKind::CodexChatgpt,
                },
                usable: true,
            }],
            dropped_sessions: Vec::new(),
        }
    }

    async fn claim_one(s: &Setup, now: DateTime<Utc>) -> Claim {
        claim(&s.db, &s.config, &request(), now)
            .await
            .expect("claim")
            .claim
            .expect("a claimed job")
    }

    /// Mounts lookup and mint of [`MINTED_TOKEN`] for `acme/node-client`.
    async fn installed(s: &Setup, now: DateTime<Utc>) {
        mount_installation(&s.github).await;
        mount_mint(&s.github, minted(now), 1).await;
    }

    #[tokio::test]
    async fn claim_places_installation_token_in_spec() {
        let capture = LogCapture::start();
        let s = setup().await;
        let job = enqueue(&s.db).await;
        let now = Utc::now();
        installed(&s, now).await;
        let claimed = claim_one(&s, now).await;
        let github = claimed.spec.access.github.expect("installation token");
        assert_eq!(github.token.expose(), MINTED_TOKEN);
        assert_eq!(github.expires_at - now, TimeDelta::hours(1));
        assert_eq!(
            github.repository,
            RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned()
            }
        );
        let stored = s.db.jobs().get(job).await.expect("job");
        assert_eq!(
            stored.spec.access,
            AgentAccess::default(),
            "the stored spec holds no credential"
        );
        let logs = capture.text();
        assert!(logs.contains("installation token minted"), "{logs}");
        assert!(!logs.contains(MINTED_TOKEN), "{logs}");
    }

    #[tokio::test]
    async fn uninstalled_repo_job_has_no_github_access() {
        let s = setup().await;
        let job = enqueue(&s.db).await;
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/node-client/installation"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&s.github)
            .await;
        mount_mint(&s.github, minted(Utc::now()), 0).await;
        let claimed = claim_one(&s, Utc::now()).await;
        assert_eq!(claimed.spec.access.github, None);
        assert_eq!(s.db.jobs().github_token(job, 1).await.expect("read"), None);
    }

    #[tokio::test]
    async fn claim_without_usable_app_key_runs_without_github_access() {
        let capture = LogCapture::start();
        let mut s = setup().await;
        s.config.github.private_key_file = s._dir.path().join("missing.pem");
        enqueue(&s.db).await;
        let claimed = claim_one(&s, Utc::now()).await;
        assert_eq!(claimed.spec.access.github, None);
        assert!(
            s.github
                .received_requests()
                .await
                .expect("recorded")
                .is_empty()
        );
        let logs = capture.text();
        assert!(
            logs.contains("ERROR") && logs.contains("SR_GITHUB_APP_PRIVATE_KEY_FILE"),
            "{logs}"
        );
    }

    #[tokio::test]
    async fn claim_fails_when_github_cannot_mint() {
        let s = setup().await;
        let job = enqueue(&s.db).await;
        mount_installation(&s.github).await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&s.github)
            .await;
        let result = claim(&s.db, &s.config, &request(), Utc::now()).await;
        assert!(
            matches!(result, Err(ClaimError::GithubAccess(_))),
            "{result:?}"
        );
        assert_eq!(s.db.jobs().github_token(job, 1).await.expect("read"), None);
    }

    #[tokio::test]
    async fn installation_token_stored_only_encrypted_until_revoke_confirmed() {
        let s = setup().await;
        let job = enqueue(&s.db).await;
        let now = Utc::now();
        installed(&s, now).await;
        claim_one(&s, now).await;
        let ciphertext =
            s.db.jobs()
                .github_token(job, 1)
                .await
                .expect("read")
                .expect("stored for revocation");
        assert!(
            !ciphertext
                .windows(MINTED_TOKEN.len())
                .any(|window| window == MINTED_TOKEN.as_bytes()),
            "never plaintext at rest"
        );
        let row_id = token_row_id(job, 1);
        let decrypted = TokenCipher::new(&s.config.secret_key)
            .decrypt(token_aad(&row_id), &ciphertext)
            .expect("bound to its row");
        assert_eq!(decrypted.expose_secret(), MINTED_TOKEN);

        revoke_github_tokens(&s.db, &s.config, now)
            .await
            .expect("sweep");
        assert!(
            s.db.jobs()
                .github_token(job, 1)
                .await
                .expect("read")
                .is_some(),
            "a running attempt keeps its token"
        );
        s.db.jobs()
            .complete(&result(job, succeeded()), now)
            .await
            .expect("complete");
        {
            let failing = Mock::given(method("DELETE"))
                .respond_with(ResponseTemplate::new(503))
                .expect(1)
                .mount_as_scoped(&s.github)
                .await;
            revoke_github_tokens(&s.db, &s.config, now)
                .await
                .expect("sweep");
            drop(failing);
        }
        assert!(
            s.db.jobs()
                .github_token(job, 1)
                .await
                .expect("read")
                .is_some(),
            "kept until the revocation is confirmed"
        );
        mount_revoke(&s.github, 204, 1).await;
        revoke_github_tokens(&s.db, &s.config, now)
            .await
            .expect("sweep");
        assert_eq!(s.db.jobs().github_token(job, 1).await.expect("read"), None);
    }

    fn succeeded() -> JobOutcome {
        JobOutcome::Succeeded {
            output: json!({"flows": []}),
        }
    }

    fn result(job: JobId, outcome: JobOutcome) -> JobResult {
        JobResult {
            job_id: job,
            attempt: 1,
            outcome,
            session: None,
            usage: None,
            skills_commit: None,
            llm_call: None,
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum End {
        Success,
        Failure,
        Cancel,
        LeaseExpiry,
    }

    /// Every way a job attempt ends revokes its token at once: results through the runner route, lease expiry
    /// through the next claim.
    #[tokio::test]
    async fn installation_token_revoked_on_job_end() {
        for end in [End::Success, End::Failure, End::Cancel, End::LeaseExpiry] {
            let capture = LogCapture::start();
            let s = setup().await;
            let job = enqueue(&s.db).await;
            let now = Utc::now();
            installed(&s, now).await;
            mount_revoke(&s.github, 204, 1).await;
            let claimed = claim_one(&s, now).await;
            let state = AppState::new(
                s.config,
                s.db.clone(),
                Arc::new(QueueExecutor::new(s.db.clone())),
            );
            let outcome = match end {
                End::Success => Some(succeeded()),
                End::Failure => Some(JobOutcome::Failed {
                    kind: JobFailureKind::ProviderFailed,
                    message: "provider failed".to_owned(),
                }),
                End::Cancel => {
                    state
                        .db
                        .jobs()
                        .request_cancel(job, now)
                        .await
                        .expect("cancel");
                    Some(JobOutcome::Cancelled)
                }
                End::LeaseExpiry => None,
            };
            match outcome {
                Some(outcome) => {
                    let response = crate::api::runner_router(state.clone())
                        .oneshot(
                            Request::post(RUNNER_RESULT.path.replace("{id}", &job.to_string()))
                                .header(
                                    header::AUTHORIZATION,
                                    format!("Bearer {}", claimed.job_token.expose()),
                                )
                                .header(header::CONTENT_TYPE, "application/json")
                                .body(Body::from(
                                    serde_json::to_vec(&result(job, outcome)).expect("json"),
                                ))
                                .expect("request"),
                        )
                        .await
                        .expect("infallible");
                    assert_eq!(response.status(), StatusCode::NO_CONTENT, "{end:?}");
                }
                None => {
                    // A runner without usable credentials claims nothing, so the job is not re-claimed.
                    let idle = ClaimRequest {
                        credentials: Vec::new(),
                        ..request()
                    };
                    let after_lease = claimed.lease_expires_at + TimeDelta::seconds(1);
                    claim(&state.db, &state.config, &idle, after_lease)
                        .await
                        .expect("claim after the lease");
                }
            }
            assert_eq!(
                state.db.jobs().github_token(job, 1).await.expect("read"),
                None,
                "{end:?}: revocation confirmed"
            );
            let logs = capture.text();
            assert!(!logs.contains(MINTED_TOKEN), "{end:?}: {logs}");
            // Dropping the server verifies exactly one revocation per ended attempt.
            drop(s.github);
        }
    }

    #[tokio::test]
    async fn cancel_at_lease_expiry_revokes_and_discards_output() {
        let s = setup().await;
        let job = enqueue(&s.db).await;
        let now = Utc::now();
        installed(&s, now).await;
        mount_revoke(&s.github, 204, 1).await;
        let claimed = claim_one(&s, now).await;
        s.db.jobs()
            .record_output(job, 1, &json!({"flows": []}), claimed.lease_expires_at)
            .await
            .expect("output");
        s.db.jobs().request_cancel(job, now).await.expect("cancel");
        let idle = ClaimRequest {
            credentials: Vec::new(),
            ..request()
        };
        claim(
            &s.db,
            &s.config,
            &idle,
            claimed.lease_expires_at + TimeDelta::seconds(1),
        )
        .await
        .expect("claim after the lease");
        let ended = s.db.jobs().get(job).await.expect("job");
        assert_eq!(ended.state, contracts::job::state::JobState::Cancelled);
        assert_eq!(ended.ready_output, None, "ready output discarded");
        assert_eq!(s.db.jobs().github_token(job, 1).await.expect("read"), None);
    }

    #[tokio::test]
    async fn pending_revocations_recovered_after_restart() {
        let s = setup().await;
        let job = enqueue(&s.db).await;
        let now = Utc::now();
        installed(&s, now).await;
        claim_one(&s, now).await;
        // The server stopped after the result was stored, before any sweep.
        s.db.jobs()
            .complete(&result(job, succeeded()), now)
            .await
            .expect("complete");
        let expired = enqueue(&s.db).await;
        let cipher = TokenCipher::new(&s.config.secret_key);
        let row_id = token_row_id(expired, 1);
        let ciphertext = cipher
            .encrypt(token_aad(&row_id), "ghs_expired")
            .expect("encrypt");
        s.db.jobs()
            .record_github_token(expired, 1, &ciphertext, now - TimeDelta::minutes(1))
            .await
            .expect("record");

        mount_revoke(&s.github, 204, 1).await;
        let start = tokio::spawn(revoke_github_tokens_periodically(
            s.db.clone(),
            Arc::new(s.config),
        ));
        for _ in 0..100 {
            let left = s.db.jobs().github_tokens_to_revoke().await.expect("list");
            if left.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        start.abort();
        assert!(
            s.db.jobs()
                .github_tokens_to_revoke()
                .await
                .expect("list")
                .is_empty(),
            "the start sweep revokes the finished attempt and confirms the expired token without a call"
        );
        let deletes = s
            .github
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .filter(|request| request.method.as_str() == "DELETE")
            .count();
        assert_eq!(deletes, 1);
    }

    #[test]
    fn tokens_are_random_and_only_their_hash_matches() {
        let (first, first_hash) = mint_token().expect("token");
        let (second, _) = mint_token().expect("token");
        assert_ne!(first.expose(), second.expose());
        assert_eq!(
            first.expose().len(),
            43,
            "32 bytes, base64url without padding"
        );
        assert_eq!(first_hash, token_hash(first.expose()));
        assert!(hashes_match(&first_hash, &token_hash(first.expose())));
        assert!(!hashes_match(&first_hash, &token_hash(second.expose())));
        assert!(!hashes_match(&first_hash, &[]));
    }

    #[test]
    fn lease_uses_the_v1_timing() {
        let now = Utc::now();
        assert_eq!(
            lease_until(now) - now,
            TimeDelta::seconds(i64::from(LeaseTiming::V1.lease_secs))
        );
    }
}
