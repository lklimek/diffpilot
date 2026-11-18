//! Disposable scratch directories on Docker volumes (`tmp-*`, ADR-013): they survive a container restart, so the
//! runner empties them at its start, before the first claim; and it takes no job while their filesystem is short
//! of free space ([`Reserve`]).

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

/// How often the warning is repeated while the reserve is missed.
const WARN_EVERY: Duration = Duration::from_secs(3600);
const MIB: u64 = 1024 * 1024;

/// Free space the scratch filesystem must keep for the runner to take a job: a start condition, not a quota.
#[derive(Debug)]
pub struct Reserve {
    dir: PathBuf,
    bytes: u64,
    /// Exists exactly while the reserve is missed; the container healthcheck reads it.
    marker: Option<PathBuf>,
    /// The last state seen (`true` = below the reserve).
    low: Option<bool>,
    last_warning: Option<Instant>,
}

/// One check of [`Reserve`].
#[derive(Debug)]
struct Observation {
    allows_claim: bool,
    /// The state differs from the previous check (or this is the first).
    changed: bool,
    /// The below-reserve warning is due (first time, or [`WARN_EVERY`] after the last one).
    warn: bool,
    /// The reserve is back after a check below it.
    recovered: bool,
}

impl Reserve {
    /// At least `bytes` free (for unprivileged users) on the filesystem holding `dir`.
    pub fn new(dir: PathBuf, bytes: u64) -> Self {
        Self {
            dir,
            bytes,
            marker: None,
            low: None,
            last_warning: None,
        }
    }

    /// Keeps `marker` present while the reserve is missed and absent otherwise.
    pub fn with_marker(mut self, marker: PathBuf) -> Self {
        self.marker = Some(marker);
        self
    }

    /// Whether a job may be taken now; a filesystem that cannot be inspected counts as full.
    pub fn allows_claim(&mut self) -> bool {
        let free = free_bytes(&self.dir);
        let seen = self.observe(&free, Instant::now());
        match (&free, seen.warn) {
            (Err(error), true) => {
                tracing::error!(dir = %self.dir.display(), %error, "cannot read the free space of the job scratch filesystem; no job is taken until it can")
            }
            (Ok(free), true) => {
                tracing::warn!(dir = %self.dir.display(), free_mib = free / MIB, required_mib = self.bytes / MIB, "the job scratch filesystem is below its free-space reserve; no job is taken until space is freed (docs/runbook.md, \"Temporary volumes\")")
            }
            (Ok(free), false) if seen.recovered => tracing::info!(
                free_mib = free / MIB,
                "the job scratch filesystem has its free-space reserve again; taking jobs"
            ),
            _ => {}
        }
        if seen.changed {
            self.update_marker(!seen.allows_claim);
        }
        seen.allows_claim
    }

    /// Records the state `free` gives at `now`.
    fn observe(&mut self, free: &io::Result<u64>, now: Instant) -> Observation {
        let low = free.as_ref().map_or(true, |free| *free < self.bytes);
        let changed = self.low != Some(low);
        let recovered = changed && !low && self.low.is_some();
        let warn = low
            && (changed
                || self
                    .last_warning
                    .is_none_or(|last| now.duration_since(last) >= WARN_EVERY));
        if warn {
            self.last_warning = Some(now);
        } else if !low {
            self.last_warning = None;
        }
        self.low = Some(low);
        Observation {
            allows_claim: !low,
            changed,
            warn,
            recovered,
        }
    }

    fn update_marker(&self, low: bool) {
        let Some(marker) = &self.marker else {
            return;
        };
        let result = if low {
            std::fs::write(marker, b"")
        } else {
            std::fs::remove_file(marker).or_else(|error| match error.kind() {
                io::ErrorKind::NotFound => Ok(()),
                _ => Err(error),
            })
        };
        if let Err(error) = result {
            tracing::error!(file = %marker.display(), %error, "cannot update the disk-reserve health marker; the container health may be wrong");
        }
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

    #[test]
    fn the_warning_repeats_hourly_while_below_the_reserve() {
        let mut reserve = Reserve::new(PathBuf::from("/unused"), 100);
        let start = Instant::now();
        assert!(
            !reserve.observe(&Ok(200), start).recovered,
            "the first check is no recovery"
        );
        let low = Ok(10);
        assert!(reserve.observe(&low, start).warn, "first time below");
        assert!(
            !reserve
                .observe(&low, start + Duration::from_secs(59 * 60))
                .warn
        );
        assert!(
            reserve
                .observe(&low, start + Duration::from_secs(60 * 60))
                .warn,
            "an hour later"
        );
        assert!(
            !reserve
                .observe(&low, start + Duration::from_secs(61 * 60))
                .warn
        );
        let back = reserve.observe(&Ok(200), start + Duration::from_secs(62 * 60));
        assert!(back.allows_claim && back.recovered && !back.warn);
        assert!(
            !reserve
                .observe(&Ok(200), start + Duration::from_secs(62 * 60))
                .recovered
        );
        let again = reserve.observe(&low, start + Duration::from_secs(63 * 60));
        assert!(
            !again.allows_claim && again.changed && again.warn,
            "warned at once on a new drop"
        );
        let unreadable = reserve.observe(
            &Err(io::Error::other("x")),
            start + Duration::from_secs(64 * 60),
        );
        assert!(!unreadable.allows_claim);
    }

    #[test]
    fn the_health_marker_exists_exactly_while_below_the_reserve() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("disk-below-reserve");
        let reserve =
            |bytes| Reserve::new(root.path().to_owned(), bytes).with_marker(marker.clone());
        let mut full = reserve(u64::MAX);
        assert!(!full.allows_claim());
        assert!(marker.exists(), "written when the reserve is missed");
        assert!(!full.allows_claim());
        assert!(marker.exists());
        // A restarted runner with enough space removes a leftover marker on its first check.
        let mut roomy = reserve(1);
        assert!(roomy.allows_claim());
        assert!(!marker.exists(), "removed when the reserve is back");
        assert!(roomy.allows_claim());
    }
}
