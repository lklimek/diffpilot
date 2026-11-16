//! The per-job copy of the Codex credential. Until compare-and-swap write-back (WP3-RUN, T13), a copy that
//! changed during the job (refresh-token rotation) is kept next to the master and the credential is blocked:
//! the master's refresh token may be spent, so no further job may use it until the operator recovers it.
//! A refresh failure blocks it too. The copy is agent-writable, so it is read only as a small regular file
//! without following symlinks and kept only if it is a credential of the same account.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use contracts::ids::JobId;
use rustix::fs::OFlags;
use serde_json::Value;

use crate::provider::ProviderError;
use crate::scrub::Scrubber;

/// The credential file inside a `CODEX_HOME`.
pub(crate) const AUTH_FILE: &str = "auth.json";
/// Largest credential file read (a real `auth.json` has a few kB).
const MAX_AUTH_BYTES: u64 = 64 * 1024;

/// Whether the credential whose master is `master_auth` is blocked until the operator recovers it.
pub fn credential_blocked(master_auth: &Path) -> bool {
    blocked_marker(master_auth).symlink_metadata().is_ok()
}

/// Whether jobs may use the credential: a readable master that is not blocked (reported in claims).
pub fn credential_usable(master_auth: &Path) -> bool {
    !credential_blocked(master_auth)
        && read_regular(master_auth).is_ok_and(|auth| parse(&auth).is_some())
}

fn blocked_marker(master_auth: &Path) -> PathBuf {
    master_auth.with_file_name(format!("{AUTH_FILE}.blocked"))
}

/// The credential copy of one job and the secrets the provider scrubs (its tokens, rotated ones included).
pub(crate) struct JobCredential {
    master_auth: PathBuf,
    copy: PathBuf,
    job: JobId,
    original: Vec<u8>,
    account: Option<String>,
    last_seen: Vec<u8>,
    scrubber: Scrubber,
    settled: bool,
}

impl JobCredential {
    /// Copies the master into `codex_home`; `known` are further secrets to scrub (the agent's tokens).
    pub(crate) fn install(
        master_auth: &Path,
        codex_home: &Path,
        job: JobId,
        known: Scrubber,
    ) -> Result<Self, ProviderError> {
        if credential_blocked(master_auth) {
            tracing::error!(
                marker = %blocked_marker(master_auth).display(),
                "the Codex credential is blocked after a rotation or refresh failure; restore auth.json (or sign in again), delete the marker and restart the runner"
            );
            return Err(ProviderError::CredentialUnusable);
        }
        let original = read_regular(master_auth).map_err(|error| {
            tracing::error!(
                path = %master_auth.display(),
                %error,
                "cannot read the master Codex credential; sign in with `codex login --device-auth` in the runner container"
            );
            ProviderError::CredentialUnusable
        })?;
        let Some((tokens, account)) = parse(&original) else {
            tracing::error!(path = %master_auth.display(), "the master Codex credential is not a ChatGPT credential; sign in again");
            return Err(ProviderError::CredentialUnusable);
        };
        let copy = codex_home.join(AUTH_FILE);
        write_new_private(&copy, &original)?;
        let mut scrubber = known;
        scrubber.extend(tokens.iter().map(String::as_str));
        Ok(Self {
            master_auth: master_auth.to_path_buf(),
            copy,
            job,
            last_seen: original.clone(),
            original,
            account,
            scrubber,
            settled: false,
        })
    }

    /// Every secret the provider knows so far.
    pub(crate) fn scrubber(&self) -> &Scrubber {
        &self.scrubber
    }

    /// Re-reads the copy; the tokens of a credential rotated meanwhile join the scrubber.
    pub(crate) fn watch(&mut self) {
        let Ok(current) = read_regular(&self.copy) else {
            return;
        };
        if current == self.last_seen {
            return;
        }
        if let Some((tokens, _)) = parse(&current) {
            self.scrubber.extend(tokens.iter().map(String::as_str));
        }
        self.last_seen = current;
    }

