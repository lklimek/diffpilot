//! File operations on agent-owned per-job directories (ADR-013 helper modes `copy-in`, `copy-out`, `list`,
//! `clear`). Every path is resolved below an opened root with `openat2(RESOLVE_BENEATH | NO_SYMLINKS |
//! NO_MAGICLINKS | NO_XDEV)`, so a planted symlink, magic link or mount is never crossed. In production they
//! run as the agent uid (a link then reaches only what the agent could read anyway); in development the
//! runner calls them in-process.

use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd as _, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use rustix::fs::{
    AtFlags, Dir, FileType, Mode, OFlags, ResolveFlags, fstat, mkdirat, openat2, renameat, statat,
    unlinkat,
};
use serde::{Deserialize, Serialize};

/// Suffix of the temp file `copy_in` renames into place.
const TEMP_SUFFIX: &str = ".sr-new";
const STRICT: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

/// One entry below a listed root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Path relative to the root, `/`-separated.
    pub path: String,
    /// Whether it is a directory.
    pub dir: bool,
    /// Whether it is a regular file.
    pub file: bool,
    /// Size in bytes.
    pub size: u64,
}

/// Writes `input` to the private file `root/rel` (0600, parents created 0700) through a fresh temp file renamed
/// over whatever was there (a planted link is replaced, never followed).
pub fn copy_in(root: &Path, rel: &Path, input: &mut impl Read) -> io::Result<()> {
    let names = parts(rel)?;
    let Some((last, parents)) = names.split_last() else {
        return Err(invalid("empty path"));
    };
    let mut dir = open_root(root)?;
    for name in parents {
        match mkdirat(&dir, *name, Mode::from_raw_mode(0o700)) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(error.into()),
        }
        dir = openat2(
            &dir,
            *name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            STRICT,
        )?;
    }
    let mut temp = last.to_os_string();
    temp.push(TEMP_SUFFIX);
    match unlinkat(&dir, temp.as_os_str(), AtFlags::empty()) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => {}
        Err(error) => return Err(error.into()),
    }
    let file = openat2(
        &dir,
        temp.as_os_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
        STRICT,
    )?;
    let mut file = std::fs::File::from(file);
    io::copy(input, &mut file)?;
    file.flush()?;
    file.sync_all()?;
    Ok(renameat(&dir, temp.as_os_str(), &dir, *last)?)
}

/// Why `copy_out` refused; each refusal has its own helper exit code (ADR-013).
#[derive(Debug, thiserror::Error)]
pub enum CopyOutError {
    /// The path could not be opened within the root (link, mount, missing): exit code 2.
    #[error("copy-out: open refused: {}", errno_name(.0))]
    Open(#[source] io::Error),
    /// Not a regular file with one link: exit code 3.
    #[error("copy-out: not a regular file with one link")]
    NotRegular,
    /// Larger than the limit: exit code 4.
    #[error("copy-out: larger than the limit")]
    TooLarge,
    /// Anything else: exit code 1.
    #[error("copy-out: {0}")]
    Io(#[from] io::Error),
}

impl CopyOutError {
    /// The helper's exit code.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Open(_) => 2,
            Self::NotRegular => 3,
            Self::TooLarge => 4,
            Self::Io(_) => 1,
        }
    }
}

