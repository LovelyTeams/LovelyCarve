//! scope implementation.
use super::*;
use crate::core::filesystem::evidence_session::EvidenceSession;

// ── 范围解析 ───────────────────────────────────────────────────────────

/// 解析编码页标签；空列表 = 全部编码。
pub fn resolve_encodings(labels: &[String]) -> anyhow::Result<Vec<SearchEncoding>> {
    if labels.is_empty() {
        return Ok(SearchEncoding::ALL.to_vec());
    }
    let mut encodings = Vec::new();
    for label in labels {
        let encoding = SearchEncoding::from_label(label)
            .ok_or_else(|| anyhow::anyhow!("未知编码：{label}"))?;
        if !encodings.contains(&encoding) {
            encodings.push(encoding);
        }
    }
    // 按 ALL 优先级归一（跨编码区间去重依赖该顺序）。
    Ok(SearchEncoding::ALL
        .iter()
        .copied()
        .filter(|encoding| encodings.contains(encoding))
        .collect())
}

/// 卷是否落在分区勾选范围内：精确键命中，或 `part:N` 连带 `lvm:N:M`。
pub(super) fn volume_in_partition_scope(volume_key: &str, selected: &[String]) -> bool {
    selected.iter().any(|selected_key| {
        if volume_key == selected_key {
            return true;
        }
        if let Some(part_id) = selected_key.strip_prefix("part:") {
            return volume_key.starts_with(&format!("lvm:{part_id}:"));
        }
        false
    })
}

/// 宿主 Docker 容器文件映射到底层卷上的文件节点（其余节点返回 `None`）。
/// 映射失败时保留容器节点：读取照常走会话的 Docker 解析，失败计为读错误。
pub(super) fn host_docker_read_id(image: &Arc<EvidenceSession>, node_id: &str) -> Option<String> {
    let id = host_docker::parse_host_docker_id(node_id)?;
    match host_docker::source_file_id(image, &id) {
        Ok(source_id) => Some(source_id),
        Err(err) => {
            tracing::debug!("global_search: unresolved Docker file {node_id}: {err:#}");
            None
        }
    }
}

