//! Fake GitHub entry point: loads the recordings, then serves until killed.

use std::error::Error;
use std::net::SocketAddr;
use std::process::ExitCode;

use clap::Parser;
use fake_github::{Config, StartError};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

/// Fake GitHub (development and tests only) serving the recordings of `fixtures/pr-482/github`.
#[derive(Parser)]
#[command(name = "fake-github")]
struct Cli {
    /// Listen address.
    #[arg(long, env = "FAKE_GITHUB_LISTEN", default_value = "127.0.0.1:3900")]
    listen: SocketAddr,
    #[command(flatten)]
    config: Config,
}

#[derive(Debug, thiserror::Error)]
enum RunError {
    #[error("the fake GitHub could not load its recordings")]
    Start(#[from] StartError),
    #[error("could not listen on {addr}; check --listen and whether the port is in use")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("the HTTP server stopped with an I/O error")]
    Serve(#[source] std::io::Error),
}

#[tokio::main]
async fn main() -> ExitCode {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let mut message = error.to_string();
            let mut source = error.source();
            while let Some(cause) = source {
                message.push_str(": ");
                message.push_str(&cause.to_string());
                source = cause.source();
            }
            tracing::error!("fake-github stopped: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), RunError> {
    let router = fake_github::router(&cli.config)?;
    let listener = TcpListener::bind(cli.listen)
        .await
        .map_err(|source| RunError::Bind {
            addr: cli.listen,
            source,
        })?;
    let addr = listener.local_addr().unwrap_or(cli.listen);
    tracing::info!(%addr, "fake-github listening");
    axum::serve(listener, router).await.map_err(RunError::Serve)
}
