//! Runner configuration from `SR_RUNNER_*` environment variables. Error messages name the variable, never
//! its value.

use std::{
    num::NonZeroUsize,
    path::{Path, PathBuf},
};

use contracts::job::access::Secret;

use crate::execute::RunnerDirs;

/// Base URL of the server's runner listener, e.g. `http://server:3001`.
pub const SERVER_URL: &str = "SR_RUNNER_SERVER_URL";
/// Secret the runner presents when claiming jobs (same value as the server's).
pub const RUNNER_SECRET: &str = "SR_RUNNER_SECRET";
/// Path of a file holding the runner secret; exactly one of it and [`RUNNER_SECRET`] may be set.
pub const RUNNER_SECRET_FILE: &str = "SR_RUNNER_SECRET_FILE";
/// Name of this runner in claims and logs.
pub const RUNNER_ID: &str = "SR_RUNNER_ID";
/// Persistent runner state and health marker.
pub const STATE_DIR: &str = "SR_RUNNER_STATE_DIR";
/// Shared bare mirror volume visible at /mirror in workers.
pub const MIRRORS_DIR: &str = "SR_RUNNER_MIRRORS_DIR";
/// Filesystem inspected by the disk reserve; defaults to the state directory.
pub const WORK_DIR: &str = "SR_RUNNER_WORK_DIR";
/// `appserver` (default) or `fake` (only when compiled with `fake-provider`).
pub const PROVIDER: &str = "SR_RUNNER_PROVIDER";
/// Launcher unix socket, shared with the dedicated launcher service.
pub const LAUNCHER_SOCKET: &str = "SR_LAUNCHER_SOCKET";
/// Keys-only gateway admin listener on the control network.
pub const GATEWAY_URL: &str = "SR_GATEWAY_ADMIN_URL";
/// File holding the runner's keys-only gateway scope.
pub const GATEWAY_KEYS_FILE: &str = "SR_GATEWAY_KEYS_TOKEN_FILE";
/// Curated skills revision baked into the worker image.
pub const SKILLS_COMMIT: &str = "SR_WORKER_SKILLS_COMMIT";
/// MiB that must stay free on the work directory's filesystem for the runner to take a job (default 2048).
pub const DISK_RESERVE_MIB: &str = "SR_RUNNER_DISK_RESERVE_MIB";

/// Concurrent jobs and running worker containers; shared by runner and launcher.
pub const WORKER_MAX_RUNNING: &str = "SR_WORKER_MAX_RUNNING";
pub(crate) const DEFAULT_MAX_RUNNING: usize = 3;
pub(crate) const CONTROL_CONNECTIONS: usize = 16;

pub(crate) fn max_running(value: Option<&str>) -> Option<NonZeroUsize> {
    value.filter(|value| !value.is_empty()).map_or(
        NonZeroUsize::new(DEFAULT_MAX_RUNNING),
        |value| {
            value
                .parse::<NonZeroUsize>()
                .ok()
                .filter(|limit| valid_max_running(limit.get()))
        },
    )
}

pub(crate) fn valid_max_running(limit: usize) -> bool {
    // Each job may use an attachment plus an independent fetch connection.
    (1..=(tokio::sync::Semaphore::MAX_PERMITS - CONTROL_CONNECTIONS) / 2).contains(&limit)
}

const DEFAULT_RUNNER_ID: &str = "runner";
const DEFAULT_STATE_DIR: &str = "/var/lib/diffpilot-runner";
const DEFAULT_DISK_RESERVE_MIB: u64 = 2048;
const MIB: u64 = 1024 * 1024;
/// Largest secret file read.
const MAX_SECRET_FILE_BYTES: u64 = 64 * 1024;

/// Which provider runs the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    /// Long-lived app-server in disposable worker containers.
    Appserver,
    /// Fixture replay (`fake-provider` builds only).
    #[cfg(feature = "fake-provider")]
    Fake,
}

