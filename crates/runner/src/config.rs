//! Runner configuration from `SR_RUNNER_*` environment variables. Error messages name the variable, never
//! its value.

use std::path::{Path, PathBuf};
use std::time::Duration;

use contracts::job::access::Secret;
use contracts::job::spec::{CredentialKind, LlmCredentialRef};

use crate::execute::RunnerDirs;

/// Base URL of the server's runner listener, e.g. `http://server:3001`.
pub const SERVER_URL: &str = "SR_RUNNER_SERVER_URL";
/// Secret the runner presents when claiming jobs (same value as the server's).
pub const RUNNER_SECRET: &str = "SR_RUNNER_SECRET";
/// Path of a file holding the runner secret; exactly one of it and [`RUNNER_SECRET`] may be set.
pub const RUNNER_SECRET_FILE: &str = "SR_RUNNER_SECRET_FILE";
/// The agent uid the supervisor runs agents as (gid = uid, ADR-013); set by the supervisor, default 10002.
pub const AGENT_UID: &str = "SR_AGENT_UID";
/// Name of this runner in claims and logs.
pub const RUNNER_ID: &str = "SR_RUNNER_ID";
/// The runner's persistent private volume: credentials, per-job `CODEX_HOME`s, mirrors, sessions.
pub const STATE_DIR: &str = "SR_RUNNER_STATE_DIR";
/// Directory for job work dirs (a disposable volume, emptied at start); default `<state>/work`.
pub const WORK_DIR: &str = "SR_RUNNER_WORK_DIR";
/// Id of the LLM credential the runner holds (`<state>/credentials/<id>/auth.json`). Reserved: v1 has one
/// credential and the server assigns every job to `default`, so any other value is rejected.
pub const CREDENTIAL_ID: &str = "SR_RUNNER_CREDENTIAL_ID";
/// The `codex` binary.
pub const CODEX_BIN: &str = "SR_RUNNER_CODEX_BIN";
/// The `github-mcp-server` binary (default `/opt/github-mcp/github-mcp-server`).
pub const GITHUB_MCP_BIN: &str = "SR_RUNNER_GITHUB_MCP_BIN";
/// Explicit egress proxy URL given to the agent, e.g. `http://egress-proxy:3128`.
pub const EGRESS_PROXY: &str = "SR_RUNNER_EGRESS_PROXY";
/// `codex` (default) or `fake` (only in builds with the `fake-provider` feature).
pub const PROVIDER: &str = "SR_RUNNER_PROVIDER";
/// Git URL of the claudius repository the agent's skills come from.
pub const CLAUDIUS_URL: &str = "SR_RUNNER_CLAUDIUS_URL";
/// Ref (branch or tag) of claudius to use (default `main`).
pub const CLAUDIUS_REF: &str = "SR_RUNNER_CLAUDIUS_REF";
/// Seconds between two refreshes of the claudius checkout (default 21600).
pub const CLAUDIUS_REFRESH_SECS: &str = "SR_RUNNER_CLAUDIUS_REFRESH_SECS";
/// Comma-separated skills a job may be given (default `severity`, ADR-002).
pub const SKILLS: &str = "SR_RUNNER_SKILLS";
/// Days an agent session is kept after its last job (default 14).
pub const SESSION_RETENTION_DAYS: &str = "SR_RUNNER_SESSION_RETENTION_DAYS";
/// MiB that must stay free on the work directory's filesystem for the runner to take a job (default 2048).
pub const DISK_RESERVE_MIB: &str = "SR_RUNNER_DISK_RESERVE_MIB";

const DEFAULT_RUNNER_ID: &str = "runner";
const DEFAULT_STATE_DIR: &str = "/var/lib/smart-review-runner";
const DEFAULT_CREDENTIAL_ID: &str = "default";
const DEFAULT_CODEX_BIN: &str = "codex";
const DEFAULT_GITHUB_MCP_BIN: &str = "/opt/github-mcp/github-mcp-server";
const DEFAULT_SESSION_RETENTION_DAYS: u64 = 14;
const DEFAULT_DISK_RESERVE_MIB: u64 = 2048;
const MIB: u64 = 1024 * 1024;
const DEFAULT_CLAUDIUS_URL: &str = "https://github.com/lklimek/claudius.git";
const DEFAULT_CLAUDIUS_REF: &str = "main";
const DEFAULT_CLAUDIUS_REFRESH_SECS: u64 = 6 * 3600;
const DEFAULT_SKILLS: &str = "severity";
const SECS_PER_DAY: u64 = 24 * 3600;
/// Largest secret file read.
const MAX_SECRET_FILE_BYTES: u64 = 64 * 1024;
/// The supervisor's default agent uid (ADR-013).
const DEFAULT_AGENT_UID: u32 = 10_002;