/// 把搜索范围解析成可扫源清单。文件行来自台账登记的各卷当前索引；未分配
/// 空间来自分区虚拟节点。返回 (源清单, 文件总数)。
pub(super) fn resolve_sources(
    params: &GlobalSearchParams<'_>,
) -> anyhow::Result<(Vec<ScanSource>, u64)> {
    let options = params.options;
    let mut sources: Vec<ScanSource> = Vec::new();
    let mut files_total: u64 = 0;

    let tag_allowed: Option<HashSet<String>> = if options.scope.tags.is_empty() {
        None
    } else {
        let tags = tag_store::load(params.case_dir, params.image_id)?;
        Some(
            tags.into_iter()
                .filter(|(_, tag)| options.scope.tags.contains(tag))
                .map(|(node_id, _)| node_id)
                .collect(),
        )
    };

    // 索引发布后不再修改:只读打开,不与任何写者排队。
    let mut indexes: Vec<(IndexedVolume, rusqlite::Connection)> = Vec::new();
    for volume in indexed_volumes(params.case_dir, params.image_id)? {
        let conn = filedb_file::open_reader(&volume.index)?;
        indexes.push((volume, conn));
    }
    let in_scope = |volume_key: &str| {
        options.scope.partitions.is_empty()
            || volume_in_partition_scope(volume_key, &options.scope.partitions)
    };

    // 树节点范围：先逐项定位到所在卷（文件 → 单个节点；目录 → 子树前缀）。
    let mut subtrees: Vec<SubtreeScope> = Vec::new();
    let mut single_nodes: HashSet<String> = HashSet::new();
    for node_or_path in &options.scope.tree_nodes {
        resolve_tree_node(&indexes, node_or_path, &mut subtrees, &mut single_nodes)?;
    }
    let tree_scoped = !subtrees.is_empty() || !single_nodes.is_empty();

    for (volume, conn) in &indexes {
        if !in_scope(&volume.volume_key) {
            continue;
        }
        let mut stmt = conn.prepare(
            "SELECT nid, name, size, full_path, full_path_low, is_delete FROM filedb
             WHERE kind = 'file' AND nid NOT LIKE 'unalloc:%' AND nid NOT LIKE 'diskgap:%'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                ScanSource {
                    node_id: row.get(0)?,
                    read_id: None,
                    name: row.get(1)?,
                    size_bytes: row.get::<_, i64>(2)?.max(0) as u64,
                    path: row.get(3)?,
                    volume_key: volume.volume_key.clone(),
                    source_kind: "file",
                },
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;
        for row in rows {
            let (mut source, path_low, is_delete) = row?;
            // 已删除过滤（1 = 已删除, 2 = 在世）。
            if is_delete == 1 && !options.search_deleted {
                continue;
            }
            // 标签范围：只留带所选标签的节点。
            if let Some(allowed) = &tag_allowed {
                if !allowed.contains(&source.node_id) {
                    continue;
                }
            }
            // 树节点范围：单文件命中 或 落在同卷任一子树前缀内。
            if tree_scoped {
                let in_subtree = subtrees.iter().any(|subtree| {
                    subtree.volume_key == volume.volume_key
                        && path_low.as_str() >= subtree.low.as_str()
                        && path_low.as_str() < subtree.high.as_str()
                });
                if !in_subtree && !single_nodes.contains(&source.node_id) {
                    continue;
                }
            }
            source.read_id = host_docker_read_id(params.image, &source.node_id);
            files_total += 1;
            sources.push(source);
        }
    }

    // 未分配空间（物理搜索）：整镜像或分区范围下生效；标签/树节点范围跳过。
    let empty_space_enabled =
        options.search_empty_space && options.scope.tags.is_empty() && !tree_scoped;
    if empty_space_enabled {
        for partition in &params.image.partitions {
            let part_key = format!("part:{}", partition.id);
            if !in_scope(&part_key) {
                continue;
            }
            for node in fs_unalloc_nodes(params.image, partition) {
                if node.size == 0 {
                    continue;
                }
                files_total += 1;
                sources.push(ScanSource {
                    node_id: node.id,
                    read_id: None,
                    name: node.name.clone(),
                    size_bytes: node.size,
                    path: node.name.clone(),
                    volume_key: part_key.clone(),
                    source_kind: "unallocated",
                });
            }
        }
        for node in disk_gap_nodes(params.image) {
            if node.size == 0 {
                continue;
            }
            files_total += 1;
            sources.push(ScanSource {
                node_id: node.id,
                read_id: None,
                name: node.name.clone(),
                size_bytes: node.size,
                path: node.name.clone(),
                volume_key: String::new(),
                source_kind: "diskgap",
            });
        }
    }

    Ok((sources, files_total))
}

/// 解析一个树节点范围项：文件 → `single_nodes`；目录 → `subtrees`。项可以
/// 是节点 id（树勾选）或以 `/` 开头的卷内路径（「按目录」输入，大小写不敏感，
/// 多个卷有同一路径时取卷键序第一个）。在所有已索引卷里定位——分区范围之外
/// 的卷上的项只是扫不到，不算错；哪个卷都找不到时报错：勾选来自这棵树，解析
/// 失败说明索引已过期。
pub(super) fn resolve_tree_node(
    indexes: &[(IndexedVolume, rusqlite::Connection)],
    node_or_path: &str,
    subtrees: &mut Vec<SubtreeScope>,
    single_nodes: &mut HashSet<String>,
) -> anyhow::Result<()> {
    for (volume, conn) in indexes {
        let row = if node_or_path.starts_with('/') {
            filedb_query::node_row_by_path(conn, node_or_path)?
        } else {
            filedb_query::node_row(conn, node_or_path)?
        };
        let Some(row) = row else {
            continue;
        };
        if row.kind == "file" {
            single_nodes.insert(row.nid);
        } else {
            let low = row.full_path.to_lowercase();
            let high = format!("{low}/\u{10FFFF}");
            subtrees.push(SubtreeScope {
                volume_key: volume.volume_key.clone(),
                low,
                high,
            });
        }
        return Ok(());
    }
    anyhow::bail!("树节点不在文件索引中（索引可能已过期）：{node_or_path}")
}