/// The contents of `root/rel`: a regular file with one link and at most `max` bytes, never through a link.
/// A file the agent made unreadable (its own, mode 000) is made owner-readable first.
pub fn copy_out(root: &Path, rel: &Path, max: u64) -> Result<Vec<u8>, CopyOutError> {
    let names = parts(rel).map_err(CopyOutError::Open)?;
    let dir = open_root(root)?;
    let joined: PathBuf = names.iter().collect();
    let open = || {
        openat2(
            &dir,
            &joined,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            STRICT,
        )
    };
    let file = match open() {
        Err(rustix::io::Errno::ACCESS) => {
            unlock(&dir, &joined, 0o600).map_err(CopyOutError::Open)?;
            open()
        }
        other => other,
    }
    .map_err(|error| CopyOutError::Open(error.into()))?;
    let stat = fstat(&file).map_err(io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_nlink != 1 {
        return Err(CopyOutError::NotRegular);
    }
    if u64::try_from(stat.st_size).unwrap_or(u64::MAX) > max {
        return Err(CopyOutError::TooLarge);
    }
    let mut bytes = Vec::new();
    std::fs::File::from(file)
        .take(max + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(CopyOutError::TooLarge);
    }
    Ok(bytes)
}

/// The symbolic name of an errno (as the prototype printed it), or its number.
fn errno_name(error: &io::Error) -> String {
    let name = match error.raw_os_error() {
        Some(2) => "ENOENT",
        Some(13) => "EACCES",
        Some(18) => "EXDEV",
        Some(20) => "ENOTDIR",
        Some(21) => "EISDIR",
        Some(40) => "ELOOP",
        Some(other) => return other.to_string(),
        None => return error.to_string(),
    };
    name.to_owned()
}

/// Gives `dir/rel` (a file or directory this uid owns, never a link) the owner mode `mode`: the owner may
/// always chmod its own entries, so an entry the agent locked (mode 000) can be read or removed again.
fn unlock(dir: &OwnedFd, rel: &Path, mode: u32) -> io::Result<()> {
    let target = openat2(
        dir,
        rel,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        STRICT,
    )?;
    chmod_fd(&target, mode)
}

/// chmod of the inode behind an `O_PATH` descriptor (which cannot `fchmod`) through its `/proc/self/fd` entry;
/// only files and directories.
fn chmod_fd(target: &OwnedFd, mode: u32) -> io::Result<()> {
    let kind = FileType::from_raw_mode(fstat(target)?.st_mode);
    if !matches!(kind, FileType::RegularFile | FileType::Directory) {
        return Err(invalid("not a file or directory"));
    }
    let by_fd = format!("/proc/self/fd/{}", target.as_raw_fd());
    Ok(rustix::fs::chmod(
        by_fd.as_str(),
        Mode::from_raw_mode(mode),
    )?)
}

/// Every entry below `root`, depth first, without following links.
pub fn list(root: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    walk(open_root(root)?, "", &mut out)?;
    Ok(out)
}

/// `"<dev> <ino> empty"` (or `nonempty`) of the directory `path`, opened without following links.
pub fn inspect(path: &Path) -> io::Result<String> {
    let dir = openat2(
        rustix::fs::CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )?;
    let stat = fstat(&dir)?;
    let empty = names(&dir)?.is_empty();
    Ok(format!(
        "{} {} {}",
        stat.st_dev,
        stat.st_ino,
        if empty { "empty" } else { "nonempty" }
    ))
}

/// Removes everything below `root`, without following links; `root` itself stays.
pub fn clear(root: &Path) -> io::Result<()> {
    clear_dir(&open_root(root)?)
}

fn walk(dir: OwnedFd, prefix: &str, out: &mut Vec<Entry>) -> io::Result<()> {
    for name in names(&dir)? {
        let stat = statat(&dir, name.as_path(), AtFlags::SYMLINK_NOFOLLOW)?;
        let kind = FileType::from_raw_mode(stat.st_mode);
        let path = format!("{prefix}{}", name.display());
        out.push(Entry {
            path: path.clone(),
            dir: kind == FileType::Directory,
            file: kind == FileType::RegularFile,
            size: u64::try_from(stat.st_size).unwrap_or(0),
        });
        if kind == FileType::Directory {
            let sub = openat2(
                &dir,
                name.as_path(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                STRICT,
            )?;
            walk(sub, &format!("{path}/"), out)?;
        }
    }
    Ok(())
}

fn clear_dir(dir: &OwnedFd) -> io::Result<()> {
    for name in names(dir)? {
        let stat = statat(dir, name.as_path(), AtFlags::SYMLINK_NOFOLLOW)?;
        if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
            // The agent may have locked it (mode 000); restore the owner's access before descending.
            unlock(dir, name.as_path(), 0o700)?;
            let sub = openat2(
                dir,
                name.as_path(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                STRICT,
            )?;
            clear_dir(&sub)?;
            unlinkat(dir, name.as_path(), AtFlags::REMOVEDIR)?;
        } else {
            unlinkat(dir, name.as_path(), AtFlags::empty())?;
        }
    }
    Ok(())
}

/// Names in `dir` other than `.` and `..`.
fn names(dir: &OwnedFd) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut entries = Dir::read_from(dir.as_fd())?;
    while let Some(entry) = entries.read() {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            found.push(PathBuf::from(OsStr::from_bytes(name)));
        }
    }
    Ok(found)
}