/// Complete runner configuration; the secret is redacted in `Debug`.
#[derive(Debug)]
pub struct Config {
    /// Base URL of the runner listener.
    pub server_url: String,
    /// Runner secret.
    pub runner_secret: Secret,
    /// Runner name.
    pub runner_id: String,
    /// Persistent private state directory.
    pub state_dir: PathBuf,
    /// Shared repository mirror cache.
    pub mirrors_dir: PathBuf,
    /// Job work directories.
    pub work_dir: PathBuf,
    /// Runtime provider.
    pub provider: ProviderKind,
    /// Private launcher socket.
    pub launcher_socket: PathBuf,
    /// Keys-only gateway admin URL (required for appserver).
    pub gateway_url: Option<url::Url>,
    /// Keys scope; never a provider credential or credentials-admin scope.
    pub gateway_keys: Option<Secret>,
    /// Worker image's curated skills revision.
    pub skills_commit: String,
    /// Bytes that must stay free on the work directory's filesystem for a claim.
    pub disk_reserve: u64,
    /// Maximum number of jobs with local attempts in progress.
    pub max_running: NonZeroUsize,
}

/// Invalid or missing configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required variable is unset or empty.
    #[error("required environment variable {0} is not set")]
    Missing(&'static str),
    /// A URL variable is not an http(s) URL.
    #[error("environment variable {0} must be an http:// or https:// URL")]
    NotHttpUrl(&'static str),
    /// A path variable is not absolute.
    #[error("environment variable {0} must be an absolute path")]
    NotAbsolute(&'static str),
    /// A number variable is not a positive whole number.
    #[error("environment variable {0} must be a positive whole number")]
    NotPositive(&'static str),
    /// Unknown provider name.
    #[cfg_attr(
        feature = "fake-provider",
        error("environment variable {0} must be 'appserver' or 'fake'")
    )]
    #[cfg_attr(
        not(feature = "fake-provider"),
        error("environment variable {0} must be 'appserver'")
    )]
    UnknownProvider(&'static str),
    /// A secret is given both directly and as a file.
    #[error("set only one of {0} and {1}")]
    BothSet(&'static str, &'static str),
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
}

impl Config {
    /// Reads the configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Reads the configuration through `lookup`; values are trimmed and empty counts as unset.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |key: &str| {
            lookup(key)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let required = |key: &'static str| get(key).ok_or(ConfigError::Missing(key));

        let server_url = required(SERVER_URL)?;
        if !(server_url.starts_with("http://") || server_url.starts_with("https://")) {
            return Err(ConfigError::NotHttpUrl(SERVER_URL));
        }
        let runner_secret = Secret::new(match (get(RUNNER_SECRET), get(RUNNER_SECRET_FILE)) {
            (Some(_), Some(_)) => {
                return Err(ConfigError::BothSet(RUNNER_SECRET, RUNNER_SECRET_FILE));
            }
            (Some(value), None) => value,
            (None, Some(path)) => read_secret_file(RUNNER_SECRET_FILE, Path::new(&path))?,
            (None, None) => return Err(ConfigError::Missing(RUNNER_SECRET)),
        });
        let state_dir = absolute(
            STATE_DIR,
            get(STATE_DIR).unwrap_or_else(|| DEFAULT_STATE_DIR.to_owned()),
        )?;
        let work_dir = match get(WORK_DIR) {
            Some(dir) => absolute(WORK_DIR, dir)?,
            None => state_dir.clone(),
        };
        let provider = match get(PROVIDER).as_deref() {
            None | Some("appserver") => ProviderKind::Appserver,
            #[cfg(feature = "fake-provider")]
            Some("fake") => ProviderKind::Fake,
            Some(_) => return Err(ConfigError::UnknownProvider(PROVIDER)),
        };
        let gateway_url = get(GATEWAY_URL)
            .map(|value| {
                let url = value
                    .parse::<url::Url>()
                    .map_err(|_| ConfigError::NotHttpUrl(GATEWAY_URL))?;
                if !matches!(url.scheme(), "http" | "https") {
                    return Err(ConfigError::NotHttpUrl(GATEWAY_URL));
                }
                Ok(url)
            })
            .transpose()?;
        let gateway_keys = get(GATEWAY_KEYS_FILE)
            .map(|path| read_secret_file(GATEWAY_KEYS_FILE, Path::new(&path)).map(Secret::new))
            .transpose()?;
        let disk_reserve_mib = match get(DISK_RESERVE_MIB) {
            Some(mib) => mib
                .parse::<u64>()
                .ok()
                .filter(|mib| (1..=u64::MAX / MIB).contains(mib))
                .ok_or(ConfigError::NotPositive(DISK_RESERVE_MIB))?,
            None => DEFAULT_DISK_RESERVE_MIB,
        };
        Ok(Self {
            server_url,
            runner_secret,
            runner_id: get(RUNNER_ID).unwrap_or_else(|| DEFAULT_RUNNER_ID.to_owned()),
            mirrors_dir: absolute(
                MIRRORS_DIR,
                get(MIRRORS_DIR)
                    .unwrap_or_else(|| state_dir.join("mirrors").to_string_lossy().into_owned()),
            )?,
            state_dir,
            work_dir,
            provider,
            launcher_socket: absolute(
                LAUNCHER_SOCKET,
                get(LAUNCHER_SOCKET).unwrap_or_else(|| "/run/launcher/launcher.sock".into()),
            )?,
            gateway_url,
            gateway_keys,
            skills_commit: get(SKILLS_COMMIT)
                .unwrap_or_else(|| "71096d7d4481977added8761b2cfd63cd84852b4".into()),
            disk_reserve: disk_reserve_mib * MIB,
            max_running: max_running(get(WORKER_MAX_RUNNING).as_deref())
                .ok_or(ConfigError::NotPositive(WORKER_MAX_RUNNING))?,
        })
    }

    /// The runner directories.
    pub fn dirs(&self) -> RunnerDirs {
        RunnerDirs {
            mirrors: self.mirrors_dir.clone(),
        }
    }
}

