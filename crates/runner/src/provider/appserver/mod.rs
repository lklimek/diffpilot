//! Codex 0.171 WebSocket JSON-RPC over the launcher's private relay; no upstream credentials.
mod config;
mod gateway;
mod rpc;
mod workspace;

pub use gateway::GatewayClient;

use super::{AttemptId, LlmProvider, ProviderCall, ProviderError, ProviderEvent, ProviderOutcome};
use contracts::{
    agent::JobKind,
    assessment::AssessmentMethod,
    job::{
        result::{JobFailureKind, SessionOutcome, SessionReport, Usage},
        spec::{JobSpec, JobTarget},
    },
    launcher::{LauncherAction, LauncherRequest},
    runtime::{AskPreflight, ContainerId, LastContext, PreflightState, SessionSeed, ThreadKind},
};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex as SyncMutex, MutexGuard, Weak},
    time::Duration,
};
use tokio::sync::{Mutex, mpsc, watch};

/// App-server provider; trait calls use the self-contained JobSpec routing, including Ask recovery.
#[derive(Clone)]
pub struct AppserverProvider {
    launcher: crate::launcher::Client,
    gateway: GatewayClient,
    skills_commit: String,
    reconciled: Arc<std::sync::atomic::AtomicBool>,
    reconcile_gate: Arc<Mutex<()>>,
    lifecycle: Arc<SyncMutex<Lifecycle>>,
    cancellations: Arc<SyncMutex<HashMap<AttemptId, watch::Sender<bool>>>>,
}

#[derive(Default)]
struct Lifecycle {
    active: HashMap<AttemptId, Active>,
    retiring: HashSet<i64>,
}

struct Retirement<'a> {
    provider: &'a AppserverProvider,
    prs: &'a [i64],
}

impl Drop for Retirement<'_> {
    fn drop(&mut self) {
        self.provider
            .lifecycle()
            .retiring
            .retain(|pr| !self.prs.contains(pr));
    }
}

struct Active {
    container: ContainerId,
    // This marker is owned across all fallback attempts and dropped with their future.
    owner: Weak<()>,
}

impl Active {
    fn is_live(&self) -> bool {
        self.owner.strong_count() != 0
    }

    fn matches_pr(&self, pull_requests: &[i64]) -> bool {
        matches!(self.container, ContainerId::Pr { pull_request_id, .. } if pull_requests.contains(&pull_request_id))
    }
}

struct CancellationRegistration<'a> {
    registry: &'a SyncMutex<HashMap<AttemptId, watch::Sender<bool>>>,
    job: AttemptId,
}

impl Drop for CancellationRegistration<'_> {
    fn drop(&mut self) {
        self.registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.job);
    }
}

fn launcher_failure(error: crate::launcher::Error, step: &'static str) -> ProviderError {
    let kind = match error {
        crate::launcher::Error::InvalidTemplate
        | crate::launcher::Error::Remote(contracts::launcher::LauncherFailure::InvalidTemplate) => {
            JobFailureKind::Configuration
        }
        _ => JobFailureKind::WorkerUnavailable,
    };
    tracing::warn!(
        method = "launcher",
        step,
        ?kind,
        "launcher control operation failed"
    );
    runtime(kind)
}

fn runtime(kind: JobFailureKind) -> ProviderError {
    ProviderError::Runtime { kind }
}

fn fault(kind: JobFailureKind, method: &str, step: &str) -> ProviderError {
    tracing::warn!(method, step, ?kind, "app-server operation failed");
    runtime(kind)
}

fn container(spec: &JobSpec) -> Result<ContainerId, ProviderError> {
    if (spec.kind == JobKind::Ask) != spec.working_session.is_some() {
        return Err(fault(
            JobFailureKind::Configuration,
            "execute_job",
            "working session routing",
        ));
    }
    if spec.payer.0.trim().is_empty() {
        return Err(fault(
            JobFailureKind::CredentialMissing,
            "execute_job",
            "missing payer",
        ));
    }
    Ok(match (&spec.target, spec.kind) {
        (JobTarget::Dashboard { .. }, JobKind::JudgeWork) => ContainerId::Judge {
            payer: spec.payer.clone(),
        },
        (JobTarget::PullRequest(target), kind) if kind != JobKind::JudgeWork => ContainerId::Pr {
            pull_request_id: target.pull_request_id,
            payer: spec.payer.clone(),
        },
        (JobTarget::PullRequestComments(target), JobKind::VerifyComments | JobKind::Ask) => {
            ContainerId::Pr {
                pull_request_id: target.pull_request_id,
                payer: spec.payer.clone(),
            }
        }
        (JobTarget::PullRequestAsk(target), JobKind::Ask)
            if spec.session.is_none()
                && spec.access.memcan.is_none()
                && spec
                    .working_session
                    .as_ref()
                    .and_then(|session| session.kind)
                    == Some(ThreadKind::Verify) =>
        {
            ContainerId::Pr {
                pull_request_id: target.pull_request_id,
                payer: spec.payer.clone(),
            }
        }
        _ => {
            return Err(fault(
                JobFailureKind::Configuration,
                "execute_job",
                "container target",
            ));
        }
    })
}

