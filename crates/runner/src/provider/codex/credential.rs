//! The Codex credential (ADR-003, threat model T13): the master `auth.json` in the runner's store, one copy per
//! job, compare-and-swap write-back of a copy Codex refreshed, recovery of copies a crashed run left on the
//! persistent volume, and whether claims may use the credential.
//!
//! A copy is agent-writable: it is read only as a small regular file without following symlinks, and written
//! back only when it is a ChatGPT credential of the master's account and user, read from the token claims (the
//! agent can write the `account_id` field), and the master is still the one the copy was taken from. A master
//! whose id token does not name its stored account is refused before any job uses it. Next to
//! each per-job `CODEX_HOME` the runner keeps `<home>.cas`, the SHA-256 of that master (runner-only directory),
//! so recovery after a crash applies the same compare-and-swap.
//! A master that can no longer be refreshed is recorded by digest in `auth.json.unusable`; signing in again
//! replaces the master, which makes the credential usable without further steps.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use contracts::ids::JobId;
use rustix::fs::OFlags;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

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
/// Suffix of the runner-only file next to a per-job `CODEX_HOME` holding the digest of the master it copied.
const BASE_SUFFIX: &str = ".cas";
/// Suffix a per-job `CODEX_HOME` gets when its copy could not be written back; recovered at the next start.
const RECOVER_SUFFIX: &str = ".recover";
/// `<home>.unknown` next to `<home>.cas`: the job's copy could not be brought back after a crash (process
/// isolation), so the staged copy proves nothing and the master it was taken from is marked unusable.
pub(crate) const UNKNOWN_SUFFIX: &str = ".unknown";
/// File next to the master naming, by digest, the master that can no longer be used.
const UNUSABLE_FILE: &str = "auth.json.unusable";

type Digest = [u8; 32];

/// The runner's Codex credential: its master `auth.json` and whether jobs may use it.
#[derive(Debug)]
pub struct CodexCredential {
    master: PathBuf,
    /// Serialises write-backs and recovery (jobs are serialised per credential; this guards the file).
    write: Mutex<()>,
    /// Digest of a master known to be unusable in this process, also when persisting it failed.
    unusable: Mutex<Option<Digest>>,
}

/// How a write-back ended.
enum WriteBack {
    /// The copy equals the master it was taken from.
    Unchanged,
    /// The refreshed copy is the new master.
    Written,
    /// The copy was not written back; the credential is marked unusable.
    Rejected,
    /// Writing the master failed; the copy is the only current credential and must be kept.
    Failed(io::Error),
}

impl CodexCredential {
    /// The credential whose master is `master` (`<state>/credentials/<id>/auth.json`).
    pub fn new(master: PathBuf) -> Self {
        Self {
            master,
            write: Mutex::new(()),
            unusable: Mutex::new(None),
        }
    }

