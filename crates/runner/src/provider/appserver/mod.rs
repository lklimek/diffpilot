//! Codex 0.171 WebSocket JSON-RPC over the launcher's private relay; no upstream credentials.
mod config;
mod gateway;
mod rpc;
mod workspace;

pub use gateway::GatewayClient;

use super::{LlmProvider, ProviderCall, ProviderError, ProviderEvent, ProviderOutcome};
use contracts::{
    agent::JobKind,
    assessment::AssessmentMethod,
    ids::JobId,
    job::{
        result::{JobFailureKind, SessionOutcome, SessionReport, Usage},
        spec::{JobSpec, JobTarget},
    },
    launcher::{LauncherAction, LauncherRequest},
    runtime::{AskPreflight, ContainerId, LastContext, PreflightState, SessionSeed, ThreadKind},
};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc, watch};

/// App-server provider; clones share the single-turn gate and cancellation state.
#[derive(Clone)]
pub struct AppserverProvider {
    launcher: crate::launcher::Client,
    gateway: GatewayClient,
    skills_commit: String,
    reconciled: Arc<std::sync::atomic::AtomicBool>,
    gate: Arc<Mutex<()>>,
    active: Arc<Mutex<HashMap<JobId, Active>>>,
}

struct Active {
    container: ContainerId,
    cancel: watch::Sender<bool>,
    done: watch::Receiver<bool>,
}

fn runtime(kind: JobFailureKind) -> ProviderError {
    ProviderError::Runtime { kind }
}

