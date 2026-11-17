//! The agent-session archive of one analysis (ADR-004, threat model T8): the Codex rollout of its thread,
//! `sessions/YYYY/MM/DD/rollout-<time>-<thread>.jsonl`, copied from the analysis's store into the per-job
//! `CODEX_HOME` before a resume and back after the job. Nothing else is ever copied: not `auth.json`, not
//! another thread's rollout. The per-job copy is agent-writable, so copy-back reads only real directories and
//! single-link regular files of bounded size, never following a symlink, and scrubs every known secret (T10).

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use contracts::job::result::Usage;
use rustix::fs::OFlags;
use serde::Deserialize;

use crate::provider::ProviderError;
use crate::scrub::Scrubber;

/// Rollout directory inside a `CODEX_HOME`.
pub(crate) const SESSIONS_DIR: &str = "sessions";
/// Largest rollout copied in either direction.
const MAX_ROLLOUT_BYTES: u64 = 256 * 1024 * 1024;
/// Longest thread id accepted (Codex uses UUIDs).
const MAX_THREAD_ID_CHARS: usize = 64;

/// Whether `thread` can be a Codex thread id: hexadecimal digits and dashes only.
pub(crate) fn valid_thread_id(thread: &str) -> bool {
    !thread.is_empty()
        && thread.len() <= MAX_THREAD_ID_CHARS
        && thread.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-')
}

/// Copies the stored rollout of `thread` into `codex_home`; [`ProviderError::SessionUnavailable`] when the store
/// has none or it is not usable. Returns the thread's cumulative usage so far.
pub(crate) fn restore(
    store: &Path,
    thread: &str,
    codex_home: &Path,
) -> Result<Usage, ProviderError> {
    if !valid_thread_id(thread) {
        return Err(ProviderError::SessionUnavailable);
    }
    let rollouts = match find(store, thread) {
        Ok(rollouts) if !rollouts.is_empty() => rollouts,
        Ok(_) => return Err(ProviderError::SessionUnavailable),
        Err(error) => {
            tracing::warn!(store = %store.display(), %error, "the stored agent session is unreadable; a new session starts");
            return Err(ProviderError::SessionUnavailable);
        }
    };
    let mut usage = Usage::default();
    for rel in rollouts {
        let target = codex_home.join(SESSIONS_DIR).join(&rel);
        create_private_dirs(target.parent().unwrap_or(codex_home))?;
        let facts = copy(&store.join(&rel), &target, &Scrubber::default())?;
        usage = facts.cumulative.unwrap_or(usage);
    }
    Ok(usage)
}

/// Saves the rollout of `thread` from `codex_home` into `store`, scrubbed with `scrubber`; whatever the store
/// held before is replaced. Fails when the per-job copy has no valid rollout of that thread.
pub(crate) fn save(
    codex_home: &Path,
    thread: &str,
    store: &Path,
    scrubber: &Scrubber,
) -> io::Result<()> {
    if !valid_thread_id(thread) {
        return Err(invalid("not a thread id"));
    }
    let source = codex_home.join(SESSIONS_DIR);
    let rollouts = find(&source, thread)?;
    if rollouts.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no rollout of the thread",
        ));
    }
    for rel in &rollouts {
        let target = store.join(rel);
        create_private_dirs(target.parent().unwrap_or(store))?;
        let mut temp = target.clone().into_os_string();
        temp.push(".tmp");
        let temp = PathBuf::from(temp);
        remove_if_present(&temp)?;
        copy(&source.join(rel), &temp, scrubber)?;
        std::fs::rename(&temp, &target)?;
    }
    remove_others(store, store, &rollouts)
}

/// Removes the analysis's stored session (lost or discarded).
pub(crate) fn forget(store: &Path) {
    match std::fs::remove_dir_all(store) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(store = %store.display(), %error, "cannot remove a lost agent session")
        }
    }
}

/// When the reached usage limit resets, from the latest rate-limit telemetry in the rollout of `thread` in
/// `codex_home` (ADR-003: never from the message text). `None` when no window is exhausted.
pub(crate) fn usage_limit_resets_at(codex_home: &Path, thread: &str) -> Option<DateTime<Utc>> {
    if !valid_thread_id(thread) {
        return None;
    }
    let source = codex_home.join(SESSIONS_DIR);
    let rel = find(&source, thread).ok()?.pop()?;
    let file = open_regular(&source.join(rel)).ok()?;
    scan(BufReader::new(file.take(MAX_ROLLOUT_BYTES)), |_| Ok(()))
        .ok()?
        .resets_at
}

/// What a rollout says about the thread.
#[derive(Debug, Default)]
struct Facts {
    /// The latest cumulative usage of the thread.
    cumulative: Option<Usage>,
    /// Reset time of the latest exhausted rate-limit window.
    resets_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: String,
    payload: Option<TokenCount>,
}

#[derive(Deserialize)]
struct TokenCount {
    #[serde(rename = "type")]
    kind: Option<String>,
    info: Option<TokenInfo>,
    rate_limits: Option<RateLimits>,
}

