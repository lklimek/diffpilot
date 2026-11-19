//! Fake MemCan entry point: serves until killed.

use std::error::Error;
use std::net::SocketAddr;
use std::process::ExitCode;

use clap::Parser;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

/// Fake MemCan (development and tests only): MCP over streamable HTTP plus `/_fake/*` inspection and control.
#[derive(Parser)]
#[command(name = "fake-memcan")]
struct Cli {
    /// Listen address.
    #[arg(long, env = "FAKE_MEMCAN_LISTEN", default_value = "127.0.0.1:3910")]
    listen: SocketAddr,
    /// Bearer token every request must present (pass it through the environment, not the command line).
    #[arg(long, env = "FAKE_MEMCAN_TOKEN", hide_env_values = true)]
    token: String,
    /// Path of the MCP endpoint.
    #[arg(long, env = "FAKE_MEMCAN_PATH", default_value = "/mcp")]
    path: String,
}

#[derive(Debug, thiserror::Error)]
enum RunError {
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
            tracing::error!("fake-memcan stopped: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), RunError> {
    let router = fake_memcan::router(fake_memcan::Config {
        token: cli.token,
        path: cli.path,
    });
    let listener = TcpListener::bind(cli.listen)
        .await
        .map_err(|source| RunError::Bind {
            addr: cli.listen,
            source,
        })?;
    let addr = listener.local_addr().unwrap_or(cli.listen);
    tracing::info!(%addr, "fake-memcan listening");
    axum::serve(listener, router).await.map_err(RunError::Serve)
}
