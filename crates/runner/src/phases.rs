//! Per-job phase durations and sizes, logged once per job attempt (`job phase timings`) to see where the time
//! of a job goes. Values are numbers and fixed words only: no paths, ids or secrets.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

/// One recorded value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Value {
    Millis(u128),
    Bytes(u64),
    Word(&'static str),
}

/// Shared, append-only log of one job attempt's phases; clones record into the same log.
#[derive(Clone, Debug, Default)]
pub struct PhaseLog(Arc<Mutex<Vec<(&'static str, Value)>>>);

/// Instrumentation only: a log never makes two calls differ.
impl PartialEq for PhaseLog {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for PhaseLog {}

impl PhaseLog {
    /// Records `phase` as the time elapsed since `since`.
    pub fn since(&self, phase: &'static str, since: Instant) {
        self.duration(phase, since.elapsed());
    }

    /// Records `phase` as `duration`.
    pub fn duration(&self, phase: &'static str, duration: Duration) {
        self.push(phase, Value::Millis(duration.as_millis()));
    }

    /// Records a size in bytes.
    pub fn bytes(&self, what: &'static str, bytes: u64) {
        self.push(what, Value::Bytes(bytes));
    }

    /// Records a fixed word, such as whether the mirror was fetched.
    pub fn word(&self, what: &'static str, word: &'static str) {
        self.push(what, Value::Word(word));
    }

    /// `name=value` pairs in recording order: durations in ms, sizes in bytes.
    pub fn summary(&self) -> String {
        let entries = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let mut text = String::new();
        for (name, value) in entries.iter() {
            if !text.is_empty() {
                text.push(' ');
            }
            let _ = match value {
                Value::Millis(ms) => write!(text, "{name}={ms}ms"),
                Value::Bytes(bytes) => write!(text, "{name}={bytes}B"),
                Value::Word(word) => write!(text, "{name}={word}"),
            };
        }
        text
    }

    fn push(&self, name: &'static str, value: Value) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((name, value));
    }
}

/// Bytes of the regular files below `dir` (links not followed); 0 when it cannot be read.
pub(crate) fn dir_bytes(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_bytes(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |meta| meta.len()),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_lists_phases_in_order_with_units() {
        let log = PhaseLog::default();
        let shared = log.clone();
        log.duration("checkout", Duration::from_millis(2100));
        shared.word("mirror", "hit");
        shared.bytes("restore", 4096);
        assert_eq!(log.summary(), "checkout=2100ms mirror=hit restore=4096B");
        assert_eq!(PhaseLog::default().summary(), "");
    }

    #[test]
    fn dir_bytes_sums_nested_files() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/x"), b"123").unwrap();
        std::fs::write(dir.path().join("a/b/y"), b"45").unwrap();
        assert_eq!(dir_bytes(dir.path()), 5);
        assert_eq!(dir_bytes(&dir.path().join("missing")), 0);
    }
}
