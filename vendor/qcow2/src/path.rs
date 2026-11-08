//! Host backing-chain resolution, with stable ownership and finite depth.
use crate::{check_signature, EncryptionKey, Error, Qcow2, Result};
use std::{collections::HashSet, fs::File, path::Path, sync::Arc};

pub(crate) fn open(path: &Path, key: Option<EncryptionKey>) -> Result<Qcow2<File>> {
    open_chain(path, key.as_ref(), &mut HashSet::new())
}

fn open_chain(
    path: &Path,
    key: Option<&EncryptionKey>,
    seen: &mut HashSet<std::path::PathBuf>,
) -> Result<Qcow2<File>> {
    let canonical = path.canonicalize()?;
    if seen.len() >= 32 || !seen.insert(canonical.clone()) {
        return Err(Error::FileFormat(
            "backing chain cycle or depth exceeds 32".into(),
        ));
    }
    let file = File::open(&canonical)?;
    let mut image = match key {
        Some(key) => Qcow2::open_with_key(file, key.clone())?,
        None => Qcow2::open(file)?,
    };
    if image.header.encryption != 0 && image.key.is_none() {
        return Err(Error::KeyRequired);
    }
    if let Some(name) = image.header.backing_name.as_ref() {
        let parent_path = if name.is_absolute() {
            name.clone()
        } else {
            path.parent()
                .ok_or_else(|| Error::FileFormat("image has no parent directory".into()))?
                .join(name)
        };
        let parent_file = File::open(&parent_path).map_err(|error| Error::BackingFile {
            path: parent_path.clone(),
            error,
        })?;
        let is_qcow = match image.header.backing_format.as_deref() {
            Some("raw") => false,
            Some("qcow" | "qcow2") => true,
            Some(format) => {
                return Err(Error::UnsupportedFeature(format!(
                    "backing format {format}"
                )))
            }
            None => check_signature(&parent_file)?,
        };
        if is_qcow {
            drop(parent_file);
            image.set_backing(Arc::new(open_chain(&parent_path, key, seen)?))?;
        } else {
            let canonical_parent = parent_path.canonicalize()?;
            if seen.len() >= 32 || !seen.insert(canonical_parent) {
                return Err(Error::FileFormat(
                    "backing chain cycle or depth exceeds 32".into(),
                ));
            }
            image.set_backing(Arc::new(parent_file))?;
        }
    }
    Ok(image)
}