#[derive(Deserialize)]
struct TokenInfo {
    total_token_usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Deserialize)]
struct RateLimits {
    primary: Option<Window>,
    secondary: Option<Window>,
}

#[derive(Deserialize)]
struct Window {
    used_percent: Option<f64>,
    resets_at: Option<i64>,
}

/// Reads rollout lines, passing each to `sink`, and collects [`Facts`] from `token_count` records.
fn scan(reader: impl BufRead, mut sink: impl FnMut(&str) -> io::Result<()>) -> io::Result<Facts> {
    let mut facts = Facts::default();
    for line in reader.lines() {
        let line = line?;
        sink(&line)?;
        if !line.contains("\"token_count\"") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Record>(&line) else {
            continue;
        };
        let Some(payload) = record.payload.filter(|_| record.kind == "event_msg") else {
            continue;
        };
        if payload.kind.as_deref() != Some("token_count") {
            continue;
        }
        if let Some(total) = payload.info.and_then(|info| info.total_token_usage) {
            facts.cumulative = Some(Usage {
                input_tokens: total.input_tokens,
                cached_input_tokens: total.cached_input_tokens,
                output_tokens: total.output_tokens,
            });
        }
        if let Some(limits) = payload.rate_limits {
            facts.resets_at = [limits.primary, limits.secondary]
                .into_iter()
                .flatten()
                .filter(|window| window.used_percent.is_some_and(|used| used >= 100.0))
                .filter_map(|window| window.resets_at)
                .filter_map(|secs| DateTime::from_timestamp(secs, 0))
                .max();
        }
    }
    Ok(facts)
}

/// Copies one rollout line by line, scrubbed, into the new private file `target`.
fn copy(source: &Path, target: &Path, scrubber: &Scrubber) -> io::Result<Facts> {
    let input = open_regular(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(OFlags::NOFOLLOW.bits().cast_signed())
        .open(target)?;
    let mut written = io::BufWriter::new(&mut output);
    let facts = scan(BufReader::new(input.take(MAX_ROLLOUT_BYTES)), |line| {
        written.write_all(scrubber.text(line).as_bytes())?;
        written.write_all(b"\n")
    })?;
    written.flush()?;
    drop(written);
    output.sync_all()?;
    Ok(facts)
}

/// Opens a single-link regular file of bounded size without following a symlink or blocking on a FIFO.
fn open_regular(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((OFlags::NOFOLLOW | OFlags::NONBLOCK).bits().cast_signed())
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("a rollout is not a regular file"));
    }
    if metadata.nlink() != 1 {
        return Err(invalid("a rollout has more than one link"));
    }
    if metadata.len() > MAX_ROLLOUT_BYTES {
        return Err(invalid("a rollout exceeds the size cap"));
    }
    Ok(file)
}

/// Paths (relative to `root`) of the rollouts of `thread`: `YYYY/MM/DD/rollout-*-<thread>.jsonl`. Every
/// directory on the way must be a real directory and every matching entry a regular file; other entries
/// (another thread, anything else) are ignored. A missing `root` has none.
fn find(root: &Path, thread: &str) -> io::Result<Vec<PathBuf>> {
    let suffix = format!("-{thread}.jsonl");
    let mut found = Vec::new();
    if !real_dir(root)? {
        return Ok(found);
    }
    for year in subdirs(root, 4)? {
        for month in subdirs(&root.join(&year), 2)? {
            for day in subdirs(&root.join(&year).join(&month), 2)? {
                let dir = root.join(&year).join(&month).join(&day);
                for entry in std::fs::read_dir(&dir)? {
                    let entry = entry?;
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else {
                        continue;
                    };
                    if !(name.starts_with("rollout-") && name.ends_with(&suffix)) {
                        continue;
                    }
                    if !entry.file_type()?.is_file() {
                        return Err(invalid("a rollout of the thread is not a regular file"));
                    }
                    found.push(PathBuf::from(&year).join(&month).join(&day).join(name));
                }
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Whether `path` is a real directory; `false` when absent, an error when it is anything else.
fn real_dir(path: &Path) -> io::Result<bool> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(invalid("a session directory is not a real directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Names of the real subdirectories of `dir` made of `digits` ASCII digits; anything else is ignored.
fn subdirs(dir: &Path, digits: usize) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.len() == digits
            && name.chars().all(|ch| ch.is_ascii_digit())
            && entry.file_type()?.is_dir()
        {
            names.push(name);
        }
    }
    Ok(names)
}

/// Removes every file below `dir` that is not one of `keep` (relative to `root`), and emptied directories.
fn remove_others(root: &Path, dir: &Path, keep: &[PathBuf]) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            remove_others(root, &path, keep)?;
            if std::fs::read_dir(&path)?.next().is_none() {
                std::fs::remove_dir(&path)?;
            }
        } else if !path
            .strip_prefix(root)
            .is_ok_and(|rel| keep.iter().any(|kept| kept == rel))
        {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn create_private_dirs(dir: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests;
