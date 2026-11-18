//! Disposable scratch directories on Docker volumes (`tmp-*`, ADR-013): they survive a container restart, so the
//! runner empties them at its start, before the first claim.

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
}
