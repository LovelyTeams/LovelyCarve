//! raid implementation.
use super::*;
use crate::core::evidence::image::DiskImage;
use crate::core::filesystem::evidence_session::EvidenceSession;

/// Detect LVM / md stacks across every loaded image (see
/// `raid::detect_with_options`) and register each assembled array — and each
/// complete LV not consumed as an md member — as a synthetic `DiskImage` in
/// `state.images`, so it shows up in the FS tree, hex view and image switcher
/// just like a regular image. `degraded_sets` names md sets the examiner chose
/// to assemble without members their metadata still expects.
/// Idempotent: re-running on the same image set reuses the previously
/// registered synthetic images instead of allocating duplicates.
pub async fn raid_auto_detect(
    app: AppHandle,
    state: State<'_, AppState>,
    degraded_sets: Vec<String>,
) -> Result<RaidStack, AppError> {
    // Snapshot only "real" loaded images — feeding a previously assembled
    // synthetic image back into detection would be double-counting and
    // could even infinite-loop when LVM PV labels happen to match. A shadow
    // copy repeats its parent volume, so it would pose as a mirror member.
    let snapshot: Vec<_> = {
        let images = state.images.lock().await;
        images
            .values()
            .filter(|img| {
                !img.path.to_string_lossy().starts_with("assembled://")
                    && !matches!(img.derived, Some(DerivedImage::ShadowCopy { .. }))
            })
            .map(|image| Arc::clone(image.disk_image()))
            .collect()
    };
    let options = raid::DetectOptions { degraded_sets };
    let progress_app = app.clone();
    let (mut stack, assembled) = tokio::task::spawn_blocking(move || {
        raid::detect_with_options_observed(&snapshot, &options, &|progress| {
            if let Err(error) = progress_app.emit("raid-detect-progress", progress) {
                tracing::warn!("RAID detection progress delivery failed: {error}");
            }
        })
    })
    .await
    .map_err(|err| AppError::task_panicked("raid_auto_detect", err))?;

    // Phase 1: snapshot the idempotency cache and the set of live image ids
    // WITHOUT holding either lock across the CPU-bound DiskImage::from_assembled
    // build below. Holding `assembled_arrays` + `images` together across
    // registration was a deadlock against `image_list` (which locks
    // images→assembled, the opposite order) and starved concurrent
    // image_load/unload on the heavy from_assembled call. Cloning a handful of
    // short ids here is negligible by comparison.
    let cache_snapshot: std::collections::HashMap<String, String> = {
        let cache = state.assembled_arrays.lock().await;
        cache.clone()
    };
    let live_image_ids: std::collections::HashSet<String> = {
        let images = state.images.lock().await;
        images.keys().cloned().collect()
    };
    // Evidence ids recorded for `assembled://` paths in the active case.
    // Re-registering an array in a fresh session must converge on the id the
    // case already knows — evidence entries and persisted volume snapshots
    // both key on image_id, so a fresh uuid per session would pile up
    // duplicate evidence rows and miss every snapshot (the same stability
    // contract `image_load` honours for real images). Known gap: a synthetic
    // registered while NO case was active keeps its session uuid on the
    // cache-hit path above, so opening a case afterwards records a second id
    // for the same path — mirroring the pre-existing behaviour for real
    // images loaded before a case is opened.
    let evidence_ids_by_path: std::collections::HashMap<String, String> = {
        let case = state.active_case.lock().await;
        case.as_ref()
            .map(|case| {
                case.evidence_refs()
                    .into_iter()
                    .map(|evidence| (evidence.path, evidence.image_id))
                    .collect()
            })
            .unwrap_or_default()
    };

    // Built synthetic images awaiting registration, paired with the device
    // they belong to. Construction (from_assembled) happens here, lock-free.
    struct PendingRegistration {
        assembled_index: usize,
        cache_key: String,
        image: DiskImage,
    }
    let mut pending: Vec<PendingRegistration> = Vec::new();
    // Ids already adopted by an earlier device in THIS loop — `live_image_ids`
    // is a pre-loop snapshot and can't see them.
    let mut adopted_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let report_registration = |current| {
        if let Err(error) = app.emit(
            "raid-detect-progress",
            raid::DetectProgress::new(
                raid::DetectStage::Registering,
                current,
                Some(assembled.len()),
            ),
        ) {
            tracing::warn!("RAID registration progress delivery failed: {error}");
        }
    };
    report_registration(0);
    for (assembled_index, materialised) in assembled.iter().enumerate() {
        // Keyed by label, not set UUID: a degraded and a complete assembly of
        // one set are different devices with different labels.
        let cache_key = materialised.label.clone();
        if let Some(existing) = cache_snapshot.get(&cache_key) {
            if live_image_ids.contains(existing) {
                record_assembled_image(&mut stack, materialised, existing.clone());
                report_registration(assembled_index + 1);
                continue;
            }
        }
        let sector_size = 512_usize;
        let partitions = match &materialised.partitions {
            Some(parts) => parts.clone(),
            None => vec![Partition {
                id: 0,
                kind: materialised.fs_kind,
                start_lba: 0,
                length_lba: materialised.size_bytes / sector_size as u64,
                label: Some(raid::synthetic_partition_label(materialised)),
                unique_guid: None,
            }],
        };
        // The label doubles as the synthetic image's `assembled://<label>`
        // path, which case evidence AND `image_load`'s by-path reuse key on;
        // core builds it injectively from the full set UUID or LV identity.
        let mut synthetic = DiskImage::from_assembled(
            materialised.label.clone(),
            materialised.source.clone(),
            partitions,
            sector_size,
        )
        .map_err(|err| {
            AppError::new(
                ErrorCode::Internal,
                format!("registering assembled array failed: {err:#}"),
            )
        })?;
        // Adopt the case-recorded id unless something live already owns it
        // or an earlier array in this loop claimed it (then the fresh uuid
        // stands — never overwrite a loaded image's slot).
        if let Some(stable_id) = evidence_ids_by_path.get(synthetic.path.to_string_lossy().as_ref())
        {
            if !live_image_ids.contains(stable_id) && adopted_ids.insert(stable_id.clone()) {
                synthetic.id = stable_id.clone();
            }
        }
        pending.push(PendingRegistration {
            assembled_index,
            cache_key,
            image: synthetic,
        });
        report_registration(assembled_index + 1);
    }

    // Phase 2: publish freshly built images. Lock images→assembled (the same
    // order `image_list` uses) and hold both only for cheap map inserts —
    // never across from_assembled. Re-check the cache under lock so two
    // concurrent detect calls converge on one synthetic image per device
    // instead of leaking duplicates.
    if !pending.is_empty() {
        let mut images = state.images.lock().await;
        let mut cache = state.assembled_arrays.lock().await;
        for PendingRegistration {
            assembled_index,
            cache_key,
            mut image,
        } in pending
        {
            let materialised = &assembled[assembled_index];
            if let Some(existing) = cache.get(&cache_key) {
                if images.contains_key(existing) {
                    record_assembled_image(&mut stack, materialised, existing.clone());
                    continue;
                }
                cache.remove(&cache_key);
            }
            // Re-validate the adopted id under the lock: a concurrent detect
            // or load may have registered it since the phase-1 snapshot.
            // Never overwrite a live slot — mint a fresh uuid instead.
            if images.contains_key(&image.id) {
                image.id = uuid::Uuid::new_v4().to_string();
            }
            let image_id = image.id.clone();
            images.insert(image_id.clone(), Arc::new(EvidenceSession::from(image)));
            cache.insert(cache_key, image_id.clone());
            record_assembled_image(&mut stack, materialised, image_id);
        }
    }

    // PVs and LV segments on assembled arrays name the arrays' images.
    raid::resolve_array_sources(&mut stack);
    if let Err(error) = app.emit(
        "raid-detect-progress",
        raid::DetectProgress::new(
            raid::DetectStage::Complete,
            assembled.len(),
            Some(assembled.len()),
        ),
    ) {
        tracing::warn!("RAID completion progress delivery failed: {error}");
    }
    Ok(stack)
}

