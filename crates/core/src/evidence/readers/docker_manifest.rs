//! Docker-save and OCI image-layout manifest resolution.
//!
//! OCI archives may point at an image manifest directly or at one or more
//! nested image indexes (the shape emitted by recent multi-platform Docker
//! exports). Resolution follows indexes with a bounded depth, prefers a
//! descriptor matching the current CPU architecture, ignores attestation
//! manifests when a runnable image is available, and only chooses blobs that
//! are actually present in the evidence archive.

use std::collections::{BTreeMap, HashSet};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use crate::evidence::readers::docker_image::ImageConfig;
use crate::evidence::readers::tar_image::TarImage;

const MAX_OCI_INDEX_DEPTH: usize = 8;

pub(crate) trait OuterMemberIndex {
    fn member_location(&self, name: &str) -> Option<(u64, u64)>;

    fn has_member(&self, name: &str) -> bool {
        self.member_location(name).is_some()
    }
}

impl OuterMemberIndex for BTreeMap<String, (u64, u64)> {
    fn member_location(&self, name: &str) -> Option<(u64, u64)> {
        self.get(name).copied()
    }
}

impl OuterMemberIndex for TarImage {
    fn member_location(&self, name: &str) -> Option<(u64, u64)> {
        self.data_location(name)
    }
}

pub(crate) struct ResolvedManifest {
    pub(crate) layers: Vec<ResolvedLayer>,
}

pub(crate) struct ResolvedLayer {
    pub(crate) member: String,
    pub(crate) digest: String,
    pub(crate) media_type: String,
    pub(crate) declared_size: u64,
}

#[derive(Debug, Deserialize)]
struct OciIndex {
    manifests: Vec<OciDescriptor>,
}

#[derive(Debug, Deserialize)]
struct OciManifest {
    config: OciDescriptor,
    layers: Vec<OciDescriptor>,
}

#[derive(Debug, Deserialize)]
struct OciDescriptor {
    #[serde(rename = "mediaType", default)]
    media_type: String,
    digest: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    annotations: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    platform: Option<OciPlatform>,
}

#[derive(Debug, Deserialize)]
struct OciPlatform {
    #[serde(default)]
    architecture: String,
    #[serde(default)]
    os: String,
    #[serde(default)]
    variant: String,
}

#[derive(Debug, Deserialize)]
struct LegacyManifest {
    #[serde(rename = "Config")]
    config: String,
    #[serde(rename = "RepoTags", default)]
    repo_tags: Vec<String>,
    #[serde(rename = "Layers")]
    layers: Vec<String>,
    #[serde(rename = "LayerSources", default)]
    layer_sources: serde_json::Map<String, serde_json::Value>,
}

pub(crate) fn load_manifest<I: OuterMemberIndex + ?Sized>(
    outer: &[u8],
    index: &I,
) -> Result<(ResolvedManifest, ImageConfig, Vec<String>)> {
    if index.has_member("index.json") {
        match load_oci_manifest(outer, index) {
            Ok(resolved) => return Ok(resolved),
            Err(oci_err) if index.has_member("manifest.json") => {
                // Hybrid archives produced by recent Docker versions contain
                // both layouts. A partial/corrupt OCI graph may still have a
                // complete legacy manifest, so keep that useful fallback.
                return load_legacy_manifest(outer, index).with_context(|| {
                    format!("resolve OCI image ({oci_err:#}) and legacy fallback")
                });
            }
            Err(err) => return Err(err),
        }
    }
    load_legacy_manifest(outer, index)
}

