//! Outbound notification requests (plan §5.5, threat model T29): `http`/`https` only, no redirects, a 10 s timeout,
//! the answer read up to 4 KiB and never shown, and with `SR_NOTIFY_DENY_PRIVATE=1` no private, loopback or
//! link-local address (checked on an address in the URL and on every address a host name resolves to, so the
//! connection uses only the checked addresses).

use std::error::Error as _;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{HeaderMap, LOCATION};
use reqwest::{Client, redirect};
use url::{Host, Url};

/// Bound of one attempt.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// Most of an answer read (and dropped: answers are never shown or stored).
pub const MAX_ANSWER_BYTES: usize = 4 * 1024;
/// Longest URL accepted.
const MAX_URL_LEN: usize = 2048;

/// A channel or push-target URL was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    /// Not a URL, no host, or too long.
    #[error("the URL is not valid")]
    Invalid,
    /// Not `http` or `https`.
    #[error("only http and https URLs are allowed")]
    Scheme,
    /// A user name or password in the URL (it would be shown in Settings).
    #[error("the URL carries a user name or password; put the credential in the secret field")]
    Credentials,
    /// A private, loopback or link-local address while the operator denies them.
    #[error(
        "the URL names a private, loopback or link-local address, which this server does not reach"
    )]
    Private,
}

/// The URL as stored: `http`/`https`, a host, no credentials, no fragment; an address literal is refused when
/// `deny_private` and it is not public.
pub fn check_url(raw: &str, deny_private: bool) -> Result<Url, UrlError> {
    let raw = raw.trim();
    if raw.len() > MAX_URL_LEN {
        return Err(UrlError::Invalid);
    }
    let mut url = Url::parse(raw).map_err(|_| UrlError::Invalid)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(UrlError::Scheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(UrlError::Credentials);
    }
    let literal = match url.host() {
        None => return Err(UrlError::Invalid),
        Some(Host::Domain(_)) => None,
        Some(Host::Ipv4(address)) => Some(IpAddr::V4(address)),
        Some(Host::Ipv6(address)) => Some(IpAddr::V6(address)),
    };
    if deny_private && literal.is_some_and(|address| !is_public(address)) {
        return Err(UrlError::Private);
    }
    url.set_fragment(None);
    Ok(url)
}

/// Whether an address is a public unicast one (not private, loopback, link-local, shared, unspecified,
/// broadcast, multicast or unique-local).
pub fn is_public(address: IpAddr) -> bool {
    match address.to_canonical() {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                // 100.64.0.0/10, carrier-grade NAT (RFC 6598).
                || (a == 100 && (64..128).contains(&b))
                || a == 0)
        }
        IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unique_local()
                || v6.is_unicast_link_local())
        }
    }
}

/// Every address of the name was private while the operator denies them.
#[derive(Debug, thiserror::Error)]
#[error("the host resolves only to private, loopback or link-local addresses")]
struct OnlyPrivateAddresses;

/// The system resolver with private addresses removed (`SR_NOTIFY_DENY_PRIVATE=1`).
struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let public: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|address| is_public(address.ip()))
                .collect();
            if public.is_empty() {
                return Err(OnlyPrivateAddresses.into());
            }
            let addresses: Addrs = Box::new(public.into_iter());
            Ok(addresses)
        })
    }
}

/// One attempt failed; `Display` is the plain-words error shown in Settings (never the answer's body).
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// The URL is not allowed (scheme, credentials, private address).
    #[error("{0}")]
    Url(#[from] UrlError),
    /// The target answered with a redirect, which is not followed.
    #[error("redirect to {host} not followed")]
    Redirect {
        /// Status code.
        status: u16,
        /// Host the redirect named ("unknown host" when none).
        host: String,
    },
    /// A status other than 2xx or 3xx.
    #[error("HTTP {0}")]
    Status(u16),
    /// No answer within [`TIMEOUT`].
    #[error("no answer within 10 s")]
    Timeout,
    /// The connection was refused.
    #[error("connection refused")]
    Refused,
    /// The host resolves only to denied addresses.
    #[error("the host resolves only to private addresses, which this server does not reach")]
    PrivateResolution,
    /// Any other transport failure (DNS, TLS, reset).
    #[error("connection failed")]
    Connect(#[source] reqwest::Error),
}

impl SendError {
    /// The HTTP status received, if any.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::Redirect { status, .. } | Self::Status(status) => Some(*status),
            _ => None,
        }
    }
}

