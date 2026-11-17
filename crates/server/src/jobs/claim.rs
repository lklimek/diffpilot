//! Job claiming for runners: job tokens, lease, credentials and session drops (`runner-protocol.md` Claim).

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, TimeDelta, Utc};
use contracts::ids::AnalysisId;
use contracts::job::access::{AgentAccess, InstallationToken, Secret};
use contracts::job::protocol::{Claim, ClaimRequest, ClaimResponse, LeaseTiming};
use contracts::job::spec::{SessionRef, WorkingSession};
use db::error::DbError;
use db::repo::analyses::SessionState;
use db::repo::jobs::{ClaimParams, JobRecord, JobStage};
use db::repo::{DEFAULT_TENANT, Db};
use futures::future::BoxFuture;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::config::Config;
use crate::github::app_token::{self, AppTokenError, Minted, Rejection};
use crate::jobs::github_tokens::{self, TokenStoreError};
use crate::jobs::queue::expire_leases;
use crate::prompts::PromptError;

/// Random bytes of a job token.
const TOKEN_BYTES: usize = 32;

/// Budget for the GitHub part of a claim's response; below the runner's 10 s request timeout. It bounds only the
/// answer: the task that received a token always stores or revokes it.
const CLAIM_GITHUB_BUDGET: Duration = Duration::from_secs(6);

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
    /// GitHub minted a token that differs from the request; it is stored and revoked, never handed out.
    #[error("GitHub minted an installation token that differs from the request")]
    TokenRejected(#[source] Rejection),
    /// GitHub did not answer within the claim's budget; the job is retried after its lease, and a token minted
    /// meanwhile is stored and revoked by the worker.
    #[error("GitHub did not mint the job's installation token in time")]
    GithubTimeout,
    /// The minted installation token could not be stored for revocation; it was revoked.
    #[error("the job's GitHub installation token could not be stored")]
    StoreToken(#[source] TokenStoreError),
    /// The task minting the job's installation token ended abnormally.
    #[error("the task minting the job's GitHub installation token failed")]
    GithubTask(#[source] tokio::task::JoinError),
    /// The job's context pack could not be built; the job was not handed out and stays queued.
    #[error("the job's context pack could not be built")]
    ContextPack(#[source] ContextPackError),
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

/// A context pack could not be built.
#[derive(Debug, thiserror::Error)]
pub enum ContextPackError {
    /// The stored results could not be read.
    #[error("the stored results could not be read")]
    Db(#[from] DbError),
    /// The pack could not be rendered.
    #[error("the context pack could not be rendered")]
    Prompt(#[from] PromptError),
}

/// Builds the context pack of a job from the results stored at claim time (`pipeline::context`; an `ask` job's
/// pack adds the asked step and its earlier answers): the claim path stays free of pipeline internals.
pub trait ContextPacks: Send + Sync {
    /// The pack of a job of `stage` in `analysis`; `None` when nothing is stored yet.
    fn pack<'a>(
        &'a self,
        analysis: AnalysisId,
        stage: &'a JobStage,
    ) -> BoxFuture<'a, Result<Option<String>, ContextPackError>>;
}

/// Serves one runner claim: housekeeping, credential registration, then the next runnable job, if any, with
/// its read-only installation token, the PR's session and the context pack of the results stored now
/// (`runner-protocol.md` Claim).
pub async fn claim(
    db: &Db,
    config: &Arc<Config>,
    packs: &dyn ContextPacks,
    request: &ClaimRequest,
    now: DateTime<Utc>,
) -> Result<ClaimResponse, ClaimError> {
    if !expire_leases(db, now).await?.is_empty() {
        github_tokens::nudge();
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
            // Session and pack first: a job whose context cannot be prepared is released before any token is
            // minted, so it stays claimable and nothing needs revoking.
            let mut spec = match prepare_spec(db, packs, &job).await {
                Ok(spec) => spec,
                Err(error) => {
                    tracing::warn!(job = %job.id, attempt = job.attempt, %error,
                        "the job's context could not be prepared; the claim is released");
                    db.jobs().release_claim(job.id, job.attempt).await?;
                    return Err(error);
                }
            };
            // Spawned, so a token received after the budget (or after the runner hung up) is still stored or
            // revoked; the unanswered attempt's lease then expires and the worker revokes the stored token.
            let access = tokio::spawn(github_access(db.clone(), config.clone(), job.clone(), now));
            let github = match tokio::time::timeout(CLAIM_GITHUB_BUDGET, access).await {
                Ok(Ok(access)) => access?,
                Ok(Err(error)) => return Err(ClaimError::GithubTask(error)),
                Err(_) => {
                    tracing::warn!(
                        job = %job.id,
                        attempt = job.attempt,
                        "GitHub did not mint the installation token within the claim budget; the job waits for its lease"
                    );
                    return Err(ClaimError::GithubTimeout);
                }
            };
            db.jobs()
                .record_github_access(job.id, job.attempt, github.is_some())
                .await?;
            spec.access = AgentAccess {
                github,
                // Shared MCP memory is not an isolation boundary: an Ask could write instructions
                // later read by analysis. Only the server's bounded profile/calibration projections apply.
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

/// The claimed job's spec with the PR's session and the context pack of the results stored now: only the
/// runner knows whether the session files survived, so every job carries the pack used when a new session is
/// started. Built at claim, an `ask` job's pack also holds the step's answers given while it waited.
async fn prepare_spec(
    db: &Db,
    packs: &dyn ContextPacks,
    job: &JobRecord,
) -> Result<contracts::job::spec::JobSpec, ClaimError> {
    let mut spec = job.spec.clone();
    spec.session = analysis_session(db, job.analysis_id).await?;
    spec.working_session = if let JobStage::Ask(question) = job.stage {
        let record = db.questions().get(question, Utc::now()).await?;
        if record.analysis_id != job.analysis_id {
            return Err(DbError::NotFound.into());
        }
        Some(WorkingSession {
            user_key: Sha256::digest(record.user_id.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            analytic_generation: db.agent_sessions().generation(spec.pull_request_id).await?,
            thread_id: db
                .agent_sessions()
                .working(spec.pull_request_id, &record.user_id)
                .await?,
        })
    } else {
        None
    };
    spec.context_pack = packs
        .pack(job.analysis_id, &job.stage)
        .await
        .map_err(ClaimError::ContextPack)?;
    Ok(spec)
}

/// The PR's agent session at claim time (ADR-004): one running job per PR, so the session a job
/// resumes is the one the previous job of the PR reported; none before the first report or once dropped.
async fn analysis_session(db: &Db, analysis: AnalysisId) -> Result<Option<SessionRef>, DbError> {
    let record = db.analyses().get(analysis).await?;
    Ok(match (record.session_state, record.agent_thread_id) {
        (SessionState::Active, Some(thread_id)) => Some(SessionRef {
            pull_request_id: record.pull_request_id,
            thread_id,
        }),
        _ => None,
    })
}

/// Mints the claimed attempt's installation token for the PR repository and stores it encrypted for revocation.
/// No token when the App is not installed there or not usable as configured (ADR-005); other GitHub failures
/// fail the claim, so the job is retried after its lease instead of running without the access its prompt
/// expects. A rejected token is stored first and then revoked, so a failed revocation is retried by the worker.
async fn github_access(
    db: Db,
    config: Arc<Config>,
    job: JobRecord,
    now: DateTime<Utc>,
) -> Result<Option<InstallationToken>, ClaimError> {
    let (db, config, job) = (&db, config.as_ref(), &job);
    let repo = db.analyses().summary(job.analysis_id).await?.pr.repo;
    let runtime = TimeDelta::seconds(i64::from(job.spec.limits.max_runtime_secs));
    let token = match app_token::mint_for_job(&config.github, &repo, now, runtime).await {
        Ok(Minted::Accepted(token)) => token,
        Ok(Minted::NotInstalled) => {
            tracing::info!(
                job = %job.id,
                owner = %repo.owner,
                repo = %repo.name,
                "GitHub App not installed on the PR repository; the job runs without GitHub access"
            );
            return Ok(None);
        }
        Ok(Minted::Rejected { token, rejection }) => {
            tracing::error!(
                job = %job.id,
                %rejection,
                "GitHub minted an installation token that differs from the request; it is revoked"
            );
            store_or_revoke(db, config, job, &token).await?;
            match app_token::revoke(&config.github, token.token.expose()).await {
                Ok(()) => {
                    db.jobs()
                        .confirm_github_token_revoked(job.id, job.attempt)
                        .await?;
                }
                Err(error) => tracing::warn!(
                    job = %job.id,
                    %error,
                    "rejected installation token not yet revoked; the worker retries after the lease"
                ),
            }
            return Err(ClaimError::TokenRejected(rejection));
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
    store_or_revoke(db, config, job, &token).await?;
    tracing::info!(
        job = %job.id,
        attempt = job.attempt,
        expires_at = %token.expires_at,
        "read-only installation token minted for the job"
    );
    Ok(Some(token))
}

/// Stores the attempt's token for revocation; if that fails, revokes it at once (it was never handed out).
async fn store_or_revoke(
    db: &Db,
    config: &Config,
    job: &JobRecord,
    token: &InstallationToken,
) -> Result<(), ClaimError> {
    let Err(error) = github_tokens::store(db, config, job.id, job.attempt, token).await else {
        return Ok(());
    };
    if let Err(revoke) = app_token::revoke(&config.github, token.token.expose()).await {
        tracing::error!(
            job = %job.id,
            error = %revoke,
            "could not revoke an unstored installation token; it expires within an hour"
        );
    }
    Err(ClaimError::StoreToken(error))
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use contracts::agent::JobKind;
    use contracts::common::RepoRef;
    use contracts::ids::{FlowId, JobId};
    use contracts::job::protocol::CredentialStatus;
    use contracts::job::result::{JobFailureKind, JobOutcome, JobResult};
    use contracts::job::spec::{CredentialKind, JobPriority, LlmCredentialRef};
    use contracts::routes::RUNNER_RESULT;
    use db::repo::analyses::testing::{CREDENTIAL, job_spec, seed_analysis};
    use db::repo::jobs::NewJob;
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
    use crate::jobs::executor::JobExecutor;
    use crate::jobs::progress::ProgressHub;
    use crate::jobs::queue::QueueExecutor;
    use crate::pipeline::context::StoredPacks;
    use crate::state::AppState;
    use crate::test_support::logs::LogCapture;
    use db::repo::documents::DocumentKind;

    struct Setup {
        db: Db,
        config: Arc<Config>,
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
            config: Arc::new(config),
            github,
            _dir: dir,
        }
    }

    /// Handler state over the setup's database and configuration.
    fn state_with(s: &Setup) -> AppState {
        let mut state = AppState::with_queue(test_config(), s.db.clone());
        state.config = s.config.clone();
        state
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

    fn packs(s: &Setup) -> StoredPacks {
        StoredPacks { db: s.db.clone() }
    }

    /// Packs of a database without stored results.
    fn no_packs() -> NoPacks {
        NoPacks
    }

    struct NoPacks;

    impl ContextPacks for NoPacks {
        fn pack<'a>(
            &'a self,
            _: AnalysisId,
            _: &'a JobStage,
        ) -> BoxFuture<'a, Result<Option<String>, ContextPackError>> {
            Box::pin(async { Ok(None) })
        }
    }

    /// Packs whose stored results cannot be read.
    struct FailingPacks;

    impl ContextPacks for FailingPacks {
        fn pack<'a>(
            &'a self,
            _: AnalysisId,
            _: &'a JobStage,
        ) -> BoxFuture<'a, Result<Option<String>, ContextPackError>> {
            Box::pin(async { Err(ContextPackError::Db(DbError::NotFound)) })
        }
    }

    /// A pack that cannot be built fails the claim before any installation token is minted (the mock allows
    /// exactly one mint, for the later claim); the job stays queued with its attempt unspent.
    #[tokio::test]
    async fn failed_context_pack_fails_the_claim_and_keeps_the_job_claimable() {
        let s = setup().await;
        let job = enqueue(&s.db).await;
        let now = Utc::now();
        installed(&s, now).await;
        let result = claim(&s.db, &s.config, &FailingPacks, &request(), now).await;
        assert!(
            matches!(result, Err(ClaimError::ContextPack(_))),
            "{result:?}"
        );
        let stored = s.db.jobs().get(job).await.expect("job");
        assert_eq!(stored.state, contracts::job::state::JobState::Queued);
        assert_eq!(stored.attempt, 0, "the attempt is not spent");
        assert_eq!(stored.lease_expires_at, None);
        assert_eq!(s.db.jobs().token_hash(job).await.expect("read"), None);
        assert_eq!(s.db.jobs().github_token(job, 1).await.expect("read"), None);
        let claimed = claim_one(&s, now).await;
        assert_eq!(claimed.spec.job_id, job);
        assert_eq!(claimed.spec.attempt, 1);
    }

    fn agent_fixture(name: &str) -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pr-482/agent")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("JSON")
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
        claim(&s.db, &s.config, &packs(s), &request(), now)
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

    /// The pack is built when the job is claimed: results stored while it waited are in it.
    #[tokio::test]
    async fn lost_session_uses_context_pack() {
        let s = setup().await;
        let job = enqueue(&s.db).await;
        let analysis = s.db.jobs().get(job).await.expect("job").analysis_id;
        let now = Utc::now();
        let documents = s.db.documents();
        documents
            .put(
                analysis,
                DocumentKind::Plan,
                "",
                &agent_fixture("plan.json"),
                now,
            )
            .await
            .expect("plan");
        let pr_context = agent_fixture("pr_context.json");
        documents
            .put(analysis, DocumentKind::PrContext, "", &pr_context, now)
            .await
            .expect("pr context");
        for flow in ["f1", "f2"] {
            let output = agent_fixture(&format!("flow-{}.json", &flow[1..]));
            documents
                .put(analysis, DocumentKind::Flow, flow, &output, now)
                .await
                .expect("flow");
        }
        let stored = s.db.jobs().get(job).await.expect("job");
        assert_eq!(
            stored.spec.context_pack, None,
            "nothing is packed at enqueue"
        );
        installed(&s, now).await;
        let claimed = claim_one(&s, now).await;
        assert_eq!(claimed.spec.session, None, "the session is lost");
        let pack = claimed.spec.context_pack.expect("context pack");
        for flow in ["\"flow_id\":\"f1\"", "\"flow_id\":\"f2\""] {
            assert!(pack.contains(flow), "{flow} missing: {pack}");
        }
        let problem = pr_context["problem"].as_str().expect("problem");
        assert!(pack.contains(problem), "pr_context missing: {pack}");
    }

    #[tokio::test]
    async fn claim_records_whether_the_job_got_github_access() {
        let s = setup().await;
        let with = enqueue(&s.db).await;
        let now = Utc::now();
        installed(&s, now).await;
        claim_one(&s, now).await;
        assert!(
            s.db.jobs()
                .get(with)
                .await
                .expect("job")
                .github_access_granted
        );

        let s = setup().await;
        let without = enqueue(&s.db).await;
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/node-client/installation"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&s.github)
            .await;
        let claimed = claim_one(&s, now).await;
        assert!(claimed.spec.access.github.is_none());
        assert!(
            !s.db
                .jobs()
                .get(without)
                .await
                .expect("job")
                .github_access_granted
        );
    }

    #[tokio::test]
    async fn claim_skips_jobs_of_a_cancelled_analysis() {
        let s = setup().await;
        let job = enqueue(&s.db).await;
        let analysis = s.db.jobs().get(job).await.expect("job").analysis_id;
        s.db.analyses()
            .set_state(
                analysis,
                &contracts::api::analyses::AnalysisState::Cancelled,
                Utc::now(),
            )
            .await
            .expect("cancelled");
        let response = claim(&s.db, &s.config, &packs(&s), &request(), Utc::now())
            .await
            .expect("claim");
        assert!(
            response.claim.is_none(),
            "a cancelled analysis's job is never run"
        );
    }

    #[tokio::test]
    async fn jobs_carry_installation_token() {
        let capture = LogCapture::start();
        let s = setup().await;
        enqueue(&s.db).await;
        let now = Utc::now();
        installed(&s, now).await;
        let claimed = claim_one(&s, now).await;
        let github = claimed.spec.access.github.expect("installation token");
        assert_eq!(github.token.expose(), MINTED_TOKEN);
        assert!(
            claimed.spec.access.memcan.is_none(),
            "agents cannot read or write shared memory"
        );
        let logs = capture.text();
        assert!(!logs.contains(MINTED_TOKEN), "{logs}");
    }

    #[tokio::test]
    async fn jobs_carry_analysis_session() {
        let s = setup().await;
        let job = enqueue(&s.db).await;
        let analysis = s.db.jobs().get(job).await.expect("job").analysis_id;
        let now = Utc::now();
        installed(&s, now).await;
        s.db.analyses()
            .set_session(analysis, "thread-7", None)
            .await
            .expect("session");
        let claimed = claim_one(&s, now).await;
        assert_eq!(
            claimed.spec.session,
            Some(SessionRef {
                pull_request_id: 1,
                thread_id: "thread-7".to_owned(),
            })
        );
        let stored = s.db.jobs().get(job).await.expect("job");
        assert_eq!(
            stored.spec.session, None,
            "the session is set at claim, not stored"
        );
    }

    #[tokio::test]
    async fn dropped_session_is_not_resumed() {
        let s = setup().await;
        let job = enqueue(&s.db).await;
        let analysis = s.db.jobs().get(job).await.expect("job").analysis_id;
        s.db.analyses()
            .set_session(analysis, "thread-7", None)
            .await
            .expect("session");
        s.db.analyses().request_session_drop(1).await.expect("drop");
        let claimed = claim_one(&s, Utc::now()).await;
        assert_eq!(claimed.spec.session, None);
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
        Arc::get_mut(&mut s.config)
            .expect("unshared")
            .github
            .private_key_file = s._dir.path().join("missing.pem");
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
        let result = claim(&s.db, &s.config, &packs(&s), &request(), Utc::now()).await;
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
        let decrypted = github_tokens::load(&s.db, &s.config, job, 1)
            .await
            .expect("bound to its row")
            .expect("stored");
        assert_eq!(decrypted.expose_secret(), MINTED_TOKEN);

        github_tokens::sweep(&s.db, &s.config, now)
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
            github_tokens::sweep(&s.db, &s.config, now)
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
        github_tokens::sweep(&s.db, &s.config, now)
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

    /// Every way a job attempt ends revokes its token: results through the runner route and lease expiry through
    /// the next claim nudge the worker.
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
            let state = state_with(&s);
            let worker = tokio::spawn(github_tokens::run_worker(
                state.db.clone(),
                state.config.clone(),
            ));
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
                    claim(&state.db, &state.config, &no_packs(), &idle, after_lease)
                        .await
                        .expect("claim after the lease");
                }
            }
            assert!(
                revoked_soon(&state.db, job, 1).await,
                "{end:?}: revocation confirmed"
            );
            worker.abort();
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
            &packs(&s),
            &idle,
            claimed.lease_expires_at + TimeDelta::seconds(1),
        )
        .await
        .expect("claim after the lease");
        github_tokens::sweep(&s.db, &s.config, Utc::now())
            .await
            .expect("sweep");
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
        github_tokens::store(
            &s.db,
            &s.config,
            expired,
            1,
            &backlog_token("ghs_expired", now - TimeDelta::minutes(1)),
        )
        .await
        .expect("record");

        mount_revoke(&s.github, 204, 1).await;
        let start = tokio::spawn(github_tokens::run_worker(s.db.clone(), s.config.clone()));
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

    fn backlog_token(token: &str, expires_at: DateTime<Utc>) -> InstallationToken {
        InstallationToken {
            token: Secret::new(token),
            expires_at,
            repository: RepoRef {
                owner: "acme".to_owned(),
                name: "node-client".to_owned(),
            },
        }
    }

    /// Waits up to 5 s for the worker to confirm the revocation of `attempt`.
    async fn revoked_soon(db: &Db, job: JobId, attempt: u32) -> bool {
        for _ in 0..250 {
            if db
                .jobs()
                .github_token(job, attempt)
                .await
                .expect("read")
                .is_none()
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    /// After a restart the pipeline cancels the analysis of a running job whose runner is gone; no follower and
    /// no further claim exist, yet the worker's sweep ends the lease and revokes the token.
    #[tokio::test]
    async fn token_revoked_after_restart_and_cancel_without_runner() {
        let s = setup().await;
        let job = enqueue(&s.db).await;
        let now = Utc::now();
        installed(&s, now).await;
        mount_revoke(&s.github, 204, 1).await;
        let claimed = claim_one(&s, now).await;
        let analysis = s.db.jobs().get(job).await.expect("job").analysis_id;
        QueueExecutor::new(s.db.clone(), ProgressHub::default())
            .cancel_analysis(analysis)
            .await
            .expect("cancel analysis");
        github_tokens::sweep(
            &s.db,
            &s.config,
            claimed.lease_expires_at + TimeDelta::seconds(1),
        )
        .await
        .expect("sweep");
        assert_eq!(
            s.db.jobs().get(job).await.expect("job").state,
            contracts::job::state::JobState::Cancelled
        );
        assert_eq!(
            s.db.jobs().github_token(job, 1).await.expect("read"),
            None,
            "revoked and ciphertext removed"
        );
    }

    /// Slow GitHub answers end the claim within its budget, below the runner's 10 s request timeout.
    #[tokio::test]
    async fn claim_github_budget_below_runner_timeout() {
        assert!(CLAIM_GITHUB_BUDGET < Duration::from_secs(10));
        let s = setup().await;
        enqueue(&s.db).await;
        let now = Utc::now();
        let slow = Duration::from_millis(3_500);
        Mock::given(method("GET"))
            .and(path("/api/repos/acme/node-client/installation"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id": 4242}))
                    .set_delay(slow),
            )
            .mount(&s.github)
            .await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(minted(now))
                    .set_delay(slow),
            )
            .mount(&s.github)
            .await;
        let started = std::time::Instant::now();
        let result = claim(&s.db, &s.config, &packs(&s), &request(), now).await;
        let took = started.elapsed();
        assert!(
            matches!(result, Err(ClaimError::GithubTimeout)),
            "{result:?}"
        );
        assert!(
            took < CLAIM_GITHUB_BUDGET + Duration::from_millis(500),
            "{took:?}"
        );
    }

    /// Claims and results only nudge the worker; a slow revocation backlog never delays their answers.
    #[tokio::test]
    async fn claim_and_result_do_not_wait_for_revocation_backlog() {
        let s = setup().await;
        let first = enqueue(&s.db).await;
        let now = Utc::now();
        mount_installation(&s.github).await;
        mount_mint(&s.github, minted(now), 2).await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(3)))
            .mount(&s.github)
            .await;
        let backlog = enqueue(&s.db).await;
        // Attempts the claim below never reaches, so its own token row does not collide.
        for attempt in 7..=9 {
            github_tokens::store(
                &s.db,
                &s.config,
                backlog,
                attempt,
                &backlog_token(&format!("ghs_backlog_{attempt}"), now + TimeDelta::hours(1)),
            )
            .await
            .expect("backlog");
        }
        let claimed = claim_one(&s, now).await;
        let state = state_with(&s);
        let started = std::time::Instant::now();
        let response = crate::api::runner_router(state.clone())
            .oneshot(
                Request::post(RUNNER_RESULT.path.replace("{id}", &first.to_string()))
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", claimed.job_token.expose()),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&result(first, succeeded())).expect("json"),
                    ))
                    .expect("request"),
            )
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(started.elapsed() < Duration::from_secs(1), "result waited");

        let second = claim(&state.db, &state.config, &no_packs(), &request(), now)
            .await
            .expect("claim")
            .claim
            .expect("the backlog job");
        let idle = ClaimRequest {
            credentials: Vec::new(),
            ..request()
        };
        let started = std::time::Instant::now();
        claim(
            &state.db,
            &state.config,
            &no_packs(),
            &idle,
            second.lease_expires_at + TimeDelta::seconds(1),
        )
        .await
        .expect("claim after an expired lease");
        assert!(started.elapsed() < Duration::from_secs(1), "claim waited");
    }

    #[tokio::test]
    async fn sweep_stops_at_rate_limit() {
        let s = setup().await;
        let job = enqueue(&s.db).await;
        let now = Utc::now();
        for attempt in 1..=2 {
            github_tokens::store(
                &s.db,
                &s.config,
                job,
                attempt,
                &backlog_token(&format!("ghs_backlog_{attempt}"), now + TimeDelta::hours(1)),
            )
            .await
            .expect("backlog");
        }
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "30"))
            .expect(1)
            .mount(&s.github)
            .await;
        let sweep = github_tokens::sweep(&s.db, &s.config, now)
            .await
            .expect("sweep");
        assert_eq!(sweep.failed, 1);
        assert_eq!(sweep.rate_limited, Some(Some(Duration::from_secs(30))));
        assert_eq!(
            s.db.jobs()
                .github_tokens_to_revoke()
                .await
                .expect("list")
                .len(),
            2,
            "both stay listed"
        );
    }

    /// A token that differs from the request is never handed out; stored before its revocation, it survives a
    /// failed DELETE and a restart until GitHub confirms the revocation.
    #[tokio::test]
    async fn rejected_token_stored_until_revoked() {
        let capture = LogCapture::start();
        let s = setup().await;
        let job = enqueue(&s.db).await;
        let now = Utc::now();
        let mut write = minted(now);
        write["permissions"]["issues"] = json!("write");
        mount_installation(&s.github).await;
        mount_mint(&s.github, write, 1).await;
        let lease = {
            let _unavailable = Mock::given(method("DELETE"))
                .respond_with(ResponseTemplate::new(503))
                .expect(1)
                .mount_as_scoped(&s.github)
                .await;
            let result = claim(&s.db, &s.config, &packs(&s), &request(), now).await;
            assert!(
                matches!(
                    result,
                    Err(ClaimError::TokenRejected(Rejection::Permissions))
                ),
                "{result:?}"
            );
            s.db.jobs()
                .get(job)
                .await
                .expect("job")
                .lease_expires_at
                .expect("claimed")
        };
        assert!(
            s.db.jobs()
                .github_token(job, 1)
                .await
                .expect("read")
                .is_some(),
            "kept for revocation"
        );
        mount_revoke(&s.github, 204, 1).await;
        github_tokens::sweep(&s.db, &s.config, lease + TimeDelta::seconds(1))
            .await
            .expect("sweep after a restart");
        assert_eq!(s.db.jobs().github_token(job, 1).await.expect("read"), None);
        assert!(!capture.text().contains(MINTED_TOKEN));
    }

    #[tokio::test]
    async fn excessive_expiry_rejected_and_revoked() {
        let s = setup().await;
        let job = enqueue(&s.db).await;
        let now = Utc::now();
        let mut long = minted(now);
        long["expires_at"] = json!((now + TimeDelta::days(1)).to_rfc3339());
        mount_installation(&s.github).await;
        mount_mint(&s.github, long, 1).await;
        mount_revoke(&s.github, 204, 1).await;
        let result = claim(&s.db, &s.config, &packs(&s), &request(), now).await;
        assert!(
            matches!(result, Err(ClaimError::TokenRejected(Rejection::Expiry))),
            "{result:?}"
        );
        assert_eq!(
            s.db.jobs().github_token(job, 1).await.expect("read"),
            None,
            "revoked at once and confirmed"
        );
    }

    /// The claim's budget ends while the store of a received token waits for the database: the claim answers
    /// `github_unavailable`, yet the token is stored afterwards and revoked (accepted: after the lease by the
    /// sweep; rejected: at once).
    #[tokio::test]
    async fn token_received_after_budget_is_stored_then_revoked() {
        for rejected in [false, true] {
            let s = setup().await;
            let job = enqueue(&s.db).await;
            let now = Utc::now();
            let mut answer = minted(now);
            if rejected {
                answer["permissions"]["issues"] = json!("write");
            }
            mount_installation(&s.github).await;
            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(201)
                        .set_body_json(answer)
                        .set_delay(Duration::from_secs(1)),
                )
                .expect(1)
                .mount(&s.github)
                .await;
            mount_revoke(&s.github, 204, 1).await;
            // Once the mint request is in flight, take the database's only connection past the budget.
            let blocker = async {
                while !s
                    .github
                    .received_requests()
                    .await
                    .unwrap_or_default()
                    .iter()
                    .any(|request| request.method.as_str() == "POST")
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                let held = db::repo::analyses::testing::hold_connection(&s.db)
                    .await
                    .expect("hold");
                tokio::time::sleep(CLAIM_GITHUB_BUDGET + Duration::from_secs(1)).await;
                drop(held);
            };
            let claim_request = request();
            let packs = packs(&s);
            let (result, ()) = tokio::join!(
                claim(&s.db, &s.config, &packs, &claim_request, now),
                blocker
            );
            assert!(
                matches!(result, Err(ClaimError::GithubTimeout)),
                "rejected {rejected}: {result:?}"
            );
            let deleted = || async {
                s.github
                    .received_requests()
                    .await
                    .unwrap_or_default()
                    .iter()
                    .filter(|request| request.method.as_str() == "DELETE")
                    .count()
            };
            if rejected {
                assert!(revoked_soon(&s.db, job, 1).await);
                for _ in 0..250 {
                    if deleted().await == 1 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                assert_eq!(deleted().await, 1, "the rejected token was revoked at once");
            } else {
                let mut stored = false;
                for _ in 0..250 {
                    if s.db
                        .jobs()
                        .github_token(job, 1)
                        .await
                        .expect("read")
                        .is_some()
                    {
                        stored = true;
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                assert!(stored, "the received token is stored after the budget");
                assert_eq!(deleted().await, 0);
                let lease =
                    s.db.jobs()
                        .get(job)
                        .await
                        .expect("job")
                        .lease_expires_at
                        .expect("claimed");
                github_tokens::sweep(&s.db, &s.config, lease + TimeDelta::seconds(1))
                    .await
                    .expect("sweep after the unanswered claim's lease");
                assert_eq!(s.db.jobs().github_token(job, 1).await.expect("read"), None);
                assert_eq!(deleted().await, 1);
            }
        }
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
