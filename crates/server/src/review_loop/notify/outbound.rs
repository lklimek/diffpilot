//! Outbound notification requests (plan §5.5, threat model T29): `http`/`https` only, no redirects, a 10 s timeout,
//! the answer read up to 4 KiB and never shown. Loopback, link-local, unspecified, multicast, broadcast and reserved
//! addresses (and IPv6 forms embedding one) are never reached; private ones only unless `SR_NOTIFY_DENY_PRIVATE=1`.
//! Both are checked on an address in the URL and on every address a host name resolves to, so the connection uses
//! only checked addresses (no proxy from the environment, which would resolve names itself).

use std::error::Error as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{HeaderMap, LOCATION};
use reqwest::{Client, redirect};
use url::{Host, Url};

use crate::config::Config;

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
    /// A private address while the operator denies them.
    #[error("the URL names a private address, which this server does not reach")]
    Private,
    /// A loopback, link-local, unspecified, multicast, broadcast or reserved address.
    #[error(
        "the URL names a loopback, link-local or reserved address, which notifications never reach"
    )]
    Address,
}

/// Which addresses outbound notifications may reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reach {
    /// Private addresses (RFC 1918, unique-local, shared, documentation) too.
    pub private: bool,
    /// Loopback too: only tests, whose HTTP targets listen on 127.0.0.1.
    pub loopback: bool,
}

impl Reach {
    /// The configured reach: private unless `SR_NOTIFY_DENY_PRIVATE=1`, loopback never.
    pub fn of(config: &Config) -> Self {
        Self {
            private: !config.notify_deny_private,
            #[cfg(test)]
            loopback: config.notify_loopback,
            #[cfg(not(test))]
            loopback: false,
        }
    }

    /// Whether a connection to `address` is allowed.
    pub fn allows(self, address: IpAddr) -> bool {
        self.check(address).is_ok()
    }

    fn check(self, address: IpAddr) -> Result<(), UrlError> {
        match class(address) {
            Class::Public => Ok(()),
            Class::Private if self.private => Ok(()),
            Class::Private => Err(UrlError::Private),
            Class::Loopback if self.loopback => Ok(()),
            Class::Loopback | Class::Refused => Err(UrlError::Address),
        }
    }
}

/// What an address is to notifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Public,
    /// Private, unique-local, shared (RFC 6598) or documentation.
    Private,
    Loopback,
    /// Link-local, unspecified, multicast, broadcast, benchmarking (198.18.0.0/15) or reserved (240.0.0.0/4).
    Refused,
}

fn class(address: IpAddr) -> Class {
    match address.to_canonical() {
        IpAddr::V4(v4) => class_v4(v4),
        IpAddr::V6(v6) if v6.is_loopback() => Class::Loopback,
        IpAddr::V6(v6) if v6.is_unspecified() => Class::Refused,
        IpAddr::V6(v6) => embedded_v4(v6).map_or_else(|| class_v6(v6), class_v4),
    }
}

fn class_v4(v4: Ipv4Addr) -> Class {
    let [a, b, ..] = v4.octets();
    if v4.is_loopback() {
        Class::Loopback
    } else if a == 0
        || a >= 240
        || v4.is_link_local()
        || v4.is_multicast()
        || (a == 198 && (b & 0xfe) == 18)
    {
        Class::Refused
    } else if v4.is_private() || v4.is_documentation() || (a == 100 && (64..128).contains(&b)) {
        Class::Private
    } else {
        Class::Public
    }
}

fn class_v6(v6: Ipv6Addr) -> Class {
    let segments = v6.segments();
    if v6.is_multicast()
        || v6.is_unicast_link_local()
        // Local-use NAT64 (RFC 8215): the embedded IPv4 address depends on the translator's prefix length.
        || segments[..3] == [0x64, 0xff9b, 1]
    {
        Class::Refused
    } else if v6.is_unique_local() || segments[..2] == [0x2001, 0xdb8] {
        Class::Private
    } else {
        Class::Public
    }
}

/// The IPv4 address an IPv6 one stands for: NAT64 (RFC 6052), 6to4 (RFC 3056), IPv4-compatible (deprecated).
fn embedded_v4(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let v4 = |high: u16, low: u16| Some(Ipv4Addr::from((u32::from(high) << 16) | u32::from(low)));
    match v6.segments() {
        [0x64, 0xff9b, 0, 0, 0, 0, high, low] | [0, 0, 0, 0, 0, 0, high, low] => v4(high, low),
        [0x2002, high, low, ..] => v4(high, low),
        _ => None,
    }
}

/// The URL as stored: `http`/`https`, a host, no credentials, no fragment; an address literal outside `reach` is
/// refused.
pub fn check_url(raw: &str, reach: Reach) -> Result<Url, UrlError> {
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
    if let Some(address) = literal {
        reach.check(address)?;
    }
    url.set_fragment(None);
    Ok(url)
}

