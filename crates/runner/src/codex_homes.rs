//! Start-up check of `<state>/codex-homes` under the supervisor (ADR-013, "codex-homes must be persistent and
//! bounded"): its own ext4 filesystem (or, for the local stack only, a tmpfs) of bounded size and inode count, so
//! an agent cannot fill the state volume or the host disk. The image's preflight checks the same; this check
//! also holds when the container's command is changed.

use std::ffi::OsStr;
use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use rustix::fs::StatVfsMountFlags;

/// Largest accepted capacity of the filesystem, in bytes.
pub const MAX_BYTES: &str = "SR_CODEX_HOMES_MAX_BYTES";
/// Largest accepted inode count of the filesystem.
pub const MAX_INODES: &str = "SR_CODEX_HOMES_MAX_INODES";
/// `1` accepts a size-capped tmpfs (local test stack only: a tmpfs loses a rotated credential on a crash, ADR-003),
/// and only with [`TMPFS_PUBLIC_URL`] on localhost.
pub const ALLOW_TMPFS: &str = "SR_CODEX_HOMES_ALLOW_TMPFS";
/// The server's public URL, given with [`ALLOW_TMPFS`]: a tmpfs is accepted only for a stack on localhost.
pub const TMPFS_PUBLIC_URL: &str = "SR_CODEX_HOMES_TMPFS_PUBLIC_URL";
/// Exit code of a runner that refuses to start because of this check (`EX_CONFIG`, as the image's preflight).
pub const EXIT_CODE: u8 = 78;

const DEFAULT_MAX_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const DEFAULT_MAX_INODES: u64 = 1024 * 1024;
/// `EXT4_SUPER_MAGIC`, shared by ext2, ext3 and ext4.
const EXT4_MAGIC: u64 = 0xEF53;
/// `TMPFS_MAGIC`.
const TMPFS_MAGIC: u64 = 0x0102_1994;
/// Directory `mkfs.ext4` creates in the root of the filesystem; root-owned, never a job home.
const LOST_FOUND: &str = "lost+found";

/// The accepted bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest capacity in bytes.
    pub max_bytes: u64,
    /// Largest inode count.
    pub max_inodes: u64,
    /// Whether a tmpfs is accepted ([`ALLOW_TMPFS`] with a localhost [`TMPFS_PUBLIC_URL`]).
    pub allow_tmpfs: bool,
}

/// What the check needs to know about the directory's filesystem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    /// Device of the directory.
    pub dev: u64,
    /// Device of the state directory holding it.
    pub state_dev: u64,
    /// Filesystem magic (`statfs` `f_type`).
    pub magic: u64,
    /// Capacity in bytes (`f_blocks * f_frsize`).
    pub bytes: u64,
    /// Inode count (`f_files`).
    pub inodes: u64,
    /// Mounted `nosuid`, `nodev` and `noexec`.
    pub restricted: bool,
}