/// Which provider runs the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    /// The Codex CLI.
    Codex,
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
    /// Job work directories.
    pub work_dir: PathBuf,
    /// The credential the runner holds.
    pub credential: LlmCredentialRef,
    /// The `codex` binary.
    pub codex_bin: PathBuf,
    /// The `github-mcp-server` binary.
    pub github_mcp_bin: PathBuf,
    /// Egress proxy for the agent.
    pub egress_proxy: Option<String>,
    /// The provider.
    pub provider: ProviderKind,
    /// How long an agent session is kept after its last job.
    pub session_retention: Duration,
    /// The claudius repository URL.
    pub claudius_url: String,
    /// The claudius ref.
    pub claudius_ref: String,
    /// Interval between claudius refreshes.
    pub claudius_refresh: Duration,
    /// Skills a job may be given.
    pub skills: Vec<String>,
    /// Bytes that must stay free on the work directory's filesystem for a claim.
    pub disk_reserve: u64,
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
    /// The state directory is under `/tmp` (per-job `CODEX_HOME`s must survive and Codex refuses `/tmp`).
    #[error(
        "environment variable {0} must not point below /tmp; use the runner's persistent volume"
    )]
    UnderTmp(&'static str),
    /// The credential id is not `default`, the only credential the server assigns jobs to in v1.
    #[error(
        "environment variable {0} is reserved: v1 supports only the credential 'default'; unset it"
    )]
    UnsupportedCredentialId(&'static str),
    /// A number variable is not a positive whole number.
    #[error("environment variable {0} must be a positive whole number")]
    NotPositive(&'static str),
    /// Unknown provider name.
    #[error("environment variable {0} must be 'codex' (or 'fake' in fake-provider builds)")]
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
    /// Under the supervisor, the agent uid could read the secret file.
    #[error("the file named by {key} ({}) is readable by the agent uid {agent}; give it to the runner uid with mode 0400 or 0600", path.display())]
    SecretFileReadableByAgent {
        /// Variable name.
        key: &'static str,
        /// The file.
        path: PathBuf,
        /// The agent uid.
        agent: u32,
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
        if state_dir.starts_with("/tmp") {
            return Err(ConfigError::UnderTmp(STATE_DIR));
        }
        let work_dir = match get(WORK_DIR) {
            Some(dir) => absolute(WORK_DIR, dir)?,
            None => state_dir.join("work"),
        };
        let credential_id = get(CREDENTIAL_ID).unwrap_or_else(|| DEFAULT_CREDENTIAL_ID.to_owned());
        if credential_id != DEFAULT_CREDENTIAL_ID {
            return Err(ConfigError::UnsupportedCredentialId(CREDENTIAL_ID));
        }
        let egress_proxy = get(EGRESS_PROXY);
        if let Some(proxy) = &egress_proxy
            && !(proxy.starts_with("http://") || proxy.starts_with("https://"))
        {
            return Err(ConfigError::NotHttpUrl(EGRESS_PROXY));
        }
        let provider = match get(PROVIDER).as_deref() {
            None | Some("codex") => ProviderKind::Codex,
            #[cfg(feature = "fake-provider")]
            Some("fake") => ProviderKind::Fake,
            Some(_) => return Err(ConfigError::UnknownProvider(PROVIDER)),
        };
        let retention_days = match get(SESSION_RETENTION_DAYS) {
            Some(days) => days
                .parse::<u64>()
                .ok()
                .filter(|days| (1..=3650).contains(days))
                .ok_or(ConfigError::NotPositive(SESSION_RETENTION_DAYS))?,
            None => DEFAULT_SESSION_RETENTION_DAYS,
        };
        let claudius_refresh = match get(CLAUDIUS_REFRESH_SECS) {
            Some(secs) => secs
                .parse::<u64>()
                .ok()
                .filter(|secs| *secs >= 60)
                .ok_or(ConfigError::NotPositive(CLAUDIUS_REFRESH_SECS))?,
            None => DEFAULT_CLAUDIUS_REFRESH_SECS,
        };
        let disk_reserve_mib = match get(DISK_RESERVE_MIB) {
            Some(mib) => mib
                .parse::<u64>()
                .ok()
                .filter(|mib| (1..=u64::MAX / MIB).contains(mib))
                .ok_or(ConfigError::NotPositive(DISK_RESERVE_MIB))?,
            None => DEFAULT_DISK_RESERVE_MIB,
        };
        let claudius_url = get(CLAUDIUS_URL).unwrap_or_else(|| DEFAULT_CLAUDIUS_URL.to_owned());
        if !(claudius_url.starts_with("https://") || claudius_url.starts_with("file://")) {
            return Err(ConfigError::NotHttpUrl(CLAUDIUS_URL));
        }
        Ok(Self {
            server_url,
            runner_secret,
            runner_id: get(RUNNER_ID).unwrap_or_else(|| DEFAULT_RUNNER_ID.to_owned()),
            state_dir,
            work_dir,
            credential: LlmCredentialRef {
                id: credential_id,
                kind: CredentialKind::CodexChatgpt,
            },
            codex_bin: PathBuf::from(
                get(CODEX_BIN).unwrap_or_else(|| DEFAULT_CODEX_BIN.to_owned()),
            ),
            github_mcp_bin: absolute(
                GITHUB_MCP_BIN,
                get(GITHUB_MCP_BIN).unwrap_or_else(|| DEFAULT_GITHUB_MCP_BIN.to_owned()),
            )?,
            egress_proxy,
            provider,
            session_retention: Duration::from_secs(retention_days * SECS_PER_DAY),
            claudius_url,
            claudius_ref: get(CLAUDIUS_REF).unwrap_or_else(|| DEFAULT_CLAUDIUS_REF.to_owned()),
            claudius_refresh: Duration::from_secs(claudius_refresh),
            skills: get(SKILLS)
                .unwrap_or_else(|| DEFAULT_SKILLS.to_owned())
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect(),
            disk_reserve: disk_reserve_mib * MIB,
        })
    }

    /// The runner directories.
    pub fn dirs(&self) -> RunnerDirs {
        RunnerDirs {
            work: self.work_dir.clone(),
            codex_homes: self.state_dir.join("codex-homes"),
            mirrors: self.state_dir.join("mirrors"),
            sessions: self.state_dir.join("sessions"),
        }
    }

    /// The runner-private claudius checkout.
    pub fn claudius_checkout(&self) -> PathBuf {
        self.state_dir.join("claudius")
    }

    /// The master `auth.json` of the runner's credential.
    pub fn master_auth(&self) -> PathBuf {
        self.state_dir
            .join("credentials")
            .join(&self.credential.id)
            .join("auth.json")
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
            "runner secret file is readable by every user, the agent included; restrict it to mode 0400 or 0600");
    }
    let mut value = std::fs::read_to_string(path).map_err(unreadable)?;
    value.truncate(value.trim_end_matches(['\n', '\r']).len());
    if value.is_empty() {
        return Err(invalid("empty"));
    }
    Ok(value)
}

