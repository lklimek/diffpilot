//! Server entry point: loads the configuration and serves the router until SIGINT or SIGTERM.

use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use db::error::DbError;
use db::repo::Db;
use server::api;
use server::config::{Config, ConfigError, LISTEN_ADDR, RUNNER_LISTEN_ADDR};
use server::state::AppState;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Debug, thiserror::Error)]
enum StartError {
    // The parse error text contains the offending line, which may hold a secret: keep only its position.
    #[error("the .env file has a syntax error at character {index} of a line")]
    DotEnvSyntax { index: usize },
    #[error("could not read the .env file")]
    DotEnv(#[source] dotenvy::Error),
    #[error("could not start the async runtime")]
    Runtime(#[source] std::io::Error),
    #[error("the configuration is invalid")]
    Config(#[from] ConfigError),
    #[error("could not create the data directory {}; check SR_DATA_DIR", path.display())]
    DataDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not open the database {}", path.display())]
    Database {
        path: PathBuf,
        #[source]
        source: DbError,
    },
    #[error("could not listen on {addr}; check {key} and whether the port is in use")]
    Bind {
        key: &'static str,
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("could not finish the clean-up of cancelled analyses")]
    Recover(#[source] server::pipeline::control::ControlError),
    #[error("could not resume the unfinished analyses")]
    Resume(#[source] DbError),
    #[error("could not follow the open questions")]
    ResumeQuestions(#[source] DbError),
    #[error("could not end the dashboard refreshes the restart interrupted")]
    RecoverDashboard(#[source] DbError),
    #[error("the HTTP server stopped with an I/O error")]
    Serve(#[source] std::io::Error),
}

fn main() -> ExitCode {
    // Before any thread exists: dotenvy sets process environment variables.
    let dotenv = dotenvy::dotenv();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .finish()
        .with(server::memcan::payload_log_cap())
        .init();
    let result = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime.block_on(run(dotenv)),
        Err(source) => Err(StartError::Runtime(source)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("server stopped: {}", error_chain(&error));
            ExitCode::FAILURE
        }
    }
}

/// File name of the SQLite database inside `SR_DATA_DIR`.
const DATABASE_FILE: &str = "smart-review.sqlite";

async fn run(dotenv: Result<PathBuf, dotenvy::Error>) -> Result<(), StartError> {
    dotenv_outcome(dotenv)?;
    let config = Config::from_env()?;
    server::auth::warn_if_public_url_insecure(&config);
    tokio::fs::create_dir_all(&config.data_dir)
        .await
        .map_err(|source| StartError::DataDir {
            path: config.data_dir.clone(),
            source,
        })?;
    let path = config.data_dir.join(DATABASE_FILE);
    let db = Db::open(&path)
        .await
        .map_err(|source| StartError::Database { path, source })?;
    let public = bind(LISTEN_ADDR, config.listen_addr).await?;
    let runner = bind(RUNNER_LISTEN_ADDR, config.runner_listen_addr).await?;
    let state = AppState::with_queue(config, db);
    tokio::spawn(server::jobs::github_tokens::run_worker(
        state.db.clone(),
        state.config.clone(),
    ));
    tokio::spawn(server::memcan::writer::run_writer(
        state.db.clone(),
        state.config.clone(),
    ));
    // Before any runner claim is served: a crash may have left a cancellation half done.
    server::pipeline::control::recover_cancellations(&state)
        .await
        .map_err(StartError::Recover)?;
    server::pipeline::resume_unfinished(&state)
        .await
        .map_err(StartError::Resume)?;
    server::ask::resume_unanswered(&state)
        .await
        .map_err(StartError::ResumeQuestions)?;
    server::dashboard::refresh::recover(&state)
        .await
        .map_err(StartError::RecoverDashboard)?;
    // After the recovery, so a scheduled start never meets a refresh the restart left live.
    server::dashboard::schedule::spawn(state.clone());

    let (stop, stopped) = (state.stopping.clone(), state.stopping.subscribe());
    tokio::spawn(async move {
        shutdown_signal().await;
        // Both servers may already be gone; nothing to notify then.
        let _ = stop.send(true);
    });
    let public = axum::serve(
        public,
        api::router(state.clone()).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(stop_requested(stopped.clone()));
    let runner = axum::serve(runner, api::runner_router(state))
        .with_graceful_shutdown(stop_requested(stopped));
    tokio::try_join!(public.into_future(), runner.into_future()).map_err(StartError::Serve)?;
    Ok(())
}

async fn bind(key: &'static str, addr: SocketAddr) -> Result<TcpListener, StartError> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| StartError::Bind { key, addr, source })?;
    tracing::info!(%addr, listener = key, "server listening");
    Ok(listener)
}

async fn stop_requested(mut stopped: watch::Receiver<bool>) {
    // An error means the sender is gone, which also ends the server.
    let _ = stopped.wait_for(|stop| *stop).await;
}

async fn shutdown_signal() {
    let interrupt = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "cannot listen for Ctrl+C; stop the server with SIGTERM");
            std::future::pending::<()>().await;
        }
    };
    let terminate = async {
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                tracing::warn!(%error, "cannot listen for SIGTERM; stop the server with Ctrl+C");
                std::future::pending::<()>().await;
            }
        }
    };
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::info!("shutdown signal received, finishing open requests");
}

/// A missing `.env` file is fine; any other failure stops the start.
fn dotenv_outcome(dotenv: Result<PathBuf, dotenvy::Error>) -> Result<(), StartError> {
    match dotenv {
        Ok(_) => Ok(()),
        Err(error) if error.not_found() => Ok(()),
        Err(dotenvy::Error::LineParse(_, index)) => Err(StartError::DotEnvSyntax { index }),
        Err(error) => Err(StartError::DotEnv(error)),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_dotenv_file_is_not_an_error() {
        let missing = dotenvy::Error::Io(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert!(dotenv_outcome(Err(missing)).is_ok());
    }

    #[test]
    fn dotenv_syntax_error_does_not_reveal_the_line() {
        let parse = dotenvy::Error::LineParse("SR_MEMCAN_TOKEN=top secret".to_owned(), 20);
        let error = dotenv_outcome(Err(parse)).expect_err("syntax error must stop the start");
        let message = error_chain(&error);
        assert!(!message.contains("top secret"), "{message}");
        assert!(message.contains("character 20"), "{message}");
    }
}