/// Reads a secret file once (a regular file, symlinks followed); trailing newlines are removed. Errors name the
/// variable and the path, never the contents; a file every user (the agent uid included) may read is reported
/// with `warn!`.
fn read_secret_file(key: &'static str, path: &Path) -> Result<String, ConfigError> {
    use std::os::unix::fs::PermissionsExt as _;
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
            "runner secret file is readable by every user; restrict it to mode 0400 or 0600");
    }
    let mut value = std::fs::read_to_string(path).map_err(unreadable)?;
    value.truncate(value.trim_end_matches(['\n', '\r']).len());
    if value.is_empty() {
        return Err(invalid("empty"));
    }
    Ok(value)
}

fn absolute(key: &'static str, value: String) -> Result<PathBuf, ConfigError> {
    let path = PathBuf::from(value);
    if Path::is_absolute(&path) {
        Ok(path)
    } else {
        Err(ConfigError::NotAbsolute(key))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn config(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Config::from_lookup(|key| map.get(key).cloned())
    }

    const BASE: [(&str, &str); 2] = [
        (SERVER_URL, "http://server:3001"),
        (RUNNER_SECRET, "s3cret-value"),
    ];

    #[test]
    fn container_provider_is_the_default() {
        assert_eq!(
            format!("{:?}", config(&BASE).unwrap().provider),
            "Appserver"
        );
    }

    #[test]
    fn provider_error_lists_only_compiled_providers() {
        let message = ConfigError::UnknownProvider(PROVIDER).to_string();
        assert_eq!(message.contains("fake"), cfg!(feature = "fake-provider"));
        assert!(message.contains("appserver"));
    }

    #[test]
    fn defaults() {
        let config = config(&BASE).unwrap();
        assert_eq!(config.state_dir, PathBuf::from("/var/lib/diffpilot-runner"));
        assert_eq!(config.work_dir, config.state_dir);
        assert_eq!(config.provider, ProviderKind::Appserver);
        assert_eq!(config.max_running.get(), 3);
        assert!(!format!("{config:?}").contains("s3cret-value"));
    }

    #[test]
    fn missing_secret_names_the_variable() {
        let error = config(&BASE[..1]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "required environment variable SR_RUNNER_SECRET is not set"
        );
    }

    #[test]
    fn disk_reserve_is_a_positive_number_of_mib_defaulting_to_2_gib() {
        assert_eq!(config(&BASE).unwrap().disk_reserve, 2048 * 1024 * 1024);
        for mib in ["0", "-1", "x", "1.5"] {
            let mut pairs = BASE.to_vec();
            pairs.push((DISK_RESERVE_MIB, mib));
            assert!(
                matches!(config(&pairs), Err(ConfigError::NotPositive(_))),
                "{mib}"
            );
        }
        let mut pairs = BASE.to_vec();
        pairs.push((DISK_RESERVE_MIB, "512"));
        assert_eq!(config(&pairs).unwrap().disk_reserve, 512 * 1024 * 1024);
    }

    #[test]
    fn capacity_accepts_an_override_and_refuses_zero() {
        let mut pairs = BASE.to_vec();
        pairs.push((WORKER_MAX_RUNNING, "6"));
        assert_eq!(config(&pairs).unwrap().max_running.get(), 6);
        pairs[2].1 = "0";
        assert!(matches!(
            config(&pairs),
            Err(ConfigError::NotPositive(WORKER_MAX_RUNNING))
        ));
    }

    #[test]
    fn server_url_must_be_http() {
        assert!(matches!(
            config(&[(SERVER_URL, "file:///x"), (RUNNER_SECRET, "s")]),
            Err(ConfigError::NotHttpUrl(SERVER_URL))
        ));
    }

    /// Writes `contents` to a file of `mode` in `dir`.
    fn secret_file(dir: &Path, contents: &str, mode: u32) -> String {
        use std::os::unix::fs::PermissionsExt as _;
        let path = dir.join("runner-secret");
        std::fs::write(&path, contents).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn secret_can_be_read_from_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = secret_file(dir.path(), "s3cret-file-value\n", 0o600);
        let config = config(&[
            (SERVER_URL, "http://server:3001"),
            (RUNNER_SECRET_FILE, &path),
        ])
        .unwrap();
        assert_eq!(config.runner_secret.expose(), "s3cret-file-value");
    }

    #[test]
    fn secret_and_its_file_together_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = secret_file(dir.path(), "s3cret-value", 0o600);
        let mut pairs = BASE.to_vec();
        pairs.push((RUNNER_SECRET_FILE, &path));
        let message = config(&pairs).unwrap_err().to_string();
        assert_eq!(
            message,
            "set only one of SR_RUNNER_SECRET and SR_RUNNER_SECRET_FILE"
        );
    }

    #[test]
    fn secret_file_must_be_a_readable_non_empty_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent").to_string_lossy().into_owned();
        let directory = dir.path().to_string_lossy().into_owned();
        let empty = secret_file(dir.path(), "\n", 0o600);
        let cases = [
            (missing, "cannot be read"),
            (directory, "not a regular file"),
            (empty, "is empty"),
        ];
        for (path, expected) in cases {
            let error = config(&[
                (SERVER_URL, "http://server:3001"),
                (RUNNER_SECRET_FILE, &path),
            ])
            .unwrap_err()
            .to_string();
            assert!(
                error.contains(RUNNER_SECRET_FILE) && error.contains(expected),
                "{path}: {error}"
            );
        }
    }

    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn world_readable_secret_file_is_named_in_a_warning() {
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let sink = std::sync::Arc::clone(&log);
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || Captured(std::sync::Arc::clone(&sink)))
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let dir = tempfile::tempdir().unwrap();
        let path = secret_file(dir.path(), "file-CANARY-7731", 0o644);
        config(&[
            (SERVER_URL, "http://server:3001"),
            (RUNNER_SECRET_FILE, &path),
        ])
        .unwrap();
        let logged = String::from_utf8(log.lock().unwrap().clone()).unwrap();
        assert!(
            logged.contains("WARN") && logged.contains(&path),
            "{logged}"
        );
        assert!(!logged.contains("CANARY"), "contents leaked: {logged}");
    }

    #[cfg(not(feature = "fake-provider"))]
    #[test]
    fn fake_provider_is_unavailable_in_release_builds() {
        let mut pairs = BASE.to_vec();
        pairs.push((PROVIDER, "fake"));
        assert!(matches!(
            config(&pairs),
            Err(ConfigError::UnknownProvider(_))
        ));
    }
}