    /// Ends the job's use of the copy (once): a changed copy is kept if valid and the credential is blocked;
    /// a refresh failure blocks it as well.
    pub(crate) fn settle(&mut self, refresh_failed: bool) {
        if std::mem::replace(&mut self.settled, true) {
            return;
        }
        let reason = match read_regular(&self.copy) {
            Ok(current) if current == self.original => {
                if !refresh_failed {
                    return;
                }
                "the credential could not be refreshed".to_owned()
            }
            Ok(current) => self.keep(&current),
            Err(error) => {
                format!("the credential copy could not be read safely after the job ({error})")
            }
        };
        self.block(&reason);
    }

    /// Keeps a rotated copy that is a credential of the same account; returns the blocking reason.
    fn keep(&mut self, current: &[u8]) -> String {
        match parse(current) {
            Some((tokens, account)) if account == self.account => {
                self.scrubber.extend(tokens.iter().map(String::as_str));
                let kept = self
                    .master_auth
                    .with_file_name(format!("{AUTH_FILE}.rotated-{}", self.job));
                match write_new_private(&kept, current) {
                    Ok(()) => format!(
                        "Codex rotated the credential during job {}; replace auth.json with {}",
                        self.job,
                        kept.display()
                    ),
                    Err(error) => format!(
                        "Codex rotated the credential during job {} and the rotated copy could not be kept ({error}); sign in again",
                        self.job
                    ),
                }
            }
            _ => "the credential copy changed but is not a credential of the same account; sign in again".to_owned(),
        }
    }

    fn block(&self, reason: &str) {
        let marker = blocked_marker(&self.master_auth);
        let written = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(OFlags::NOFOLLOW.bits().cast_signed())
            .open(&marker)
            .and_then(|mut file| file.write_all(reason.as_bytes()));
        match written {
            Ok(()) => tracing::error!(
                job = %self.job,
                marker = %marker.display(),
                %reason,
                "Codex credential blocked; recover it, delete the marker and restart the runner"
            ),
            Err(error) => tracing::error!(
                job = %self.job,
                %error,
                %reason,
                "Codex credential must not be used again but the block marker could not be written; stop the runner"
            ),
        }
    }
}

impl Drop for JobCredential {
    /// A timed-out, cancelled or fenced call still keeps a rotated credential before the job dir is removed.
    fn drop(&mut self) {
        self.settle(false);
    }
}

/// Token values and account id of a ChatGPT `auth.json`; `None` if it is not one.
fn parse(auth: &[u8]) -> Option<(Vec<String>, Option<String>)> {
    let json = serde_json::from_slice::<Value>(auth).ok()?;
    if json.get("auth_mode").and_then(Value::as_str) != Some("chatgpt") {
        return None;
    }
    let tokens = json.get("tokens")?.as_object()?;
    tokens
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())?;
    let account = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut values: Vec<String> = tokens
        .iter()
        .filter(|(key, _)| key.as_str() != "account_id")
        .filter_map(|(_, value)| value.as_str())
        .map(str::to_owned)
        .collect();
    if let Some(key) = json.get("OPENAI_API_KEY").and_then(Value::as_str) {
        values.push(key.to_owned());
    }
    Some((values, account))
}

/// Reads a regular file of at most [`MAX_AUTH_BYTES`] without following a symlink or blocking on a FIFO.
fn read_regular(path: &Path) -> io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((OFlags::NOFOLLOW | OFlags::NONBLOCK).bits().cast_signed())
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a regular file",
        ));
    }
    if metadata.len() > MAX_AUTH_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file too large"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_AUTH_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_AUTH_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file too large"));
    }
    Ok(bytes)
}

