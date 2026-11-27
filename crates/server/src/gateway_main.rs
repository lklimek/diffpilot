//! Standalone plan-only gateway; no application database or CLI credential import.

use base64::{Engine, engine::general_purpose::STANDARD};
use contracts::job::access::Secret;
use secrecy::SecretBox;
use server::gateway::{Error, Gateway};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tracing_subscriber::EnvFilter;

fn setting(name: &'static str) -> Result<String, Error> {
    std::env::var(name).map_err(|_| Error::ConfigurationSetting(name))
}
fn secret_mode_allowed(name: &str, mode: u32) -> bool {
    let mode = mode & 0o777;
    mode == 0o400 || name == "SR_GATEWAY_CREDENTIALS_TOKEN_FILE" && mode == 0o440
}
fn secret_file(name: &'static str) -> Result<String, Error> {
    let path = PathBuf::from(setting(name)?);
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(&path).map_err(|source| Error::SecretFile {
        setting: name,
        source,
    })?;
    if !metadata.is_file() || !secret_mode_allowed(name, metadata.permissions().mode()) {
        return Err(Error::ConfigurationSetting(name));
    }
    Ok(std::fs::read_to_string(path)
        .map_err(|source| Error::SecretFile {
            setting: name,
            source,
        })?
        .trim()
        .to_owned())
}

fn judge_address(
    value: Option<&str>,
    model: SocketAddr,
    admin: SocketAddr,
) -> Result<Option<SocketAddr>, Error> {
    value
        .map(|value| {
            let address: SocketAddr = value.parse().map_err(|_| Error::Configuration)?;
            if address.ip().is_unspecified() || address.ip() == admin.ip() || address == model {
                return Err(Error::Configuration);
            }
            Ok(address)
        })
        .transpose()
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("server::gateway=warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
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
    let judge = judge_address(
        std::env::var("SR_GATEWAY_JUDGE_ADDR").ok().as_deref(),
        model,
        admin,
    )?;
    let key: [u8; 32] = STANDARD
        .decode(secret_file("SR_GATEWAY_SECRET_KEY_FILE")?)
        .map_err(|_| {
            Error::ConfigurationSetting("SR_GATEWAY_SECRET_KEY_FILE (canonical base64 required)")
        })?
        .try_into()
        .map_err(|_| {
            Error::ConfigurationSetting(
                "SR_GATEWAY_SECRET_KEY_FILE (exactly 32 decoded bytes required)",
            )
        })?;
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
    let judge = match judge {
        Some(address) => Some(tokio::net::TcpListener::bind(address).await?),
        None => None,
    };
    let judge_model = async {
        match judge {
            Some(listener) => axum::serve(listener, gateway.model_router()).await,
            None => std::future::pending().await,
        }
    };
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let result = tokio::select! {
        result = axum::serve(model, gateway.model_router()) => result,
        result = axum::serve(admin, gateway.admin_router()) => result,
        result = judge_model => result,
        result = tokio::signal::ctrl_c() => result,
        _ = terminate.recv() => Ok(()),
    };
    gateway.shutdown().await;
    result.map_err(Error::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_scope_can_be_shared_read_only_with_the_local_server_group() {
        assert!(secret_mode_allowed(
            "SR_GATEWAY_CREDENTIALS_TOKEN_FILE",
            0o440
        ));
        assert!(secret_mode_allowed(
            "SR_GATEWAY_CREDENTIALS_TOKEN_FILE",
            0o400
        ));
        for mode in [0o444, 0o460, 0o441, 0o640] {
            assert!(!secret_mode_allowed(
                "SR_GATEWAY_CREDENTIALS_TOKEN_FILE",
                mode
            ));
        }
        for setting in ["SR_GATEWAY_SECRET_KEY_FILE", "SR_GATEWAY_KEYS_TOKEN_FILE"] {
            assert!(secret_mode_allowed(setting, 0o400));
            assert!(!secret_mode_allowed(setting, 0o440));
        }
    }
    #[test]
    fn judge_listener_uses_its_own_interface_and_keeps_admin_private() {
        let model = "172.29.75.2:8090".parse().unwrap();
        let admin = "172.29.71.5:8091".parse().unwrap();
        assert_eq!(judge_address(None, model, admin).unwrap(), None);
        assert_eq!(
            judge_address(Some("172.29.76.2:8090"), model, admin).unwrap(),
            Some("172.29.76.2:8090".parse().unwrap())
        );
        for address in [
            "0.0.0.0:8090",
            "172.29.71.5:8090",
            "172.29.75.2:8090",
            "invalid",
        ] {
            assert!(
                judge_address(Some(address), model, admin).is_err(),
                "{address}"
            );
        }
    }
}