    /// Whether jobs may use the credential (reported in claims): a readable ChatGPT master that is not marked
    /// unusable. A master without an [`identity`] is marked here, before a job can spend its refresh token: a
    /// refreshed copy of it could not be told from a foreign credential.
    pub fn usable(&self) -> bool {
        let Ok(master) = read_regular(&self.master) else {
            return false;
        };
        if parse(&master).is_none() {
            return false;
        }
        let digest = sha256(&master);
        if *self.unusable.lock().unwrap_or_else(PoisonError::into_inner) == Some(digest) {
            return false;
        }
        let persisted = read_regular(&self.unusable_file())
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned());
        if persisted.is_ok_and(|marked| marked == hex(&digest)) {
            return false;
        }
        if identity(&master).is_none() {
            self.mark_unusable(
                digest,
                "its id token does not name the stored account, so a refreshed copy could not be verified",
            );
            return false;
        }
        true
    }

    /// Applies the compare-and-swap write-back to every per-job `CODEX_HOME` a previous run left in
    /// `codex_homes`, then removes them; call before the first claim. Returns how many copies were written back.
    pub fn recover(&self, codex_homes: &Path) -> io::Result<usize> {
        let mut written = 0;
        let mut kept = Vec::new();
        for entry in std::fs::read_dir(codex_homes)? {
            let path = entry?.path();
            let Some(key) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(BASE_SUFFIX))
            else {
                continue;
            };
            let home = codex_homes.join(key);
            let parked = codex_homes.join(format!("{key}{RECOVER_SUFFIX}"));
            let unknown = codex_homes
                .join(format!("{key}{UNKNOWN_SUFFIX}"))
                .symlink_metadata()
                .is_ok();
            match self.recover_one(&path, &home, &parked, unknown) {
                Ok(true) => written += 1,
                Ok(false) => {}
                Err(error) => {
                    tracing::error!(dir = %home.display(), %error, "a crashed job's Codex credential could not be written back; it stays for the next start");
                    kept.extend([path.clone(), home, parked]);
                    continue;
                }
            }
            for leftover in [&home, &parked] {
                remove_tree(leftover);
            }
            remove_file(&path);
        }
        for entry in std::fs::read_dir(codex_homes)? {
            let path = entry?.path();
            if !kept.contains(&path) && !crate::codex_homes::is_lost_found(&path) {
                remove_tree(&path);
            }
        }
        Ok(written)
    }

    /// One leftover: `Ok(true)` when its copy became the master, `Err` when it must stay for the next start.
    /// `unknown`: the job's copy could not be brought back, so only the base digest counts (a newer master is
    /// left alone).
    fn recover_one(
        &self,
        base_file: &Path,
        home: &Path,
        parked: &Path,
        unknown: bool,
    ) -> io::Result<bool> {
        let Some(base) = read_regular(base_file)
            .ok()
            .and_then(|bytes| unhex(String::from_utf8_lossy(&bytes).trim()))
        else {
            tracing::warn!(file = %base_file.display(), "ignoring an unreadable credential base file of a crashed job");
            return Ok(false);
        };
        if unknown {
            self.mark_unusable_if_master(
                base,
                "the credential copy of a crashed job could not be brought back from the agent",
            );
            return Ok(false);
        }
        let copy = [parked, home]
            .into_iter()
            .map(|dir| dir.join(AUTH_FILE))
            .find(|copy| copy.symlink_metadata().is_ok());
        let Some(copy) = copy else {
            return Ok(false);
        };
        let current = match read_regular(&copy) {
            Ok(current) => current,
            Err(error) => {
                self.mark_unusable_if_master(
                    base,
                    &format!(
                        "the credential copy of a crashed job could not be read safely ({error})"
                    ),
                );
                return Ok(false);
            }
        };
        if !read_regular(&self.master).is_ok_and(|master| sha256(&master) == base) {
            tracing::warn!(dir = %home.display(), "the credential copy of an interrupted job was taken from an older master; it is discarded and the current master is kept");
            return Ok(false);
        }
        match self.write_back(&current, base) {
            WriteBack::Written => {
                tracing::info!(dir = %home.display(), "recovered the refreshed Codex credential of a crashed job");
                Ok(true)
            }
            WriteBack::Unchanged | WriteBack::Rejected => Ok(false),
            WriteBack::Failed(error) => {
                self.mark_unusable_in_memory(base);
                Err(error)
            }
        }
    }

    /// Writes `copy` to the master if it changed, the master is still the one with digest `base`, and the copy
    /// is a ChatGPT credential of the master's account and user.
    fn write_back(&self, copy: &[u8], base: Digest) -> WriteBack {
        if sha256(copy) == base {
            return WriteBack::Unchanged;
        }
        let _write = self.write.lock().unwrap_or_else(PoisonError::into_inner);
        let master = match read_regular(&self.master) {
            Ok(master) => master,
            Err(error) => {
                tracing::error!(path = %self.master.display(), %error, "the master Codex credential disappeared during the job; sign in again");
                return WriteBack::Rejected;
            }
        };
        let current = sha256(&master);
        if current != base {
            self.mark_unusable(
                current,
                "the master Codex credential changed while a job used a copy",
            );
            return WriteBack::Rejected;
        }
        let same_identity = parse(copy).is_some()
            && matches!((identity(&master), identity(copy)), (Some(old), Some(new)) if old.kept_by(&new));
        if !same_identity {
            self.mark_unusable(
                current,
                "the job's credential copy changed but is not a credential of the same account and user",
            );
            return WriteBack::Rejected;
        }
        match replace_private(&self.master, copy) {
            Ok(()) => {
                tracing::info!(path = %self.master.display(), "wrote the refreshed Codex credential back to the master");
                WriteBack::Written
            }
            Err(error) => WriteBack::Failed(error),
        }
    }

    /// Marks the master with digest `digest` unusable, in memory and on disk.
    fn mark_unusable(&self, digest: Digest, reason: &str) {
        self.mark_unusable_in_memory(digest);
        match replace_private(&self.unusable_file(), hex(&digest).as_bytes()) {
            Ok(()) => {
                tracing::error!(%reason, "the Codex credential is unusable; sign in again with `codex login --device-auth` in the runner container")
            }
            Err(error) => {
                tracing::error!(%reason, %error, "the Codex credential is unusable in this process; its state could not be saved; sign in again before restarting the runner")
            }
        }
    }

    /// Marks the current master unusable if it is still the one with digest `base`.
    fn mark_unusable_if_master(&self, base: Digest, reason: &str) {
        if read_regular(&self.master).is_ok_and(|master| sha256(&master) == base) {
            self.mark_unusable(base, reason);
        }
    }

    /// Marks the current master unusable.
    fn mark_current_unusable(&self, reason: &str) {
        match read_regular(&self.master) {
            Ok(master) => self.mark_unusable(sha256(&master), reason),
            Err(error) => {
                tracing::error!(%reason, %error, "the master Codex credential cannot be read; sign in again")
            }
        }
    }

    fn mark_unusable_in_memory(&self, digest: Digest) {
        *self.unusable.lock().unwrap_or_else(PoisonError::into_inner) = Some(digest);
    }

    fn unusable_file(&self) -> PathBuf {
        self.master.with_file_name(UNUSABLE_FILE)
    }
}

