//! Bounded, read-only Linux timezone discovery through evidence filesystem APIs.
//! Symlink targets are interpreted within the evidence, never by the host OS.
use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{parse, CliError, Selection};
use crate::core::{
    evidence::partitions::PartitionKind,
    filesystem::{
        evidence_session::EvidenceSession,
        fs_all::{directory_root, FsAll},
        fs_dispatch,
        tree::fs_tree::{FsNode, FsSession, NodeKind},
    },
};

const MAX_TZIF: usize = 1024 * 1024;
const MAX_TEXT: usize = 4096;
const MAX_LINKS: usize = 40;
const MAX_PATH_COMPONENTS: usize = 256;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Detection {
    pub candidates: Vec<Selection>,
    pub warnings: Vec<String>,
    pub unresolved_volumes: BTreeSet<String>,
}

impl Detection {
    pub fn select(&self, node: Option<&str>, strict: bool) -> Result<Selection, CliError> {
        let volume = node.and_then(fs_dispatch::volume_root_key).map(|key| {
            if let Some(index) = key.find("lvm:") {
                key[index..].to_owned()
            } else if let Some(index) = key.find("part:") {
                key[index..].to_owned()
            } else {
                key
            }
        });
        let local: Vec<_> = self
            .candidates
            .iter()
            .filter(|candidate| volume.is_some() && candidate.volume_key == volume)
            .collect();
        let unresolved = if local.is_empty() {
            !self.unresolved_volumes.is_empty()
        } else {
            volume
                .as_ref()
                .is_some_and(|volume| self.unresolved_volumes.contains(volume))
        };
        let candidates = if local.is_empty() {
            self.candidates.iter().collect::<Vec<_>>()
        } else {
            local
        };
        if let Some(first) = candidates.first().filter(|_| !unresolved) {
            if candidates
                .iter()
                .all(|candidate| candidate.zone == first.zone)
            {
                let mut selected = (*first).clone();
                selected.warnings.extend(self.warnings.clone());
                return Ok(selected);
            }
        }
        let message = if unresolved {
            "检材时区配置存在未解决的读取错误或冲突"
        } else if candidates.is_empty() {
            "未能从检材配置识别时区"
        } else {
            "检材中存在多个不同的系统时区，无法唯一选择"
        };
        if strict {
            return Err(CliError {
                details: Some(
                    serde_json::json!({"candidates": self.candidates, "warnings": self.warnings, "unresolvedVolumes": self.unresolved_volumes}),
                ),
                ..CliError::new(
                    "timezoneUnknown",
                    format!(
                        "{message}；用 timezones get 检查，或 --timezone Asia/Shanghai 明确指定"
                    ),
                )
            });
        }
        let mut selected = Selection::configured("UTC", "fallback")?;
        selected.warnings = self.warnings.clone();
        selected
            .warnings
            .push(format!("{message}；暂按 UTC 显示，可用 --timezone 指定"));
        Ok(selected)
    }
}

pub(super) fn detect(image: Option<&Arc<EvidenceSession>>) -> Detection {
    let mut result = Detection::default();
    let Some(image) = image else {
        return result;
    };
    if image.is_index_only() {
        result
            .warnings
            .push("检材仅有离线索引，无法读取时区配置；可使用已保存的案件/检材时区设置".into());
        return result;
    }
    let mut session = FsSession::new(image);
    for entry in FsAll::for_image(image).entries() {
        if !matches!(
            entry.kind,
            PartitionKind::Ext4 | PartitionKind::Btrfs | PartitionKind::Unknown
        ) {
            continue;
        }
        let root = match directory_root(image, &entry.volume_key) {
            Ok(root) => root,
            Err(error) => {
                result.unresolved_volumes.insert(entry.volume_key.clone());
                result
                    .warnings
                    .push(format!("{} 时区目录不可访问：{error}", entry.volume_key));
                continue;
            }
        };
        let mut roots = vec![(root.clone(), String::new())];
        // Btrfs systems commonly put /etc below @ or a default subvolume.
        // Inspect one level only, with a hard cap, never walk user data.
        if entry.kind == PartitionKind::Btrfs {
            match session.children(&root) {
                Ok(nodes) => {
                    let directories: Vec<_> = nodes
                        .into_iter()
                        .filter(|node| node.kind == NodeKind::Dir && node.link_target.is_none())
                        .collect();
                    if directories.len() > 32 {
                        result.unresolved_volumes.insert(entry.volume_key.clone());
                        result.warnings.push(format!(
                            "{} 子卷候选超过 32 个，请明确指定时区",
                            entry.volume_key
                        ));
                    } else {
                        roots.extend(
                            directories
                                .into_iter()
                                .map(|node| (node.id, format!("/{}", node.name))),
                        );
                    }
                }
                Err(error) => {
                    result.unresolved_volumes.insert(entry.volume_key.clone());
                    result
                        .warnings
                        .push(format!("{} 时区子卷不可访问：{error}", entry.volume_key));
                }
            }
        }
        for (root, prefix) in roots {
            match probe(&mut session, &root, &entry.volume_key, &prefix) {
                Ok(Some(candidate)) => result.candidates.push(candidate),
                Ok(None) => {}
                Err(error) => {
                    result.unresolved_volumes.insert(entry.volume_key.clone());
                    result.warnings.push(format!(
                        "{}{prefix} 时区配置无法读取：{error}",
                        entry.volume_key
                    ));
                }
            }
        }
    }
    result
}