/// No address of the name is within reach.
#[derive(Debug, thiserror::Error)]
#[error("the host resolves to no address notifications may reach")]
struct NoReachableAddress;

/// The system resolver keeping only the addresses within reach.
struct Within(Reach);

impl Resolve for Within {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        let reach = self.0;
        Box::pin(async move {
            let kept: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|address| reach.allows(address.ip()))
                .collect();
            if kept.is_empty() {
                return Err(NoReachableAddress.into());
            }
            let addresses: Addrs = Box::new(kept.into_iter());
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
    /// The host resolves only to addresses out of reach.
    #[error("the host resolves only to addresses this server does not send notifications to")]
    Unreachable,
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
        if cause.is::<NoReachableAddress>() {
            return SendError::Unreachable;
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
    reach: Reach,
}

impl Outbound {
    /// A client without redirects, with [`TIMEOUT`], whose resolver drops addresses out of `reach`; no proxy from the
    /// environment is used (a proxy would resolve names itself).
    pub fn new(reach: Reach) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .redirect(redirect::Policy::none())
            .timeout(TIMEOUT)
            .user_agent("diffpilot")
            .no_proxy()
            .dns_resolver(Within(reach))
            .build()?;
        Ok(Self { client, reach })
    }

    /// One `POST` of `body` with `headers`; the HTTP status of a 2xx answer.
    pub async fn post(
        &self,
        url: &Url,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> Result<u16, SendError> {
        let url = check_url(url.as_str(), self.reach)?;
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

    const LAN: Reach = Reach {
        private: true,
        loopback: false,
    };
    const PUBLIC: Reach = Reach {
        private: false,
        loopback: false,
    };

    #[test]
    fn notify_rejects_non_http_scheme() {
        for raw in [
            "ftp://ntfy.example/topic",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "gopher://ntfy.example",
        ] {
            assert_eq!(check_url(raw, LAN), Err(UrlError::Scheme), "{raw}");
        }
        assert_eq!(check_url("ntfy.example", LAN), Err(UrlError::Invalid));
        assert_eq!(
            check_url("https://user:pass@ntfy.example", LAN),
            Err(UrlError::Credentials)
        );
        assert_eq!(
            check_url("https://ntfy.example/#frag", LAN)
                .map(String::from)
                .as_deref(),
            Ok("https://ntfy.example/")
        );
    }

    #[test]
    fn private_literals_pass_unless_denied() {
        for raw in [
            "http://192.168.1.10/",
            "http://10.0.0.1/",
            "http://172.16.5.4:8123/api/webhook/x",
            "http://[fd00::1]/",
            "http://100.64.0.1/",
            "http://[64:ff9b::c0a8:10a]/",
            "http://[2002:c0a8:10a::1]/",
        ] {
            assert!(check_url(raw, LAN).is_ok(), "{raw}");
            assert_eq!(check_url(raw, PUBLIC), Err(UrlError::Private), "{raw}");
        }
        assert!(check_url("https://1.1.1.1/", PUBLIC).is_ok());
        assert!(check_url("https://[2606:4700::1111]/", PUBLIC).is_ok());
        assert!(check_url("https://ntfy.sh/topic", PUBLIC).is_ok());
    }

    #[test]
    fn notify_refuses_loopback_and_link_local_even_when_private_allowed() {
        for raw in [
            "http://127.0.0.1:8123/api/webhook/x",
            "http://127.1.2.3/",
            "http://[::1]/",
            "http://169.254.169.254/latest/meta-data",
            "http://[fe80::1]/",
            "http://0.0.0.0/",
            "http://[::]/",
            "http://0.1.2.3/",
            "http://224.0.0.1/",
            "http://[ff02::1]/",
            "http://255.255.255.255/",
            "http://198.18.0.1/",
            "http://198.19.255.254/",
            "http://240.0.0.1/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:169.254.169.254]/",
            "http://[64:ff9b::7f00:1]/",
            "http://[64:ff9b::a9fe:a9fe]/",
            "http://[64:ff9b:1::1]/",
            "http://[2002:7f00:1::1]/",
            "http://[2002:a9fe:a9fe::1]/",
            "http://[::127.0.0.1]/",
        ] {
            assert_eq!(check_url(raw, LAN), Err(UrlError::Address), "{raw}");
            assert!(check_url(raw, PUBLIC).is_err(), "{raw}");
        }
        assert!(
            check_url(
                "http://127.0.0.1/",
                Reach {
                    private: true,
                    loopback: true
                }
            )
            .is_ok(),
            "tests reach their own targets"
        );
        assert!(check_url("http://198.20.0.1/", PUBLIC).is_ok());
        assert!(check_url("http://[2002:0101:0101::1]/", PUBLIC).is_ok());
    }
}
