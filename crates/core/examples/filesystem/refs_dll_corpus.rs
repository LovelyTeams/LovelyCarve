//! Legacy DLL oracle for complete corpus comparisons during the Rust port.
use anyhow::{ensure, Context, Result};
use lovelycarve_core::filesystem::tree::fs_tree::NodeKind;
use lovelycarve_core::filesystem::volumes::refs_dll_oracle::RefsVolume;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) fn probe(volume: &RefsVolume) -> Result<Value> {
    let mut pending = vec![String::new()];
    let mut visited = std::collections::HashSet::new();
    let mut entries = Vec::new();
    let mut failures = Vec::new();
    while let Some(path) = pending.pop() {
        ensure!(visited.insert(path.clone()), "directory cycle at {path}");
        let children = match volume
            .children(&path)
            .with_context(|| format!("list {path}"))
        {
            Ok(children) => children,
            Err(error) => {
                failures.push(path.clone());
                entries.push(json!({"path": path, "listing_error": format!("{error:#}")}));
                continue;
            }
        };
        for entry in children {
            let path = format!("{path}/{}", entry.name);
            let mut observation =
                json!({"path": path, "directory": entry.kind == NodeKind::Dir, "size": entry.size});
            if entry.kind == NodeKind::Dir {
                pending.push(path);
            } else {
                let result = (|| -> Result<String> {
                    let mut digest = Sha256::new();
                    let mut total = 0;
                    let count = volume.stream_file(&path, 1024 * 1024, |bytes| {
                        digest.update(bytes);
                        total += bytes.len() as u64;
                        Ok(())
                    })?;
                    ensure!(
                        count == entry.size && total == entry.size,
                        "short DLL file read: {total} of {}",
                        entry.size
                    );
                    Ok(hex::encode(digest.finalize()))
                })();
                match result {
                    Ok(hash) => observation["sha256"] = json!(hash),
                    Err(error) => {
                        observation["error"] = json!(format!("{error:#}"));
                        failures.push(path.clone());
                    }
                }
                let result = (|| -> Result<Vec<Value>> {
                    let mut output = Vec::new();
                    for stream in volume.list_streams(&path)? {
                        let mut total = 0;
                        let mut digest = Sha256::new();
                        let count = volume.stream_named_stream(
                            &path,
                            &stream.name,
                            1024 * 1024,
                            |bytes| {
                                total += bytes.len() as u64;
                                digest.update(bytes);
                                Ok(())
                            },
                        )?;
                        ensure!(
                            count == stream.size && total == stream.size,
                            "short DLL named stream read"
                        );
                        output.push(json!({"name": stream.name, "size": stream.size, "sha256": hex::encode(digest.finalize())}));
                    }
                    Ok(output)
                })();
                match result {
                    Ok(streams) => observation["streams"] = json!(streams),
                    Err(error) => {
                        observation["stream_error"] = json!(format!("{error:#}"));
                        failures.push(format!("{path}:streams"));
                    }
                }
            }
            entries.push(observation);
        }
    }
    Ok(json!({"directories": visited.len(), "entries": entries,
        "failed_files": failures.len(), "failures": failures}))
}