fn lookup(session: &mut FsSession<'_>, root: &str, path: &str) -> anyhow::Result<Option<FsNode>> {
    let mut pending: VecDeque<_> = path
        .split('/')
        .filter(|component| !component.is_empty())
        .map(|component| (component.to_owned(), false))
        .collect();
    let mut parents = vec![root.to_owned()];
    let mut links = 0;
    let mut inspected = 0;
    while let Some((component, from_link)) = pending.pop_front() {
        inspected += 1;
        if inspected > MAX_PATH_COMPONENTS || pending.len() > MAX_PATH_COMPONENTS {
            anyhow::bail!("时区配置路径超过 {MAX_PATH_COMPONENTS} 个分量的解析上限");
        }
        if component == "." {
            continue;
        }
        if component == ".." {
            if parents.len() == 1 {
                anyhow::bail!("配置路径不能越过检材根目录");
            }
            parents.pop();
            continue;
        }
        let parent = parents
            .last()
            .ok_or_else(|| anyhow::anyhow!("时区配置解析缺少检材根目录"))?;
        let node = session
            .children(parent)?
            .into_iter()
            .find(|node| node.name == component && !node.deleted);
        let Some(node) = node else {
            if from_link {
                anyhow::bail!("配置目录链接的目标不存在：{component}");
            }
            return Ok(None);
        };
        // Keep a terminal link intact: /etc/localtime's zoneinfo target is
        // itself provenance, even when the target TZif is unavailable.
        if pending.is_empty() {
            return Ok(Some(node));
        }
        if node.kind == NodeKind::Symlink || node.link_target.is_some() {
            links += 1;
            if links > MAX_LINKS {
                anyhow::bail!("配置目录链接超过 {MAX_LINKS} 次跳转，可能成环");
            }
            let target = node
                .link_target
                .as_deref()
                .filter(|target| !target.is_empty())
                .ok_or_else(|| anyhow::anyhow!("配置目录链接 {component} 缺少目标"))?;
            if target.len() > MAX_TEXT
                || target.contains(['\\', '\0'])
                || target.as_bytes().get(1) == Some(&b':')
                || target.starts_with("//")
            {
                anyhow::bail!("配置目录链接 {component} 的目标不是受支持的检材内路径");
            }
            if target.starts_with('/') {
                parents.truncate(1);
            }
            for component in target.split('/').filter(|part| !part.is_empty()).rev() {
                pending.push_front((component.to_owned(), true));
            }
            continue;
        }
        if node.kind != NodeKind::Dir {
            if from_link {
                anyhow::bail!("配置目录链接目标 {component} 不是目录");
            }
            return Ok(None);
        }
        parents.push(node.id);
    }
    Ok(None)
}

fn read(session: &mut FsSession<'_>, node: &FsNode, cap: usize) -> anyhow::Result<Vec<u8>> {
    if node.link_target.is_some() || node.kind != NodeKind::File {
        anyhow::bail!("只读取普通配置文件，不读取未解析的链接或目录");
    }
    if node.size > cap as u64 {
        anyhow::bail!("{} 超过时区配置读取上限 {cap} 字节", node.name);
    }
    let bytes = session.read_file_limited(&node.id, cap)?;
    if bytes.len() as u64 != node.size {
        anyhow::bail!("{} 时区配置读取不完整", node.name);
    }
    Ok(bytes)
}

