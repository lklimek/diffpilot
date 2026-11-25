//! Server configuration from `SR_*` environment variables (ADR-001).

use std::net::{AddrParseError, IpAddr, SocketAddr};
use std::num::ParseIntError;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ipnet::IpNet;
use secrecy::zeroize::Zeroizing;
use secrecy::{SecretBox, SecretString};
use url::Url;

/// Public base URL of the instance, e.g. `https://review.klimek.ws`.
pub const PUBLIC_URL: &str = "SR_PUBLIC_URL";
/// Comma-separated GitHub logins allowed to sign in (default tenant), applied at every start (ADR-007).
pub const ALLOWED_LOGINS: &str = "SR_ALLOWED_LOGINS";
/// Address the HTTP server listens on.
pub const LISTEN_ADDR: &str = "SR_LISTEN_ADDR";
/// Address the runner-only listener (`/runner/v1/*`) listens on; never exposed through the reverse proxy.
pub const RUNNER_LISTEN_ADDR: &str = "SR_RUNNER_LISTEN_ADDR";
/// Address for GitHub App deliveries.
pub const WEBHOOK_LISTEN_ADDR: &str = "SR_WEBHOOK_LISTEN_ADDR";
/// Comma-separated IP addresses or networks of reverse proxies whose forwarding headers are trusted.
pub const TRUSTED_PROXIES: &str = "SR_TRUSTED_PROXIES";
/// Directory for the SQLite database and the bare mirrors.
pub const DATA_DIR: &str = "SR_DATA_DIR";
/// Base64 of the 32-byte AES-GCM key that encrypts stored tokens.
pub const SECRET_KEY: &str = "SR_SECRET_KEY";
/// Shared secret runners present when claiming jobs.
pub const RUNNER_SECRET: &str = "SR_RUNNER_SECRET";
/// GitHub App id.
pub const GITHUB_APP_ID: &str = "SR_GITHUB_APP_ID";
/// GitHub App OAuth client id.
pub const GITHUB_CLIENT_ID: &str = "SR_GITHUB_CLIENT_ID";
/// GitHub App OAuth client secret.
pub const GITHUB_CLIENT_SECRET: &str = "SR_GITHUB_CLIENT_SECRET";
/// Path of the GitHub App private key (.pem, read-only mount).
pub const GITHUB_APP_PRIVATE_KEY_FILE: &str = "SR_GITHUB_APP_PRIVATE_KEY_FILE";
/// GitHub REST API base URL (GraphQL is `<base>/graphql`).
pub const GITHUB_API_URL: &str = "SR_GITHUB_API_URL";
/// GitHub OAuth base URL (`<base>/login/oauth/...`).
pub const GITHUB_OAUTH_URL: &str = "SR_GITHUB_OAUTH_URL";
/// Base URL repositories are cloned from (`<base>/<owner>/<name>.git`); `file://` allowed for development.
pub const GITHUB_CLONE_URL: &str = "SR_GITHUB_CLONE_URL";
/// Optional comma-separated trusted maintainer relay logins; empty by default.
pub const MAINTAINER_DELEGATES: &str = "SR_MAINTAINER_DELEGATES";
/// MemCan MCP endpoint URL.
pub const MEMCAN_URL: &str = "SR_MEMCAN_URL";
/// MemCan bearer token.
pub const MEMCAN_TOKEN: &str = "SR_MEMCAN_TOKEN";
/// MemCan web UI base including `/ui`; dashboard TODO links are `<base>/tasks/<uuid>` (optional).
pub const MEMCAN_UI_URL: &str = "SR_MEMCAN_UI_URL";
/// Base URL of the PR Hygiene service; set, the dashboard's PR Hygiene plug-in is on (optional, ADR-022).
pub const PR_HYGIENE_URL: &str = "SR_PR_HYGIENE_URL";
/// URL of thepastaclaw's review-system `status.json`; set, the dashboard's thepastaclaw plug-in is on (optional,
/// ADR-022).
pub const THEPASTACLAW_STATUS_URL: &str = "SR_THEPASTACLAW_STATUS_URL";
/// `1`: notification channels and agent push targets may not reach private, loopback or link-local addresses
/// (plan §5.5); unset or `0`: they may (LAN targets such as Home Assistant).
pub const NOTIFY_DENY_PRIVATE: &str = "SR_NOTIFY_DENY_PRIVATE";
/// Path of a file holding `SR_SECRET_KEY`; exactly one of the two may be set.
pub const SECRET_KEY_FILE: &str = "SR_SECRET_KEY_FILE";
/// Path of a file holding `SR_RUNNER_SECRET`; exactly one of the two may be set.
pub const RUNNER_SECRET_FILE: &str = "SR_RUNNER_SECRET_FILE";
/// Path of a file holding `SR_GITHUB_CLIENT_SECRET`; exactly one of the two may be set.
pub const GITHUB_CLIENT_SECRET_FILE: &str = "SR_GITHUB_CLIENT_SECRET_FILE";
/// Path of a file holding `SR_MEMCAN_TOKEN`; exactly one of the two may be set.
pub const MEMCAN_TOKEN_FILE: &str = "SR_MEMCAN_TOKEN_FILE";

const DEFAULT_LISTEN_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 3000);
const DEFAULT_RUNNER_LISTEN_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 3001);
const DEFAULT_WEBHOOK_LISTEN_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 3002);
const DEFAULT_DATA_DIR: &str = "data";
const DEFAULT_GITHUB_API_URL: &str = "https://api.github.com";
const DEFAULT_GITHUB_WEB_URL: &str = "https://github.com";
const SECRET_KEY_LEN: usize = 32;
/// Longest GitHub login.
const MAX_LOGIN_LEN: usize = 39;
/// Largest secret file read.
const MAX_SECRET_FILE_BYTES: u64 = 64 * 1024;

