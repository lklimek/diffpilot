//! Standalone plan-only gateway; no application database or CLI credential import.

use base64::{Engine, engine::general_purpose::STANDARD};
use contracts::job::access::Secret;
use secrecy::SecretBox;
use server::gateway::{Error, Gateway};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

fn setting(name: &str) -> Result<String, Error> {
    std::env::var(name).map_err(|_| Error::Configuration)
}
fn secret_file(name: &str) -> Result<String, Error> {
    let path = PathBuf::from(setting(name)?);
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(Error::Configuration);
    }
    Ok(std::fs::read_to_string(path)?.trim().to_owned())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let model: SocketAddr = setting("SR_GATEWAY_MODEL_ADDR")?
        .parse()
        .map_err(|_| Error::Configuration)?;
    let admin: SocketAddr = setting("SR_GATEWAY_ADMIN_ADDR")?
        .parse()
        .map_err(|_| Error::Configuration)?;
    // W3 binds the admin listener to its private server/runner-network interface.
    if admin.ip().is_unspecified() || admin.ip() == model.ip() || admin.port() == model.port() {
        return Err(Error::Configuration);
    }
    let key: [u8; 32] = STANDARD
        .decode(secret_file("SR_GATEWAY_SECRET_KEY_FILE")?)
        .map_err(|_| Error::Configuration)?
        .try_into()
        .map_err(|_| Error::Configuration)?;
    let ttl = std::env::var("SR_GATEWAY_KEY_TTL")
        .unwrap_or_else(|_| "5400".into())
        .parse()
        .map_err(|_| Error::Configuration)?;
    let gateway = Arc::new(Gateway::open(
        &PathBuf::from(setting("SR_GATEWAY_STORE")?),
        &SecretBox::new(Box::new(key)),
        Secret::new(secret_file("SR_GATEWAY_CREDENTIALS_TOKEN_FILE")?),
        Secret::new(secret_file("SR_GATEWAY_KEYS_TOKEN_FILE")?),
        Duration::from_secs(ttl),
    )?);
    let model = tokio::net::TcpListener::bind(model).await?;
    let admin = tokio::net::TcpListener::bind(admin).await?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let result = tokio::select! {
        result = axum::serve(model, gateway.model_router()) => result,
        result = axum::serve(admin, gateway.admin_router()) => result,
        result = tokio::signal::ctrl_c() => result,
        _ = terminate.recv() => Ok(()),
    };
    gateway.shutdown().await;
    result.map_err(Error::Io)
}
