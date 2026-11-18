//! Disposable scratch directories on Docker volumes (`tmp-*`, ADR-013): they survive a container restart, so the
//! runner empties them at its start, before the first claim; and it claims no job while their filesystem is short
//! of free space ([`Reserve`]).

use std::io;
use std::path::{Path, PathBuf};

/// What [`clear`] removed and what it could not.
#[derive(Debug, Default)]
pub struct Cleared {
    /// Entries removed.
    pub removed: usize,
    /// Entries that could not be removed, with the reason.
    pub failed: Vec<(PathBuf, io::Error)>,
}

/// Removes every entry below `dir`, which stays with its owner and mode; symbolic links are removed, never
/// followed. Fails only when `dir` cannot be listed.
pub fn clear(dir: &Path) -> io::Result<Cleared> {
    let mut cleared = Cleared::default();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let result = match entry.file_type() {
            Ok(kind) if kind.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => std::fs::remove_file(&path),
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => cleared.removed += 1,
            Err(error) => cleared.failed.push((path, error)),
        }
    }
    Ok(cleared)
}

/// Free space the scratch filesystem must keep for the runner to claim a job: a start condition, not a quota.
#[derive(Debug)]
pub struct Reserve {
    dir: PathBuf,
    bytes: u64,
    /// The last state seen, so each change is logged once.
    low: Option<bool>,
}

impl Reserve {
    /// At least `bytes` free (for unprivileged users) on the filesystem holding `dir`.
    pub fn new(dir: PathBuf, bytes: u64) -> Self {
        Self {
            dir,
            bytes,
            low: None,
        }
    }

    /// Whether a job may be claimed now; a filesystem that cannot be inspected counts as full.
    pub fn allows_claim(&mut self) -> bool {
        let free = free_bytes(&self.dir);
        let low = free.as_ref().map_or(true, |free| *free < self.bytes);
        if self.low != Some(low) {
            match (&free, low) {
                (Err(error), _) => {
                    tracing::error!(dir = %self.dir.display(), %error, "cannot read the free space of the job scratch filesystem; no job is claimed until it can")
                }
                (Ok(free), true) => {
                    tracing::warn!(dir = %self.dir.display(), free_bytes = free, reserve_bytes = self.bytes, "the job scratch filesystem is below its free-space reserve; no job is claimed until space is freed (docs/runbook.md, \"Temporary volumes\")")
                }
                (Ok(free), false) if self.low.is_some() => tracing::info!(
                    free_bytes = free,
                    "the job scratch filesystem has its free-space reserve again; claiming jobs"
                ),
                (Ok(_), false) => {}
            }
            self.low = Some(low);
        }
        !low
    }
}

/// Bytes available to unprivileged users on the filesystem holding `dir`.
pub fn free_bytes(dir: &Path) -> io::Result<u64> {
    let stat = rustix::fs::statvfs(dir)?;
    Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn leftovers_of_a_previous_run_are_removed_and_the_directory_stays() {
        let root = tempfile::tempdir().unwrap();
        let scratch = root.path().join("tmp");
        fs::create_dir(&scratch).unwrap();
        fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(scratch.join("file"), "x").unwrap();
        fs::create_dir_all(scratch.join("job-1/repo/.git")).unwrap();
        fs::write(scratch.join("job-1/repo/.git/HEAD"), "ref").unwrap();
        fs::write(scratch.join(".hidden"), "x").unwrap();

        let cleared = clear(&scratch).expect("listed");

        assert_eq!(cleared.removed, 3);
        assert!(cleared.failed.is_empty(), "{:?}", cleared.failed);
        assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0);
        let mode = fs::metadata(&scratch).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o700, "the directory keeps its mode");
    }

    #[test]
    fn a_link_is_removed_without_touching_its_target() {
        let root = tempfile::tempdir().unwrap();
        let scratch = root.path().join("tmp");
        let outside = root.path().join("outside");
        fs::create_dir(&scratch).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, scratch.join("link")).unwrap();

        let cleared = clear(&scratch).expect("listed");

        assert_eq!(cleared.removed, 1);
        assert!(scratch.join("link").symlink_metadata().is_err());
        assert!(
            outside.join("keep").exists(),
            "the target is never followed"
        );
    }

    #[test]
    fn an_entry_that_cannot_be_removed_is_reported_and_the_rest_removed() {
        if rustix::process::getuid().as_raw() == 0 {
            return; // root removes it anyway
        }
        let root = tempfile::tempdir().unwrap();
        let scratch = root.path().join("tmp");
        let locked = scratch.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("inner"), "x").unwrap();
        fs::write(scratch.join("loose"), "x").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();

        let cleared = clear(&scratch).expect("listed");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(cleared.removed, 1);
        assert_eq!(cleared.failed.len(), 1);
        assert_eq!(cleared.failed[0].0, locked);
        assert!(!scratch.join("loose").exists());
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        assert!(clear(&root.path().join("absent")).is_err());
    }

    #[test]
    fn the_reserve_follows_the_free_space_and_an_unreadable_filesystem_counts_as_full() {
        let root = tempfile::tempdir().unwrap();
        let free = free_bytes(root.path()).unwrap();
        assert!(free > 0);
        assert!(Reserve::new(root.path().to_owned(), 1).allows_claim());
        let mut short = Reserve::new(root.path().to_owned(), free.saturating_add(1 << 40));
        assert!(!short.allows_claim());
        assert!(!short.allows_claim(), "stays refused");
        assert!(!Reserve::new(root.path().join("absent"), 1).allows_claim());
    }
}
