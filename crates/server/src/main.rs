//! Server entry point: loads the configuration and serves the router until SIGINT or SIGTERM.

use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use server::api;
use server::config::{Config, ConfigError};
use server::state::AppState;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

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
    #[error("could not listen on {addr}; check SR_LISTEN_ADDR and whether the port is in use")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("the HTTP server stopped with an I/O error")]
    Serve(#[source] std::io::Error),
}

fn main() -> ExitCode {
    // Before any thread exists: dotenvy sets process environment variables.
    let dotenv = dotenvy::dotenv();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
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

async fn run(dotenv: Result<PathBuf, dotenvy::Error>) -> Result<(), StartError> {
    dotenv_outcome(dotenv)?;
    let config = Config::from_env()?;
    let addr = config.listen_addr;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|source| StartError::Bind { addr, source })?;
    tracing::info!(%addr, "server listening");
    axum::serve(listener, api::router(AppState::new(config)))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(StartError::Serve)
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