fn load_oci_manifest<I: OuterMemberIndex + ?Sized>(
    outer: &[u8],
    index: &I,
) -> Result<(ResolvedManifest, ImageConfig, Vec<String>)> {
    let bytes = outer_slice(outer, index, "index.json")?;
    let root: OciIndex = serde_json::from_slice(bytes).context("parse OCI index.json")?;
    let mut visited = HashSet::new();
    let mut repo_tags = Vec::new();
    let descriptor = select_descriptor(&root.manifests, index)
        .ok_or_else(|| anyhow!("OCI index has no locally available image descriptors"))?;
    let manifest =
        resolve_oci_descriptor(outer, index, descriptor, 0, &mut visited, &mut repo_tags)?;

    let config_member = blob_member(&manifest.config.digest)?;
    let config_bytes = outer_slice(outer, index, &config_member)
        .with_context(|| format!("OCI config blob {config_member}"))?;
    let config: ImageConfig =
        serde_json::from_slice(config_bytes).context("parse OCI image config")?;
    let layers = manifest
        .layers
        .into_iter()
        .map(|layer| {
            Ok(ResolvedLayer {
                member: blob_member(&layer.digest)?,
                digest: layer.digest,
                media_type: layer.media_type,
                declared_size: layer.size,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((ResolvedManifest { layers }, config, repo_tags))
}

fn resolve_oci_descriptor<I: OuterMemberIndex + ?Sized>(
    outer: &[u8],
    index: &I,
    descriptor: &OciDescriptor,
    depth: usize,
    visited: &mut HashSet<String>,
    repo_tags: &mut Vec<String>,
) -> Result<OciManifest> {
    if depth >= MAX_OCI_INDEX_DEPTH {
        bail!("OCI image index nesting exceeds {MAX_OCI_INDEX_DEPTH}");
    }
    if !visited.insert(descriptor.digest.clone()) {
        bail!("OCI image index cycle at {}", descriptor.digest);
    }
    collect_repo_tag(descriptor, repo_tags);

    let member = blob_member(&descriptor.digest)?;
    let bytes = outer_slice(outer, index, &member)
        .with_context(|| format!("OCI descriptor blob {member}"))?;

    // The media type is authoritative, but accepting the JSON shape as a
    // fallback handles older tools that omit or mislabel it.
    let value: serde_json::Value =
        serde_json::from_slice(bytes).with_context(|| format!("parse OCI blob {member}"))?;
    if is_index_media_type(&descriptor.media_type) || value.get("manifests").is_some() {
        let nested: OciIndex = serde_json::from_value(value)
            .with_context(|| format!("parse OCI image index {member}"))?;
        let selected = select_descriptor(&nested.manifests, index).ok_or_else(|| {
            anyhow!("OCI image index {member} has no locally available image descriptors")
        })?;
        return resolve_oci_descriptor(outer, index, selected, depth + 1, visited, repo_tags);
    }

    serde_json::from_value(value).with_context(|| format!("parse OCI image manifest {member}"))
}

fn select_descriptor<'a, I: OuterMemberIndex + ?Sized>(
    descriptors: &'a [OciDescriptor],
    index: &I,
) -> Option<&'a OciDescriptor> {
    descriptors
        .iter()
        .enumerate()
        .filter(|(_, descriptor)| {
            blob_member(&descriptor.digest)
                .ok()
                .is_some_and(|member| index.has_member(&member))
        })
        .max_by_key(|(position, descriptor)| (descriptor_score(descriptor), usize::MAX - position))
        .map(|(_, descriptor)| descriptor)
}

fn descriptor_score(descriptor: &OciDescriptor) -> i32 {
    let mut score = if is_index_media_type(&descriptor.media_type) {
        20
    } else {
        10
    };
    if descriptor
        .annotations
        .get("vnd.docker.reference.type")
        .and_then(serde_json::Value::as_str)
        == Some("attestation-manifest")
    {
        score -= 1_000;
    }
    if let Some(platform) = &descriptor.platform {
        if platform.architecture == preferred_architecture() {
            score += 100;
        } else if platform.architecture == "unknown" || platform.architecture.is_empty() {
            score -= 20;
        }
        if platform.os == "unknown" {
            score -= 10;
        }
        if platform.variant == "unknown" {
            score -= 1;
        }
    }
    score
}

fn preferred_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "x86" => "386",
        "aarch64" => "arm64",
        architecture => architecture,
    }
}

fn is_index_media_type(media_type: &str) -> bool {
    media_type.contains("image.index") || media_type.contains("manifest.list")
}

fn collect_repo_tag(descriptor: &OciDescriptor, repo_tags: &mut Vec<String>) {
    for key in [
        "io.containerd.image.name",
        "org.opencontainers.image.ref.name",
    ] {
        let Some(tag) = descriptor
            .annotations
            .get(key)
            .and_then(|value| value.as_str())
        else {
            continue;
        };
        if !repo_tags.iter().any(|existing| existing == tag) {
            repo_tags.push(tag.to_string());
        }
    }
}

