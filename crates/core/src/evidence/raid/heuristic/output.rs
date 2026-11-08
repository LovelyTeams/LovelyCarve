//! Detected array and filesystem result projection.
use super::*;

pub(super) fn emit(
    group: &[Arc<DiskImage>],
    winner: Candidate,
    deep_score: u32,
    probed: usize,
    out: &mut HeuristicOutput,
) {
    let n = group.len();
    let fs_part = largest_validatable_fs(&winner.partitions);
    let fs_summary = fs_part.and_then(|p| filesystem_summary(winner.source.as_ref(), p));

    let set_uuid = fs_summary
        .as_ref()
        .and_then(|s| s.fsid_hex.clone())
        .unwrap_or_else(|| synthetic_set_uuid(group, &winner));
    let set_name = format!("auto-raid{}", winner.level_tag);

    let members: Vec<RaidArrayMember> = winner
        .order
        .iter()
        .enumerate()
        .map(|(slot, &disk)| {
            let image = &group[disk];
            let reconstructed = winner.missing_disk == Some(disk);
            RaidArrayMember {
                dev_number: slot as u32,
                slot: Some(slot as u32),
                device_uuid: String::new(),
                data_size_sectors: (image.len().saturating_sub(winner.data_offset_bytes)) / 512,
                reconstructed,
                state: if reconstructed {
                    RaidMemberState::Stale
                } else {
                    RaidMemberState::Active
                },
                events: None,
                origin: RaidMemberOrigin::Whole {
                    image_id: image.id.clone(),
                    partition_id: 0,
                    byte_offset: 0,
                    byte_length: image.len(),
                },
            }
        })
        .collect();

    out.arrays.push(RaidArray {
        set_uuid: set_uuid.clone(),
        set_name: set_name.clone(),
        level: winner.level_tag,
        layout: winner.layout_raw,
        chunksize_sectors: (winner.chunk_bytes / 512) as u32,
        raid_disks: n as u32,
        data_offset_sectors: winner.data_offset_bytes / 512,
        assembled_bytes: winner.capacity,
        members,
        assembled_image_id: None,
        heuristic: true,
        metadata_version: String::new(),
        degraded: winner.missing_disk.is_some(),
        degradable: false,
    });

    let fs_kind = fs_part.map(|p| p.kind).unwrap_or(PartitionKind::Unknown);
    if let Some(s) = &fs_summary {
        out.filesystems.push(s.record(&set_uuid));
    }

    out.assembled.push(AssembledArray {
        label: array_label(&set_name, &set_uuid),
        set_uuid,
        set_name,
        target: AssembledTarget::Array,
        source: winner.source,
        size_bytes: winner.capacity,
        fs_kind,
        partitions: Some(winner.partitions),
    });

    let layout_label = if winner.level_tag == 5 {
        Raid5Layout::from_raw(winner.layout_raw)
            .map(|l| l.as_str())
            .unwrap_or("unknown")
    } else {
        "none"
    };
    let order_disks: Vec<String> = winner.order.iter().map(|&i| i.to_string()).collect();
    let missing = winner
        .missing_disk
        .map(|d| format!("disk {d} (parity-rebuilt)"))
        .unwrap_or_else(|| "none".to_string());
    let fs_label = fs_summary
        .as_ref()
        .map(|s| kind_label(s.kind))
        .unwrap_or("unknown");
    out.diagnostics.push(format!(
        "metadata-less RAID{level} reconstructed: {n} disks, {chunk}K chunk, {layout}, \
         order [{order}], data_offset {off} sectors, missing {missing}; \
         {fs_label} deep score {deep_score}/{probed}",
        level = winner.level_tag,
        chunk = winner.chunk_bytes / 1024,
        layout = layout_label,
        order = order_disks.join(","),
        off = winner.data_offset_bytes / 512,
    ));
}

/// Deterministic non-empty id for arrays with no ext4 UUID to anchor on, so
/// the IPC idempotency cache and `assembled://` path stay stable across runs.
pub(super) fn synthetic_set_uuid(group: &[Arc<DiskImage>], winner: &Candidate) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut paths: Vec<&str> = group.iter().map(|i| i.id.as_str()).collect();
    paths.sort_unstable();
    for p in paths {
        p.hash(&mut hasher);
    }
    winner.level_tag.hash(&mut hasher);
    winner.chunk_bytes.hash(&mut hasher);
    winner.layout_raw.hash(&mut hasher);
    let h = hasher.finish();
    format!("auto{h:016x}{h:016x}")
}