fn container(spec: &JobSpec) -> Result<ContainerId, ProviderError> {
    if (spec.kind == JobKind::Ask) != spec.working_session.is_some() {
        return Err(runtime(JobFailureKind::Configuration));
    }
    if spec.payer.0.trim().is_empty() {
        return Err(runtime(JobFailureKind::CredentialMissing));
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
        _ => return Err(runtime(JobFailureKind::Configuration)),
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
            gate: Arc::default(),
            active: Arc::default(),
        }
    }

    /// Startup gate: stop managed workers and retire their local virtual keys before accepting jobs.
    pub async fn reconcile(&self, retired_prs: &[i64]) -> Result<(), ProviderError> {
        let _guard = self.gate.lock().await;
        self.reconcile_locked(retired_prs).await
    }

    async fn reconcile_locked(&self, retired_prs: &[i64]) -> Result<(), ProviderError> {
        let workers = self
            .launcher
            .reconcile(retired_prs)
            .await
            .map_err(|_| runtime(JobFailureKind::WorkerUnavailable))?;
        for worker in workers {
            self.gateway.revoke(&worker).await?;
        }
        self.reconciled
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    async fn control(&self, spec: &JobSpec, action: LauncherAction) -> Result<(), ProviderError> {
        self.launcher
            .call(LauncherRequest {
                id: format!("{}-{}", spec.job_id.0, spec.attempt),
                action,
            })
            .await
            .map_err(|_| runtime(JobFailureKind::WorkerUnavailable))?;
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
            Err(_) => return Err(runtime(JobFailureKind::WorkerUnavailable)),
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
                activity: contracts::job::event::ActivityKind::AgentMessage,
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
        let session = spec
            .working_session
            .as_ref()
            .and_then(|w| w.thread_id.as_deref())
            .or_else(|| spec.session.as_ref().map(|s| s.thread_id.as_str()));
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
        match self.perform(&effective, session, events.clone()).await {
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
                        .perform(&fresh, Some(&parent.thread_id), events.clone())
                        .await
                    {
                        Err(ProviderError::SessionUnavailable) => {}
                        result => return result,
                    }
                }
                fresh.session = None;
                let mut outcome = self.perform(&fresh, None, events).await?;
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
    ) -> Result<ProviderOutcome, ProviderError> {
        let _guard = self.gate.lock().await;
        if !self.reconciled.load(std::sync::atomic::Ordering::Acquire) {
            self.reconcile_locked(&[]).await?;
        }
        // A dropped caller cannot run asynchronous cleanup: fence that orphan before accepting another job.
        let orphans: Vec<_> = self
            .active
            .lock()
            .await
            .iter()
            .map(|(job, active)| (*job, active.container.clone()))
            .collect();
        for (job, worker) in orphans {
            self.stop(spec, &worker).await?;
            self.gateway.revoke(&worker).await?;
            self.active.lock().await.remove(&job);
        }
        let worker = container(spec)?;
        let (cancel, mut cancelled) = watch::channel(false);
        let (finished, done) = watch::channel(false);
        self.active.lock().await.insert(
            spec.job_id,
            Active {
                container: worker.clone(),
                cancel,
                done,
            },
        );
        let mut rpc = None;
        let mut key = None;
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
                .map_err(|_| runtime(JobFailureKind::WorkerUnavailable))?;
            rpc = Some(rpc::Rpc::connect(socket, events.clone(), scrubber).await?);
            let rpc = rpc
                .as_mut()
                .ok_or_else(|| runtime(JobFailureKind::WorkerLost))?;
            let minted = key
                .as_ref()
                .ok_or_else(|| runtime(JobFailureKind::WorkerLost))?;
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
            self.run_attached(rpc, spec, session, &minted.id, &mut cancelled)
                .await
        })
        .await
        .unwrap_or(Err(ProviderError::Timeout));
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
            .unwrap_or(Err(runtime(JobFailureKind::WorkerLost)));
            if let Err(error) = scrubbed
                && cleanup.is_ok()
            {
                cleanup = Err(error);
            }
        }
        if cleanup.is_err() {
            let _ = self.stop(spec, &worker).await;
        }
        drop(rpc);
        if cleanup.is_ok() {
            self.active.lock().await.remove(&spec.job_id);
        }
        let _ = finished.send(true);
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
            .ok_or_else(|| runtime(JobFailureKind::WorkerLost))?
            .to_owned();
        let history = result["thread"]["turns"]
            .as_array()
            .is_some_and(|v| !v.is_empty());
        rpc.restored_usage(&thread, history).await?;
        Ok(thread)
    }

    async fn run_attached(
        &self,
        rpc: &mut rpc::Rpc,
        spec: &JobSpec,
        session: Option<&str>,
        key_id: &str,
        cancelled: &mut watch::Receiver<bool>,
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
            return Err(runtime(JobFailureKind::ModelUnavailable));
        }
        let mut preflight = None;
        let fork = spec.kind == JobKind::Ask
            && spec
                .working_session
                .as_ref()
                .is_some_and(|w| w.thread_id.is_none())
            && session.is_some();
        let thread = if fork {
            let source_kind = if kind == ThreadKind::Verify {
                kind
            } else {
                ThreadKind::Analytic
            };
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
                    .ok_or_else(|| runtime(JobFailureKind::WorkerLost))?
                    .seed
            } else {
                SessionSeed::ContextPack
            };
            let mut recovered = working.clone();
            if fork {
                let parent = session
                    .and_then(|parent| rpc.last.get(parent))
                    .copied()
                    .ok_or_else(|| runtime(JobFailureKind::WorkerLost))?;
                recovered.parent_context = Some(LastContext {
                    tokens: parent,
                    generation: working.analytic_generation,
                });
                recovered.preflight = None;
            }
            let compact = config::needs_compaction(&recovered, seed)?;
            let state = if compact {
                self.thread(rpc, spec, kind, "thread/resume", Some(&thread))
                    .await?;
                let (delta, _) = self.turn(rpc, spec, &thread, true, cancelled).await?;
                usage = usage
                    .checked_add(delta)
                    .map_err(|_| runtime(JobFailureKind::ProviderFailed))?;
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
        // Resume again after preflight: fixed policy and attachment precede every operation.
        if kind != ThreadKind::Judge {
            self.thread(rpc, spec, kind, "thread/resume", Some(&thread))
                .await?;
        }
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
                    return Err(runtime(JobFailureKind::Configuration));
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
        usage = usage
            .checked_add(delta)
            .map_err(|_| runtime(JobFailureKind::ProviderFailed))?;
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
            serde_json::from_str(&text).map_err(|_| ProviderError::OutputNotJson)?
        } else {
            serde_json::from_str(message.as_deref().ok_or(ProviderError::OutputNotJson)?)
                .map_err(|_| ProviderError::OutputNotJson)?
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
        for attempt in 0..20 {
            let metered = self.gateway.usage(key_id).await?;
            if metered.user != spec.payer {
                return Err(runtime(JobFailureKind::ProviderFailed));
            }
            let usage = metered.usage;
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
        let baseline = *rpc
            .totals
            .get(thread)
            .ok_or_else(|| runtime(JobFailureKind::WorkerLost))?;
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
                    return Err(runtime(JobFailureKind::WorkerLost));
                }
                match completed["status"].as_str() {
                    Some("completed") => {}
                    Some("interrupted") => return Err(ProviderError::Cancelled),
                    _ => return Err(rpc::failure(&completed["error"])),
                }
                if !rpc.turn.updated {
                    return Err(runtime(JobFailureKind::ProviderFailed));
                }
                let end = *rpc
                    .totals
                    .get(thread)
                    .ok_or_else(|| runtime(JobFailureKind::WorkerLost))?;
                let delta = end
                    .delta_from(baseline)
                    .map_err(|_| runtime(JobFailureKind::ProviderFailed))?;
                return Ok((delta, rpc.turn.output.take()));
            }
            tokio::select! {
                result = rpc.next_notification() => result?,
                _ = cancelled.changed() => {
                    // Interrupt on the same subscribed connection; the enclosing failure path stops the worker.
                    let _ = tokio::time::timeout(Duration::from_secs(5), rpc.request("turn/interrupt", json!({"threadId":thread,"turnId":turn_id}))).await;
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
        Box::pin(async move {
            let mut spec = call
                .job_spec
                .as_deref()
                .ok_or_else(|| runtime(JobFailureKind::Configuration))?
                .clone();
            spec.prompt = call.prompt.clone();
            self.perform(&spec, None, events).await
        })
    }
    fn resume<'a>(
        &'a self,
        session_id: &'a str,
        call: &'a ProviderCall,
        events: mpsc::Sender<ProviderEvent>,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderError>> {
        Box::pin(async move {
            let mut spec = call
                .job_spec
                .as_deref()
                .ok_or_else(|| runtime(JobFailureKind::Configuration))?
                .clone();
            spec.prompt = call.prompt.clone();
            self.perform(&spec, Some(session_id), events).await
        })
    }
    fn cancel(&self, job: JobId) -> BoxFuture<'_, Result<(), ProviderError>> {
        Box::pin(async move {
            let (mut done, container) = {
                let registry = self.active.lock().await;
                let Some(active) = registry.get(&job) else {
                    return Ok(());
                };
                let _ = active.cancel.send(true);
                (active.done.clone(), active.container.clone())
            };
            // Copy the state before waiting; never hold the registry lock through cleanup.
            let finished = tokio::time::timeout(Duration::from_secs(10), async {
                while !*done.borrow_and_update() {
                    if done.changed().await.is_err() {
                        return false;
                    }
                }
                true
            })
            .await
            .unwrap_or(false);
            if !finished {
                self.launcher
                    .call(LauncherRequest {
                        id: format!("cancel-{}", job.0),
                        action: LauncherAction::Stop {
                            container: container.clone(),
                        },
                    })
                    .await
                    .map_err(|_| runtime(JobFailureKind::WorkerUnavailable))?;
                self.gateway.revoke(&container).await?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
