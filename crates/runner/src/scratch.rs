//! Disposable scratch directories on Docker volumes (`tmp-*`, ADR-013): they survive a container restart, so the
//! runner empties them at its start, before the first claim; and it takes no job while their filesystem is short
//! of free space ([`Reserve`]).

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

    use super::*;

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