impl AppserverProvider {
    /// The launcher socket, keys-only gateway client and pinned image skills revision.
    pub fn new(
        launcher: crate::launcher::Client,
        gateway: GatewayClient,
        skills_commit: String,
    ) -> Self {
        Self {
            launcher,
            gateway,
            skills_commit,
            reconciled: Arc::default(),
            reconcile_gate: Arc::default(),
            lifecycle: Arc::default(),
            cancellations: Arc::default(),
        }
    }

    fn lifecycle(&self) -> MutexGuard<'_, Lifecycle> {
        self.lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Startup gate: stop managed workers and retire their local virtual keys before accepting jobs.
    pub async fn reconcile(&self, retired_prs: &[i64]) -> Result<(), ProviderError> {
        let _guard = self.reconcile_gate.lock().await;
        if self.reconciled.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        self.reconcile_locked(retired_prs).await
    }

    async fn reconcile_locked(&self, retired_prs: &[i64]) -> Result<(), ProviderError> {
        let workers = self
            .launcher
            .reconcile(retired_prs)
            .await
            .map_err(|error| launcher_failure(error, "startup reconciliation"))?;
        for worker in workers {
            self.gateway.revoke(&worker).await?;
        }
        self.reconciled
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    async fn control(&self, spec: &JobSpec, action: LauncherAction) -> Result<(), ProviderError> {
        let step = match &action {
            LauncherAction::Ensure { .. } => "ensure",
            LauncherAction::Stop { .. } => "stop",
            _ => "control",
        };
        self.launcher
            .call(LauncherRequest {
                id: format!("{}-{}", spec.job_id.0, spec.attempt),
                action,
            })
            .await
            .map_err(|error| launcher_failure(error, step))?;
        Ok(())
    }

    async fn fetch(
        &self,
        spec: &JobSpec,
        worker: &ContainerId,
        events: &mpsc::Sender<ProviderEvent>,
    ) -> Result<(), ProviderError> {
        use contracts::launcher::{DependencySources, LauncherFailure};
        let response = self
            .launcher
            .call(LauncherRequest {
                id: format!("fetch-{}", spec.job_id.0),
                action: LauncherAction::Fetch {
                    container: worker.clone(),
                },
            })
            .await;
        let outcome = match response {
            Ok(response) => response.fetch,
            Err(crate::launcher::Error::Remote(LauncherFailure::FetchUnavailable)) => {
                Some(DependencySources::Unavailable)
            }
            Err(error) => return Err(launcher_failure(error, "fetch")),
        };
        let summary = match outcome {
            Some(DependencySources::ResolvedWithoutLock) => {
                Some("Dependency sources resolved without a lock file")
            }
            Some(DependencySources::Unavailable) => {
                Some("Dependency sources unavailable for this job")
            }
            _ => None,
        };
        if let Some(summary) = summary {
            let _ = events.try_send(ProviderEvent::Activity(contracts::job::event::JobEvent {
                seq: 0,
                at: contracts::time::Timestamp::now(),
                activity: contracts::job::event::ActivityKind::Command,
                summary: summary.into(),
            }));
        }
        Ok(())
    }

    async fn stop(&self, spec: &JobSpec, worker: &ContainerId) -> Result<(), ProviderError> {
        self.control(
            spec,
            LauncherAction::Stop {
                container: worker.clone(),
            },
        )
        .await
    }

    /// Run a self-contained container job. The caller prepares its mirror and retains this future through cleanup.
    /// Session routing, frozen input reset, timeout and cancellation are owned by the provider.
    pub async fn execute_job(
        &self,
        spec: &JobSpec,
        events: mpsc::Sender<ProviderEvent>,
    ) -> Result<ProviderOutcome, ProviderError> {
        let attempt = AttemptId::from(spec);
        let (cancel, mut cancelled) = watch::channel(false);
        {
            let mut registry = self
                .cancellations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if registry.keys().any(|id| id.job == attempt.job) {
                return Err(fault(
                    JobFailureKind::Configuration,
                    "execute_job",
                    "duplicate job registration",
                ));
            }
            registry.insert(attempt, cancel);
        }
        let _registration = CancellationRegistration {
            registry: &self.cancellations,
            job: attempt,
        };
        let worker = container(spec)?;
        let owner = Arc::new(());
        self.admit(attempt, worker, Arc::downgrade(&owner), &cancelled)
            .await?;
        let mut clean = true;
        let outcome = self
            .execute_attempts(spec, events, &mut cancelled, &mut clean)
            .await;
        if clean {
            self.lifecycle().active.remove(&attempt);
        }
        drop(owner);
        outcome
    }

    async fn admit(
        &self,
        job: AttemptId,
        worker: ContainerId,
        owner: Weak<()>,
        cancelled: &watch::Receiver<bool>,
    ) -> Result<(), ProviderError> {
        if !self.reconciled.load(std::sync::atomic::Ordering::Acquire) {
            self.reconcile(&[]).await?;
        }
        let orphans: Vec<_> = self
            .lifecycle()
            .active
            .iter()
            .filter(|(_, active)| !active.is_live())
            .map(|(job, _)| *job)
            .collect();
        // Fencing one abandoned container leaves the other admission and cancellation paths free.
        futures::future::join_all(orphans.into_iter().map(|job| async move {
            let result = tokio::time::timeout(Duration::from_secs(5), self.reap(job, false)).await;
            if !matches!(result, Ok(Ok(()))) {
                tracing::warn!(job = %job.job, attempt = job.number, "could not fence an orphaned worker before admission");
            }
        }))
        .await;
        if *cancelled.borrow() {
            return Err(ProviderError::Cancelled);
        }
        let mut lifecycle = self.lifecycle();
        if matches!(&worker, ContainerId::Pr { pull_request_id, .. } if lifecycle.retiring.contains(pull_request_id))
            || lifecycle
                .active
                .iter()
                .any(|(id, active)| id.job == job.job || active.container == worker)
        {
            return Err(fault(
                JobFailureKind::WorkerUnavailable,
                "execute_job",
                "container already owned by an attempt",
            ));
        }
        lifecycle.active.insert(
            job,
            Active {
                container: worker,
                owner,
            },
        );
        Ok(())
    }

    async fn reap(&self, job: AttemptId, retirement: bool) -> Result<(), ProviderError> {
        let (worker, owner) = {
            let mut lifecycle = self.lifecycle();
            let Lifecycle { active, retiring } = &mut *lifecycle;
            let Some(attempt) = active.get_mut(&job) else {
                return Ok(());
            };
            if attempt.is_live()
                || (!retirement
                    && matches!(&attempt.container, ContainerId::Pr { pull_request_id, .. } if retiring.contains(pull_request_id)))
            {
                return Ok(());
            }
            let owner = Arc::new(());
            attempt.owner = Arc::downgrade(&owner);
            (attempt.container.clone(), owner)
        };
        let stop = async {
            self.launcher
                .call(LauncherRequest {
                    id: format!("orphan-{}-{}", job.job, job.number),
                    action: LauncherAction::Stop {
                        container: worker.clone(),
                    },
                })
                .await
                .map_err(|error| launcher_failure(error, "orphan stop"))
        };
        let (stopped, revoked) = tokio::join!(stop, self.gateway.revoke(&worker));
        stopped?;
        revoked?;
        self.lifecycle().active.remove(&job);
        drop(owner);
        Ok(())
    }

    async fn execute_attempts(
        &self,
        spec: &JobSpec,
        events: mpsc::Sender<ProviderEvent>,
        cancelled: &mut watch::Receiver<bool>,
        clean: &mut bool,
    ) -> Result<ProviderOutcome, ProviderError> {
        let session = match spec.kind {
            JobKind::VerifyComments | JobKind::JudgeWork => None,
            _ => spec
                .working_session
                .as_ref()
                .and_then(|w| w.thread_id.as_deref())
                .or_else(|| spec.session.as_ref().map(|s| s.thread_id.as_str())),
        };
        let mut effective = spec.clone();
        if (session.is_none()
            || spec
                .working_session
                .as_ref()
                .is_some_and(|w| w.thread_id.is_none()))
            && let Some(pack) = &spec.context_pack
        {
            effective.prompt = format!("{pack}\n\n{}", spec.prompt);
        }
        match self
            .perform(&effective, session, events.clone(), cancelled, clean)
            .await
        {
            Err(ProviderError::SessionUnavailable) => {
                let mut fresh = spec.clone();
                if let Some(working) = &mut fresh.working_session {
                    working.thread_id = None;
                    working.preflight = None;
                }
                if let Some(pack) = &spec.context_pack {
                    fresh.prompt = format!("{pack}\n\n{}", spec.prompt);
                }
                if spec
                    .working_session
                    .as_ref()
                    .is_some_and(|working| working.thread_id.is_some())
                    && let Some(parent) = &spec.session
                {
                    match self
                        .perform(
                            &fresh,
                            Some(&parent.thread_id),
                            events.clone(),
                            cancelled,
                            clean,
                        )
                        .await
                    {
                        Err(ProviderError::SessionUnavailable) => {}
                        result => return result,
                    }
                }
                fresh.session = None;
                let mut outcome = self.perform(&fresh, None, events, cancelled, clean).await?;
                if let Some(session) = &mut outcome.runtime_session {
                    session.outcome = SessionOutcome::StartedAfterLoss;
                }
                Ok(outcome)
            }
            result => result,
        }
    }

    async fn perform(
        &self,
        spec: &JobSpec,
        session: Option<&str>,
        events: mpsc::Sender<ProviderEvent>,
        cancelled: &mut watch::Receiver<bool>,
        clean: &mut bool,
    ) -> Result<ProviderOutcome, ProviderError> {
        if *cancelled.borrow() {
            return Err(ProviderError::Cancelled);
        }
        let worker = container(spec)?;
        let mut rpc = None;
        let mut key = None;
        let mut checkpoint = None;
        let cap = spec
            .limits
            .max_runtime_secs
            .min(contracts::job::spec::JobLimits::MAX_RUNTIME_SECS);
        let mut result = tokio::time::timeout(Duration::from_secs(u64::from(cap)), async {
            self.control(
                spec,
                LauncherAction::Ensure {
                    container: worker.clone(),
                    memcan: if spec.kind == JobKind::JudgeWork {
                        None
                    } else {
                        spec.access.memcan.clone()
                    },
                },
            )
            .await?;
            if *cancelled.borrow() {
                return Err(ProviderError::Cancelled);
            }
            let minted = self.gateway.mint(&worker).await?;
            let scrubber = crate::scrub::Scrubber::new(
                std::iter::once(minted.key.expose())
                    .chain(spec.access.github.iter().map(|v| v.token.expose()))
                    .chain(spec.access.memcan.iter().map(|v| v.token.expose())),
            );
            key = Some(minted);
            let socket = self
                .launcher
                .attach(format!("attach-{}", spec.job_id.0), worker.clone())
                .await
                .map_err(|error| launcher_failure(error, "attach"))?;
            rpc = Some(rpc::Rpc::connect(socket, events.clone(), scrubber).await?);
            let rpc = rpc.as_mut().ok_or_else(|| {
                fault(
                    JobFailureKind::WorkerLost,
                    "execute_job",
                    "missing attached RPC",
                )
            })?;
            let minted = key.as_ref().ok_or_else(|| {
                fault(
                    JobFailureKind::WorkerLost,
                    "execute_job",
                    "missing minted key",
                )
            })?;
            rpc.mkdir("/run/dp").await?;
            rpc.secret("/run/dp/model-key", minted.key.expose().as_bytes())
                .await?;
            if spec.kind != JobKind::JudgeWork {
                rpc.secret(
                    "/run/dp/github-token",
                    spec.access
                        .github
                        .as_ref()
                        .map_or("", |v| v.token.expose())
                        .as_bytes(),
                )
                .await?;
            }
            workspace::prepare(rpc, spec).await?;
            if spec.kind != JobKind::JudgeWork {
                // The first Ensure runs before a checkout exists; fetch again against the frozen tree.
                self.fetch(spec, &worker, &events).await?;
            }
            self.run_attached(rpc, spec, session, &minted.id, cancelled, &mut checkpoint)
                .await
        })
        .await
        .unwrap_or_else(|_| {
            tracing::warn!(
                method = "execute_job",
                step = "runtime cap",
                "container job exceeded runtime limit"
            );
            Err(ProviderError::Timeout)
        });
        if *cancelled.borrow() {
            result = Err(ProviderError::Cancelled);
        }
        // Stop uncertain turns before any new work. Cleanup is bounded and never needs model commands.
        let must_stop = result.is_err() || spec.kind == JobKind::JudgeWork;
        let mut cleanup = if must_stop {
            self.stop(spec, &worker).await
        } else {
            Ok(())
        };
        if let Err(error) = self.gateway.revoke(&worker).await
            && cleanup.is_ok()
        {
            cleanup = Err(error);
        }
        if !must_stop && let Some(rpc) = &mut rpc {
            let scrubbed = tokio::time::timeout(Duration::from_secs(15), async {
                rpc.secret("/run/dp/model-key", b"").await?;
                rpc.secret("/run/dp/github-token", b"").await?;
                Ok::<(), ProviderError>(())
            })
            .await
            .unwrap_or_else(|_| {
                Err(fault(
                    JobFailureKind::WorkerLost,
                    "cleanup",
                    "15 second token scrub timeout",
                ))
            });
            if let Err(error) = scrubbed
                && cleanup.is_ok()
            {
                cleanup = Err(error);
            }
        }
        if cleanup.is_err() {
            tracing::warn!(
                method = "cleanup",
                step = "retire job access",
                "worker cleanup failed"
            );
            if self.stop(spec, &worker).await.is_err() {
                tracing::error!(
                    method = "launcher/stop",
                    step = "best effort cleanup",
                    "worker may still be running after cleanup stop failed"
                );
            }
        }
        drop(rpc);
        *clean = cleanup.is_ok();
        match cleanup.and(result) {
            Ok(outcome) => {
                let _ = tokio::time::timeout(
                    Duration::from_secs(1),
                    events.send(ProviderEvent::OutputReady(outcome.output.clone())),
                )
                .await;
                Ok(outcome)
            }
            Err(error) => {
                tracing::warn!(method = "execute_job", step = "attempt result", error = %error, "container job attempt failed");
                let error = match checkpoint {
                    Some(session) => ProviderError::Checkpoint {
                        error: Box::new(error),
                        session: Box::new(session),
                    },
                    None => error,
                };
                if let Some(key) = key
                    && let Ok(metered) = self.gateway.usage(&key.id).await
                    && metered.user == spec.payer
                    && (metered.usage.requests.is_some_and(|n| n > 0)
                        || metered.usage.input_tokens > 0
                        || metered.usage.output_tokens > 0)
                {
                    return Err(ProviderError::Metered {
                        error: Box::new(error),
                        usage: metered.usage,
                    });
                }
                Err(error)
            }
        }
    }

    async fn thread(
        &self,
        rpc: &mut rpc::Rpc,
        spec: &JobSpec,
        kind: ThreadKind,
        method: &str,
        thread: Option<&str>,
    ) -> Result<String, ProviderError> {
        workspace::hygiene(rpc, spec).await?;
        let mut params = config::thread_params(
            kind,
            &spec.model,
            config::model_window(&spec.model.model)?,
            spec.access.memcan.as_ref(),
        );
        if let Some(thread) = thread {
            params["threadId"] = json!(thread);
        }
        if method == "thread/start" {
            params["ephemeral"] = json!(
                matches!(kind, ThreadKind::Verify | ThreadKind::Judge) && spec.kind != JobKind::Ask
            );
        }
        let result = rpc.request(method, params).await?;
        let thread = result["thread"]["id"]
            .as_str()
            .ok_or_else(|| fault(JobFailureKind::WorkerLost, method, "missing thread.id"))?
            .to_owned();
        let history = result["thread"]["turns"]
            .as_array()
            .is_some_and(|v| !v.is_empty());
        let fork_resume = method == "thread/resume" && result["thread"]["forkedFromId"].is_string();
        rpc.restored_usage(&thread, history, fork_resume).await?;
        Ok(thread)
    }

    async fn run_attached(
        &self,
        rpc: &mut rpc::Rpc,
        spec: &JobSpec,
        session: Option<&str>,
        key_id: &str,
        cancelled: &mut watch::Receiver<bool>,
        checkpoint: &mut Option<SessionReport>,
    ) -> Result<ProviderOutcome, ProviderError> {
        let kind = spec
            .working_session
            .as_ref()
            .map(config::working_kind)
            .transpose()?
            .unwrap_or_else(|| ThreadKind::for_job(spec.kind));
        rpc.kind = kind;
        let catalog = rpc
            .request("model/list", json!({"includeHidden":true,"limit":100}))
            .await
            .map_err(|error| match error {
                ProviderError::Runtime {
                    kind: JobFailureKind::ProviderFailed,
                } => runtime(JobFailureKind::ModelUnavailable),
                other => other,
            })?;
        if !catalog["data"]
            .as_array()
            .is_some_and(|models| models.iter().any(|m| m["model"] == spec.model.model))
        {
            return Err(fault(
                JobFailureKind::ModelUnavailable,
                "model/list",
                "selected model absent",
            ));
        }
        let mut preflight = None;
        let fork = spec.kind == JobKind::Ask
            && spec
                .working_session
                .as_ref()
                .is_some_and(|w| w.thread_id.is_none())
            && session.is_some();
        let thread = if fork {
            let source_kind = ThreadKind::Analytic;
            let parent = self
                .thread(rpc, spec, source_kind, "thread/resume", session)
                .await?;
            self.thread(rpc, spec, kind, "thread/fork", Some(&parent))
                .await?
        } else if session.is_some() {
            self.thread(rpc, spec, kind, "thread/resume", session)
                .await?
        } else {
            self.thread(rpc, spec, kind, "thread/start", None).await?
        };
        let mut usage = Usage::default();
        if let Some(working) = &spec.working_session {
            let seed = if fork {
                SessionSeed::Fork
            } else if working.thread_id.is_some() {
                working
                    .preflight
                    .as_ref()
                    .ok_or_else(|| {
                        tracing::warn!(
                            method = "thread/resume",
                            step = "working preflight",
                            "working thread has no bound preflight"
                        );
                        runtime(JobFailureKind::Configuration)
                    })?
                    .seed
            } else {
                SessionSeed::ContextPack
            };
            let mut recovered = working.clone();
            if fork {
                let parent = session
                    .and_then(|parent| rpc.last.get(parent))
                    .copied()
                    .ok_or_else(|| {
                        tracing::warn!(
                            method = "thread/fork",
                            step = "parent context",
                            "analytic parent has no restored context"
                        );
                        runtime(JobFailureKind::Configuration)
                    })?;
                recovered.parent_context = Some(LastContext {
                    tokens: parent,
                    generation: working.analytic_generation,
                });
                recovered.preflight = None;
            }
            let compact = config::needs_compaction(&recovered, seed)?;
            *checkpoint = Some(SessionReport {
                thread_id: thread.clone(),
                outcome: if session.is_some() {
                    SessionOutcome::Resumed
                } else {
                    SessionOutcome::Started
                },
                last_context: rpc.last.get(&thread).map(|tokens| {
                    Box::new(LastContext {
                        tokens: *tokens,
                        generation: working.analytic_generation,
                    })
                }),
                preflight: Some(Box::new(AskPreflight {
                    seed,
                    generation: working.analytic_generation,
                    state: if compact {
                        PreflightState::Pending
                    } else {
                        recovered
                            .preflight
                            .as_ref()
                            .map_or(PreflightState::NotNeeded, |p| p.state)
                    },
                })),
            });
            let state = if compact {
                // Fork initialization materializes inherited history (0.171 core/src/session/mod.rs:1883-1954).
                self.thread(rpc, spec, kind, "thread/resume", Some(&thread))
                    .await?;
                let (delta, _) = self.turn(rpc, spec, &thread, true, cancelled).await?;
                usage = usage.checked_add(delta).map_err(|_| {
                    fault(
                        JobFailureKind::ProviderFailed,
                        "execute_job",
                        "usage sum overflow",
                    )
                })?;
                if let Some(report) = checkpoint {
                    if let Some(preflight) = &mut report.preflight {
                        preflight.state = PreflightState::Done;
                    }
                    report.last_context = rpc.last.get(&thread).map(|tokens| {
                        Box::new(LastContext {
                            tokens: *tokens,
                            generation: working.analytic_generation,
                        })
                    });
                }
                self.thread(rpc, spec, kind, "thread/resume", Some(&thread))
                    .await?;
                PreflightState::Done
            } else if recovered
                .preflight
                .as_ref()
                .is_some_and(|p| p.state == PreflightState::Done)
            {
                PreflightState::Done
            } else {
                PreflightState::NotNeeded
            };
            preflight = Some(Box::new(AskPreflight {
                seed,
                generation: working.analytic_generation,
                state,
            }));
        }
        // New starts have no rollout before their first turn; ephemeral threads never have one.
        // 0.171 app-server/tests/suite/v2/thread_resume.rs:516-566.
        let mut answer = spec.clone();
        if session.is_none() && !spec.skills.is_empty() {
            let mut files = Vec::new();
            for skill in &spec.skills {
                if !matches!(
                    skill.as_str(),
                    "severity"
                        | "report-format"
                        | "coding-best-practices"
                        | "review-changes"
                        | "git-and-github"
                        | "check-pr-comments"
                ) {
                    return Err(fault(
                        JobFailureKind::Configuration,
                        "turn/start",
                        "unknown required skill",
                    ));
                }
                files.push(format!("/etc/codex/skills/{skill}/SKILL.md"));
            }
            answer.prompt = format!(
                "Read the required skills once for this new session: {}. Apply them throughout this session.\n\n{}",
                files.join(", "),
                spec.prompt
            );
        }
        let (delta, message) = self.turn(rpc, &answer, &thread, false, cancelled).await?;
        usage = usage.checked_add(delta).map_err(|_| {
            fault(
                JobFailureKind::ProviderFailed,
                "execute_job",
                "usage sum overflow",
            )
        })?;
        let key_usage = self.reconcile_usage(spec, key_id, usage).await?;
        usage.requests = key_usage.requests;
        let output = if spec.flow_file.is_some() {
            Value::String(rpc.read_output("/work/out/flow.md", 1024 * 1024).await?)
        } else if spec
            .assessment
            .as_ref()
            .is_some_and(|run| run.method == AssessmentMethod::ClaudiusHost)
        {
            let text = rpc
                .read_output("/work/out/report.json", 4 * 1024 * 1024)
                .await?;
            serde_json::from_str(&text).map_err(|_| {
                tracing::warn!(
                    method = "fs/readFile",
                    step = "report JSON",
                    "report output is not JSON"
                );
                ProviderError::OutputNotJson
            })?
        } else {
            serde_json::from_str(message.as_deref().ok_or_else(|| {
                tracing::warn!(
                    method = "turn/completed",
                    step = "final message",
                    "completed turn has no final message"
                );
                ProviderError::OutputNotJson
            })?)
            .map_err(|_| {
                tracing::warn!(
                    method = "turn/completed",
                    step = "answer JSON",
                    "final answer is not JSON"
                );
                ProviderError::OutputNotJson
            })?
        };
        let generation = spec
            .working_session
            .as_ref()
            .map_or(0, |w| w.analytic_generation);
        let ephemeral =
            kind == ThreadKind::Judge || (kind == ThreadKind::Verify && spec.kind != JobKind::Ask);
        let runtime_session = (!ephemeral).then_some(SessionReport {
            thread_id: thread.clone(),
            outcome: if session.is_some() {
                SessionOutcome::Resumed
            } else {
                SessionOutcome::Started
            },
            last_context: rpc.last.get(&thread).map(|tokens| {
                Box::new(LastContext {
                    tokens: *tokens,
                    generation,
                })
            }),
            preflight,
        });
        *checkpoint = runtime_session.clone();
        Ok(ProviderOutcome {
            runtime_session,
            output: rpc.scrubber.value(output),
            session_id: (!ephemeral).then_some(thread),
            session_turns: None,
            usage,
            skills_commit: Some(self.skills_commit.clone()),
        })
    }

    async fn reconcile_usage(
        &self,
        spec: &JobSpec,
        key_id: &str,
        delta: Usage,
    ) -> Result<Usage, ProviderError> {
        let mut observed = Usage::default();
        for attempt in 0..20 {
            let metered = self.gateway.usage(key_id).await?;
            if metered.user != spec.payer {
                return Err(fault(
                    JobFailureKind::ProviderFailed,
                    "gateway/usage",
                    "payer mismatch",
                ));
            }
            let usage = metered.usage;
            observed = usage;
            if (
                usage.input_tokens,
                usage.cached_input_tokens,
                usage.output_tokens,
            ) == (
                delta.input_tokens,
                delta.cached_input_tokens,
                delta.output_tokens,
            ) {
                return Ok(usage);
            }
            if attempt < 19 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        tracing::warn!(method = "gateway/usage", step = "reconcile", expected = ?delta, actual = ?observed, "usage reconciliation mismatch");
        Err(runtime(JobFailureKind::ProviderFailed))
    }

    async fn turn(
        &self,
        rpc: &mut rpc::Rpc,
        spec: &JobSpec,
        thread: &str,
        compact: bool,
        cancelled: &mut watch::Receiver<bool>,
    ) -> Result<(Usage, Option<String>), ProviderError> {
        if *cancelled.borrow() {
            return Err(ProviderError::Cancelled);
        }
        let baseline = *rpc.totals.get(thread).ok_or_else(|| {
            fault(
                JobFailureKind::WorkerLost,
                "turn/start",
                "missing baseline usage",
            )
        })?;
        rpc.turn = rpc::TurnLog {
            thread: thread.into(),
            ..Default::default()
        };
        let mut params = json!({"threadId":thread});
        let method = if compact {
            "thread/compact/start"
        } else {
            let mut input = vec![json!({"type":"text","text":spec.prompt})];
            let host = spec
                .assessment
                .as_ref()
                .is_some_and(|r| r.method == AssessmentMethod::ClaudiusHost);
            if host {
                input.push(json!({"type":"skill","name":"check-pr-comments","path":"/etc/codex/skills/check-pr-comments/SKILL.md"}));
            }
            params["input"] = json!(input);
            params["model"] = json!(spec.model.model);
            params["effort"] = json!(spec.model.reasoning_effort);
            if spec.flow_file.is_none() && !host {
                params["outputSchema"] = spec.output_schema.clone();
            }
            "turn/start"
        };
        let started = rpc.request(method, params).await?;
        let turn_id = started
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        loop {
            if let Some(completed) = &rpc.turn.completed {
                if turn_id
                    .as_ref()
                    .is_some_and(|id| completed["id"].as_str() != Some(id))
                {
                    return Err(fault(
                        JobFailureKind::WorkerLost,
                        method,
                        "completed turn id mismatch",
                    ));
                }
                match completed["status"].as_str() {
                    Some("completed") => {}
                    Some("interrupted") => return Err(ProviderError::Cancelled),
                    _ => {
                        tracing::warn!(method, step = "turn/completed", "app-server turn failed");
                        return Err(rpc::failure(&completed["error"]));
                    }
                }
                if !rpc.turn.updated {
                    return Err(fault(
                        JobFailureKind::ProviderFailed,
                        method,
                        "completed without token usage update",
                    ));
                }
                let end = *rpc.totals.get(thread).ok_or_else(|| {
                    fault(
                        JobFailureKind::WorkerLost,
                        method,
                        "missing final total usage",
                    )
                })?;
                let delta = end.delta_from(baseline).map_err(|_| {
                    fault(
                        JobFailureKind::ProviderFailed,
                        method,
                        "invalid turn usage delta",
                    )
                })?;
                return Ok((delta, rpc.turn.output.take()));
            }
            tokio::select! {
                result = rpc.next_notification() => result?,
                _ = cancelled.changed() => {
                    // Interrupt on the same subscribed connection; the enclosing failure path stops the worker.
                    if !matches!(tokio::time::timeout(Duration::from_secs(5), rpc.request("turn/interrupt", json!({"threadId":thread,"turnId":turn_id}))).await, Ok(Ok(_))) {
                        tracing::warn!(method = "turn/interrupt", step = "cancel", "turn interrupt was not acknowledged; stopping worker");
                    }
                    return Err(ProviderError::Cancelled);
                }
            }
        }
    }
}

impl LlmProvider for AppserverProvider {
    fn run<'a>(
        &'a self,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(self.execute_job(&call.job_spec, events))
    }
    fn resume<'a>(
        &'a self,
        _session_id: &'a str,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(self.execute_job(&call.job_spec, events))
    }
    fn retire<'a>(&'a self, pull_requests: &'a [i64]) -> BoxFuture<'a, Result<(), ProviderError>> {
        Box::pin(async move {
            if pull_requests.is_empty() {
                return Ok(());
            }
            let (_retirement, orphans) = {
                let mut lifecycle = self.lifecycle();
                if pull_requests
                    .iter()
                    .any(|pr| lifecycle.retiring.contains(pr))
                {
                    return Err(runtime(JobFailureKind::WorkerUnavailable));
                }
                let mut orphans = Vec::new();
                for (job, worker) in lifecycle
                    .active
                    .iter()
                    .filter(|(_, worker)| worker.matches_pr(pull_requests))
                {
                    if worker.is_live() {
                        return Err(runtime(JobFailureKind::WorkerUnavailable));
                    }
                    orphans.push(*job);
                }
                lifecycle.retiring.extend(pull_requests.iter().copied());
                (
                    Retirement {
                        provider: self,
                        prs: pull_requests,
                    },
                    orphans,
                )
            };
            for job in orphans {
                self.reap(job, true).await?;
            }
            let response = self
                .launcher
                .call(LauncherRequest {
                    id: "retire-list".into(),
                    action: LauncherAction::List,
                })
                .await
                .map_err(|e| launcher_failure(e, "retirement list"))?;
            for worker in response.workers {
                if !matches!(worker.container, ContainerId::Pr { pull_request_id, .. } if pull_requests.contains(&pull_request_id))
                {
                    continue;
                }
                self.gateway.revoke(&worker.container).await?;
                self.launcher
                    .call(LauncherRequest {
                        id: "retire-remove".into(),
                        action: LauncherAction::Remove {
                            container: worker.container,
                        },
                    })
                    .await
                    .map_err(|e| launcher_failure(e, "retirement remove"))?;
            }
            Ok(())
        })
    }
    fn cancel(&self, job: AttemptId) -> BoxFuture<'_, Result<(), ProviderError>> {
        Box::pin(async move {
            if let Some(cancel) = self
                .cancellations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&job)
            {
                cancel.send_replace(true);
            }
            Ok(())
        })
    }

    fn reap_abandoned(&self, job: AttemptId) -> BoxFuture<'_, Result<(), ProviderError>> {
        Box::pin(self.reap(job, false))
    }
}

#[cfg(test)]
mod tests;