fn load_legacy_manifest<I: OuterMemberIndex + ?Sized>(
    outer: &[u8],
    index: &I,
) -> Result<(ResolvedManifest, ImageConfig, Vec<String>)> {
    let bytes = outer_slice(outer, index, "manifest.json")
        .context("archive has neither usable index.json nor manifest.json")?;
    let entries: Vec<LegacyManifest> =
        serde_json::from_slice(bytes).context("parse legacy manifest.json")?;
    let entry = entries
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("legacy manifest.json is empty"))?;
    let config_bytes = outer_slice(outer, index, &entry.config)
        .with_context(|| format!("config blob {}", entry.config))?;
    let config: ImageConfig =
        serde_json::from_slice(config_bytes).context("parse legacy image config")?;
    let layers = entry
        .layers
        .iter()
        .map(|member| {
            let digest = digest_from_member(member);
            let (media_type, declared_size) = entry
                .layer_sources
                .get(&digest)
                .and_then(|value| value.as_object())
                .map(|object| {
                    let media_type = object
                        .get("mediaType")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string();
                    let size = object
                        .get("size")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0);
                    (media_type, size)
                })
                .unwrap_or_default();
            ResolvedLayer {
                member: member.clone(),
                digest,
                media_type,
                declared_size,
            }
        })
        .collect();
    Ok((ResolvedManifest { layers }, config, entry.repo_tags))
}

fn outer_slice<'a, I: OuterMemberIndex + ?Sized>(
    outer: &'a [u8],
    index: &I,
    name: &str,
) -> Result<&'a [u8]> {
    let (offset, size) = index
        .member_location(name)
        .ok_or_else(|| anyhow!("missing archive member: {name}"))?;
    let start =
        usize::try_from(offset).with_context(|| format!("member offset overflow: {name}"))?;
    let size = usize::try_from(size).with_context(|| format!("member size overflow: {name}"))?;
    let end = start
        .checked_add(size)
        .ok_or_else(|| anyhow!("member size overflow: {name}"))?;
    outer
        .get(start..end)
        .ok_or_else(|| anyhow!("member out of range: {name}"))
}

fn blob_member(digest: &str) -> Result<String> {
    let (algorithm, hex) = digest
        .split_once(':')
        .ok_or_else(|| anyhow!("malformed digest: {digest}"))?;
    if algorithm.is_empty()
        || hex.is_empty()
        || !hex.chars().all(|character| character.is_ascii_hexdigit())
    {
        bail!("invalid digest: {digest}");
    }
    Ok(format!("blobs/{algorithm}/{hex}"))
}