fn classify(error: reqwest::Error) -> SendError {
    if error.is_timeout() {
        return SendError::Timeout;
    }
    let mut source = error.source();
    while let Some(cause) = source {
        if cause.is::<OnlyPrivateAddresses>() {
            return SendError::PrivateResolution;
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>()
            && io.kind() == std::io::ErrorKind::ConnectionRefused
        {
            return SendError::Refused;
        }
        source = cause.source();
    }
    SendError::Connect(error)
}

/// The HTTP client of notifications.
#[derive(Clone, Debug)]
pub struct Outbound {
    client: Client,
    deny_private: bool,
}

impl Outbound {
    /// A client without redirects, with [`TIMEOUT`]; with `deny_private` its resolver drops non-public addresses
    /// and no proxy from the environment is used (a proxy would resolve names itself).
    pub fn new(deny_private: bool) -> Result<Self, reqwest::Error> {
        let mut builder = Client::builder()
            .redirect(redirect::Policy::none())
            .timeout(TIMEOUT)
            .user_agent("diffpilot");
        if deny_private {
            builder = builder.no_proxy().dns_resolver(PublicOnly);
        }
        Ok(Self {
            client: builder.build()?,
            deny_private,
        })
    }

    /// One `POST` of `body` with `headers`; the HTTP status of a 2xx answer.
    pub async fn post(
        &self,
        url: &Url,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> Result<u16, SendError> {
        let url = check_url(url.as_str(), self.deny_private)?;
        let mut response = self
            .client
            .post(url.clone())
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(classify)?;
        let status = response.status();
        // Read a bounded part of the answer so the connection can be reused; it is never shown or stored.
        let mut read = 0;
        while read < MAX_ANSWER_BYTES {
            match response.chunk().await {
                Ok(Some(chunk)) => read += chunk.len(),
                Ok(None) | Err(_) => break,
            }
        }
        if status.is_redirection() {
            let host = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|location| url.join(location).ok())
                .and_then(|target| target.host_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown host".to_owned());
            return Err(SendError::Redirect {
                status: status.as_u16(),
                host,
            });
        }
        if !status.is_success() {
            return Err(SendError::Status(status.as_u16()));
        }
        Ok(status.as_u16())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_rejects_non_http_scheme() {
        for raw in [
            "ftp://ntfy.example/topic",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "gopher://ntfy.example",
        ] {
            assert_eq!(check_url(raw, false), Err(UrlError::Scheme), "{raw}");
        }
        assert_eq!(check_url("ntfy.example", false), Err(UrlError::Invalid));
        assert_eq!(
            check_url("https://user:pass@ntfy.example", false),
            Err(UrlError::Credentials)
        );
        assert_eq!(
            check_url("https://ntfy.example/#frag", false)
                .map(String::from)
                .as_deref(),
            Ok("https://ntfy.example/")
        );
    }

    #[test]
    fn private_literals_pass_unless_denied() {
        for raw in [
            "http://127.0.0.1:8123/api/webhook/x",
            "http://192.168.1.10/",
            "http://10.0.0.1/",
            "http://169.254.169.254/latest/meta-data",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://100.64.0.1/",
            "http://0.0.0.0/",
        ] {
            assert!(check_url(raw, false).is_ok(), "{raw}");
            assert_eq!(check_url(raw, true), Err(UrlError::Private), "{raw}");
        }
        assert!(check_url("https://1.1.1.1/", true).is_ok());
        assert!(check_url("https://ntfy.sh/topic", true).is_ok());
    }
}
