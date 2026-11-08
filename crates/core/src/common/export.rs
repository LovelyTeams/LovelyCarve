//! Publish host exports only after a complete write, without replacing files.

use std::{fs::File, io::Write, path::Path};

/// Stages an export beside its destination. Failed writes and failed publication
/// remove the staging file; publication refuses even a concurrently created target.
pub fn write_new<T>(
    destination: &Path,
    write: impl FnOnce(&mut File) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    let result = write(file.as_file_mut())?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist_noclobber(destination)
        .map_err(|error| error.error)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_stream_leaves_no_output_and_can_be_retried() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("export.bin");
        let result: anyhow::Result<()> = write_new(&destination, |file| {
            file.write_all(b"partial")?;
            anyhow::bail!("source failed mid-stream")
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        write_new(&destination, |file| Ok(file.write_all(b"complete")?)).unwrap();
        assert_eq!(std::fs::read(destination).unwrap(), b"complete");
    }

    #[test]
    fn publication_never_clobbers_a_concurrent_destination() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("export.bin");
        let result = write_new(&destination, |file| {
            file.write_all(b"export")?;
            std::fs::write(&destination, b"other writer")?;
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"other writer");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