/// The directory is not a bounded filesystem of its own; the runner does not start.
#[derive(Debug, thiserror::Error)]
pub enum CodexHomesError {
    /// A limit variable is not a positive whole number.
    #[error("environment variable {0} must be a positive whole number")]
    InvalidLimit(&'static str),
    /// The directory or its filesystem cannot be inspected.
    #[error("cannot inspect {}", path.display())]
    Probe {
        /// The directory.
        path: PathBuf,
        /// The failure.
        #[source]
        source: io::Error,
    },
    /// The directory is on the state volume's filesystem.
    #[error("{} is on the state volume's filesystem; mount the bounded codex-homes filesystem there (docs/runbook.md, \"Bounded codex-homes\")", path.display())]
    NotSeparate {
        /// The directory.
        path: PathBuf,
    },
    /// The directory is a tmpfs and [`ALLOW_TMPFS`] is not set.
    #[error("{} is a tmpfs, which loses a rotated credential on a crash; production needs the bounded ext4 filesystem ({ALLOW_TMPFS}=1 with {TMPFS_PUBLIC_URL} on localhost is for the local stack only)", path.display())]
    Tmpfs {
        /// The directory.
        path: PathBuf,
    },
    /// The filesystem is neither ext4 nor an allowed tmpfs.
    #[error("{} is not the bounded ext4 filesystem (filesystem type {magic:#x})", path.display())]
    UnsupportedFilesystem {
        /// The directory.
        path: PathBuf,
        /// Its filesystem magic.
        magic: u64,
    },
    /// The filesystem is not mounted `nosuid,nodev,noexec`.
    #[error("{} must be mounted nosuid,nodev,noexec", path.display())]
    MountOptions {
        /// The directory.
        path: PathBuf,
    },
    /// The filesystem reports no finite capacity or inode count.
    #[error("{} has no finite capacity or inode count", path.display())]
    Unbounded {
        /// The directory.
        path: PathBuf,
    },
    /// The filesystem is larger than [`MAX_BYTES`].
    #[error("{} holds {bytes} bytes, more than {MAX_BYTES}={max}", path.display())]
    TooLarge {
        /// The directory.
        path: PathBuf,
        /// Its capacity.
        bytes: u64,
        /// The limit.
        max: u64,
    },
    /// The filesystem has more inodes than [`MAX_INODES`].
    #[error("{} has {inodes} inodes, more than {MAX_INODES}={max}", path.display())]
    TooManyInodes {
        /// The directory.
        path: PathBuf,
        /// Its inode count.
        inodes: u64,
        /// The limit.
        max: u64,
    },
}

impl Limits {
    /// Reads the limits from the process environment.
    pub fn from_env() -> Result<Self, CodexHomesError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Reads the limits through `lookup`; values are trimmed and empty counts as unset.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, CodexHomesError> {
        let get = |key: &str| {
            lookup(key)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let limit = |key: &'static str, default: u64| match get(key) {
            Some(value) => value
                .parse::<u64>()
                .ok()
                .filter(|limit| *limit > 0)
                .ok_or(CodexHomesError::InvalidLimit(key)),
            None => Ok(default),
        };
        Ok(Self {
            max_bytes: limit(MAX_BYTES, DEFAULT_MAX_BYTES)?,
            max_inodes: limit(MAX_INODES, DEFAULT_MAX_INODES)?,
            allow_tmpfs: get(ALLOW_TMPFS).as_deref() == Some("1")
                && get(TMPFS_PUBLIC_URL).is_some_and(|url| on_localhost(&url)),
        })
    }
}

/// Checks that `dir` (below `state_dir`) is a bounded filesystem of its own.
pub fn verify(dir: &Path, state_dir: &Path, limits: &Limits) -> Result<Facts, CodexHomesError> {
    let facts = probe(dir, state_dir).map_err(|source| CodexHomesError::Probe {
        path: dir.to_path_buf(),
        source,
    })?;
    check(dir, &facts, limits)?;
    Ok(facts)
}

/// Reads the facts of `dir` and its state directory.
fn probe(dir: &Path, state_dir: &Path) -> io::Result<Facts> {
    let statfs = rustix::fs::statfs(dir)?;
    let statvfs = rustix::fs::statvfs(dir)?;
    Ok(Facts {
        dev: std::fs::metadata(dir)?.dev(),
        state_dev: std::fs::metadata(state_dir)?.dev(),
        magic: u64::try_from(statfs.f_type).unwrap_or(u64::MAX),
        bytes: statvfs.f_blocks.saturating_mul(statvfs.f_frsize),
        inodes: statvfs.f_files,
        restricted: statvfs.f_flag.contains(
            StatVfsMountFlags::NOSUID | StatVfsMountFlags::NODEV | StatVfsMountFlags::NOEXEC,
        ),
    })
}

/// `http://localhost`, `http://127.0.0.1` or `http://[::1]`, with an optional port and trailing slash.
fn on_localhost(url: &str) -> bool {
    let Some(authority) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !authority.ends_with(']') => (host, Some(port)),
        _ => (authority, None),
    };
    ["localhost", "127.0.0.1", "[::1]"].contains(&host)
        && port.is_none_or(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
}

/// The decision on the probed facts.
fn check(dir: &Path, facts: &Facts, limits: &Limits) -> Result<(), CodexHomesError> {
    let path = || dir.to_path_buf();
    if facts.dev == facts.state_dev {
        return Err(CodexHomesError::NotSeparate { path: path() });
    }
    match facts.magic {
        EXT4_MAGIC => {}
        TMPFS_MAGIC if limits.allow_tmpfs => {}
        TMPFS_MAGIC => return Err(CodexHomesError::Tmpfs { path: path() }),
        magic => {
            return Err(CodexHomesError::UnsupportedFilesystem {
                path: path(),
                magic,
            });
        }
    }
    if !facts.restricted {
        return Err(CodexHomesError::MountOptions { path: path() });
    }
    if facts.bytes == 0 || facts.inodes == 0 {
        return Err(CodexHomesError::Unbounded { path: path() });
    }
    if facts.bytes > limits.max_bytes {
        return Err(CodexHomesError::TooLarge {
            path: path(),
            bytes: facts.bytes,
            max: limits.max_bytes,
        });
    }
    if facts.inodes > limits.max_inodes {
        return Err(CodexHomesError::TooManyInodes {
            path: path(),
            inodes: facts.inodes,
            max: limits.max_inodes,
        });
    }
    Ok(())
}

/// Whether `path` is the root-owned `lost+found` directory of the filesystem: never a job home, never removed.
pub fn is_lost_found(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| {
        reserved(
            path.file_name().unwrap_or_default(),
            metadata.is_dir(),
            metadata.uid(),
        )
    })
}

