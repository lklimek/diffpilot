//! Runner entry point: the polling loop around `execute`, until SIGINT or SIGTERM.

use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use runner::client::{ClientError, ServerClient};
use runner::config::{Config, ConfigError, ProviderKind};
use runner::execute::Executor;
use runner::provider::LlmProvider;
use runner::provider::ProviderError;
use runner::provider::appserver::{AppserverProvider, GatewayClient};
use runner::scratch;
use runner::worker::{Timing, Worker};
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

/// Per-request timeout towards the server; below the heartbeat interval.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Exit code of a start refused for its configuration (`EX_CONFIG`).
const EX_CONFIG: u8 = 78;

#[derive(Debug, thiserror::Error)]
enum StartError {
    #[error("container launcher failed")]
    Launcher(#[from] runner::launcher::Error),
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
    #[error("worker runtime configuration failed")]
    Provider(#[from] ProviderError),
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
                StartError::Config(_) => ExitCode::from(EX_CONFIG),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

async fn run() -> Result<(), StartError> {
    if std::env::args().nth(1).as_deref() == Some("--launcher") {
        let socket = std::env::var_os("SR_LAUNCHER_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/run/launcher/launcher.sock"));
        runner::launcher::serve(&socket, runner::launcher::Settings::from_env()?).await?;
        return Ok(());
    }
    let config = Config::from_env()?;
    let dirs = config.dirs();
    for dir in [&config.state_dir, &config.work_dir, &dirs.mirrors] {
        private_dir(dir).await?;
    }
    let provider: Arc<dyn LlmProvider> = match config.provider {
        ProviderKind::Appserver => {
            let gateway = GatewayClient::new(
                config
                    .gateway_url
                    .ok_or(ConfigError::Missing(runner::config::GATEWAY_URL))?,
                config
                    .gateway_keys
                    .ok_or(ConfigError::Missing(runner::config::GATEWAY_KEYS_FILE))?,
            )?;
            let provider = AppserverProvider::new(
                runner::launcher::Client::new(config.launcher_socket),
                gateway,
                config.skills_commit,
            );
            provider.reconcile(&[]).await?;
            Arc::new(provider)
        }
        #[cfg(feature = "fake-provider")]
        ProviderKind::Fake => {
            tracing::warn!("using the fake provider: agent output is replayed from fixtures");
            Arc::new(runner::provider::fake::FakeProvider::new(Duration::ZERO))
        }
    };
    let executor = Arc::new(Executor::new(provider, dirs));
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
    )
    .with_disk_reserve(
        scratch::Reserve::new(config.work_dir.clone(), config.disk_reserve)
            .with_marker(config.state_dir.join(DISK_RESERVE_MARKER)),
    )
    .run(shutdown_signal())
    .await;
    tracing::info!("runner stopped");
    Ok(())
}

/// Present while the mirror disk is below its reserve; read by the container healthcheck.
const DISK_RESERVE_MARKER: &str = "disk-below-reserve";

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
