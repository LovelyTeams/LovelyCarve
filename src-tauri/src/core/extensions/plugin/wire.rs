//! Frozen filesystem wire types of the `fs.*` methods (unchanged since the
//! first LPP revision; LPP v2 carries them as they were).
//!
//! These DTOs deliberately do not reuse the host's evolving `FsNode` and
//! `FsStat` structs. New host-only fields therefore cannot accidentally become
//! required plugin ABI, while `attributes` gives plugins a namespaced extension
//! point that older hosts can safely ignore.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use rmpv::Value;
use serde::{Deserialize, Serialize};

use crate::core::filesystem::tree::fs_tree::{FsNode, FsStat, Mactb, NodeKind};

const MAX_NODE_ID_BYTES: usize = 4 * 1024;
const MAX_NODE_NAME_BYTES: usize = 32 * 1024;
const MAX_LINK_TARGET_BYTES: usize = 64 * 1024;
const MAX_TIMESTAMP_BYTES: usize = 128;
const MAX_MIME_BYTES: usize = 256;
const MAX_ATTRIBUTES: usize = 64;

#[derive(Debug, Default, Deserialize, Serialize)]
pub(super) struct LppTimestampsV1 {
    #[serde(default)]
    modified: Option<String>,
    #[serde(default)]
    accessed: Option<String>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    mft_changed: Option<String>,
    #[serde(default)]
    birth: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct LppFsNodeV1 {
    id: String,
    name: String,
    #[serde(default)]
    parent_id: Option<String>,
    kind: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    inode: Option<u64>,
    #[serde(default)]
    timestamps: LppTimestampsV1,
    #[serde(default)]
    link_target: Option<String>,
    #[serde(default)]
    attributes: HashMap<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct LppFsStatV1 {
    #[serde(default)]
    node_id: Option<String>,
    size: u64,
    #[serde(default)]
    inode: Option<u64>,
    #[serde(default)]
    mode: Option<u16>,
    #[serde(default = "default_mime")]
    mime: String,
    #[serde(default)]
    magic_hex: Option<String>,
    #[serde(default)]
    entropy: Option<f64>,
    #[serde(default)]
    attributes: HashMap<String, Value>,
}

fn default_mime() -> String {
    "application/octet-stream".to_owned()
}

impl LppFsNodeV1 {
    pub(super) fn into_host(self, host_parent_id: &str) -> Result<FsNode> {
        validate_text("node id", &self.id, 1, MAX_NODE_ID_BYTES)?;
        validate_text("node name", &self.name, 0, MAX_NODE_NAME_BYTES)?;
        if let Some(parent_id) = &self.parent_id {
            validate_text("parent node id", parent_id, 1, MAX_NODE_ID_BYTES)?;
        }
        if let Some(link_target) = &self.link_target {
            validate_text("link target", link_target, 0, MAX_LINK_TARGET_BYTES)?;
        }
        validate_timestamps(&self.timestamps)?;
        validate_attributes(&self.attributes)?;
        Ok(FsNode {
            id: self.id,
            name: self.name,
            parent_id: Some(host_parent_id.to_owned()),
            kind: match self.kind.as_str() {
                "dir" => NodeKind::Dir,
                "file" => NodeKind::File,
                "symlink" => NodeKind::Symlink,
                "unallocated" => NodeKind::Unallocated,
                kind => return Err(anyhow!("unsupported node kind {kind:?}")),
            },
            size: self.size,
            deleted: self.deleted,
            inode: self.inode,
            timestamps: Mactb {
                modified: self.timestamps.modified,
                accessed: self.timestamps.accessed,
                created: self.timestamps.created,
                mft_changed: self.timestamps.mft_changed,
                birth: self.timestamps.birth,
            },
            link_target: self.link_target,
            has_snapshot: false,
        })
    }
}

impl LppFsStatV1 {
    pub(super) fn into_host(self, host_node_id: &str) -> Result<FsStat> {
        if let Some(node_id) = &self.node_id {
            validate_text("stat node id", node_id, 1, MAX_NODE_ID_BYTES)?;
        }
        validate_text("MIME type", &self.mime, 1, MAX_MIME_BYTES)?;
        validate_attributes(&self.attributes)?;
        if self.entropy.is_some_and(|entropy| !entropy.is_finite()) {
            return Err(anyhow!("stat entropy must be finite"));
        }
        Ok(FsStat {
            node_id: host_node_id.to_owned(),
            size: self.size,
            inode: self.inode,
            mode: self.mode,
            mime: self.mime,
            magic_hex: self.magic_hex,
            entropy: self.entropy,
            data_runs: Vec::new(),
        })
    }
}

fn validate_timestamps(timestamps: &LppTimestampsV1) -> Result<()> {
    for (name, value) in [
        ("modified", &timestamps.modified),
        ("accessed", &timestamps.accessed),
        ("created", &timestamps.created),
        ("mft_changed", &timestamps.mft_changed),
        ("birth", &timestamps.birth),
    ] {
        if let Some(value) = value {
            validate_text(name, value, 1, MAX_TIMESTAMP_BYTES)?;
            chrono::DateTime::parse_from_rfc3339(value)
                .map_err(|error| anyhow!("invalid {name} timestamp: {error}"))?;
        }
    }
    Ok(())
}

fn validate_attributes(attributes: &HashMap<String, Value>) -> Result<()> {
    if attributes.len() > MAX_ATTRIBUTES {
        return Err(anyhow!(
            "node has {} attributes; maximum is {MAX_ATTRIBUTES}",
            attributes.len()
        ));
    }
    for key in attributes.keys() {
        validate_text("attribute key", key, 1, 128)?;
        if !key.contains('.') {
            return Err(anyhow!("attribute key {key:?} must be namespaced"));
        }
    }
    Ok(())
}

fn validate_text(name: &str, value: &str, minimum: usize, maximum: usize) -> Result<()> {
    if value.len() < minimum || value.len() > maximum {
        return Err(anyhow!(
            "{name} length {} is outside {minimum}..={maximum} bytes",
            value.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_optional_node_fields_and_ignores_host_only_fields() {
        let value = rmpv::Value::Map(vec![
            (Value::from("id"), Value::from("inode:1")),
            (Value::from("name"), Value::from("file.bin")),
            (Value::from("kind"), Value::from("file")),
            (Value::from("has_snapshot"), Value::Boolean(true)),
        ]);
        let node: LppFsNodeV1 = rmpv::ext::from_value(value).expect("wire node");
        let node = node.into_host("part:1").expect("host node");
        assert_eq!(node.parent_id.as_deref(), Some("part:1"));
        assert_eq!(node.size, 0);
        assert!(!node.has_snapshot);
    }

    #[test]
    fn rejects_unbounded_or_unnamespaced_extensions() {
        let node = LppFsNodeV1 {
            id: "inode:1".to_owned(),
            name: "file".to_owned(),
            parent_id: None,
            kind: "file".to_owned(),
            size: 0,
            deleted: false,
            inode: None,
            timestamps: LppTimestampsV1::default(),
            link_target: None,
            attributes: HashMap::from([("snapshot".to_owned(), Value::Boolean(true))]),
        };
        assert!(node.into_host("root").is_err());
    }
}
