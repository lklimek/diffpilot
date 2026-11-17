//! Runner entry point: the polling loop around `execute`, until SIGINT or SIGTERM.

use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use runner::client::{ClientError, ServerClient};
use runner::codex_homes::{self, CodexHomesError};
use runner::config::{Config, ConfigError, ProviderKind};
use runner::execute::Executor;
use runner::provider::LlmProvider;
use runner::provider::ProviderError;
use runner::provider::codex::{CodexCredential, CodexProvider, CodexSettings, Isolation};
use runner::skills::Skills;
use runner::supervisor::client::{Supervisor, SupervisorError};
use runner::worker::{CredentialCheck, Timing, Worker};
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

/// Per-request timeout towards the server; below the heartbeat interval.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
enum StartError {
    #[error("could not start the async runtime")]
    Runtime(#[source] io::Error),
    #[error("the configuration is invalid")]
    Config(#[from] ConfigError),
    #[error("could not prepare the directory {}; check SR_RUNNER_STATE_DIR and SR_RUNNER_WORK_DIR", path.display())]
    Dir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not create the server client")]
    Client(#[from] ClientError),
    #[error(
        "no supervisor: start the runner under smart-review-supervisor, or set SR_RUNNER_DEV_NO_ISOLATION=1 for development only"
    )]
    NoIsolation,
    #[error("cannot use the supervisor (SR_SUPERVISOR_FD)")]
    Supervisor(#[from] SupervisorError),
    #[error("cannot recover the agent directories of a previous run")]
    Recover(#[source] ProviderError),
    #[error("the per-job CODEX_HOME root is not a bounded filesystem of its own")]
    CodexHomes(#[from] CodexHomesError),
    #[error("the runner secret file is not hidden from the agent")]
    SecretFile(#[source] ConfigError),
}

fn main() -> ExitCode {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let result = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime.block_on(run()),
        Err(source) => Err(StartError::Runtime(source)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("runner stopped: {}", error_chain(&error));
            match error {
                StartError::CodexHomes(_) | StartError::SecretFile(_) => {
                    ExitCode::from(codex_homes::EXIT_CODE)
                }
                _ => ExitCode::FAILURE,
            }
        }
    }
}

async fn run() -> Result<(), StartError> {
    let config = Config::from_env()?;
    let mut dirs = config.dirs();
    let isolation = Supervisor::from_env()?
        .map(|supervisor| Isolation::new(supervisor, dirs.codex_homes.clone()));
    let dev_no_isolation = std::env::var_os(DEV_NO_ISOLATION).is_some_and(|value| value == "1");
    if isolation.is_none() && !dev_no_isolation {
        return Err(StartError::NoIsolation);
    }
    for dir in [
        &config.state_dir,
        &dirs.codex_homes,
        &dirs.mirrors,
        &dirs.sessions,
        &dirs.work,
    ] {
        private_dir(dir).await?;
    }
    match &isolation {
        Some(isolation) => {
            runner::config::check_secret_file_hidden_from_agent(|key| std::env::var(key).ok())
                .map_err(StartError::SecretFile)?;
            let limits = codex_homes::Limits::from_env()?;
            let facts = codex_homes::verify(&isolation.codex_homes, &config.state_dir, &limits)?;
            tracing::info!(
                bytes = facts.bytes,
                inodes = facts.inodes,
                "the per-job CODEX_HOME root is a bounded filesystem of its own"
            );
            // ADR-013 layout: the agent may traverse these roots, not list them; the runner works on a
            // private staging copy of each per-job CODEX_HOME.
            for dir in [&config.state_dir, &isolation.codex_homes, &dirs.work] {
                set_mode(dir, 0o711)?;
            }
            dirs.codex_homes = config.state_dir.join(CODEX_STAGING);
            private_dir(&dirs.codex_homes).await?;
            let removed = isolation
                .recover(&dirs.codex_homes, &dirs.work)
                .await
                .map_err(StartError::Recover)?;
            tracing::info!(
                removed,
                "process isolation on: agents run as their own uid through the supervisor"
            );
        }
        None => tracing::warn!(
            "process isolation off (development): agents run as the runner's uid; production runs under smart-review-supervisor (ADR-013)"
        ),
    }
    remove_leftover_work_dirs(&dirs.work).await?;
    let skills = Arc::new(Skills::new(
        config.claudius_url.clone(),
        config.claudius_ref.clone(),
        config.claudius_checkout(),
        config.skills.clone(),
        config.claudius_refresh,
    ));
    let (provider, credential_check): (Arc<dyn LlmProvider>, CredentialCheck) =
        match config.provider {
            ProviderKind::Codex => {
                let credential = Arc::new(CodexCredential::new(config.master_auth()));
                recover_credential(&credential, &dirs.codex_homes)?;
                let provider = CodexProvider::new(CodexSettings {
                    binary: config.codex_bin.clone(),
                    credential: Arc::clone(&credential),
                    egress_proxy: config.egress_proxy.clone(),
                    term_grace: Timing::v1().term_grace,
                    github_mcp: config.github_mcp_bin.clone(),
                    skills: Some(Arc::clone(&skills)),
                    isolation: isolation.clone(),
                });
                let isolation = isolation.clone();
                (
                    Arc::new(provider),
                    Arc::new(move || {
                        credential.usable() && isolation.as_ref().is_none_or(Isolation::healthy)
                    }),
                )
            }
            #[cfg(feature = "fake-provider")]
            ProviderKind::Fake => {
                tracing::warn!("using the fake provider: agent output is replayed from fixtures");
                (
                    Arc::new(runner::provider::fake::FakeProvider::new(Duration::ZERO)),
                    Arc::new(|| true),
                )
            }
        };
    let executor = Executor::new(provider, dirs, config.credential.clone());
    // Under the supervisor an attempt runs to its own end (stop, sweep, copy-back) even when the job loop
    // stops waiting for it.
    let executor = Arc::new(if isolation.is_some() {
        executor.with_detached_attempts()
    } else {
        executor
    });
    let client = ServerClient::new(
        &config.server_url,
        config.runner_secret.clone(),
        REQUEST_TIMEOUT,
    )?;
    tracing::info!(runner = %config.runner_id, server = %config.server_url, "runner started");
    Worker::new(
        client,
        executor,
        config.runner_id,
        config.runner_secret,
        Timing::v1(),
        credential_check,
    )
    .with_session_retention(config.session_retention)
    .with_skills(skills)
    .run(shutdown_signal())
    .await;
    tracing::info!("runner stopped");
    Ok(())
}

/// Runs agents as the runner's own uid when no supervisor is present (development only; never set in the
/// image).
const DEV_NO_ISOLATION: &str = "SR_RUNNER_DEV_NO_ISOLATION";

/// The runner-private staging `CODEX_HOME`s under the supervisor, below the state directory.
const CODEX_STAGING: &str = "codex-staging";

fn set_mode(path: &Path, mode: u32) -> Result<(), StartError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|source| {
        StartError::Dir {
            path: path.to_path_buf(),
            source,
        }
    })
}

async fn private_dir(path: &Path) -> Result<(), StartError> {
    tokio::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .await
        .map_err(|source| StartError::Dir {
            path: path.to_path_buf(),
            source,
        })
}

/// Writes back the refreshed credential copies a previous run left in per-job `CODEX_HOME`s, then removes them
/// (T13); before the first claim.
fn recover_credential(credential: &CodexCredential, codex_homes: &Path) -> Result<(), StartError> {
    let recovered = credential
        .recover(codex_homes)
        .map_err(|source| StartError::Dir {
            path: codex_homes.to_path_buf(),
            source,
        })?;
    if recovered > 0 {
        tracing::info!(
            recovered,
            "recovered refreshed Codex credentials of interrupted jobs"
        );
    }
    Ok(())
}

/// Work dirs of a previous run hold only checkouts.
async fn remove_leftover_work_dirs(work: &Path) -> Result<(), StartError> {
    let dir_error = |source| StartError::Dir {
        path: work.to_path_buf(),
        source,
    };
    let mut entries = tokio::fs::read_dir(work).await.map_err(dir_error)?;
    while let Some(entry) = entries.next_entry().await.map_err(dir_error)? {
        let path = entry.path();
        if let Err(error) = tokio::fs::remove_dir_all(&path).await {
            tracing::warn!(dir = %path.display(), %error, "cannot remove a leftover job directory");
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    let interrupt = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "cannot listen for Ctrl+C; stop the runner with SIGTERM");
            std::future::pending::<()>().await;
        }
    };
    let terminate = async {
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                tracing::warn!(%error, "cannot listen for SIGTERM; stop the runner with Ctrl+C");
                std::future::pending::<()>().await;
            }
        }
    };
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::info!("shutdown signal received, stopping the running job");
}

fn error_chain(error: &dyn Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}