fn digest_from_member(member: &str) -> String {
    member
        .strip_prefix("blobs/")
        .and_then(|rest| rest.split_once('/'))
        .map(|(algorithm, hex)| format!("{algorithm}:{hex}"))
        .unwrap_or_else(|| member.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_member(
        outer: &mut Vec<u8>,
        index: &mut BTreeMap<String, (u64, u64)>,
        name: String,
        bytes: &[u8],
    ) {
        let offset = outer.len() as u64;
        outer.extend_from_slice(bytes);
        index.insert(name, (offset, bytes.len() as u64));
    }

    #[test]
    fn descriptor_selection_prefers_local_arch_and_skips_attestation() {
        let digest_attestation = format!("sha256:{}", "a".repeat(64));
        let digest_arm = format!("sha256:{}", "b".repeat(64));
        let digest_preferred = format!("sha256:{}", "c".repeat(64));
        let json = serde_json::json!({
            "manifests": [
                {
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "digest": digest_attestation,
                    "annotations": {"vnd.docker.reference.type": "attestation-manifest"},
                    "platform": {"architecture": "unknown", "os": "unknown"}
                },
                {
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "digest": digest_arm,
                    "platform": {"architecture": "arm", "os": "linux"}
                },
                {
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "digest": digest_preferred,
                    "platform": {"architecture": preferred_architecture(), "os": "linux"}
                }
            ]
        });
        let parsed: OciIndex = serde_json::from_value(json).expect("parse fixture");
        let mut index = BTreeMap::new();
        for descriptor in &parsed.manifests {
            index.insert(
                blob_member(&descriptor.digest).expect("valid digest"),
                (0, 1),
            );
        }

        let selected = select_descriptor(&parsed.manifests, &index).expect("select descriptor");
        assert_eq!(selected.digest, digest_preferred);
    }

    #[test]
    fn resolves_nested_oci_index_and_preserves_outer_tag() {
        let nested_digest = format!("sha256:{}", "1".repeat(64));
        let manifest_digest = format!("sha256:{}", "2".repeat(64));
        let missing_digest = format!("sha256:{}", "3".repeat(64));
        let config_digest = format!("sha256:{}", "4".repeat(64));
        let layer_digest = format!("sha256:{}", "5".repeat(64));

        let root = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "manifests": [{
                "mediaType": "application/vnd.oci.image.index.v1+json",
                "digest": nested_digest,
                "annotations": {"io.containerd.image.name": "example/postgres:15"}
            }]
        });
        let nested = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "manifests": [
                {
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "digest": missing_digest,
                    "platform": {"architecture": "arm64", "os": "linux"}
                },
                {
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "digest": manifest_digest,
                    "platform": {"architecture": preferred_architecture(), "os": "linux"}
                }
            ]
        });
        let manifest = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": "application/vnd.oci.image.config.v1+json",
                "digest": config_digest,
                "size": 42
            },
            "layers": [{
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                "digest": layer_digest,
                "size": 99
            }]
        });
        let config = serde_json::json!({
            "architecture": preferred_architecture(),
            "os": "linux"
        });

        let mut outer = Vec::new();
        let mut index = BTreeMap::new();
        add_member(
            &mut outer,
            &mut index,
            "index.json".to_string(),
            &serde_json::to_vec(&root).expect("encode root index"),
        );
        add_member(
            &mut outer,
            &mut index,
            blob_member(&nested_digest).expect("nested member"),
            &serde_json::to_vec(&nested).expect("encode nested index"),
        );
        add_member(
            &mut outer,
            &mut index,
            blob_member(&manifest_digest).expect("manifest member"),
            &serde_json::to_vec(&manifest).expect("encode manifest"),
        );
        add_member(
            &mut outer,
            &mut index,
            blob_member(&config_digest).expect("config member"),
            &serde_json::to_vec(&config).expect("encode config"),
        );
        add_member(
            &mut outer,
            &mut index,
            blob_member(&layer_digest).expect("layer member"),
            b"layer-bytes",
        );

        let (resolved, config, tags) =
            load_manifest(&outer, &index).expect("resolve nested OCI index");
        assert_eq!(
            config.architecture.as_deref(),
            Some(preferred_architecture())
        );
        assert_eq!(config.os.as_deref(), Some("linux"));
        assert_eq!(tags, vec!["example/postgres:15"]);
        assert_eq!(resolved.layers.len(), 1);
        assert_eq!(resolved.layers[0].digest, layer_digest);
    }

    #[test]
    fn hybrid_archive_falls_back_to_legacy_manifest() {
        let config_member = "config.json";
        let layer_member = "layer.tar";
        let legacy = serde_json::json!([{
            "Config": config_member,
            "RepoTags": ["example/postgres:legacy"],
            "Layers": [layer_member]
        }]);
        let config = serde_json::json!({"architecture": "amd64", "os": "linux"});

        let mut outer = Vec::new();
        let mut index = BTreeMap::new();
        add_member(
            &mut outer,
            &mut index,
            "index.json".to_string(),
            b"not-json",
        );
        add_member(
            &mut outer,
            &mut index,
            "manifest.json".to_string(),
            &serde_json::to_vec(&legacy).expect("encode legacy manifest"),
        );
        add_member(
            &mut outer,
            &mut index,
            config_member.to_string(),
            &serde_json::to_vec(&config).expect("encode config"),
        );
        add_member(
            &mut outer,
            &mut index,
            layer_member.to_string(),
            b"layer-bytes",
        );

        let (resolved, _, tags) =
            load_manifest(&outer, &index).expect("fall back to legacy manifest");
        assert_eq!(tags, vec!["example/postgres:legacy"]);
        assert_eq!(resolved.layers.len(), 1);
        assert_eq!(resolved.layers[0].member, layer_member);
    }
}