/// Complete server configuration; secrets are redacted in `Debug`.
#[derive(Debug)]
pub struct Config {
    /// Public base URL; OAuth callback is `<public_url>/auth/github/callback`.
    pub public_url: Url,
    /// HTTP listen address (default `0.0.0.0:3000`); TLS is terminated by the reverse proxy.
    pub listen_addr: SocketAddr,
    /// Runner-only listen address (default `0.0.0.0:3001`); serves only the runner protocol.
    pub runner_listen_addr: SocketAddr,
    /// GitHub webhook-only listen address (default `127.0.0.1:3002`).
    pub webhook_listen_addr: SocketAddr,
    /// GitHub logins allowed to sign in to the default tenant; at least one, unique ignoring ASCII case.
    pub allowed_logins: Vec<String>,
    /// Explicit trusted relays, independent of tenant membership or bot status.
    pub maintainer_delegates: Vec<String>,
    /// Reverse proxies whose forwarding headers are trusted (default: none).
    pub trusted_proxies: Vec<IpNet>,
    /// Directory for the database and the bare mirrors (default `data`).
    pub data_dir: PathBuf,
    /// AES-GCM key for tokens at rest.
    pub secret_key: SecretBox<[u8; SECRET_KEY_LEN]>,
    /// Secret runners authenticate with.
    pub runner_secret: SecretString,
    /// Credentials-scoped gateway administration; absent until configured.
    pub codex_gateway: Option<crate::gateway::client::Admin>,
    /// GitHub App and endpoints.
    pub github: GithubConfig,
    /// MemCan endpoint and token.
    pub memcan: MemcanConfig,
    /// MemCan web UI base, no trailing slash; links are `<value>/tasks/<uuid>` (default: none, no links).
    pub memcan_ui_url: Option<String>,
    /// PR Hygiene service base, the dashboard plug-in's source (default: none, plug-in off; ADR-022).
    pub pr_hygiene_url: Option<Url>,
    /// thepastaclaw's status file, the dashboard plug-in's source (default: none, plug-in off; ADR-022).
    pub thepastaclaw_status_url: Option<Url>,
    /// Outbound notifications refuse private addresses (`SR_NOTIFY_DENY_PRIVATE=1`).
    pub notify_deny_private: bool,
    /// Tests only: notifications may reach loopback, where the in-test HTTP targets listen.
    #[cfg(test)]
    pub notify_loopback: bool,
}

/// GitHub App credentials and GitHub base URLs (configurable for the fake GitHub).
#[derive(Debug)]
pub struct GithubConfig {
    /// GitHub App id.
    pub app_id: u64,
    /// OAuth client id of the App.
    pub client_id: String,
    /// OAuth client secret of the App.
    pub client_secret: SecretString,
    /// Path of the App private key; read only by `github::app_token`.
    pub private_key_file: PathBuf,
    /// REST API base URL.
    pub api_url: Url,
    /// OAuth base URL.
    pub oauth_url: Url,
    /// Clone base URL.
    pub clone_url: Url,
}

/// MemCan connection settings.
#[derive(Debug)]
pub struct MemcanConfig {
    /// Observations shared by all calls using this configuration.
    pub observed: crate::memcan::status::ObservedStatus,
    /// MCP endpoint URL.
    pub url: Url,
    /// Bearer token.
    pub token: SecretString,
}

/// Invalid or missing configuration. Messages name the variable, never its value.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The credentials administration client could not be configured.
    #[error("the gateway administration client is invalid")]
    Gateway(#[from] crate::gateway::client::Error),
    /// A required variable is unset or empty.
    #[error("required environment variable {key} is not set")]
    Missing {
        /// Variable name.
        key: &'static str,
    },
    /// The value is not valid UTF-8.
    #[error("environment variable {key} is not valid UTF-8")]
    NotUnicode {
        /// Variable name.
        key: &'static str,
    },
    /// The value is not a URL.
    #[error("environment variable {key} is not a valid URL")]
    InvalidUrl {
        /// Variable name.
        key: &'static str,
        /// Parse error.
        #[source]
        source: url::ParseError,
    },
    /// The URL scheme is not allowed for this variable.
    #[error("environment variable {key} must be an http or https URL")]
    UnsupportedUrlScheme {
        /// Variable name.
        key: &'static str,
    },
    /// The value is not a non-negative integer.
    #[error("environment variable {key} is not a valid number")]
    InvalidNumber {
        /// Variable name.
        key: &'static str,
        /// Parse error.
        #[source]
        source: ParseIntError,
    },
    /// The value is neither `0` nor `1`.
    #[error("environment variable {key} must be 0 or 1")]
    InvalidFlag {
        /// Variable name.
        key: &'static str,
    },
    /// The value is not a socket address.
    #[error("environment variable {key} is not a valid address such as 0.0.0.0:3000")]
    InvalidSocketAddr {
        /// Variable name.
        key: &'static str,
        /// Parse error.
        #[source]
        source: AddrParseError,
    },
    /// One entry of the trusted-proxy list is neither an IP address nor a network.
    #[error("environment variable {key} contains '{entry}', which is not an IP address or network")]
    InvalidTrustedProxy {
        /// Variable name.
        key: &'static str,
        /// The rejected entry (not secret).
        entry: String,
        /// Parse error.
        #[source]
        source: ipnet::AddrParseError,
    },
    /// The allowed-login list names no login.
    #[error("environment variable {key} names no GitHub login, so nobody could sign in")]
    NoAllowedLogins {
        /// Variable name.
        key: &'static str,
    },
    /// One entry of the allowed-login list is not a GitHub login.
    #[error(
        "environment variable {key} contains '{entry}', which is not a GitHub login (letters, digits, - or _, at most 39 characters)"
    )]
    InvalidLogin {
        /// Variable name.
        key: &'static str,
        /// The rejected entry (not secret).
        entry: String,
    },
    /// An entry still holds the `.env.example` placeholder.
    #[error(
        "environment variable {key} still holds the placeholder '{entry}'; list the GitHub logins allowed to sign in"
    )]
    PlaceholderLogin {
        /// Variable name.
        key: &'static str,
        /// The placeholder entry.
        entry: String,
    },
    /// A login is listed twice (logins are case-insensitive).
    #[error("environment variable {key} lists '{entry}' twice; GitHub logins are case-insensitive")]
    DuplicateLogin {
        /// Variable name.
        key: &'static str,
        /// The repeated entry as written the second time.
        entry: String,
    },
    /// The secret key is not base64.
    #[error("environment variable {key} is not valid base64")]
    SecretKeyNotBase64 {
        /// Variable name.
        key: &'static str,
        /// Decode error.
        #[source]
        source: base64::DecodeError,
    },
    /// The public and the runner listener would share one address.
    #[error("{key} and {other} must differ: the runner protocol has its own listener")]
    ListenersCollide {
        /// Variable name.
        key: &'static str,
        /// The other variable.
        other: &'static str,
    },
    /// A secret is given both directly and as a file.
    #[error("set only one of {key} and {file_key}")]
    BothSet {
        /// Variable name.
        key: &'static str,
        /// Name of its file variant.
        file_key: &'static str,
    },
    /// A secret file cannot be read.
    #[error("the file named by {key} ({}) cannot be read", path.display())]
    SecretFileUnreadable {
        /// Variable name.
        key: &'static str,
        /// The file.
        path: PathBuf,
        /// Read error (never the contents).
        #[source]
        source: std::io::Error,
    },
    /// A secret file is not a regular file, is empty or is larger than any secret.
    #[error("the file named by {key} ({}) is {problem}", path.display())]
    SecretFileInvalid {
        /// Variable name.
        key: &'static str,
        /// The file.
        path: PathBuf,
        /// What is wrong.
        problem: &'static str,
    },
    /// The secret key does not decode to 32 bytes.
    #[error("environment variable {key} must decode to exactly 32 bytes, got {len}")]
    SecretKeyLength {
        /// Variable name.
        key: &'static str,
        /// Decoded length.
        len: usize,
    },
}

