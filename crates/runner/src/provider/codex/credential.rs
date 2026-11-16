//! The per-job copy of the Codex credential. Until compare-and-swap write-back (WP3-RUN, T13), a copy that
//! changed during the job (refresh-token rotation) is kept next to the master and the credential is blocked:
//! the master's refresh token may be spent, so no further job may use it until the operator recovers it.
//! A refresh failure blocks it too. The copy is agent-writable, so it is read only as a small regular file
//! without following symlinks and kept only if it is a credential of the same account.
//!
//! The block is set in this process first and then persisted as a marker file; the file work runs on its own
//! thread with a bounded wait, so a stalled filesystem or a planted FIFO cannot hang the async runtime.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError, mpsc};
use std::time::Duration;

use contracts::ids::JobId;
use rustix::fs::OFlags;
use serde_json::Value;

use crate::provider::ProviderError;
use crate::scrub::Scrubber;

/// The credential file inside a `CODEX_HOME`.
pub(crate) const AUTH_FILE: &str = "auth.json";
/// Largest credential file read (a real `auth.json` has a few kB).
const MAX_AUTH_BYTES: u64 = 64 * 1024;
/// Most token values remembered from rewrites of the copy during one call; more ends the call.
const MAX_REMEMBERED_TOKENS: usize = 16;
/// Most bytes of token values remembered from rewrites of the copy during one call.
const MAX_REMEMBERED_BYTES: usize = 64 * 1024;
/// How long settling the copy may take before the credential simply stays blocked.
const SETTLE_WAIT: Duration = Duration::from_secs(2);

/// Masters whose settle thread stalls past [`SETTLE_WAIT`] (tests of the timeout path).
#[cfg(test)]
pub(crate) static STALL_SETTLE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Masters blocked in this process; consulted even when the marker file could not be written.
static BLOCKED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Whether the credential whose master is `master_auth` is blocked until the operator recovers it: blocked in
/// this process, or anything exists at the marker path.
pub fn credential_blocked(master_auth: &Path) -> bool {
    blocked_here(master_auth) || blocked_marker(master_auth).symlink_metadata().is_ok()
}

/// Whether jobs may use the credential: a readable master that is not blocked (reported in claims).
pub fn credential_usable(master_auth: &Path) -> bool {
    !credential_blocked(master_auth)
        && read_regular(master_auth).is_ok_and(|auth| parse(&auth).is_some())
}

fn blocked_marker(master_auth: &Path) -> PathBuf {
    master_auth.with_file_name(format!("{AUTH_FILE}.blocked"))
}

fn blocked_here(master_auth: &Path) -> bool {
    BLOCKED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .any(|blocked| blocked == master_auth)
}

/// Blocks `master_auth` in this process; false if it already was.
fn block_here(master_auth: &Path) -> bool {
    let mut blocked = BLOCKED.lock().unwrap_or_else(PoisonError::into_inner);
    if blocked.iter().any(|path| path == master_auth) {
        return false;
    }
    blocked.push(master_auth.to_path_buf());
    true
}

fn unblock_here(master_auth: &Path) {
    BLOCKED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|path| path != master_auth);
}

