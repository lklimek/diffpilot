use super::*;
use contracts::{
    gateway::{KeyRevokeReason, MintKey, MintedKey, PayerUsage, RevokeKeys},
    job::access::Secret,
};
use reqwest::{Client, StatusCode};
use std::time::Duration;
use url::Url;

/// Gateway admin client limited to local virtual keys and response-ID usage sums.
#[derive(Clone)]
pub struct GatewayClient {
    client: Client,
    base: Url,
    scope: Secret,
}

impl GatewayClient {
    /// Use the control-network address and the keys scope, never a credential-management scope.
    pub fn new(base: Url, scope: Secret) -> Result<Self, ProviderError> {
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || base.path() != "/"
        {
            return Err(fault(
                JobFailureKind::Configuration,
                "gateway/config",
                "invalid admin base URL",
            ));
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| {
                fault(
                    JobFailureKind::Configuration,
                    "gateway/config",
                    "HTTP client construction",
                )
            })?;
        Ok(Self {
            client,
            base,
            scope,
        })
    }

    async fn request<T: serde::Serialize>(
        &self,
        path: &str,
        body: Option<&T>,
    ) -> Result<reqwest::Response, ProviderError> {
        let step = if path.ends_with("/mint") {
            "mint"
        } else if path.ends_with("/revoke") {
            "revoke"
        } else {
            "usage"
        };
        let url = self
            .base
            .join(path)
            .map_err(|_| fault(JobFailureKind::Configuration, "gateway", "admin URL"))?;
        let request = match body {
            Some(body) => self.client.post(url).json(body),
            None => self.client.get(url),
        };
        let response = request
            .bearer_auth(self.scope.expose())
            .send()
            .await
            .map_err(|_| fault(JobFailureKind::WorkerUnavailable, "gateway", step))?;
        match response.status() {
            StatusCode::NOT_FOUND => Err(fault(
                JobFailureKind::CredentialMissing,
                "gateway",
                "credential not found",
            )),
            StatusCode::FORBIDDEN => Err(fault(
                JobFailureKind::CredentialRelink,
                "gateway",
                "credential requires relink",
            )),
            status if status.is_success() => Ok(response),
            _ => Err(fault(
                JobFailureKind::WorkerUnavailable,
                "gateway",
                "admin rejected request",
            )),
        }
    }

    pub(super) async fn mint(&self, container: &ContainerId) -> Result<MintedKey, ProviderError> {
        self.request(
            "admin/keys/mint",
            Some(&MintKey {
                container: container.clone(),
            }),
        )
        .await?
        .json()
        .await
        .map_err(|_| {
            fault(
                JobFailureKind::WorkerUnavailable,
                "gateway/mint",
                "invalid mint response",
            )
        })
    }

    pub(super) async fn revoke(&self, container: &ContainerId) -> Result<(), ProviderError> {
        self.request(
            "admin/keys/revoke",
            Some(&RevokeKeys {
                container: container.clone(),
                reason: KeyRevokeReason::Rotation,
            }),
        )
        .await?;
        Ok(())
    }

    pub(super) async fn usage(&self, key: &str) -> Result<PayerUsage, ProviderError> {
        if key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(fault(
                JobFailureKind::WorkerUnavailable,
                "gateway/usage",
                "invalid key identifier",
            ));
        }
        self.request::<()>(&format!("admin/keys/{key}/usage"), None)
            .await?
            .json()
            .await
            .map_err(|_| {
                fault(
                    JobFailureKind::WorkerUnavailable,
                    "gateway/usage",
                    "invalid usage response",
                )
            })
    }
}