/// Writes a new 0600 file; fails if `path` exists (also as a symlink).
fn write_new_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file: File = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn auth(refresh: &str, account: &str) -> Vec<u8> {
        json!({"auth_mode": "chatgpt", "tokens": {"access_token": format!("at-{refresh}"), "refresh_token": refresh, "account_id": account}})
            .to_string()
            .into_bytes()
    }

    struct Dirs {
        _root: TempDir,
        master: PathBuf,
        home: PathBuf,
    }

    fn dirs() -> Dirs {
        let root = TempDir::new().unwrap();
        let master = root.path().join("credentials/auth.json");
        std::fs::create_dir_all(master.parent().unwrap()).unwrap();
        std::fs::write(&master, auth("rt-original", "acct-1")).unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        Dirs {
            _root: root,
            master,
            home,
        }
    }

    fn rotated(dirs: &Dirs, job: JobId) -> PathBuf {
        dirs.master
            .with_file_name(format!("auth.json.rotated-{job}"))
    }

    #[test]
    fn unchanged_copy_leaves_the_credential_usable() {
        let d = dirs();
        let job = JobId::new_random();
        drop(JobCredential::install(&d.master, &d.home, job, Scrubber::default()).unwrap());
        assert!(!credential_blocked(&d.master));
        assert!(!rotated(&d, job).exists());
    }

    #[test]
    fn rotation_is_kept_and_blocks_the_credential_even_when_dropped() {
        let d = dirs();
        let job = JobId::new_random();
        let credential =
            JobCredential::install(&d.master, &d.home, job, Scrubber::default()).unwrap();
        std::fs::write(d.home.join(AUTH_FILE), auth("rt-rotated", "acct-1")).unwrap();
        drop(credential);
        assert_eq!(
            std::fs::read(rotated(&d, job)).unwrap(),
            auth("rt-rotated", "acct-1")
        );
        assert!(credential_blocked(&d.master));
        assert!(matches!(
            JobCredential::install(&d.master, &d.home, JobId::new_random(), Scrubber::default()),
            Err(ProviderError::CredentialUnusable)
        ));
    }

    #[test]
    fn rotated_tokens_join_the_scrubber() {
        let d = dirs();
        let mut credential =
            JobCredential::install(&d.master, &d.home, JobId::new_random(), Scrubber::default())
                .unwrap();
        std::fs::remove_file(d.home.join(AUTH_FILE)).unwrap();
        std::fs::write(d.home.join(AUTH_FILE), auth("rt-new-SECRET", "acct-1")).unwrap();
        credential.watch();
        assert_eq!(
            credential
                .scrubber()
                .text("rt-new-SECRET at-rt-new-SECRET rt-original"),
            "[redacted] [redacted] [redacted]"
        );
    }

    #[test]
    fn refresh_failure_blocks_the_credential() {
        let d = dirs();
        let mut credential =
            JobCredential::install(&d.master, &d.home, JobId::new_random(), Scrubber::default())
                .unwrap();
        credential.settle(true);
        assert!(credential_blocked(&d.master));
    }

    #[test]
    fn symlinked_copy_is_not_followed_or_kept() {
        let d = dirs();
        let job = JobId::new_random();
        let credential =
            JobCredential::install(&d.master, &d.home, job, Scrubber::default()).unwrap();
        let target = d.home.join("other-secret");
        std::fs::write(&target, auth("rt-other", "acct-1")).unwrap();
        std::fs::remove_file(d.home.join(AUTH_FILE)).unwrap();
        symlink(&target, d.home.join(AUTH_FILE)).unwrap();
        drop(credential);
        assert!(
            !rotated(&d, job).exists(),
            "a symlink target must not be kept"
        );
        assert!(
            credential_blocked(&d.master),
            "unknown state blocks the credential"
        );
    }

    #[test]
    fn oversized_or_foreign_copy_is_not_kept() {
        for contents in [
            vec![b' '; 1024 * 1024],
            auth("rt-x", "acct-OTHER"),
            b"not json".to_vec(),
        ] {
            let d = dirs();
            let job = JobId::new_random();
            let credential =
                JobCredential::install(&d.master, &d.home, job, Scrubber::default()).unwrap();
            std::fs::write(d.home.join(AUTH_FILE), &contents).unwrap();
            drop(credential);
            assert!(!rotated(&d, job).exists());
            assert!(credential_blocked(&d.master));
        }
    }

    #[test]
    fn fifo_copy_does_not_block_the_reader() {
        let d = dirs();
        let credential =
            JobCredential::install(&d.master, &d.home, JobId::new_random(), Scrubber::default())
                .unwrap();
        std::fs::remove_file(d.home.join(AUTH_FILE)).unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(d.home.join(AUTH_FILE))
            .status()
            .unwrap();
        assert!(status.success());
        drop(credential);
        assert!(credential_blocked(&d.master));
    }
}
