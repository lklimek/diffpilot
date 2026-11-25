//! Credentials-scoped admin client; only common, secret-free plan DTOs cross into the server.

use contracts::{
    gateway::{CredentialStatus, DeviceLink, LinkCredential, UnlinkCredential},
    ids::UserId,
};
use reqwest::{Client, Method};
use secrecy::{ExposeSecret, SecretString};
use url::Url;

/// Server connection to the gateway's credentials administration listener.
#[derive(Debug)]
pub struct Admin {
    base: Url,
    token: SecretString,
    client: Client,
}

/// An admin operation failed; upstream bodies are never returned or logged.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Transport, HTTP status or JSON decoding failure.
    #[error("the Codex gateway is unavailable")]
    Http(#[from] reqwest::Error),
    /// The admin route did not acknowledge the operation.
    #[error("the gateway administration response has an unexpected status")]
    Status,
    /// The configured base cannot hold route segments.
    #[error("the gateway administration URL is invalid")]
    Url,
    /// Status exceeds the admin response limit.
    #[error("the gateway administration response is too large")]
    TooLarge,
    /// A common DTO could not be decoded.
    #[error("the gateway administration response is invalid")]
    Json(#[from] serde_json::Error),
}

impl Admin {
    /// Uses a dedicated credentials scope, bounded calls and no redirects or implicit retries.
    pub fn new(base: Url, token: SecretString) -> Result<Self, Error> {
        if base.cannot_be_a_base() || !base.username().is_empty() || base.password().is_some() {
            return Err(Error::Url);
        }
        Ok(Self {
            base,
            token,
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .timeout(std::time::Duration::from_secs(15))
                .build()?,
        })
    }

    async fn request(
        &self,
        user: &UserId,
        suffix: Option<&str>,
        method: Method,
        body: Option<&impl serde::Serialize>,
    ) -> Result<reqwest::Response, Error> {
        let mut url = self.base.clone();
        url.set_query(None);
        url.set_fragment(None);
        {
            let mut path = url.path_segments_mut().map_err(|_| Error::Url)?;
            path.pop_if_empty()
                .extend(["admin", "credentials", &user.0]);
            if let Some(suffix) = suffix {
                path.push(suffix);
            }
        }
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(self.token.expose_secret());
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?.error_for_status()?;
        if !response.status().is_success() {
            return Err(Error::Status);
        }
        Ok(response)
    }

    async fn decode<T: serde::de::DeserializeOwned>(
        mut response: reqwest::Response,
    ) -> Result<T, Error> {
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 16_384 {
                return Err(Error::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Reads canonical connection status, or the staged-link view while polling.
    pub async fn status(&self, user: &UserId, linking: bool) -> Result<CredentialStatus, Error> {
        Self::decode(
            self.request(user, linking.then_some("link"), Method::GET, None::<&()>)
                .await?,
        )
        .await
    }

    /// Starts a gateway-owned device flow; the server never receives provider tokens.
    pub async fn link(&self, user: &UserId, command: &LinkCredential) -> Result<DeviceLink, Error> {
        Self::decode(
            self.request(user, Some("link"), Method::POST, Some(command))
                .await?,
        )
        .await
    }

    /// Explicit user unlink is the only server action that authorizes upstream revocation.
    pub async fn unlink(&self, user: &UserId) -> Result<(), Error> {
        let response = self
            .request(
                user,
                None,
                Method::DELETE,
                Some(&UnlinkCredential { user: user.clone() }),
            )
            .await?;
        if response.status() != reqwest::StatusCode::NO_CONTENT {
            return Err(Error::Status);
        }
        Ok(())
    }
}