/// The credential copy of one job and the secrets the provider scrubs (its tokens, rotated ones included).
pub(crate) struct JobCredential {
    master_auth: PathBuf,
    codex_home: PathBuf,
    copy: PathBuf,
    job: JobId,
    original: Vec<u8>,
    account: Option<String>,
    last_seen: Vec<u8>,
    scrubber: Scrubber,
    remembered_tokens: usize,
    remembered_bytes: usize,
    /// Set once settled: whether the scrubber is known complete and the copy was handled.
    settled: Option<bool>,
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
            codex_home: codex_home.to_path_buf(),
            copy,
            job,
            last_seen: original.clone(),
            original,
            account,
            scrubber,
            remembered_tokens: 0,
            remembered_bytes: 0,
            settled: None,
        })
    }

    /// Every secret the provider knows so far.
    pub(crate) fn scrubber(&self) -> &Scrubber {
        &self.scrubber
    }

    /// Re-reads the copy; the tokens of a credential rotated meanwhile join the scrubber. False when the copy
    /// was rewritten more than a real refresh does: the caller ends the call (secrets are never forgotten).
    pub(crate) fn watch(&mut self) -> bool {
        let Ok(current) = read_regular(&self.copy) else {
            return true;
        };
        if current == self.last_seen {
            return true;
        }
        self.last_seen = current;
        let Some((tokens, _)) = parse(&self.last_seen) else {
            return true;
        };
        self.remembered_tokens += tokens.len();
        self.remembered_bytes += tokens.iter().map(String::len).sum::<usize>();
        if self.remembered_tokens > MAX_REMEMBERED_TOKENS
            || self.remembered_bytes > MAX_REMEMBERED_BYTES
        {
            tracing::error!(job = %self.job, "the Codex credential copy was rewritten too often; ending the call");
            return false;
        }
        self.scrubber.extend(tokens.iter().map(String::as_str));
        true
    }

    /// Ends the job's use of the copy (once). The credential is blocked in this process first; the copy is
    /// read inline (bounded, non-blocking) so rotated tokens join the scrubber before anything else. An unchanged
    /// copy (and no refresh failure) unblocks it; otherwise the file work runs on its own thread, waited for at
    /// most [`SETTLE_WAIT`]. False when the outcome is uncertain (unreadable copy, thread not started or not
    /// finished): the caller must not forward agent text then.
    pub(crate) fn settle(&mut self, refresh_failed: bool) -> bool {
        if let Some(certain) = self.settled {
            return certain;
        }
        let certain = self.settle_once(refresh_failed);
        self.settled = Some(certain);
        certain
    }

    fn settle_once(&mut self, refresh_failed: bool) -> bool {
        let newly_blocked = block_here(&self.master_auth);
        let current = read_regular(&self.copy);
        if let Ok(current) = &current
            && *current != self.original
            && let Some((tokens, _)) = parse(current)
        {
            self.scrubber.extend(tokens.iter().map(String::as_str));
        }
        if !refresh_failed
            && current
                .as_ref()
                .is_ok_and(|current| *current == self.original)
        {
            if newly_blocked {
                unblock_here(&self.master_auth);
            }
            return true;
        }
        let readable = current.is_ok();
        let work = Settle {
            master_auth: self.master_auth.clone(),
            codex_home: self.codex_home.clone(),
            job: self.job,
            original: self.original.clone(),
            account: self.account.clone(),
            current: current.map_err(|error| error.to_string()),
        };
        let (done, finished) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("credential-settle".to_owned())
            .spawn(move || {
                work.run();
                let _ = done.send(());
            });
        let settled = spawned.map_err(|error| error.to_string()).and_then(|_| {
            finished
                .recv_timeout(SETTLE_WAIT)
                .map_err(|error| error.to_string())
        });
        match settled {
            Ok(()) => readable,
            Err(error) => {
                tracing::error!(
                    job = %self.job,
                    %error,
                    "settling the Codex credential copy did not finish; the credential stays blocked until the runner restarts"
                );
                false
            }
        }
    }
}

impl Drop for JobCredential {
    /// A timed-out, cancelled or fenced call still keeps a rotated credential before the job dir is removed.
    fn drop(&mut self) {
        self.settle(false);
    }
}

/// The file work of [`JobCredential::settle`] for a blocked credential, on its own thread.
struct Settle {
    master_auth: PathBuf,
    codex_home: PathBuf,
    job: JobId,
    original: Vec<u8>,
    account: Option<String>,
    /// The copy as read by `settle`, or why it could not be read.
    current: Result<Vec<u8>, String>,
}