impl Config {
    /// Reads the configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| match std::env::var(key) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::NotUnicode { key }),
        })
    }

    /// Reads the configuration through `lookup`; values are trimmed and empty values count as unset.
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&'static str) -> Result<Option<String>, ConfigError>,
    {
        let get = |key| -> Result<Option<String>, ConfigError> {
            Ok(lookup(key)?
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()))
        };
        let require = |key| get(key)?.ok_or(ConfigError::Missing { key });
        let secret = |key, file_key| -> Result<String, ConfigError> {
            match (get(key)?, get(file_key)?) {
                (Some(_), Some(_)) => Err(ConfigError::BothSet { key, file_key }),
                (Some(value), None) => Ok(value),
                (None, Some(path)) => read_secret_file(file_key, Path::new(&path)),
                (None, None) => Err(ConfigError::Missing { key }),
            }
        };
        let or_default = |key, default: &'static str| -> Result<String, ConfigError> {
            Ok(get(key)?.unwrap_or_else(|| default.to_owned()))
        };

        let socket_addr = |key, default| -> Result<SocketAddr, ConfigError> {
            match get(key)? {
                Some(value) => value
                    .parse()
                    .map_err(|source| ConfigError::InvalidSocketAddr { key, source }),
                None => Ok(default),
            }
        };
        let listen_addr = socket_addr(LISTEN_ADDR, DEFAULT_LISTEN_ADDR)?;
        let runner_listen_addr = socket_addr(RUNNER_LISTEN_ADDR, DEFAULT_RUNNER_LISTEN_ADDR)?;
        let webhook_listen_addr = socket_addr(WEBHOOK_LISTEN_ADDR, DEFAULT_WEBHOOK_LISTEN_ADDR)?;
        for (key, addr, other, other_addr) in [
            (
                RUNNER_LISTEN_ADDR,
                runner_listen_addr,
                LISTEN_ADDR,
                listen_addr,
            ),
            (
                WEBHOOK_LISTEN_ADDR,
                webhook_listen_addr,
                LISTEN_ADDR,
                listen_addr,
            ),
            (
                WEBHOOK_LISTEN_ADDR,
                webhook_listen_addr,
                RUNNER_LISTEN_ADDR,
                runner_listen_addr,
            ),
        ] {
            if addr.port() == other_addr.port()
                && (addr.ip() == other_addr.ip()
                    || addr.ip().is_unspecified()
                    || other_addr.ip().is_unspecified())
            {
                return Err(ConfigError::ListenersCollide { key, other });
            }
        }
        let trusted_proxies = match get(TRUSTED_PROXIES)? {
            Some(value) => parse_trusted_proxies(&value)?,
            None => Vec::new(),
        };

        Ok(Self {
            public_url: parse_http_url(PUBLIC_URL, &require(PUBLIC_URL)?)?,
            allowed_logins: parse_allowed_logins(&require(ALLOWED_LOGINS)?)?,
            maintainer_delegates: get(MAINTAINER_DELEGATES)?
                .map(|value| parse_logins(MAINTAINER_DELEGATES, &value))
                .transpose()?
                .unwrap_or_default(),
            listen_addr,
            runner_listen_addr,
            webhook_listen_addr,
            trusted_proxies,
            data_dir: PathBuf::from(or_default(DATA_DIR, DEFAULT_DATA_DIR)?),
            secret_key: parse_secret_key(&secret(SECRET_KEY, SECRET_KEY_FILE)?)?,
            runner_secret: SecretString::from(secret(RUNNER_SECRET, RUNNER_SECRET_FILE)?),
            codex_gateway: get("SR_GATEWAY_ADMIN_URL")?
                .map(|value| {
                    let url = parse_http_url("SR_GATEWAY_ADMIN_URL", &value)?;
                    let token = secret(
                        "SR_GATEWAY_CREDENTIALS_TOKEN",
                        "SR_GATEWAY_CREDENTIALS_TOKEN_FILE",
                    )?;
                    crate::gateway::client::Admin::new(url, token.into())
                        .map_err(ConfigError::Gateway)
                })
                .transpose()?,
            github: GithubConfig {
                app_id: require(GITHUB_APP_ID)?.parse().map_err(|source| {
                    ConfigError::InvalidNumber {
                        key: GITHUB_APP_ID,
                        source,
                    }
                })?,
                client_id: require(GITHUB_CLIENT_ID)?,
                client_secret: SecretString::from(secret(
                    GITHUB_CLIENT_SECRET,
                    GITHUB_CLIENT_SECRET_FILE,
                )?),
                private_key_file: PathBuf::from(require(GITHUB_APP_PRIVATE_KEY_FILE)?),
                api_url: parse_http_url(
                    GITHUB_API_URL,
                    &or_default(GITHUB_API_URL, DEFAULT_GITHUB_API_URL)?,
                )?,
                oauth_url: parse_http_url(
                    GITHUB_OAUTH_URL,
                    &or_default(GITHUB_OAUTH_URL, DEFAULT_GITHUB_WEB_URL)?,
                )?,
                clone_url: parse_url(
                    GITHUB_CLONE_URL,
                    &or_default(GITHUB_CLONE_URL, DEFAULT_GITHUB_WEB_URL)?,
                )?,
            },
            memcan: MemcanConfig {
                observed: Default::default(),
                url: parse_http_url(MEMCAN_URL, &require(MEMCAN_URL)?)?,
                token: SecretString::from(secret(MEMCAN_TOKEN, MEMCAN_TOKEN_FILE)?),
            },
            memcan_ui_url: get(MEMCAN_UI_URL)?
                .map(|value| parse_http_url(MEMCAN_UI_URL, &value))
                .transpose()?
                .map(|url| {
                    let url = String::from(url);
                    url.strip_suffix('/').map(str::to_owned).unwrap_or(url)
                }),
            pr_hygiene_url: get(PR_HYGIENE_URL)?
                .map(|value| parse_http_url(PR_HYGIENE_URL, &value))
                .transpose()?,
            thepastaclaw_status_url: get(THEPASTACLAW_STATUS_URL)?
                .map(|value| parse_http_url(THEPASTACLAW_STATUS_URL, &value))
                .transpose()?,
            notify_deny_private: match get(NOTIFY_DENY_PRIVATE)?.as_deref() {
                None | Some("0") => false,
                Some("1") => true,
                Some(_) => {
                    return Err(ConfigError::InvalidFlag {
                        key: NOTIFY_DENY_PRIVATE,
                    });
                }
            },
            #[cfg(test)]
            notify_loopback: true,
        })
    }
}

