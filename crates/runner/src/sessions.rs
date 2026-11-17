//! The agent-session store on the runner's private volume (ADR-004, threat model T8a): one directory per
//! analysis, filled and read only by the provider for that analysis's jobs, deleted when the server drops the
//! session (cancel, re-analysis) or after the retention period.

use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use contracts::ids::AnalysisId;

use crate::scrub::Scrubber;

/// The session store below one root directory.
#[derive(Clone, Debug)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    /// The store in `root`.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The directory of `analysis`'s session.
    pub fn dir(&self, analysis: AnalysisId) -> PathBuf {
        self.root.join(analysis.to_string())
    }

    /// Deletes `analysis`'s session; an absent one counts as deleted.
    pub fn remove(&self, analysis: AnalysisId) -> io::Result<()> {
        match std::fs::remove_dir_all(self.dir(analysis)) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// Deletes every session not written for `retention`; returns how many were deleted.
    pub fn remove_expired(&self, retention: Duration) -> io::Result<usize> {
        let Some(cutoff) = SystemTime::now().checked_sub(retention) else {
            return Ok(0);
        };
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error),
        };
        let mut removed = 0;
        for entry in entries {
            let path = entry?.path();
            if newest_change(&path)? < cutoff {
                remove_tree(&path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Replaces every value `scrubber` knows in the files of `analysis`'s session (secrets only the caller
    /// holds, such as the job token and the runner secret).
    pub fn scrub(&self, analysis: AnalysisId, scrubber: &Scrubber) -> io::Result<()> {
        scrub_tree(&self.dir(analysis), scrubber)
    }
}

/// The latest modification time of `path` or anything below it (symlinks are not followed).
fn newest_change(path: &Path) -> io::Result<SystemTime> {
    let metadata = path.symlink_metadata()?;
    let mut newest = metadata.modified()?;
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            newest = newest.max(newest_change(&entry?.path())?);
        }
    }
    Ok(newest)
}

fn remove_tree(path: &Path) -> io::Result<()> {
    if path.symlink_metadata()?.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Rewrites, line by line, every regular file below `dir` that contains a value `scrubber` knows.
fn scrub_tree(dir: &Path, scrubber: &Scrubber) -> io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            scrub_tree(&entry.path(), scrubber)?;
        } else if kind.is_file() {
            scrub_file(&entry.path(), scrubber)?;
        }
    }
    Ok(())
}

fn scrub_file(path: &Path, scrubber: &Scrubber) -> io::Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".scrub");
    let temp = PathBuf::from(temp);
    let mut changed = false;
    {
        let reader = BufReader::new(std::fs::File::open(path)?);
        let mut writer = io::BufWriter::new(std::fs::File::create(&temp)?);
        for line in reader.lines() {
            let line = line?;
            let clean = scrubber.text(&line);
            changed |= clean != line;
            writer.write_all(clean.as_bytes())?;
            writer.write_all(b"\n")?;
        }
        writer.flush()?;
    }
    if changed {
        std::fs::rename(&temp, path)
    } else {
        std::fs::remove_file(&temp)
    }
}

#[cfg(test)]
mod tests {
    use contracts::ids::AnalysisId;
    use tempfile::TempDir;

    use super::*;

    fn store_with(analysis: AnalysisId) -> (TempDir, SessionStore) {
        let root = TempDir::new().unwrap();
        let store = SessionStore::new(root.path().join("sessions"));
        let dir = store.dir(analysis).join("2026/11/16");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("rollout-x-t.jsonl"), "{}\n").unwrap();
        (root, store)
    }

    #[test]
    fn session_deleted_on_cancel_supersede_retention() {
        let analysis = AnalysisId::new_random();
        let (_root, store) = store_with(analysis);
        store.remove(analysis).unwrap();
        assert!(!store.dir(analysis).exists(), "dropped by the server");
        store.remove(analysis).unwrap();

        let old = AnalysisId::new_random();
        let (_root, store) = store_with(old);
        let fresh = AnalysisId::new_random();
        std::fs::create_dir_all(store.dir(fresh)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let keep_recent = Duration::from_secs(3600);
        assert_eq!(store.remove_expired(keep_recent).unwrap(), 0);
        assert_eq!(store.remove_expired(Duration::from_millis(10)).unwrap(), 2);
        assert!(!store.dir(old).exists() && !store.dir(fresh).exists());
    }

    #[test]
    fn retention_counts_the_newest_file_of_a_session() {
        let analysis = AnalysisId::new_random();
        let (_root, store) = store_with(analysis);
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(
            store.dir(analysis).join("2026/11/16/rollout-x-t.jsonl"),
            "{}\n{}\n",
        )
        .unwrap();
        assert_eq!(store.remove_expired(Duration::from_millis(40)).unwrap(), 0);
    }

    #[test]
    fn scrub_replaces_secrets_in_stored_files() {
        let analysis = AnalysisId::new_random();
        let (_root, store) = store_with(analysis);
        let file = store.dir(analysis).join("2026/11/16/rollout-x-t.jsonl");
        std::fs::write(&file, "{\"a\":\"job-token-CANARY\"}\n").unwrap();
        store
            .scrub(analysis, &Scrubber::new(["job-token-CANARY"]))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "{\"a\":\"[redacted]\"}\n"
        );
    }
}
