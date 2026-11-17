//! Collects bounded flow files without following agent-controlled links.

use crate::provider::ProviderError;
use contracts::job::spec::FlowFile;
use std::path::Path;

pub(crate) const MAX_BYTES: usize = 1024 * 1024;

/// Prepares only trusted fixed paths before the agent owns the output directory.
pub(crate) fn prepare(job: &Path, spec: &FlowFile) -> Result<(), std::io::Error> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(job.join("out"))?;
    std::fs::write(
        job.join("flow-context.json"),
        serde_json::to_vec(&spec.context)?,
    )?;
    std::fs::set_permissions(
        job.join("flow-context.json"),
        std::fs::Permissions::from_mode(0o444),
    )?;
    if let Some(previous) = &spec.previous {
        if previous.len() > MAX_BYTES {
            return Err(std::io::Error::other("repair file exceeds 1 MiB"));
        }
        std::fs::write(job.join("out/flow.md"), previous)?;
    }
    Ok(())
}

pub(crate) fn refused() -> ProviderError {
    ProviderError::Failed { message: "out/flow.md is missing, unsafe, invalid UTF-8, or exceeds 1 MiB; write a regular UTF-8 flow file and run sr-check".into() }
}

pub(crate) fn collect(job: &Path) -> Result<String, ProviderError> {
    let bytes = crate::supervisor::fsops::copy_out(job, Path::new("out/flow.md"), MAX_BYTES as u64)
        .map_err(|_| refused())?;
    String::from_utf8(bytes).map_err(|_| refused())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collects_file_and_rejects_missing_oversized_and_symlink_output() {
        let root = tempfile::tempdir().unwrap();
        assert!(collect(root.path()).is_err());
        std::fs::create_dir(root.path().join("out")).unwrap();
        std::fs::write(root.path().join("out/flow.md"), "# flow f1\n").unwrap();
        assert_eq!(collect(root.path()).unwrap(), "# flow f1\n");
        std::fs::write(root.path().join("out/flow.md"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert!(
            collect(root.path())
                .unwrap_err()
                .to_string()
                .contains("1 MiB")
        );
        std::fs::remove_file(root.path().join("out/flow.md")).unwrap();
        std::os::unix::fs::symlink("../other", root.path().join("out/flow.md")).unwrap();
        assert!(collect(root.path()).is_err());
    }
}

#[cfg(test)]
mod prepare_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn repair_restores_document_and_context_is_readable_but_not_agent_writable() {
        let root = tempfile::tempdir().unwrap();
        let spec = FlowFile {
            context: serde_json::json!({"version":1}),
            previous: Some("# flow f1\n".into()),
        };
        prepare(root.path(), &spec).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("out/flow.md")).unwrap(),
            spec.previous.unwrap()
        );
        assert_eq!(
            std::fs::metadata(root.path().join("flow-context.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o444
        );
        assert_eq!(
            std::fs::metadata(root.path().join("out"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