impl Settle {
    fn run(self) {
        #[cfg(test)]
        if STALL_SETTLE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&self.master_auth)
        {
            std::thread::sleep(SETTLE_WAIT + Duration::from_millis(500));
        }
        let reason = match &self.current {
            Ok(current) if *current == self.original => {
                "the credential could not be refreshed".to_owned()
            }
            Ok(current) => self.keep(current),
            Err(error) => {
                format!("the credential copy could not be read safely after the job ({error})")
            }
        };
        self.write_marker(&reason);
    }

    /// Keeps a rotated copy of the same (non-empty) account; returns the blocking reason.
    fn keep(&self, current: &[u8]) -> String {
        let Some((_, account)) = parse(current) else {
            return "the credential copy changed but is not a credential; sign in again".to_owned();
        };
        let same_account = matches!((&account, &self.account), (Some(new), Some(old)) if !new.is_empty() && new == old);
        if !same_account {
            return "the credential copy changed but is not a credential of the same account; sign in again"
                .to_owned();
        }
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
                "Codex rotated the credential during job {} and the rotated copy could not be kept ({error}); {}",
                self.job,
                self.preserve_job_home()
            ),
        }
    }

    /// Moves the per-job `CODEX_HOME` out of the way of the job cleanup, so its copy is not deleted.
    fn preserve_job_home(&self) -> String {
        let mut kept = self.codex_home.clone().into_os_string();
        kept.push("-kept");
        let kept = PathBuf::from(kept);
        match std::fs::rename(&self.codex_home, &kept) {
            Ok(()) => {
                tracing::error!(job = %self.job, dir = %kept.display(), "the rotated Codex credential is left in this directory for recovery");
                format!("the current credential is in {}", kept.display())
            }
            Err(error) => {
                tracing::error!(job = %self.job, dir = %self.codex_home.display(), %error, "the rotated Codex credential could not be preserved; sign in again");
                "the current credential could not be preserved; sign in again".to_owned()
            }
        }
    }

    /// Persists the block. Never opens an existing path: anything at the marker path already means blocked.
    fn write_marker(&self, reason: &str) {
        let marker = blocked_marker(&self.master_auth);
        let written = OpenOptions::new()
            .write(true)
            .create_new(true)
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
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => tracing::error!(
                job = %self.job,
                marker = %marker.display(),
                %reason,
                "Codex credential blocked (marker already present); recover it, delete the marker and restart the runner"
            ),
            Err(error) => tracing::error!(
                job = %self.job,
                %error,
                %reason,
                "Codex credential blocked in this process only: the marker could not be written; recover it before restarting the runner"
            ),
        }
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

    #[test]
    fn master_without_account_id_never_keeps_a_rotated_copy() {
        let d = dirs();
        let no_account = |refresh: &str| {
            json!({"auth_mode": "chatgpt", "tokens": {"refresh_token": refresh}})
                .to_string()
                .into_bytes()
        };
        std::fs::write(&d.master, no_account("rt-original")).unwrap();
        let job = JobId::new_random();
        let credential =
            JobCredential::install(&d.master, &d.home, job, Scrubber::default()).unwrap();
        std::fs::write(d.home.join(AUTH_FILE), no_account("rt-rotated")).unwrap();
        drop(credential);
        assert!(
            !rotated(&d, job).exists(),
            "None == None must not count as the same account"
        );
        assert!(credential_blocked(&d.master));
    }

    fn kept_home(d: &Dirs) -> PathBuf {
        d.home.with_file_name("home-kept")
    }

    #[test]
    fn unwritable_credentials_dir_blocks_in_process_and_preserves_the_job_home() {
        use std::os::unix::fs::PermissionsExt;
        let d = dirs();
        let credentials = d.master.parent().unwrap().to_path_buf();
        let job = JobId::new_random();
        let credential =
            JobCredential::install(&d.master, &d.home, job, Scrubber::default()).unwrap();
        std::fs::write(d.home.join(AUTH_FILE), auth("rt-rotated", "acct-1")).unwrap();
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o500)).unwrap();
        drop(credential);
        let marker_written = blocked_marker(&d.master).exists();
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            !marker_written && !rotated(&d, job).exists(),
            "both writes failed"
        );
        assert!(
            credential_blocked(&d.master),
            "blocked in this process without the marker"
        );
        assert!(!credential_usable(&d.master));
        assert_eq!(
            std::fs::read(kept_home(&d).join(AUTH_FILE)).unwrap(),
            auth("rt-rotated", "acct-1"),
            "the only current copy is left for the operator"
        );
    }

    #[test]
    fn occupied_rotated_path_preserves_the_job_home() {
        let d = dirs();
        let job = JobId::new_random();
        let credential =
            JobCredential::install(&d.master, &d.home, job, Scrubber::default()).unwrap();
        std::fs::write(rotated(&d, job), "occupied").unwrap();
        std::fs::write(d.home.join(AUTH_FILE), auth("rt-rotated", "acct-1")).unwrap();
        drop(credential);
        assert!(blocked_marker(&d.master).exists());
        assert_eq!(
            std::fs::read(kept_home(&d).join(AUTH_FILE)).unwrap(),
            auth("rt-rotated", "acct-1")
        );
    }

    #[test]
    fn fifo_at_the_marker_path_does_not_hang_settle() {
        let d = dirs();
        let credential =
            JobCredential::install(&d.master, &d.home, JobId::new_random(), Scrubber::default())
                .unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(blocked_marker(&d.master))
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::write(d.home.join(AUTH_FILE), auth("rt-rotated", "acct-1")).unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(credential);
            let _ = done.send(());
        });
        assert!(
            finished.recv_timeout(Duration::from_secs(10)).is_ok(),
            "settle hung on the FIFO"
        );
        assert!(credential_blocked(&d.master));
    }

    #[test]
    fn rewriting_the_copy_too_often_ends_the_call_without_forgetting_secrets() {
        let d = dirs();
        let mut credential =
            JobCredential::install(&d.master, &d.home, JobId::new_random(), Scrubber::default())
                .unwrap();
        let mut accepted = 0;
        for round in 0..20 {
            std::fs::write(
                d.home.join(AUTH_FILE),
                auth(&format!("rt-round-{round}"), "acct-1"),
            )
            .unwrap();
            if !credential.watch() {
                break;
            }
            accepted += 1;
        }
        assert!(accepted < 20, "the number of remembered tokens is capped");
        assert_eq!(
            credential.scrubber().text("rt-round-0"),
            "[redacted]",
            "older secrets are kept"
        );
    }
}
