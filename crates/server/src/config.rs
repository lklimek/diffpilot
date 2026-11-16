//! Server configuration from `SR_*` environment variables (ADR-001).

use std::net::{AddrParseError, IpAddr, SocketAddr};
use std::num::ParseIntError;
use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ipnet::IpNet;
use secrecy::zeroize::Zeroizing;
use secrecy::{SecretBox, SecretString};
use url::Url;

/// Public base URL of the instance, e.g. `https://review.klimek.ws`.
pub const PUBLIC_URL: &str = "SR_PUBLIC_URL";
/// Address the HTTP server listens on.
pub const LISTEN_ADDR: &str = "SR_LISTEN_ADDR";
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
/// MemCan MCP endpoint URL.
pub const MEMCAN_URL: &str = "SR_MEMCAN_URL";
/// MemCan bearer token.
pub const MEMCAN_TOKEN: &str = "SR_MEMCAN_TOKEN";

const DEFAULT_LISTEN_ADDR: SocketAddr =
    SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 3000);
const DEFAULT_DATA_DIR: &str = "data";
const DEFAULT_GITHUB_API_URL: &str = "https://api.github.com";
const DEFAULT_GITHUB_WEB_URL: &str = "https://github.com";
const SECRET_KEY_LEN: usize = 32;

/// Complete server configuration; secrets are redacted in `Debug`.
#[derive(Debug)]
pub struct Config {
    /// Public base URL; OAuth callback is `<public_url>/auth/github/callback`.
    pub public_url: Url,
    /// HTTP listen address (default `0.0.0.0:3000`); TLS is terminated by the reverse proxy.
    pub listen_addr: SocketAddr,
    /// Reverse proxies whose forwarding headers are trusted (default: none).
    pub trusted_proxies: Vec<IpNet>,
    /// Directory for the database and the bare mirrors (default `data`).
    pub data_dir: PathBuf,
    /// AES-GCM key for tokens at rest.
    pub secret_key: SecretBox<[u8; SECRET_KEY_LEN]>,
    /// Secret runners authenticate with.
    pub runner_secret: SecretString,
    /// GitHub App and endpoints.
    pub github: GithubConfig,
    /// MemCan endpoint and token.
    pub memcan: MemcanConfig,
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
    /// MCP endpoint URL.
    pub url: Url,
    /// Bearer token.
    pub token: SecretString,
}

/// Invalid or missing configuration. Messages name the variable, never its value.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
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
    /// The secret key is not base64.
    #[error("environment variable {key} is not valid base64")]
    SecretKeyNotBase64 {
        /// Variable name.
        key: &'static str,
        /// Decode error.
        #[source]
        source: base64::DecodeError,
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
        let or_default = |key, default: &'static str| -> Result<String, ConfigError> {
            Ok(get(key)?.unwrap_or_else(|| default.to_owned()))
        };

        let listen_addr = match get(LISTEN_ADDR)? {
            Some(value) => value
                .parse()
                .map_err(|source| ConfigError::InvalidSocketAddr {
                    key: LISTEN_ADDR,
                    source,
                })?,
            None => DEFAULT_LISTEN_ADDR,
        };
        let trusted_proxies = match get(TRUSTED_PROXIES)? {
            Some(value) => parse_trusted_proxies(&value)?,
            None => Vec::new(),
        };

        Ok(Self {
            public_url: parse_http_url(PUBLIC_URL, &require(PUBLIC_URL)?)?,
            listen_addr,
            trusted_proxies,
            data_dir: PathBuf::from(or_default(DATA_DIR, DEFAULT_DATA_DIR)?),
            secret_key: parse_secret_key(&require(SECRET_KEY)?)?,
            runner_secret: SecretString::from(require(RUNNER_SECRET)?),
            github: GithubConfig {
                app_id: require(GITHUB_APP_ID)?.parse().map_err(|source| {
                    ConfigError::InvalidNumber {
                        key: GITHUB_APP_ID,
                        source,
                    }
                })?,
                client_id: require(GITHUB_CLIENT_ID)?,
                client_secret: SecretString::from(require(GITHUB_CLIENT_SECRET)?),
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
                url: parse_http_url(MEMCAN_URL, &require(MEMCAN_URL)?)?,
                token: SecretString::from(require(MEMCAN_TOKEN)?),
            },
        })
    }
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

    const REQUIRED: [&str; 9] = [
        PUBLIC_URL,
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
    fn required_variables_only_use_defaults() {
        let config = parse(&required_env()).expect("valid");
        assert_eq!(config.public_url.as_str(), "https://review.example.test/");
        assert_eq!(config.listen_addr, "0.0.0.0:3000".parse().expect("addr"));
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
    }

    #[test]
    fn required_variable_names_match_adr_001() {
        let env: HashMap<&str, &str> = HashMap::from([
            ("SR_PUBLIC_URL", "https://review.example.test"),
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
        env.insert(TRUSTED_PROXIES, " 172.18.0.0/16 , 10.0.0.5,::1 ".to_owned());
        env.insert(DATA_DIR, "/var/lib/smart-review".to_owned());
        env.insert(GITHUB_API_URL, "http://127.0.0.1:9000/api".to_owned());
        env.insert(GITHUB_OAUTH_URL, "http://127.0.0.1:9000".to_owned());
        env.insert(GITHUB_CLONE_URL, "file:///data/tmp/remotes".to_owned());
        let config = parse(&env).expect("valid");
        assert_eq!(config.listen_addr, "127.0.0.1:8080".parse().expect("addr"));
        let proxies: Vec<String> = config
            .trusted_proxies
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(proxies, ["172.18.0.0/16", "10.0.0.5/32", "::1/128"]);
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/smart-review"));
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
