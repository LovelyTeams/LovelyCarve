//! open implementation.
use super::*;

impl DiskImage {
    /// Create an evidence identity for browsing verified, persisted metadata only.
    /// This deliberately skips geometry detection and cannot supply source bytes.
    pub fn from_index_metadata(
        id: String,
        path: PathBuf,
        format: ImageFormat,
        size_bytes: u64,
    ) -> anyhow::Result<Self> {
        Self::from_source(
            id,
            path,
            ImageSource::Indexed { format, size_bytes },
            &|_| {},
        )
    }

    pub fn is_index_only(&self) -> bool {
        matches!(self.source, ImageSource::Indexed { .. })
    }

    pub fn require_source(&self) -> anyhow::Result<()> {
        if self.is_index_only() {
            anyhow::bail!(INDEX_ONLY_READ_ERROR);
        }
        Ok(())
    }
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::open_with_id(uuid::Uuid::new_v4().to_string(), path)
    }

    pub fn open_with_id(id: String, path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::open_with_id_observed(id, path, &noop_observer)
    }

    /// Like [`Self::open_with_id`], but reports coarse [`OpenProgress`] to
    /// `progress` so a caller (the `image_load` command) can surface load phases
    /// to the UI. Runs synchronously on the caller's thread (the command runs it
    /// under `spawn_blocking`), so `progress` is invoked inline.
    pub fn open_with_id_observed(
        id: String,
        path: impl AsRef<Path>,
        progress: OpenObserver<'_>,
    ) -> anyhow::Result<Self> {
        Self::open_with_id_observed_qcow_key(id, path, progress, None)
    }

    /// Open a legacy AES-encrypted QCOW disk. The source format has no password
    /// verifier; decrypted bytes require independent validation by the examiner.
    pub fn open_with_qcow_key(
        path: impl AsRef<Path>,
        key: qcow2::EncryptionKey,
    ) -> anyhow::Result<Self> {
        Self::open_with_id_observed_qcow_key(
            uuid::Uuid::new_v4().to_string(),
            path,
            &noop_observer,
            Some(key),
        )
    }

    pub(crate) fn open_with_id_observed_qcow_key(
        id: String,
        path: impl AsRef<Path>,
        progress: OpenObserver<'_>,
        qcow_key: Option<qcow2::EncryptionKey>,
    ) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        progress(OpenProgress::Opening);
        if let Some(key) = qcow_key {
            let reader = qcow2::Qcow2::open_path_with_key(&path, key).with_context(|| {
                format!("failed to open encrypted QCOW image: {}", path.display())
            })?;
            let size_bytes = reader.guest_size();
            return Self::from_source(
                id,
                path,
                ImageSource::Qcow2 { reader, size_bytes },
                progress,
            );
        }
        // LDPlayer `.ldbk` backups are 7z containers of VMDK disks, not a single
        // block device — decompress + assemble them on a dedicated path that
        // builds partitions per-VMDK rather than scanning one byte stream.
        if lower_extension(&path).as_deref() == Some("ldbk") {
            return Self::open_ldplayer(id, path, progress);
        }
        let source = ImageSource::open(&path)?;
        Self::from_source(id, path, source, progress)
    }

    /// Construct a session-only logical view of an unlocked iOS backup.
    pub fn unlock_ios(
        &self,
        password: ios_backup_core::Password,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<Self> {
        let folder = self.folder.as_ref().context("evidence is not a folder")?;
        if !folder.ios_info().is_some_and(|info| info.locked) {
            anyhow::bail!("evidence is not a locked iOS backup");
        }
        let folder = Arc::new(FolderImage::open_ios(folder.root_path(), password, cancel)?);
        let source = ImageSource::Folder {
            size_bytes: folder.total_size(),
            image: folder,
        };
        let id = uuid::Uuid::new_v4().to_string();
        let mut image = Self::from_source(
            id.clone(),
            PathBuf::from(format!("decrypted://ios/{}/{id}", self.id)),
            source,
            &noop_observer,
        )?;
        image.derived = Some(DerivedImage::Decrypted {
            parent: self.id.clone(),
        });
        Ok(image)
    }

    fn from_source(
        id: String,
        path: PathBuf,
        source: ImageSource,
        progress: OpenObserver<'_>,
    ) -> anyhow::Result<Self> {
        let block_cache = source
            .is_ffi_backed()
            .then(|| SharedBlockCache::new(configured_block_cache_slot_count()));
        let bad_regions = Mutex::new(Vec::new());

        let mut sector_size = source.sector_size();
        // Docker / OCI archives have no partition table — the outer tar is
        // just a container of layer blobs. Plain tarballs, zip archives, and
        // folder sources are similarly partitionless (the archive / OS
        // filesystem already provides the tree). For all of them, skip partition
        // detection and surface the appropriate merged-view through
        // `core::filesystem::tree::fs_tree` instead.
        let (partitions, docker, folder, tar, zip, sevenz, table_scheme, disk_id) = match &source {
            ImageSource::Raw {
                mmap,
                format: ImageFormat::AndroidBackup,
                ..
            } => {
                let label = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                let tar = crate::core::evidence::readers::mi_bak::open_bak_archive_from_reader(
                    std::io::Cursor::new(&mmap[..]),
                    &path,
                    label,
                )
                .with_context(|| format!("open Android backup: {}", path.display()))?;
                (
                    Vec::new(),
                    None,
                    None,
                    Some(Arc::new(tar)),
                    None,
                    None,
                    None,
                    None,
                )
            }
            ImageSource::Docker { image } => (
                Vec::new(),
                Some(Arc::clone(image)),
                None,
                None,
                None,
                None,
                None,
                None,
            ),
            ImageSource::Folder { image, .. } => (
                Vec::new(),
                None,
                Some(Arc::clone(image)),
                None,
                None,
                None,
                None,
                None,
            ),
            ImageSource::Tar { image } => (
                Vec::new(),
                None,
                None,
                Some(Arc::clone(image)),
                None,
                None,
                None,
                None,
            ),
            ImageSource::Zip { image } => (
                Vec::new(),
                None,
                None,
                None,
                Some(Arc::clone(image)),
                None,
                None,
                None,
            ),
            ImageSource::Sevenz { image } => (
                Vec::new(),
                None,
                None,
                None,
                None,
                Some(Arc::clone(image)),
                None,
                None,
            ),
            // Kept in `ad1` below.
            ImageSource::Indexed { .. } | ImageSource::Ad1 { .. } => {
                (Vec::new(), None, None, None, None, None, None, None)
            }
            _ => {
                let probe_source = OpenSectorSource {
                    source: &source,
                    cache: block_cache.as_ref(),
                    bad_regions: &bad_regions,
                };
                progress(OpenProgress::DetectingPartitions);
                let can_infer_geometry = matches!(
                    &source,
                    ImageSource::Raw { .. }
                        | ImageSource::Dmg { .. }
                        | ImageSource::Vmdk { .. }
                        | ImageSource::Qcow2 { .. }
                        | ImageSource::Assembled { .. }
                        | ImageSource::Ldplayer { .. }
                );
                if sector_size == 512 && can_infer_geometry {
                    let inferred = partitions::gpt::infer_sector_size(&probe_source, sector_size)?;
                    if inferred != sector_size {
                        tracing::info!(
                            sector_size = inferred,
                            "adopting CRC-validated GPT geometry"
                        );
                        sector_size = inferred;
                    }
                }
                let mut detected =
                    partitions::detect_partitions_with_meta(&probe_source, sector_size)?;
                if sector_size == 512
                    && matches!(detected.scheme, Some(TableScheme::Mbr) | None)
                    && can_infer_geometry
                {
                    let inferred = partitions::mbr::infer_sector_size(&probe_source, sector_size)?;
                    if inferred != sector_size {
                        tracing::info!(
                            sector_size = inferred,
                            "adopting MBR geometry validated by EBR"
                        );
                        sector_size = inferred;
                        detected =
                            partitions::detect_partitions_with_meta(&probe_source, sector_size)?;
                    }
                }
                // 4Kn fallback (P1-14): mmap-backed Raw/DMG/VMDK report no real
                // sector size, so they default to 512. If a 512-sector scan
                // finds nothing but the image is actually 4Kn, the GPT/MBR/BSD
                // tables sit at 4096-relative LBAs and were missed — retry at
                // 4096 and adopt it only when that genuinely yields a table.
                if detected.partitions.is_empty()
                    && sector_size == 512
                    && can_infer_geometry
                    && detected.scheme != Some(TableScheme::Gpt)
                {
                    if let Ok(retry) = partitions::detect_partitions_with_meta(&probe_source, 4096)
                    {
                        if !retry.partitions.is_empty() {
                            tracing::info!(
                                "partition scan empty at 512-byte sectors; adopting 4096-byte (4Kn) geometry"
                            );
                            sector_size = 4096;
                            detected = retry;
                        }
                    }
                }
                let partitions::DetectedTable {
                    mut partitions,
                    scheme,
                    disk_id,
                } = detected;
                progress(OpenProgress::DetectingFilesystems {
                    done: 0,
                    total: partitions.len(),
                });
                partitions::refine_partition_kinds(&probe_source, &mut partitions, sector_size)?;
                progress(OpenProgress::DetectingFilesystems {
                    done: partitions.len(),
                    total: partitions.len(),
                });
                (partitions, None, None, None, None, None, scheme, disk_id)
            }
        };
        let ad1 = match &source {
            ImageSource::Ad1 { image } => Some(Arc::clone(image)),
            _ => None,
        };

        let (on_disk_bytes, modified_unix_ms, created_unix_ms) = source_file_meta(&path, &source);
        let ewf_acquiry = match &source {
            ImageSource::Ewf { reader, .. } => Some(reader.lock().acquiry_info().clone()),
            _ => None,
        };

        Ok(Self {
            id,
            path,
            source,
            sector_size,
            partitions,
            docker,
            folder,
            tar,
            zip,
            sevenz,
            ad1,
            unlocked_bde: Mutex::new(HashMap::new()),
            block_cache,
            bad_regions,
            table_scheme,
            disk_id,
            on_disk_bytes,
            modified_unix_ms,
            created_unix_ms,
            ewf_acquiry,
            derived: None,
        })
    }

    /// Build a `DiskImage` from an LDPlayer `.ldbk` backup. The heavy lifting
    /// (7z decompression, native VMDK de-sparsing, per-VMDK partition detection)
    /// lives in `core::evidence::readers::ldplayer_image`; here we just wrap the prepared concat
    /// source + partitions into the standard `DiskImage` shape so every
    /// downstream view (fs tree, hex, search, report) treats it like any other
    /// multi-partition disk. Runs on the `spawn_blocking` open thread, so the
    /// synchronous decompression does not stall the async runtime.
    pub(super) fn open_ldplayer(
        id: String,
        path: PathBuf,
        progress: OpenObserver<'_>,
    ) -> anyhow::Result<Self> {
        let prepared = crate::core::evidence::readers::ldplayer_image::open(&path, progress)
            .with_context(|| format!("failed to open LDPlayer backup: {}", path.display()))?;
        let crate::core::evidence::readers::ldplayer_image::PreparedLdplayer {
            source,
            partitions,
            sector_size,
            size_bytes,
        } = prepared;
        let source = ImageSource::Ldplayer { source, size_bytes };
        let (on_disk_bytes, modified_unix_ms, created_unix_ms) = source_file_meta(&path, &source);
        Ok(Self {
            id,
            path,
            source,
            sector_size,
            partitions,
            docker: None,
            folder: None,
            tar: None,
            zip: None,
            sevenz: None,
            ad1: None,
            unlocked_bde: Mutex::new(HashMap::new()),
            block_cache: None,
            bad_regions: Mutex::new(Vec::new()),
            table_scheme: None,
            disk_id: None,
            on_disk_bytes,
            modified_unix_ms,
            created_unix_ms,
            ewf_acquiry: None,
            derived: None,
        })
    }

    /// Build a `DiskImage` whose backing storage is an arbitrary
    /// `SectorSource` constructed at runtime — e.g. an mdadm RAID0 array
    /// assembled across LVM LVs that themselves span multiple physical
    /// images. The caller is responsible for supplying a sensible
    /// `partitions` vec; for the typical case of a whole-device filesystem
    /// (Btrfs on RAID0), pass a single `Partition { id: 0, kind: ...,
    /// start_lba: 0, length_lba: size_bytes / sector_size }`.
    ///
    /// `path` is set to `assembled://<label>` so the frontend has a stable
    /// human-readable identifier; `id` is a fresh UUID.
    pub fn from_assembled(
        label: String,
        source: Arc<dyn SectorSource + Send + Sync>,
        partitions: Vec<Partition>,
        sector_size: usize,
    ) -> anyhow::Result<Self> {
        let size_bytes = source.size();
        let path = PathBuf::from(format!("assembled://{label}"));
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            path,
            source: ImageSource::Assembled {
                source,
                size_bytes,
                label,
            },
            sector_size,
            partitions,
            docker: None,
            folder: None,
            tar: None,
            zip: None,
            sevenz: None,
            ad1: None,
            unlocked_bde: Mutex::new(HashMap::new()),
            block_cache: None,
            bad_regions: Mutex::new(Vec::new()),
            table_scheme: None,
            disk_id: None,
            on_disk_bytes: None,
            modified_unix_ms: None,
            created_unix_ms: None,
            ewf_acquiry: None,
            derived: None,
        })
    }
}