/// Point the stack record a synthetic image was built from at that image.
fn record_assembled_image(
    stack: &mut RaidStack,
    assembled: &raid::AssembledArray,
    image_id: String,
) {
    match &assembled.target {
        raid::AssembledTarget::Array => {
            if let Some(array) = stack
                .arrays
                .iter_mut()
                .find(|array| array.set_uuid == assembled.set_uuid)
            {
                array.assembled_image_id = Some(image_id);
            }
        }
        raid::AssembledTarget::LogicalVolume { vg_uuid, lv_name } => {
            if let Some(lv) = stack
                .volume_groups
                .iter_mut()
                .filter(|vg| &vg.vg_uuid == vg_uuid)
                .flat_map(|vg| vg.logical_volumes.iter_mut())
                .find(|lv| &lv.name == lv_name)
            {
                lv.assembled_image_id = Some(image_id);
            }
        }
    }
}

/// Assemble a RAID device from an explicit, user-supplied geometry (the manual
/// "RAID builder" fallback for arrays the automatic heuristic cannot crack) and
/// register it as a synthetic image so the rest of the app can browse / export
/// it. Idempotent on the geometry: re-running with the same parameters reuses
/// the previously registered synthetic image.
pub async fn raid_manual_assemble(
    state: State<'_, AppState>,
    params: raid::ManualRaidParams,
) -> Result<ManualRaidResult, AppError> {
    let images: Vec<Arc<DiskImage>> = {
        let imgs = state.images.lock().await;
        imgs.values()
            .map(|image| Arc::clone(image.disk_image()))
            .collect()
    };
    let array = tokio::task::spawn_blocking(move || raid::manual_assemble(&images, &params))
        .await
        .map_err(|err| AppError::task_panicked("raid_manual_assemble", err))?
        .map_err(AppError::from)?;

    let partitions: Vec<ManualPartitionInfo> = array
        .partitions
        .as_ref()
        .map(|ps| {
            ps.iter()
                .map(|p| ManualPartitionInfo {
                    kind: raid::kind_label(p.kind).to_string(),
                    start_lba: p.start_lba,
                    length_lba: p.length_lba,
                    label: p.label.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    let image_id = register_assembled_image(&state, &array).await?;
    Ok(ManualRaidResult {
        image_id,
        size_bytes: array.size_bytes,
        partitions,
    })
}

/// Register an [`raid::AssembledArray`] as a synthetic `EvidenceSession`, reusing an
/// existing one with the same `assembled://` path when present. A streamlined
/// sibling of the `raid_auto_detect` registration (no case-evidence id
/// stability — manual builds are transient and user-initiated).
async fn register_assembled_image(
    state: &State<'_, AppState>,
    array: &raid::AssembledArray,
) -> Result<String, AppError> {
    let label = array.label.clone();
    let path_str = format!("assembled://{label}");
    {
        let images = state.images.lock().await;
        if let Some((id, _)) = images
            .iter()
            .find(|(_, im)| im.path.to_string_lossy() == path_str)
        {
            return Ok(id.clone());
        }
    }
    let length_lba = array.size_bytes / 512;
    let partitions = match &array.partitions {
        Some(parts) => parts.clone(),
        None => vec![Partition {
            id: 0,
            kind: array.fs_kind,
            start_lba: 0,
            length_lba,
            label: Some(raid::synthetic_partition_label(array)),
            unique_guid: None,
        }],
    };
    let synthetic = DiskImage::from_assembled(label, array.source.clone(), partitions, 512)
        .map_err(|err| {
            AppError::new(
                ErrorCode::Internal,
                format!("registering assembled array failed: {err:#}"),
            )
        })?;
    let id = synthetic.id.clone();
    let mut images = state.images.lock().await;
    images.insert(id.clone(), Arc::new(EvidenceSession::from(synthetic)));
    Ok(id)
}