/// Reads a secret file once (a regular file, symlinks followed); trailing newlines are removed. Errors name the
/// variable and the path, never the contents; a file every user may read is reported with `warn!`.
fn read_secret_file(key: &'static str, path: &Path) -> Result<String, ConfigError> {
    let invalid = |problem| ConfigError::SecretFileInvalid {
        key,
        path: path.to_path_buf(),
        problem,
    };
    let unreadable = |source| ConfigError::SecretFileUnreadable {
        key,
        path: path.to_path_buf(),
        source,
    };
    let metadata = std::fs::metadata(path).map_err(unreadable)?;
    if !metadata.is_file() {
        return Err(invalid("not a regular file"));
    }
    if metadata.len() > MAX_SECRET_FILE_BYTES {
        return Err(invalid("larger than any secret"));
    }
    if metadata.permissions().mode() & 0o004 != 0 {
        tracing::warn!(variable = key, path = %path.display(),
            "secret file is readable by every user; restrict it to mode 0400 or 0600");
    }
    let mut value = std::fs::read_to_string(path).map_err(unreadable)?;
    let kept = value.trim_end_matches(['\n', '\r']).len();
    value.truncate(kept);
    if value.is_empty() {
        return Err(invalid("empty"));
    }
    Ok(value)
}

fn parse_url(key: &'static str, value: &str) -> Result<Url, ConfigError> {
    Url::parse(value).map_err(|source| ConfigError::InvalidUrl { key, source })
}

fn parse_http_url(key: &'static str, value: &str) -> Result<Url, ConfigError> {
    let url = parse_url(key, value)?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        _ => Err(ConfigError::UnsupportedUrlScheme { key }),
    }
}

fn parse_trusted_proxies(value: &str) -> Result<Vec<IpNet>, ConfigError> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .parse::<IpNet>()
                .or_else(|error| entry.parse::<IpAddr>().map(IpNet::from).map_err(|_| error))
                .map_err(|source| ConfigError::InvalidTrustedProxy {
                    key: TRUSTED_PROXIES,
                    entry: entry.to_owned(),
                    source,
                })
        })
        .collect()
}

/// A GitHub login as the dashboard settings accept one: ASCII letters, digits, `-` or `_` (Enterprise Managed Users).
fn is_github_login(entry: &str) -> bool {
    entry.len() <= MAX_LOGIN_LEN
        && entry
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn parse_allowed_logins(value: &str) -> Result<Vec<String>, ConfigError> {
    parse_logins(ALLOWED_LOGINS, value)
}

fn parse_logins(key: &'static str, value: &str) -> Result<Vec<String>, ConfigError> {
    let mut logins: Vec<String> = Vec::new();
    for entry in value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        // Checked as check-production-config.sh does: a start with the example value would drop every bound entry.
        if entry.contains("REPLACE") {
            return Err(ConfigError::PlaceholderLogin {
                key,
                entry: entry.to_owned(),
            });
        }
        if !is_github_login(entry) {
            return Err(ConfigError::InvalidLogin {
                key,
                entry: entry.to_owned(),
            });
        }
        if logins.iter().any(|login| login.eq_ignore_ascii_case(entry)) {
            return Err(ConfigError::DuplicateLogin {
                key,
                entry: entry.to_owned(),
            });
        }
        logins.push(entry.to_owned());
    }
    if logins.is_empty() {
        return Err(ConfigError::NoAllowedLogins { key });
    }
    Ok(logins)
}

