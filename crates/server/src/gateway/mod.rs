//! Plan-only Codex gateway; credentials and refresh coordination never enter the application database.

mod auth;
mod http;
mod proxy;
mod store;

use crate::secrets::{TokenCipher, random_token};
use axum::{
    Router,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use contracts::{
    gateway::{MintedKey, PayerUsage},
    ids::UserId,
    job::{access::Secret, result::Usage},
    runtime::ContainerId,
    time::Timestamp,
};
use secrecy::SecretBox;
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};
use store::{Store, digest};
use tokio::{
    sync::Mutex,
    task::{AbortHandle, JoinSet},
};

/// A gateway failure with a redacted public HTTP mapping.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Local durable storage is unavailable.
    #[error("gateway storage is unavailable")]
    Io(#[from] std::io::Error),
    /// Gateway encryption failed.
    #[error("gateway encryption failed")]
    Secret(#[from] crate::secrets::SecretError),
    /// A provider message could not be decoded.
    #[error("gateway message is malformed")]
    Json(#[from] serde_json::Error),
    /// Upstream communication failed; never include its URL in HTTP errors.
    #[error("gateway upstream is unavailable")]
    Http(#[from] reqwest::Error),
    /// Lock ownership could not be acquired.
    #[error("gateway lock task failed")]
    Join(#[source] tokio::task::JoinError),
    /// No valid local virtual key.
    #[error("invalid_virtual_key")]
    Key,
    /// Owner must link the account again.
    #[error("credential_unlinked")]
    Unlinked,
    /// Admin credentials have the wrong scope.
    #[error("forbidden")]
    Forbidden,
    /// Request violates gateway policy.
    #[error("upstream_rejected")]
    Body,
    /// Device linking is unavailable for this account.
    #[error("device-code login is not enabled for this account")]
    DeviceDisabled,
    /// A staged link did not complete.
    #[error("device-code linking did not complete")]
    Link,
    /// Usage counters are invalid.
    #[error("gateway usage counters are inconsistent")]
    Usage,
    /// Startup configuration is invalid.
    #[error("gateway configuration is invalid")]
    Configuration,
    /// Model concurrency or retained accounting capacity is exhausted.
    #[error("gateway capacity is temporarily exhausted")]
    Busy,
}

impl Error {
    fn log_storage(&self) {
        match self {
            Self::Io(error) => {
                tracing::error!(kind = ?error.kind(), "gateway durable storage operation failed")
            }
            Self::Secret(_) => tracing::error!("gateway encrypted storage operation failed"),
            Self::Json(_) => tracing::error!("gateway stored record could not be decoded"),
            _ => (),
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        self.log_storage();
        let (status, code) = match &self {
            Self::Busy => (StatusCode::SERVICE_UNAVAILABLE, "gateway_busy"),
            Self::Key => (StatusCode::UNAUTHORIZED, "invalid_virtual_key"),
            Self::Unlinked => (StatusCode::FORBIDDEN, "credential_unlinked"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::Body => (StatusCode::BAD_REQUEST, "upstream_rejected"),
            Self::DeviceDisabled => (
                StatusCode::BAD_REQUEST,
                "device-code login is not enabled for this account",
            ),
            Self::Link => (StatusCode::BAD_GATEWAY, "link_failed"),
            _ => (StatusCode::BAD_GATEWAY, "gateway_unavailable"),
        };
        (
            status,
            axum::Json(serde_json::json!({"error":{"code":code}})),
        )
            .into_response()
    }
}

#[derive(Clone)]
struct Upstream {
    client: reqwest::Client,
    model_client: reqwest::Client,
    revoke_client: reqwest::Client,
    auth: String,
    backend: String,
}

impl Upstream {
    fn new(auth: &str, backend: &str) -> Result<Self, Error> {
        let client = |timeout| {
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .timeout(timeout)
                .build()
        };
        Ok(Self {
            client: client(Duration::from_secs(45))?,
            model_client: client(Duration::from_secs(2700))?,
            revoke_client: client(Duration::from_secs(10))?,
            auth: auth.into(),
            backend: backend.into(),
        })
    }
}

struct LinkAttempt {
    id: uuid::Uuid,
    task: Option<AbortHandle>,
}
type LinkAttempts = Arc<Mutex<HashMap<UserId, LinkAttempt>>>;

/// Gateway state; construct with an isolated store and its own encryption/admin key files.
pub struct Gateway {
    store: Arc<Store>,
    upstream: Upstream,
    credentials_scope: Secret,
    keys_scope: Secret,
    key_ttl: i64,
    links: LinkAttempts,
    tasks: Mutex<JoinSet<()>>,
    models: Mutex<HashMap<UserId, proxy::Models>>,
    refreshes: Mutex<JoinSet<()>>,
    stopping: std::sync::atomic::AtomicBool,
    key_streams: Mutex<HashMap<String, std::sync::Weak<tokio::sync::Semaphore>>>,
    drains: Arc<tokio::sync::Semaphore>,
    streams: Arc<tokio::sync::Semaphore>,
}

impl Gateway {
    /// Opens an isolated gateway volume; upstream endpoints are fixed in production.
    pub fn open(
        root: &Path,
        key: &SecretBox<[u8; 32]>,
        credentials_scope: Secret,
        keys_scope: Secret,
        key_ttl: Duration,
    ) -> Result<Self, Error> {
        if key_ttl.is_zero()
            || key_ttl > Duration::from_secs(5400)
            || credentials_scope.expose().len() < 32
            || keys_scope.expose().len() < 32
            || crate::secrets::secrets_equal(credentials_scope.expose(), keys_scope.expose())
        {
            return Err(Error::Configuration);
        }
        Ok(Self {
            store: Store::new(root, TokenCipher::new(key))?,
            upstream: Upstream::new(
                "https://auth.openai.com",
                "https://chatgpt.com/backend-api/codex",
            )?,
            credentials_scope,
            keys_scope,
            key_ttl: key_ttl.as_secs() as i64,
            links: Arc::default(),
            tasks: Mutex::new(JoinSet::new()),
            models: Mutex::default(),
            refreshes: Mutex::new(JoinSet::new()),
            stopping: std::sync::atomic::AtomicBool::new(false),
            key_streams: Mutex::default(),
            drains: Arc::new(tokio::sync::Semaphore::new(16)),
            streams: Arc::new(tokio::sync::Semaphore::new(64)),
        })
    }

    /// Worker listener: two model routes only; no admin routes.
    pub fn model_router(self: &Arc<Self>) -> Router {
        http::model_router(self.clone())
    }

    /// Separate admin listener with credentials/keys bearer scopes.
    pub fn admin_router(self: &Arc<Self>) -> Router {
        http::admin_router(self.clone())
    }

    /// Settles in-flight refreshes for up to 50 seconds, then joins background tasks.
    pub async fn shutdown(&self) {
        let mut refreshes = self.refreshes.lock().await;
        self.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if tokio::time::timeout(Duration::from_secs(50), async {
            while refreshes.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            tracing::warn!(
                "gateway refresh shutdown deadline expired; unresolved generations require relink"
            );
            refreshes.abort_all();
            while refreshes.join_next().await.is_some() {}
        }
        drop(refreshes);
        let mut tasks = self.tasks.lock().await;
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }

    async fn mint(&self, container: &ContainerId) -> Result<MintedKey, Error> {
        let user = payer(container);
        let mut tx = self.store.lock(user).await?;
        if tx.record.bundle.is_none() || tx.record.reason.is_some() {
            return Err(Error::Unlinked);
        }
        tx.key_capacity()?;
        for (id, key) in tx
            .record
            .keys
            .iter_mut()
            .filter(|(_, k)| !k.revoked && &k.container == container)
        {
            key.revoked = true;
            self.store.retire_key(id)?;
        }
        let key = random_token()?;
        let id = digest(key.as_bytes());
        let expires_at = now() + self.key_ttl;
        tx.record.keys.insert(
            id.clone(),
            store::Key {
                container: container.clone(),
                expires_at,
                revoked: false,
                archived_usage: Usage::default(),
            },
        );
        tx.save()?;
        self.store.index_key(&id, user)?;
        Ok(MintedKey {
            id,
            key: Secret::new(key),
            expires_at: Timestamp::from_second(expires_at).ok_or(Error::Configuration)?,
        })
    }

    async fn revoke(&self, container: &ContainerId) -> Result<(), Error> {
        let mut tx = self.store.lock(payer(container)).await?;
        for (id, key) in tx
            .record
            .keys
            .iter_mut()
            .filter(|(_, k)| !k.revoked && &k.container == container)
        {
            key.revoked = true;
            self.store.retire_key(id)?;
        }
        tx.save()
    }

    async fn authenticate(&self, key: &str) -> Result<(UserId, String), Error> {
        let id = digest(key.as_bytes());
        let user = self.store.key_user(&id)?;
        let tx = self.store.lock(&user).await?;
        if !self.store.key_live(&id)
            || !tx
                .record
                .keys
                .get(&id)
                .is_some_and(|k| !k.revoked && k.expires_at > now())
        {
            return Err(Error::Key);
        }
        Ok((user, id))
    }

    async fn usage(&self, user: &UserId, key: Option<&str>) -> Result<PayerUsage, Error> {
        let tx = self.store.lock(user).await?;
        let mut usage = match key {
            Some(id) => tx
                .record
                .keys
                .get(id)
                .map(|key| key.archived_usage)
                .unwrap_or_default(),
            None => tx.record.archived_usage,
        };
        usage.requests = Some(usage.requests.unwrap_or(0));
        for meter in tx
            .record
            .responses
            .values()
            .filter(|m| key.is_none_or(|k| k == m.key))
        {
            usage = usage.checked_add(meter.usage).map_err(|_| Error::Usage)?;
        }
        Ok(PayerUsage {
            user: user.clone(),
            usage,
            resets_at: tx.record.resets_at.and_then(Timestamp::from_second),
        })
    }

    #[cfg(test)]
    fn for_test(root: &Path, url: &str) -> Result<Self, Error> {
        let mut gateway = Self::open(
            root,
            &SecretBox::new(Box::new([42; 32])),
            Secret::new("c".repeat(32)),
            Secret::new("k".repeat(32)),
            Duration::from_secs(5400),
        )?;
        gateway.upstream = Upstream::new(url, url)?;
        Ok(gateway)
    }
}

fn now() -> i64 {
    Timestamp::now().as_second()
}
fn payer(container: &ContainerId) -> &UserId {
    match container {
        ContainerId::Pr { payer, .. } | ContainerId::Judge { payer } => payer,
    }
}

#[cfg(test)]
fn validate_body(value: &serde_json::Value) -> Result<(), Error> {
    proxy::validate_body(value)
}

#[cfg(test)]
mod tests;