fn open_root(root: &Path) -> io::Result<OwnedFd> {
    if !root.is_absolute() {
        return Err(invalid("the root must be absolute"));
    }
    let open = || {
        rustix::fs::open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
    };
    match open() {
        Err(rustix::io::Errno::ACCESS) => {
            // A root the agent locked: restore the owner's access through a no-follow descriptor.
            let target = rustix::fs::open(
                root,
                OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
            chmod_fd(&target, 0o700)?;
            Ok(open()?)
        }
        other => Ok(other?),
    }
}

/// The normal components of a relative path; `..`, `.`, roots and empty names are refused.
fn parts(rel: &Path) -> io::Result<Vec<&OsStr>> {
    let mut names = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(name) => names.push(name),
            _ => return Err(invalid("not a plain relative path")),
        }
    }
    if names.is_empty() {
        return Err(invalid("empty path"));
    }
    Ok(names)
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn copy_in_creates_private_files_and_replaces_old_ones() {
        let root = TempDir::new().unwrap();
        copy_in(
            root.path(),
            Path::new("sessions/2026/a.jsonl"),
            &mut &b"x\n"[..],
        )
        .unwrap();
        let file = root.path().join("sessions/2026/a.jsonl");
        assert_eq!(std::fs::read(&file).unwrap(), b"x\n");
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(root.path().join("sessions"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        copy_in(
            root.path(),
            Path::new("sessions/2026/a.jsonl"),
            &mut &b"y"[..],
        )
        .unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"y");
    }

    #[test]
    fn copy_in_never_follows_a_planted_link() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), root.path().join("sessions")).unwrap();
        assert!(copy_in(root.path(), Path::new("sessions/a"), &mut &b"x"[..]).is_err());
        symlink(
            outside.path().join("target"),
            root.path().join("config.toml"),
        )
        .unwrap();
        copy_in(root.path(), Path::new("config.toml"), &mut &b"x"[..]).unwrap();
        assert!(
            !outside.path().join("target").exists(),
            "the link is replaced, not followed"
        );
        assert!(
            root.path()
                .join("config.toml")
                .symlink_metadata()
                .unwrap()
                .is_file()
        );
        for bad in ["../x", "/etc/passwd", "", "a/../../x"] {
            assert!(
                copy_in(root.path(), Path::new(bad), &mut &b"x"[..]).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn copy_back_as_agent_cannot_reach_runner_files() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("master-auth.json"), "secret").unwrap();
        symlink(
            outside.path().join("master-auth.json"),
            root.path().join("auth.json"),
        )
        .unwrap();
        assert!(
            copy_out(root.path(), Path::new("auth.json"), 1024).is_err(),
            "symlink"
        );
        symlink(outside.path(), root.path().join("dir")).unwrap();
        assert!(
            copy_out(root.path(), Path::new("dir/master-auth.json"), 1024).is_err(),
            "symlinked dir"
        );
        std::fs::write(root.path().join("one"), "x").unwrap();
        std::fs::hard_link(root.path().join("one"), root.path().join("two")).unwrap();
        assert!(
            copy_out(root.path(), Path::new("two"), 1024).is_err(),
            "hard link"
        );
        assert!(
            copy_out(root.path(), Path::new("../x"), 1024).is_err(),
            "escape"
        );
    }

    #[test]
    fn copy_out_reads_a_regular_file_within_the_cap() {
        let root = TempDir::new().unwrap();
        std::fs::write(root.path().join("auth.json"), "{}").unwrap();
        assert_eq!(
            copy_out(root.path(), Path::new("auth.json"), 2).unwrap(),
            b"{}"
        );
        assert!(matches!(
            copy_out(root.path(), Path::new("auth.json"), 1),
            Err(CopyOutError::TooLarge)
        ));
        std::fs::set_permissions(
            root.path().join("auth.json"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        assert_eq!(
            copy_out(root.path(), Path::new("auth.json"), 2).unwrap(),
            b"{}",
            "a file the agent locked is unlocked by its owner"
        );
        let status = std::process::Command::new("mkfifo")
            .arg(root.path().join("fifo"))
            .status()
            .unwrap();
        assert!(status.success());
        assert!(
            copy_out(root.path(), Path::new("fifo"), 10).is_err(),
            "a FIFO never blocks"
        );
    }

    #[test]
    fn list_and_clear_do_not_follow_links() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("keep"), "x").unwrap();
        std::fs::create_dir_all(root.path().join("sessions/2026")).unwrap();
        std::fs::write(root.path().join("sessions/2026/r.jsonl"), "abc").unwrap();
        symlink(outside.path(), root.path().join("link")).unwrap();
        let entries = list(root.path()).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(
            paths.contains(&"sessions/2026/r.jsonl") && paths.contains(&"link"),
            "{paths:?}"
        );
        assert!(!paths.iter().any(|p| p.starts_with("link/")));
        let file = entries
            .iter()
            .find(|e| e.path == "sessions/2026/r.jsonl")
            .unwrap();
        assert!(file.file && file.size == 3);
        clear(root.path()).unwrap();
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        assert!(
            outside.path().join("keep").exists(),
            "the link target survives"
        );
    }

    #[test]
    fn clear_restores_owner_access_to_locked_directories() {
        let root = TempDir::new().unwrap();
        let locked = root.path().join("locked");
        std::fs::create_dir_all(locked.join("deeper")).unwrap();
        std::fs::write(locked.join("deeper/secret"), "left over").unwrap();
        std::fs::set_permissions(
            locked.join("deeper"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        clear(root.path()).unwrap();
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
        clear(root.path()).expect("a locked root is unlocked too");
    }
}