fn reserved(name: &OsStr, is_dir: bool, uid: u32) -> bool {
    name == LOST_FOUND && is_dir && uid == 0
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn limits() -> Limits {
        Limits {
            max_bytes: 16 * GIB,
            max_inodes: 1024 * 1024,
            allow_tmpfs: false,
        }
    }

    fn ext4(bytes: u64, inodes: u64) -> Facts {
        Facts {
            dev: 2049,
            state_dev: 64,
            magic: EXT4_MAGIC,
            bytes,
            inodes,
            restricted: true,
        }
    }

    fn checked(facts: Facts, limits: Limits) -> Result<(), CodexHomesError> {
        check(Path::new("/state/codex-homes"), &facts, &limits)
    }

    #[test]
    fn own_bounded_ext4_filesystem_is_accepted() {
        checked(ext4(2 * GIB, 65_536), limits()).expect("accepted");
        checked(ext4(16 * GIB, 1024 * 1024), limits()).expect("at the limits");
    }

    #[test]
    fn directory_on_the_state_filesystem_is_refused() {
        let mut facts = ext4(2 * GIB, 65_536);
        facts.dev = facts.state_dev;
        assert!(matches!(
            checked(facts, limits()),
            Err(CodexHomesError::NotSeparate { .. })
        ));
    }

    #[test]
    fn tmpfs_is_refused_unless_allowed_and_then_still_size_capped() {
        let tmpfs = Facts {
            magic: TMPFS_MAGIC,
            ..ext4(64 * 1024 * 1024, 2048)
        };
        assert!(matches!(
            checked(tmpfs, limits()),
            Err(CodexHomesError::Tmpfs { .. })
        ));
        let allowed = Limits {
            allow_tmpfs: true,
            ..limits()
        };
        checked(tmpfs, allowed).expect("local stack");
        let uncapped = Facts {
            bytes: 32 * GIB,
            ..tmpfs
        };
        assert!(matches!(
            checked(uncapped, allowed),
            Err(CodexHomesError::TooLarge { .. })
        ));
    }

    #[test]
    fn other_filesystems_are_refused() {
        let overlay = Facts {
            magic: 0x794c_7630,
            ..ext4(2 * GIB, 65_536)
        };
        let allowed = Limits {
            allow_tmpfs: true,
            ..limits()
        };
        let error = checked(overlay, allowed).expect_err("refused");
        assert!(
            matches!(error, CodexHomesError::UnsupportedFilesystem { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("0x794c7630"), "{error}");
    }

    #[test]
    fn filesystem_without_nosuid_nodev_noexec_is_refused() {
        let open = Facts {
            restricted: false,
            ..ext4(2 * GIB, 65_536)
        };
        let error = checked(open, limits()).expect_err("refused");
        assert!(error.to_string().contains("noexec"), "{error}");
    }

    #[test]
    fn unbounded_oversized_or_inode_rich_filesystems_are_refused() {
        assert!(matches!(
            checked(ext4(0, 65_536), limits()),
            Err(CodexHomesError::Unbounded { .. })
        ));
        assert!(matches!(
            checked(ext4(2 * GIB, 0), limits()),
            Err(CodexHomesError::Unbounded { .. })
        ));
        let error = checked(ext4(16 * GIB + 1, 65_536), limits()).expect_err("too large");
        assert!(
            error.to_string().contains("SR_CODEX_HOMES_MAX_BYTES"),
            "{error}"
        );
        assert!(matches!(
            checked(ext4(2 * GIB, 1024 * 1024 + 1), limits()),
            Err(CodexHomesError::TooManyInodes { .. })
        ));
    }

    #[test]
    fn limits_have_defaults_and_name_an_invalid_variable() {
        let parse = |pairs: &[(&str, &str)]| {
            let map: HashMap<String, String> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            Limits::from_lookup(|key| map.get(key).cloned())
        };
        assert_eq!(parse(&[]).unwrap(), limits());
        assert_eq!(
            parse(&[
                (MAX_BYTES, " 2147483648 "),
                (MAX_INODES, "65536"),
                (ALLOW_TMPFS, "1"),
                (TMPFS_PUBLIC_URL, "http://localhost:3000"),
            ])
            .unwrap(),
            Limits {
                max_bytes: 2 * GIB,
                max_inodes: 65_536,
                allow_tmpfs: true,
            }
        );
        for url in [
            "http://localhost:3000",
            "http://127.0.0.1/",
            "http://[::1]:8080",
        ] {
            assert!(
                parse(&[(ALLOW_TMPFS, "1"), (TMPFS_PUBLIC_URL, url)])
                    .unwrap()
                    .allow_tmpfs,
                "{url}"
            );
        }
        for pairs in [
            &[
                (ALLOW_TMPFS, "true"),
                (TMPFS_PUBLIC_URL, "http://localhost"),
            ][..],
            &[(ALLOW_TMPFS, "1")],
            &[
                (ALLOW_TMPFS, "1"),
                (TMPFS_PUBLIC_URL, "https://review.example.com"),
            ],
            &[
                (ALLOW_TMPFS, "1"),
                (TMPFS_PUBLIC_URL, "http://localhost.example.com"),
            ],
            &[
                (ALLOW_TMPFS, "1"),
                (TMPFS_PUBLIC_URL, "http://localhost:3000/x"),
            ],
        ] {
            assert!(!parse(pairs).unwrap().allow_tmpfs, "{pairs:?}");
        }
        for (key, value) in [(MAX_BYTES, "0"), (MAX_BYTES, "16G"), (MAX_INODES, "-1")] {
            let error = parse(&[(key, value)]).expect_err("invalid");
            assert_eq!(
                error.to_string(),
                format!("environment variable {key} must be a positive whole number")
            );
        }
    }

    #[test]
    fn plain_directory_of_the_state_volume_is_refused() {
        let state = tempfile::tempdir().unwrap();
        let homes = state.path().join("codex-homes");
        std::fs::create_dir(&homes).unwrap();
        let limits = Limits {
            allow_tmpfs: true,
            ..limits()
        };
        let error = verify(&homes, state.path(), &limits).expect_err("not separate");
        assert!(
            matches!(error, CodexHomesError::NotSeparate { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn missing_directory_cannot_be_inspected() {
        let state = tempfile::tempdir().unwrap();
        let error =
            verify(&state.path().join("absent"), state.path(), &limits()).expect_err("missing");
        assert!(matches!(error, CodexHomesError::Probe { .. }), "{error:?}");
    }

    #[test]
    fn only_a_root_owned_lost_found_directory_is_reserved() {
        let name = OsStr::new(LOST_FOUND);
        assert!(reserved(name, true, 0));
        assert!(!reserved(name, false, 0), "a file or link of that name");
        assert!(!reserved(name, true, 10_002), "not made by mkfs");
        assert!(!reserved(OsStr::new("job-1"), true, 0));
        let dir = tempfile::tempdir().unwrap();
        let imitation = dir.path().join(LOST_FOUND);
        std::fs::create_dir(&imitation).unwrap();
        let me = rustix::process::getuid().as_raw();
        assert_eq!(is_lost_found(&imitation), me == 0);
    }
}