fn probe(
    session: &mut FsSession<'_>,
    root: &str,
    volume: &str,
    prefix: &str,
) -> anyhow::Result<Option<Selection>> {
    let mut warnings = Vec::new();
    if let Some(node) = lookup(session, root, "etc/localtime")? {
        if let Some(target) = &node.link_target {
            if let Some((_, name)) = target.split_once("zoneinfo/") {
                if let Ok(mut selection) = Selection::configured(name, "evidenceLocaltimeLink") {
                    selection.configuration =
                        Some(format!("{volume}{prefix}/etc/localtime -> {target}"));
                    selection.volume_key = Some(volume.into());
                    // The original TZif takes precedence over the bundled rules.
                    // Only use the conventional zoneinfo tree within this root.
                    match lookup(session, root, &format!("usr/share/zoneinfo/{name}")) {
                        Ok(Some(original)) if original.link_target.is_none() => {
                            match read(session, &original, MAX_TZIF).and_then(|bytes| {
                                jiff::tz::TimeZone::tzif(name, &bytes).map_err(anyhow::Error::from)
                            }) {
                                Ok(timezone) => {
                                    selection.timezone = timezone;
                                    selection.source = "evidenceTzif".into();
                                }
                                Err(error) => selection.warnings.push(format!(
                                    "检材 zoneinfo 无法解析，使用内置 IANA 规则：{error}"
                                )),
                            }
                        }
                        Ok(Some(_)) => selection
                            .warnings
                            .push("检材 zoneinfo 是未解析链接，使用内置 IANA 规则".into()),
                        Ok(None) => {}
                        Err(error) => selection.warnings.push(format!(
                            "检材 zoneinfo 无法访问，使用内置 IANA 规则：{error}"
                        )),
                    }
                    return Ok(Some(selection));
                }
            }
            warnings.push(format!("无法识别 /etc/localtime 链接：{target}"));
        } else {
            match read(session, &node, MAX_TZIF).and_then(|bytes| {
                let digest = hex::encode(Sha256::digest(&bytes));
                jiff::tz::TimeZone::tzif("evidence/localtime", &bytes)
                    .map(|timezone| (timezone, digest))
                    .map_err(anyhow::Error::from)
            }) {
                Ok((timezone, digest)) => {
                    return Ok(Some(Selection {
                        zone: format!("TZif:sha256:{digest}"),
                        source: "evidenceTzif".into(),
                        configuration: Some(format!("{volume}{prefix}/etc/localtime")),
                        volume_key: Some(volume.into()),
                        warnings,
                        timezone,
                    }))
                }
                Err(error) => warnings.push(format!("/etc/localtime 无法解析：{error}")),
            }
        }
    }
    let mut candidates = Vec::new();
    for path in [
        "etc/timezone",
        "etc/sysconfig/clock",
        "etc/TZ",
        "etc/config/system",
    ] {
        let Some(node) = lookup(session, root, path)? else {
            continue;
        };
        let bytes = read(session, &node, MAX_TEXT)?;
        let text = std::str::from_utf8(&bytes)?;
        let names: Vec<_> = if matches!(path, "etc/timezone" | "etc/TZ") {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .collect()
        } else {
            text.lines()
                .filter_map(|line| {
                    let line = line.trim();
                    if let Some((key, value)) = line.split_once('=') {
                        matches!(key.trim(), "ZONE" | "TIMEZONE")
                            .then(|| value.trim().trim_matches(['\'', '"']))
                    } else {
                        line.strip_prefix("option zonename ")
                            .map(|value| value.trim().trim_matches(['\'', '"']))
                    }
                })
                .collect()
        };
        for name in names {
            match parse(name) {
                Ok(timezone) => {
                    let zone = timezone.iana_name().unwrap_or(name).to_owned();
                    candidates.push(Selection {
                        zone,
                        source: "evidenceConfiguration".into(),
                        configuration: Some(format!("{volume}{prefix}/{path}")),
                        volume_key: Some(volume.into()),
                        warnings: vec![],
                        timezone,
                    });
                }
                Err(error) => warnings.push(format!("{path}：{error}")),
            }
        }
    }
    if let Some(mut selected) = candidates.first().cloned() {
        if candidates
            .iter()
            .any(|candidate| candidate.zone != selected.zone)
        {
            anyhow::bail!("{volume}{prefix} 的时区配置相互冲突");
        }
        selected.warnings.extend(warnings);
        Ok(Some(selected))
    } else if warnings.is_empty() {
        Ok(None)
    } else {
        anyhow::bail!("{}", warnings.join("；"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguous_systems_do_not_silently_select_the_first() {
        let mut first = Selection::configured("Asia/Shanghai", "test").unwrap();
        first.volume_key = Some("part:1".into());
        let mut second = Selection::configured("America/New_York", "test").unwrap();
        second.volume_key = Some("part:2".into());
        let detection = Detection {
            candidates: vec![first, second],
            warnings: vec![],
            ..Detection::default()
        };
        assert_eq!(detection.select(None, false).unwrap().source, "fallback");
        assert!(detection.select(None, true).is_err());
        assert_eq!(
            detection.select(Some("ext4:part:1:2"), true).unwrap().zone,
            "Asia/Shanghai"
        );
    }

    #[test]
    fn unresolved_volume_cannot_inherit_another_systems_zone() {
        let mut candidate = Selection::configured("Asia/Shanghai", "test").unwrap();
        candidate.volume_key = Some("part:1".into());
        let detection = Detection {
            candidates: vec![candidate],
            unresolved_volumes: BTreeSet::from(["part:2".into()]),
            ..Detection::default()
        };
        assert_eq!(
            detection.select(Some("ext4:part:1:2"), true).unwrap().zone,
            "Asia/Shanghai"
        );
        assert_eq!(
            detection
                .select(Some("ext4:part:2:2"), false)
                .unwrap()
                .source,
            "fallback"
        );
        assert!(detection.select(Some("ext4:part:2:2"), true).is_err());
        assert!(detection.select(None, true).is_err());
    }
}