/// The credential copy of one job and the secrets the provider scrubs (its tokens, rotated ones included).
pub(crate) struct JobCredential<'a> {
    credential: &'a CodexCredential,
    codex_home: PathBuf,
    copy: PathBuf,
    job: JobId,
    base: Digest,
    last_seen: Vec<u8>,
    scrubber: Scrubber,
    remembered_tokens: usize,
    remembered_bytes: usize,
    /// The agent holds the only current copy (process isolation, from its start to the copy-back): the staged
    /// copy proves nothing.
    away: bool,
    /// Set once settled: whether the scrubber is known complete.
    settled: Option<bool>,
}

impl<'a> JobCredential<'a> {
    /// Copies the master into `codex_home`; `known` are further secrets to scrub (the agent's tokens).
    pub(crate) fn install(
        credential: &'a CodexCredential,
        codex_home: &Path,
        job: JobId,
        known: Scrubber,
    ) -> Result<Self, ProviderError> {
        if !credential.usable() {
            tracing::error!(
                path = %credential.master.display(),
                "the Codex credential is missing or unusable; sign in with `codex login --device-auth` in the runner container"
            );
            return Err(ProviderError::CredentialUnusable);
        }
        let original =
            read_regular(&credential.master).map_err(|_| ProviderError::CredentialUnusable)?;
        let Some(tokens) = parse(&original) else {
            return Err(ProviderError::CredentialUnusable);
        };
        let base = sha256(&original);
        replace_private(&base_file(codex_home), hex(&base).as_bytes())?;
        let copy = codex_home.join(AUTH_FILE);
        // A copy from an earlier call of the same job (a resume that fell back to a new session) is settled.
        remove_file(&copy);
        write_new_private(&copy, &original)?;
        let mut scrubber = known;
        scrubber.extend(tokens.iter().map(String::as_str));
        Ok(Self {
            credential,
            codex_home: codex_home.to_path_buf(),
            copy,
            job,
            base,
            last_seen: original,
            scrubber,
            remembered_tokens: 0,
            remembered_bytes: 0,
            away: false,
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
        let Some(tokens) = parse(&self.last_seen) else {
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

    /// Re-reads the copy so that a rotation since the last activity joins the scrubber; false when the copy
    /// cannot be read safely (its tokens may then be unknown).
    pub(crate) fn rescan(&mut self) -> bool {
        match read_regular(&self.copy) {
            Ok(current) => {
                if let Some(tokens) = parse(&current) {
                    self.scrubber.extend(tokens.iter().map(String::as_str));
                }
                true
            }
            Err(_) => false,
        }
    }

    /// Ends the job's use of the copy (once, after the agent exited): writes a refreshed copy back
    /// (compare-and-swap) and marks the credential unusable after a refresh failure, a rejected copy or a
    /// conflict. False when the copy could not be read safely: its tokens may be missing from the scrubber, so
    /// the caller must not forward agent text.
    /// The job's copy could not be brought back (process isolation): its state is unknown, so the master is
    /// marked unusable (a new login is needed) and nothing is written back.
    pub(crate) fn unknown(&mut self, reason: &str) {
        self.credential.mark_unusable_if_master(self.base, reason);
        remove_file(&base_file(&self.codex_home));
        self.settled = Some(false);
    }

    /// The agent started with its own copy (process isolation): from now on only its `CODEX_HOME` holds the
    /// current credential, until [`Self::returned`].
    pub(crate) fn handed_over(&mut self) {
        self.away = true;
    }

    /// The agent's copy is back in the staging home.
    pub(crate) fn returned(&mut self) {
        self.away = false;
    }

    pub(crate) fn settle(&mut self, refresh_failed: bool) -> bool {
        if let Some(certain) = self.settled {
            return certain;
        }
        if self.away {
            // Settling the stale staged copy would discard a refresh Codex made: the base file stays, so
            // start-up recovery applies the compare-and-swap to the copy left in the agent's home.
            tracing::warn!(job = %self.job, "the job ended before the agent's Codex credential was copied back; its compare-and-swap base is kept for the next runner start");
            self.settled = Some(false);
            return false;
        }
        let certain = self.settle_once(refresh_failed);
        self.settled = Some(certain);
        certain
    }

    fn settle_once(&mut self, refresh_failed: bool) -> bool {
        let base_file = base_file(&self.codex_home);
        let current = match read_regular(&self.copy) {
            Ok(current) => current,
            Err(error) => {
                self.credential.mark_unusable_if_master(
                    self.base,
                    &format!("the job's credential copy could not be read safely ({error})"),
                );
                remove_file(&base_file);
                return false;
            }
        };
        if let Some(tokens) = parse(&current) {
            self.scrubber.extend(tokens.iter().map(String::as_str));
        }
        match self.credential.write_back(&current, self.base) {
            WriteBack::Unchanged | WriteBack::Written | WriteBack::Rejected => {
                remove_file(&base_file);
            }
            WriteBack::Failed(error) => {
                self.credential.mark_unusable_in_memory(self.base);
                self.park(&error);
            }
        }
        if refresh_failed {
            self.credential
                .mark_current_unusable("Codex could not refresh the credential");
        }
        true
    }

    /// Moves the per-job `CODEX_HOME` out of the job cleanup's way; the next start writes its copy back.
    fn park(&self, error: &io::Error) {
        let mut parked = self.codex_home.clone().into_os_string();
        parked.push(RECOVER_SUFFIX);
        match std::fs::rename(&self.codex_home, &parked) {
            Ok(()) => {
                tracing::error!(job = %self.job, %error, dir = %PathBuf::from(&parked).display(), "the refreshed Codex credential could not be written back; it is kept for the next runner start")
            }
            Err(rename) => {
                tracing::error!(job = %self.job, %error, %rename, "the refreshed Codex credential could not be written back or kept; sign in again")
            }
        }
    }
}

impl Drop for JobCredential<'_> {
    /// A timed-out, cancelled or fenced call still writes a refreshed copy back before the job dir is removed;
    /// a copy still with the agent is left to start-up recovery.
    fn drop(&mut self) {
        self.settle(false);
    }
}

/// The runner-only file next to `codex_home` holding the digest of the master its copy came from.
fn base_file(codex_home: &Path) -> PathBuf {
    let mut path = codex_home.as_os_str().to_owned();
    path.push(BASE_SUFFIX);
    PathBuf::from(path)
}

/// Token values of a ChatGPT `auth.json`; `None` if it is not one.
fn parse(auth: &[u8]) -> Option<Vec<String>> {
    let json = serde_json::from_slice::<Value>(auth).ok()?;
    if json.get("auth_mode").and_then(Value::as_str) != Some("chatgpt") {
        return None;
    }
    let tokens = json.get("tokens")?.as_object()?;
    tokens
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())?;
    let mut values: Vec<String> = tokens
        .iter()
        .filter(|(key, _)| key.as_str() != "account_id")
        .filter_map(|(_, value)| value.as_str())
        .map(str::to_owned)
        .collect();
    if let Some(key) = json.get("OPENAI_API_KEY").and_then(Value::as_str) {
        values.push(key.to_owned());
    }
    Some(values)
}

/// Whom a ChatGPT `auth.json` acts as. Its id token binds it to the stored account; every other claim is
/// optional in Codex, and the access token need not be a JWT.
struct Identity {
    /// The stored `account_id`, which the id token names as well.
    account: String,
    /// The id token's user.
    user: Option<String>,
    /// What the access token names.
    access: Claims,
}

/// Account and user in the `https://api.openai.com/auth` claim of a token; nothing for a token without it.
#[derive(Default)]
struct Claims {
    account: Option<String>,
    user: Option<String>,
}

impl Identity {
    /// Whether `copy`, a refreshed copy of this credential, acts as the same account and user: it keeps every
    /// claim this one has (a refresh may add one) and its access token names no other account.
    fn kept_by(&self, copy: &Self) -> bool {
        let kept = |old: &Option<String>, new: &Option<String>| old.is_none() || old == new;
        copy.account == self.account
            && kept(&self.user, &copy.user)
            && kept(&self.access.account, &copy.access.account)
            && kept(&self.access.user, &copy.access.user)
            && copy
                .access
                .account
                .as_ref()
                .is_none_or(|account| *account == copy.account)
    }
}

/// The identity of `auth`; `None` unless it stores an account id and its id token names that account.
fn identity(auth: &[u8]) -> Option<Identity> {
    let json = serde_json::from_slice::<Value>(auth).ok()?;
    let tokens = json.get("tokens")?;
    let account = nonempty(tokens, "account_id")?;
    let id_token = claims(nonempty(tokens, "id_token")?);
    if id_token.account.as_deref() != Some(account) {
        return None;
    }
    Some(Identity {
        account: account.to_owned(),
        user: id_token.user,
        access: nonempty(tokens, "access_token")
            .map(claims)
            .unwrap_or_default(),
    })
}

/// The claims of `token`, read as Codex reads them (`chatgpt_user_id`, else `user_id`).
/// No signature check: identities are compared, not authenticated (a forged token fails at the backend).
fn claims(token: &str) -> Claims {
    let mut parts = token.split('.');
    let (Some(_), Some(payload), Some(_), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Claims::default();
    };
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .ok()
        .and_then(|payload| serde_json::from_slice::<Value>(&payload).ok());
    let Some(auth) = payload
        .as_ref()
        .and_then(|payload| payload.get("https://api.openai.com/auth"))
    else {
        return Claims::default();
    };
    Claims {
        account: nonempty(auth, "chatgpt_account_id").map(str::to_owned),
        user: nonempty(auth, "chatgpt_user_id")
            .or_else(|| nonempty(auth, "user_id"))
            .map(str::to_owned),
    }
}

fn nonempty<'a>(object: &'a Value, key: &str) -> Option<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

fn sha256(bytes: &[u8]) -> Digest {
    Sha256::digest(bytes).into()
}

fn hex(digest: &Digest) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Digest> {
    let mut digest = [0_u8; 32];
    if text.len() != 64 {
        return None;
    }
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(digest)
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

/// Atomically replaces `path` with a 0600 file holding `contents` (new temp file, fsync, rename, fsync dir).
fn replace_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    remove_file(&temp);
    let written = write_new_private(&temp, contents).and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        remove_file(&temp);
    }
    written?;
    if let Some(dir) = path.parent() {
        File::open(dir)?.sync_all()?;
    }
    Ok(())
}

fn remove_file(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "cannot remove a credential work file")
        }
    }
}

fn remove_tree(path: &Path) {
    let removed = match path.symlink_metadata() {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) => Err(error),
    };
    match removed {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "cannot remove a leftover per-job Codex home")
        }
    }
}

#[cfg(test)]
mod tests;