/// Under the supervisor: refuses a runner secret file the agent could read (see [`agent_may_read`]). Only the
/// file's own owner and mode are checked; the directories leading to it are the operator's.
pub fn check_secret_file_hidden_from_agent(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<(), ConfigError> {
    use std::os::unix::fs::MetadataExt as _;
    let get = |key: &str| {
        lookup(key)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    let Some(path) = get(RUNNER_SECRET_FILE).map(PathBuf::from) else {
        return Ok(());
    };
    let agent = match get(AGENT_UID) {
        Some(uid) => uid
            .parse::<u32>()
            .map_err(|_| ConfigError::NotPositive(AGENT_UID))?,
        None => DEFAULT_AGENT_UID,
    };
    let metadata =
        std::fs::metadata(&path).map_err(|source| ConfigError::SecretFileUnreadable {
            key: RUNNER_SECRET_FILE,
            path: path.clone(),
            source,
        })?;
    if agent_may_read(metadata.mode(), metadata.uid(), metadata.gid(), agent) {
        return Err(ConfigError::SecretFileReadableByAgent {
            key: RUNNER_SECRET_FILE,
            path,
            agent,
        });
    }
    Ok(())
}

/// Whether the agent (uid and primary gid `agent`, no supplementary groups) may reach a file of `mode`, `owner`
/// and `group`: it owns it, any `other` bit is set, or its group is the agent's with any group bit set.
fn agent_may_read(mode: u32, owner: u32, group: u32, agent: u32) -> bool {
    owner == agent || mode & 0o007 != 0 || (group == agent && mode & 0o070 != 0)
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
    use std::os::unix::fs::PermissionsExt as _;

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
    fn defaults() {
        let config = config(&BASE).unwrap();
        assert_eq!(
            config.state_dir,
            PathBuf::from("/var/lib/smart-review-runner")
        );
        assert_eq!(config.work_dir, config.state_dir.join("work"));
        assert_eq!(config.credential.id, "default");
        assert_eq!(config.provider, ProviderKind::Codex);
        assert_eq!(
            config.session_retention,
            Duration::from_secs(14 * 24 * 3600)
        );
        assert_eq!(
            config.master_auth(),
            PathBuf::from("/var/lib/smart-review-runner/credentials/default/auth.json")
        );
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
    fn state_dir_under_tmp_is_rejected() {
        let mut pairs = BASE.to_vec();
        pairs.push((STATE_DIR, "/tmp/runner"));
        assert!(matches!(config(&pairs), Err(ConfigError::UnderTmp(_))));
    }

    #[test]
    fn credential_id_other_than_default_is_rejected() {
        for id in ["work", "../x", "..", "a/b"] {
            let mut pairs = BASE.to_vec();
            pairs.push((CREDENTIAL_ID, id));
            assert!(
                matches!(config(&pairs), Err(ConfigError::UnsupportedCredentialId(_))),
                "{id}"
            );
        }
        let mut pairs = BASE.to_vec();
        pairs.push((CREDENTIAL_ID, "default"));
        assert_eq!(config(&pairs).unwrap().credential.id, "default");
    }

    #[test]
    fn session_retention_must_be_a_positive_number_of_days() {
        for days in ["0", "-1", "x", "1.5"] {
            let mut pairs = BASE.to_vec();
            pairs.push((SESSION_RETENTION_DAYS, days));
            assert!(
                matches!(config(&pairs), Err(ConfigError::NotPositive(_))),
                "{days}"
            );
        }
        let mut pairs = BASE.to_vec();
        pairs.push((SESSION_RETENTION_DAYS, "3"));
        assert_eq!(
            config(&pairs).unwrap().session_retention,
            Duration::from_secs(3 * 24 * 3600)
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

    #[test]
    fn agent_may_read_through_owner_other_or_its_group() {
        const AGENT: u32 = 10_002;
        const RUNNER: u32 = 10_001;
        for (mode, owner, group) in [
            (0o400, AGENT, RUNNER),
            (0o404, RUNNER, RUNNER),
            (0o401, 0, 0),
            (0o440, RUNNER, AGENT),
            (0o420, 0, AGENT),
        ] {
            assert!(
                agent_may_read(mode, owner, group, AGENT),
                "{mode:o} {owner}:{group}"
            );
        }
        for (mode, owner, group) in [
            (0o400, RUNNER, RUNNER),
            (0o600, RUNNER, AGENT),
            (0o440, 0, RUNNER),
            (0o640, RUNNER, 0),
        ] {
            assert!(
                !agent_may_read(mode, owner, group, AGENT),
                "{mode:o} {owner}:{group}"
            );
        }
    }

    #[test]
    fn supervised_runner_refuses_a_secret_file_the_agent_can_read() {
        let dir = tempfile::tempdir().unwrap();
        let me = rustix::process::getuid().as_raw().to_string();
        let other = (rustix::process::getuid().as_raw() + 1).to_string();
        let private = secret_file(dir.path(), "s3cret", 0o600);
        let check = |path: &str, agent: &str| {
            let map: HashMap<&str, &str> =
                HashMap::from([(RUNNER_SECRET_FILE, path), (AGENT_UID, agent)]);
            check_secret_file_hidden_from_agent(|key| map.get(key).map(|v| (*v).to_owned()))
        };
        check(&private, &other).expect("only the runner reads it");
        let error = check(&private, &me).expect_err("owned by the agent uid");
        assert!(
            matches!(error, ConfigError::SecretFileReadableByAgent { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains(&private), "{error}");
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o604)).unwrap();
        assert!(check(&private, &other).is_err(), "readable by every user");
        let none: HashMap<&str, &str> = HashMap::new();
        check_secret_file_hidden_from_agent(|key| none.get(key).map(|v| (*v).to_owned()))
            .expect("no file, nothing to check");
    }

    /// A log writer appending to a shared buffer.
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