fn parse_secret_key(value: &str) -> Result<SecretBox<[u8; SECRET_KEY_LEN]>, ConfigError> {
    let bytes =
        Zeroizing::new(
            BASE64
                .decode(value)
                .map_err(|source| ConfigError::SecretKeyNotBase64 {
                    key: SECRET_KEY,
                    source,
                })?,
        );
    let key: [u8; SECRET_KEY_LEN] =
        bytes
            .as_slice()
            .try_into()
            .map_err(|_| ConfigError::SecretKeyLength {
                key: SECRET_KEY,
                len: bytes.len(),
            })?;
    Ok(SecretBox::new(Box::new(key)))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;

    use secrecy::ExposeSecret;

    use super::*;

    const KEY_B64: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
    const CLIENT_SECRET: &str = "client-secret-value-123";
    const RUNNER_SECRET_VALUE: &str = "runner-secret-value-456";
    const MEMCAN_TOKEN_VALUE: &str = "memcan-token-value-789";

    const REQUIRED: [&str; 10] = [
        PUBLIC_URL,
        ALLOWED_LOGINS,
        SECRET_KEY,
        RUNNER_SECRET,
        GITHUB_APP_ID,
        GITHUB_CLIENT_ID,
        GITHUB_CLIENT_SECRET,
        GITHUB_APP_PRIVATE_KEY_FILE,
        MEMCAN_URL,
        MEMCAN_TOKEN,
    ];

    fn required_env() -> HashMap<&'static str, String> {
        HashMap::from([
            (PUBLIC_URL, "https://review.example.test".to_owned()),
            (ALLOWED_LOGINS, "lklimek,Claudius-Maginificent".to_owned()),
            (SECRET_KEY, KEY_B64.to_owned()),
            (RUNNER_SECRET, RUNNER_SECRET_VALUE.to_owned()),
            (GITHUB_APP_ID, "123456".to_owned()),
            (GITHUB_CLIENT_ID, "Iv1.client".to_owned()),
            (GITHUB_CLIENT_SECRET, CLIENT_SECRET.to_owned()),
            (
                GITHUB_APP_PRIVATE_KEY_FILE,
                "/run/secrets/app.pem".to_owned(),
            ),
            (MEMCAN_URL, "http://memcan.example.test:8190/mcp".to_owned()),
            (MEMCAN_TOKEN, MEMCAN_TOKEN_VALUE.to_owned()),
        ])
    }

    fn parse(env: &HashMap<&'static str, String>) -> Result<Config, ConfigError> {
        Config::from_lookup(|key| Ok(env.get(key).cloned()))
    }

    /// A valid configuration for tests of other modules.
    pub(crate) fn test_config() -> Config {
        parse(&required_env()).expect("required test environment is valid")
    }

    #[test]
    fn maintainer_relays_require_explicit_valid_configuration() {
        let mut env = required_env();
        assert!(parse(&env).unwrap().maintainer_delegates.is_empty());
        env.insert(MAINTAINER_DELEGATES, "Relay-Bot, relay_user".into());
        assert_eq!(
            parse(&env).unwrap().maintainer_delegates,
            vec!["Relay-Bot", "relay_user"]
        );
        env.insert(MAINTAINER_DELEGATES, "bad/login".into());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::InvalidLogin {
                key: MAINTAINER_DELEGATES,
                ..
            })
        ));
    }

    #[test]
    fn required_variables_only_use_defaults() {
        let config = parse(&required_env()).expect("valid");
        assert_eq!(config.public_url.as_str(), "https://review.example.test/");
        assert_eq!(config.listen_addr, "0.0.0.0:3000".parse().expect("addr"));
        assert_eq!(
            config.runner_listen_addr,
            "0.0.0.0:3001".parse().expect("addr")
        );
        assert!(config.trusted_proxies.is_empty());
        assert_eq!(config.data_dir, PathBuf::from("data"));
        let expected_key: Vec<u8> = (0u8..32).collect();
        assert_eq!(
            config.secret_key.expose_secret().as_slice(),
            expected_key.as_slice()
        );
        assert_eq!(config.runner_secret.expose_secret(), RUNNER_SECRET_VALUE);
        assert_eq!(config.github.app_id, 123_456);
        assert_eq!(config.github.client_id, "Iv1.client");
        assert_eq!(config.github.client_secret.expose_secret(), CLIENT_SECRET);
        assert_eq!(
            config.github.private_key_file,
            PathBuf::from("/run/secrets/app.pem")
        );
        assert_eq!(config.github.api_url.as_str(), "https://api.github.com/");
        assert_eq!(config.github.oauth_url.as_str(), "https://github.com/");
        assert_eq!(config.github.clone_url.as_str(), "https://github.com/");
        assert_eq!(
            config.memcan.url.as_str(),
            "http://memcan.example.test:8190/mcp"
        );
        assert_eq!(config.memcan.token.expose_secret(), MEMCAN_TOKEN_VALUE);
        assert_eq!(config.allowed_logins, ["lklimek", "Claudius-Maginificent"]);
    }

    #[test]
    fn allowed_logins_are_trimmed_and_empty_entries_skipped() {
        let mut env = required_env();
        env.insert(ALLOWED_LOGINS, " alice , ,Bob-2,carol_acme,".to_owned());
        let config = parse(&env).expect("valid");
        assert_eq!(config.allowed_logins, ["alice", "Bob-2", "carol_acme"]);
    }

    #[test]
    fn allowed_logins_must_name_at_least_one_login() {
        let mut env = required_env();
        env.insert(ALLOWED_LOGINS, " , ,".to_owned());
        assert!(
            matches!(
                parse(&env),
                Err(ConfigError::NoAllowedLogins {
                    key: ALLOWED_LOGINS
                })
            ),
            "only separators"
        );
    }

    #[test]
    fn allowed_login_that_is_not_a_github_login_is_named() {
        let too_long = "a".repeat(40);
        for entry in [
            "@alice",
            "alice smith",
            "alice.smith",
            "ąlice",
            "alice;bob",
            too_long.as_str(),
        ] {
            let mut env = required_env();
            env.insert(ALLOWED_LOGINS, format!("bob,{entry}"));
            match parse(&env) {
                Err(error @ ConfigError::InvalidLogin { .. }) => {
                    assert!(error.to_string().contains(&format!("'{entry}'")), "{error}");
                }
                other => panic!("{entry}: expected InvalidLogin, got {other:?}"),
            }
        }
        let mut env = required_env();
        env.insert(ALLOWED_LOGINS, "a".repeat(39));
        assert!(parse(&env).is_ok(), "39 characters is GitHub's maximum");
    }

    #[test]
    fn allowed_login_still_holding_the_example_placeholder_is_refused() {
        for value in ["REPLACE_WITH_GITHUB_LOGINS", "alice,REPLACE_ME"] {
            let mut env = required_env();
            env.insert(ALLOWED_LOGINS, value.to_owned());
            match parse(&env) {
                Err(error @ ConfigError::PlaceholderLogin { .. }) => {
                    assert!(error.to_string().contains("placeholder"), "{error}");
                }
                other => panic!("{value}: expected PlaceholderLogin, got {other:?}"),
            }
        }
    }

    #[test]
    fn allowed_login_listed_twice_is_named_case_insensitively() {
        let mut env = required_env();
        env.insert(ALLOWED_LOGINS, "alice,bob,ALICE".to_owned());
        match parse(&env) {
            Err(error @ ConfigError::DuplicateLogin { .. }) => {
                assert!(error.to_string().contains("'ALICE'"), "{error}");
            }
            other => panic!("expected DuplicateLogin, got {other:?}"),
        }
    }

    #[test]
    fn required_variable_names_match_adr_001() {
        let env: HashMap<&str, &str> = HashMap::from([
            ("SR_PUBLIC_URL", "https://review.example.test"),
            ("SR_ALLOWED_LOGINS", "octocat"),
            ("SR_SECRET_KEY", KEY_B64),
            ("SR_RUNNER_SECRET", "runner"),
            ("SR_GITHUB_APP_ID", "1"),
            ("SR_GITHUB_CLIENT_ID", "Iv1.client"),
            ("SR_GITHUB_CLIENT_SECRET", "secret"),
            ("SR_GITHUB_APP_PRIVATE_KEY_FILE", "/run/secrets/app.pem"),
            ("SR_MEMCAN_URL", "http://memcan.example.test"),
            ("SR_MEMCAN_TOKEN", "token"),
        ]);
        let result = Config::from_lookup(|key| Ok(env.get(key).map(|value| (*value).to_owned())));
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn optional_variables_override_defaults() {
        let mut env = required_env();
        env.insert(LISTEN_ADDR, "127.0.0.1:8080".to_owned());
        env.insert(RUNNER_LISTEN_ADDR, "172.20.0.2:9001".to_owned());
        env.insert(TRUSTED_PROXIES, " 172.18.0.0/16 , 10.0.0.5,::1 ".to_owned());
        env.insert(DATA_DIR, "/var/lib/diffpilot".to_owned());
        env.insert(GITHUB_API_URL, "http://127.0.0.1:9000/api".to_owned());
        env.insert(GITHUB_OAUTH_URL, "http://127.0.0.1:9000".to_owned());
        env.insert(GITHUB_CLONE_URL, "file:///data/tmp/remotes".to_owned());
        let config = parse(&env).expect("valid");
        assert_eq!(config.listen_addr, "127.0.0.1:8080".parse().expect("addr"));
        assert_eq!(
            config.runner_listen_addr,
            "172.20.0.2:9001".parse().expect("addr")
        );
        let proxies: Vec<String> = config
            .trusted_proxies
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(proxies, ["172.18.0.0/16", "10.0.0.5/32", "::1/128"]);
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/diffpilot"));
        assert_eq!(config.github.api_url.as_str(), "http://127.0.0.1:9000/api");
        assert_eq!(config.github.oauth_url.as_str(), "http://127.0.0.1:9000/");
        assert_eq!(config.github.clone_url.as_str(), "file:///data/tmp/remotes");
    }

    #[test]
    fn each_missing_required_variable_is_named() {
        for key in REQUIRED {
            let mut env = required_env();
            env.remove(key);
            match parse(&env) {
                Err(ConfigError::Missing { key: missing }) => assert_eq!(missing, key),
                other => panic!("{key}: expected Missing, got {other:?}"),
            }
        }
    }

    #[test]
    fn blank_value_counts_as_missing() {
        let mut env = required_env();
        env.insert(MEMCAN_TOKEN, "   ".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::Missing { key: MEMCAN_TOKEN })
        ));
    }

    #[test]
    fn blank_optional_value_uses_default() {
        let mut env = required_env();
        env.insert(LISTEN_ADDR, String::new());
        let config = parse(&env).expect("valid");
        assert_eq!(config.listen_addr, DEFAULT_LISTEN_ADDR);
    }

    #[test]
    fn lookup_error_is_propagated() {
        let result = Config::from_lookup(|key| Err(ConfigError::NotUnicode { key }));
        assert!(matches!(result, Err(ConfigError::NotUnicode { .. })));
    }

    #[test]
    fn secret_key_must_be_base64() {
        let mut env = required_env();
        env.insert(SECRET_KEY, "not base64 !!".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::SecretKeyNotBase64 {
                key: SECRET_KEY,
                ..
            })
        ));
    }

    #[test]
    fn secret_key_must_be_32_bytes() {
        let mut env = required_env();
        env.insert(SECRET_KEY, BASE64.encode([7u8; 16]));
        assert!(matches!(
            parse(&env),
            Err(ConfigError::SecretKeyLength {
                key: SECRET_KEY,
                len: 16
            })
        ));
    }

    #[test]
    fn invalid_url_is_rejected() {
        let mut env = required_env();
        env.insert(PUBLIC_URL, "review.example.test".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::InvalidUrl {
                key: PUBLIC_URL,
                ..
            })
        ));
    }

    #[test]
    fn http_urls_reject_other_schemes() {
        for key in [PUBLIC_URL, GITHUB_API_URL, GITHUB_OAUTH_URL, MEMCAN_URL] {
            let mut env = required_env();
            env.insert(key, "file:///etc/passwd".to_owned());
            match parse(&env) {
                Err(ConfigError::UnsupportedUrlScheme { key: rejected }) => {
                    assert_eq!(rejected, key)
                }
                other => panic!("{key}: expected UnsupportedUrlScheme, got {other:?}"),
            }
        }
    }

    #[test]
    fn pr_hygiene_url_is_optional_and_must_be_http() {
        assert_eq!(parse(&required_env()).expect("valid").pr_hygiene_url, None);
        let mut env = required_env();
        env.insert(PR_HYGIENE_URL, "https://pr-hygiene.example.test".to_owned());
        let url = parse(&env).expect("valid").pr_hygiene_url;
        assert_eq!(
            url.map(String::from).as_deref(),
            Some("https://pr-hygiene.example.test/")
        );
        env.insert(PR_HYGIENE_URL, "pr-hygiene.example.test".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::InvalidUrl {
                key: PR_HYGIENE_URL,
                ..
            })
        ));
        env.insert(PR_HYGIENE_URL, "ftp://pr-hygiene.example.test".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::UnsupportedUrlScheme {
                key: PR_HYGIENE_URL
            })
        ));
    }

    #[test]
    fn notify_deny_private_is_off_unless_one() {
        assert!(!parse(&required_env()).expect("valid").notify_deny_private);
        let mut env = required_env();
        for (value, expected) in [("0", false), ("1", true)] {
            env.insert(NOTIFY_DENY_PRIVATE, value.to_owned());
            assert_eq!(parse(&env).expect("valid").notify_deny_private, expected);
        }
        env.insert(NOTIFY_DENY_PRIVATE, "yes".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::InvalidFlag {
                key: NOTIFY_DENY_PRIVATE
            })
        ));
    }

    #[test]
    fn thepastaclaw_status_url_is_optional_and_must_be_http() {
        assert_eq!(
            parse(&required_env())
                .expect("valid")
                .thepastaclaw_status_url,
            None
        );
        let mut env = required_env();
        let file = "https://thepastaclaw.example.test/review-system/data/status.json";
        env.insert(THEPASTACLAW_STATUS_URL, file.to_owned());
        let url = parse(&env).expect("valid").thepastaclaw_status_url;
        assert_eq!(url.map(String::from).as_deref(), Some(file));
        env.insert(THEPASTACLAW_STATUS_URL, "status.json".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::InvalidUrl {
                key: THEPASTACLAW_STATUS_URL,
                ..
            })
        ));
        env.insert(
            THEPASTACLAW_STATUS_URL,
            "file:///srv/status.json".to_owned(),
        );
        assert!(matches!(
            parse(&env),
            Err(ConfigError::UnsupportedUrlScheme {
                key: THEPASTACLAW_STATUS_URL
            })
        ));
    }

    #[test]
    fn memcan_ui_url_is_optional_and_must_be_http() {
        assert_eq!(parse(&required_env()).expect("valid").memcan_ui_url, None);
        let mut env = required_env();
        env.insert(
            MEMCAN_UI_URL,
            "http://memcan.example.test:8190/ui".to_owned(),
        );
        let url = parse(&env).expect("valid").memcan_ui_url;
        assert_eq!(url.as_deref(), Some("http://memcan.example.test:8190/ui"));
        for (value, stored) in [
            (
                "http://memcan.example.test:8190/ui/",
                "http://memcan.example.test:8190/ui",
            ),
            ("https://memcan.example.test", "https://memcan.example.test"),
        ] {
            env.insert(MEMCAN_UI_URL, value.to_owned());
            let url = parse(&env).expect("valid").memcan_ui_url;
            assert_eq!(url.as_deref(), Some(stored), "{value}");
        }
        env.insert(MEMCAN_UI_URL, "memcan.example.test/ui".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::InvalidUrl {
                key: MEMCAN_UI_URL,
                ..
            })
        ));
        env.insert(MEMCAN_UI_URL, "ftp://memcan.example.test/ui".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::UnsupportedUrlScheme { key: MEMCAN_UI_URL })
        ));
    }

    #[test]
    fn app_id_must_be_a_number() {
        let mut env = required_env();
        env.insert(GITHUB_APP_ID, "12ab".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::InvalidNumber {
                key: GITHUB_APP_ID,
                ..
            })
        ));
    }

    #[test]
    fn invalid_listen_addr_is_rejected() {
        let mut env = required_env();
        env.insert(LISTEN_ADDR, "3000".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::InvalidSocketAddr {
                key: LISTEN_ADDR,
                ..
            })
        ));
    }

    #[test]
    fn invalid_runner_listen_addr_is_rejected() {
        let mut env = required_env();
        env.insert(RUNNER_LISTEN_ADDR, "runner".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::InvalidSocketAddr {
                key: RUNNER_LISTEN_ADDR,
                ..
            })
        ));
    }

    #[test]
    fn runner_listener_must_not_share_the_public_address() {
        for (public, runner) in [
            ("0.0.0.0:3000", "0.0.0.0:3000"),
            ("127.0.0.1:3000", "0.0.0.0:3000"),
            ("0.0.0.0:4000", "10.0.0.2:4000"),
        ] {
            let mut env = required_env();
            env.insert(LISTEN_ADDR, public.to_owned());
            env.insert(RUNNER_LISTEN_ADDR, runner.to_owned());
            assert!(
                matches!(parse(&env), Err(ConfigError::ListenersCollide { .. })),
                "{public} / {runner}"
            );
        }
        let mut env = required_env();
        env.insert(LISTEN_ADDR, "127.0.0.1:3000".to_owned());
        env.insert(RUNNER_LISTEN_ADDR, "10.0.0.2:3000".to_owned());
        assert!(parse(&env).is_ok(), "same port on different addresses");
    }

    #[test]
    fn webhook_listener_address_is_separate_and_configurable() {
        let mut env = required_env();
        assert_eq!(
            parse(&env).unwrap().webhook_listen_addr,
            "127.0.0.1:3002".parse().unwrap()
        );
        env.insert(WEBHOOK_LISTEN_ADDR, "127.0.0.1:4321".to_owned());
        assert_eq!(
            parse(&env).unwrap().webhook_listen_addr,
            "127.0.0.1:4321".parse().unwrap()
        );
        env.insert(WEBHOOK_LISTEN_ADDR, "127.0.0.1:3000".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::ListenersCollide {
                key: WEBHOOK_LISTEN_ADDR,
                other: LISTEN_ADDR
            })
        ));
        env.insert(WEBHOOK_LISTEN_ADDR, "127.0.0.1:3001".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::ListenersCollide {
                key: WEBHOOK_LISTEN_ADDR,
                other: RUNNER_LISTEN_ADDR
            })
        ));
        env.insert(WEBHOOK_LISTEN_ADDR, "invalid".to_owned());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::InvalidSocketAddr {
                key: WEBHOOK_LISTEN_ADDR,
                ..
            })
        ));
    }

    #[test]
    fn invalid_trusted_proxy_entry_is_named() {
        let mut env = required_env();
        env.insert(TRUSTED_PROXIES, "10.0.0.0/8,proxy.lan".to_owned());
        match parse(&env) {
            Err(error @ ConfigError::InvalidTrustedProxy { .. }) => {
                assert!(error.to_string().contains("'proxy.lan'"), "{error}");
            }
            other => panic!("expected InvalidTrustedProxy, got {other:?}"),
        }
    }

    const FILE_VARIANTS: [(&str, &str); 4] = [
        (SECRET_KEY, SECRET_KEY_FILE),
        (RUNNER_SECRET, RUNNER_SECRET_FILE),
        (GITHUB_CLIENT_SECRET, GITHUB_CLIENT_SECRET_FILE),
        (MEMCAN_TOKEN, MEMCAN_TOKEN_FILE),
    ];

    /// Writes `contents` to a 0600 file named `name` in `dir`.
    fn secret_file(dir: &std::path::Path, name: &str, contents: &str) -> String {
        use std::os::unix::fs::PermissionsExt as _;
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("mode");
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn secrets_can_be_read_from_files() {
        let dir = tempfile::tempdir().expect("dir");
        let mut env = required_env();
        for (key, file_key) in FILE_VARIANTS {
            let value = env.remove(key).expect("required");
            env.insert(
                file_key,
                secret_file(dir.path(), key, &format!("{value}\n")),
            );
        }
        let config = parse(&env).expect("valid");
        let expected_key: Vec<u8> = (0u8..32).collect();
        assert_eq!(
            config.secret_key.expose_secret().as_slice(),
            expected_key.as_slice()
        );
        assert_eq!(config.runner_secret.expose_secret(), RUNNER_SECRET_VALUE);
        assert_eq!(config.github.client_secret.expose_secret(), CLIENT_SECRET);
        assert_eq!(config.memcan.token.expose_secret(), MEMCAN_TOKEN_VALUE);
    }

    #[test]
    fn secret_and_its_file_variable_together_are_refused() {
        let dir = tempfile::tempdir().expect("dir");
        for (key, file_key) in FILE_VARIANTS {
            let mut env = required_env();
            let value = env[key].clone();
            env.insert(file_key, secret_file(dir.path(), key, &value));
            let error = parse(&env).expect_err("both set");
            assert!(
                matches!(error, ConfigError::BothSet { key: k, file_key: f } if k == key && f == file_key),
                "{error:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains(key) && message.contains(file_key),
                "{message}"
            );
            assert!(!message.contains(&value), "value leaked: {message}");
        }
    }

    #[test]
    fn secret_file_must_be_a_readable_non_empty_regular_file() {
        let dir = tempfile::tempdir().expect("dir");
        let missing = dir.path().join("absent").to_string_lossy().into_owned();
        let empty = secret_file(dir.path(), "empty", "\n");
        let directory = dir.path().to_string_lossy().into_owned();
        let large = secret_file(dir.path(), "large", &"k".repeat(65 * 1024));
        for (path, expected) in [
            (missing, "cannot be read"),
            (empty, "is empty"),
            (directory, "not a regular file"),
            (large, "larger than"),
        ] {
            let mut env = required_env();
            env.remove(MEMCAN_TOKEN);
            env.insert(MEMCAN_TOKEN_FILE, path.clone());
            let message = parse(&env).expect_err("refused").to_string();
            assert!(
                message.contains(MEMCAN_TOKEN_FILE) && message.contains(expected),
                "{path}: {message}"
            );
        }
    }

    #[test]
    fn secret_file_contents_never_reach_errors() {
        let dir = tempfile::tempdir().expect("dir");
        let mut env = required_env();
        env.remove(SECRET_KEY);
        env.insert(
            SECRET_KEY_FILE,
            secret_file(dir.path(), "key", "not-base64-CANARY-9931\n"),
        );
        let error = parse(&env).expect_err("not base64");
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains("CANARY"), "contents leaked: {rendered}");
    }

    #[test]
    fn world_readable_secret_file_is_named_in_a_warning() {
        use std::os::unix::fs::PermissionsExt as _;
        let capture = crate::test_support::logs::LogCapture::start();
        let dir = tempfile::tempdir().expect("dir");
        let path = secret_file(dir.path(), "token", "memcan-file-CANARY-5521\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("mode");
        let mut env = required_env();
        env.remove(MEMCAN_TOKEN);
        env.insert(MEMCAN_TOKEN_FILE, path.clone());
        let config = parse(&env).expect("valid");
        assert_eq!(
            config.memcan.token.expose_secret(),
            "memcan-file-CANARY-5521"
        );
        let logs = capture.text();
        assert!(
            logs.contains("WARN") && logs.contains(&path) && logs.contains(MEMCAN_TOKEN_FILE),
            "{logs}"
        );
        assert!(!logs.contains("CANARY"), "contents leaked: {logs}");
    }

    #[test]
    fn credentials_gateway_requires_its_own_scope_and_redacts_it() {
        let mut env = required_env();
        env.insert("SR_GATEWAY_ADMIN_URL", "http://127.0.0.1:9001".into());
        assert!(matches!(
            parse(&env),
            Err(ConfigError::Missing {
                key: "SR_GATEWAY_CREDENTIALS_TOKEN"
            })
        ));
        env.insert(
            "SR_GATEWAY_CREDENTIALS_TOKEN",
            "fake-gateway-scope-canary".into(),
        );
        let config = parse(&env).unwrap();
        assert!(config.codex_gateway.is_some());
        assert!(!format!("{config:?}").contains("fake-gateway-scope-canary"));
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let rendered = format!("{:?}", test_config());
        for secret in [
            CLIENT_SECRET,
            RUNNER_SECRET_VALUE,
            MEMCAN_TOKEN_VALUE,
            KEY_B64,
        ] {
            assert!(
                !rendered.contains(secret),
                "secret leaked in Debug: {secret}"
            );
        }
        assert!(
            !rendered.contains("[0, 1, 2, 3"),
            "secret key bytes leaked in Debug"
        );
    }
}
