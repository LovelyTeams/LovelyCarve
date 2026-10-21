//! Evidence container recognition and source opening.
use super::*;
impl ImageSource {
    #[cfg(test)]
    pub(in crate::core::evidence::image) fn open(path: &Path) -> anyhow::Result<Self> {
        Self::open_observed(
            path,
            &noop_observer,
            EvidenceCache::Disabled,
            &tokio_util::sync::CancellationToken::new(),
        )
    }

    pub(in crate::core::evidence::image) fn open_observed(
        path: &Path,
        progress: OpenObserver<'_>,
        cache: EvidenceCache<'_>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<Self> {
        if path.is_dir()
            && (lower_extension(path).as_deref() == Some("hdd")
                || path.join("DiskDescriptor.xml").is_file())
        {
            let reader = phdi::PhdiImage::open(path)
                .with_context(|| format!("failed to open Parallels bundle: {}", path.display()))?;
            return Ok(Self::Phdi { reader });
        }
        // Folder selection: when the user picks a directory, treat it as a
        // logical-evidence source. Has to come before everything else so we
        // don't try to mmap a directory handle.
        if path.is_dir() {
            let folder = FolderImage::open(path)?;
            let size_bytes = folder.total_size();
            return Ok(Self::Folder {
                image: Arc::new(folder),
                size_bytes,
            });
        }

        if lower_extension(path).as_deref() == Some("cue") {
            let image = crate::core::evidence::readers::cue::CueImage::open(path, cancel)?;
            return Ok(Self::Cue { image });
        }

        // Raw block devices (Windows `\\.\PhysicalDriveN`, Linux `/dev/sdX`)
        // need positioned reads against an open handle — mmap doesn't work
        // on Windows physical drives and is unreliable on Linux block
        // devices. Detect the path shape and route through the Block source
        // before falling into the regular file probe.
        if physical_disk::is_block_device_path(path) {
            let (file, size_bytes, sector_size) = physical_disk::open_block_device(path)?;
            return Ok(Self::Block {
                reader: FairMutex::new(file),
                size_bytes,
                sector_size,
                path: path.to_path_buf(),
            });
        }

        let descriptor_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("DiskDescriptor.xml")
                    || name.eq_ignore_ascii_case("DiskDescriptor.xml.Backup")
            });
        if descriptor_name || matches!(lower_extension(path).as_deref(), Some("hds" | "hdd")) {
            if !descriptor_name
                && path
                    .parent()
                    .is_some_and(|parent| parent.join("DiskDescriptor.xml").is_file())
            {
                anyhow::bail!(
                    "请打开 Parallels DiskDescriptor.xml 或 .hdd 目录，以保留分段与差分父链：{}",
                    path.display()
                );
            }
            let reader = phdi::PhdiImage::open(path)
                .with_context(|| format!("failed to open Parallels image: {}", path.display()))?;
            return Ok(Self::Phdi { reader });
        }

        // Docker / OCI image archives and plain filesystem tarballs both ship
        // as `.tar`, and indexing either costs one header-only
        // (`entries_with_seek`) pass. Build the plain `TarImage` first, then
        // detect docker markers (`index.json` / `manifest.json` at the archive
        // root) directly from its index — handing off to the layer-composing
        // `DockerImage` only when they are present. A phone / filesystem
        // extraction (the common case, often tens of GiB) thus pays for ONE
        // scan instead of two; previously the docker probe walked every header
        // and bailed, then `TarImage` walked them all again. Real docker
        // archives reuse that same mmap and member index while composing their
        // layers, so neither path performs a redundant second outer-tar scan.
        // Gated on the `.tar` extension to leave the RAW / EWF / VMDK fast path
        // untouched.
        if lower_extension(path).as_deref() == Some("tar") {
            match TarImage::open(path) {
                Ok(tar) => {
                    let looks_docker =
                        tar.lookup("index.json").is_some() || tar.lookup("manifest.json").is_some();
                    if looks_docker {
                        match DockerImage::from_tar(tar) {
                            Ok(image) => {
                                return Ok(Self::Docker {
                                    image: Arc::new(image),
                                });
                            }
                            Err(error) => {
                                let (tar, docker_err) = *error;
                                // Markers present but layer composition failed
                                // (corrupt / partial / unsupported image) — fall
                                // back to the already-built plain tar tree, not RAW.
                                tracing::debug!(
                                    "docker markers present in {} but compose failed ({docker_err:#}); using plain tar",
                                    path.display()
                                );
                                return Ok(Self::Tar {
                                    image: Arc::new(tar),
                                });
                            }
                        }
                    }
                    return Ok(Self::Tar {
                        image: Arc::new(tar),
                    });
                }
                Err(tar_err) => {
                    tracing::debug!(
                        "tar probe: {} is not a usable tar archive ({tar_err:#}); falling back to RAW",
                        path.display()
                    );
                }
            }
        }

        if is_tar_gz_path(path) {
            let file = std::fs::File::open(path)?;
            let total = file.metadata()?.len();
            let label = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let report = |done| {
                progress(OpenProgress::Decompressing {
                    label: label.clone(),
                    done,
                    total,
                })
            };
            let reader = crate::core::evidence::readers::load_reader::LoadReader {
                input: file,
                cancel,
                progress: &report,
                consumed: 0,
            };
            let mut decoder = gzipf::GzipDecoder::new(reader);
            match TarImage::open_from_tar_reader(path.to_path_buf(), None, &mut decoder) {
                Ok(tar) => {
                    return Ok(Self::Tar {
                        image: Arc::new(tar),
                    });
                }
                Err(tar_err) => {
                    anyhow::ensure!(!cancel.is_cancelled(), "image load cancelled");
                    tracing::debug!(
                        "tar.gz probe: {} is not a usable gzip-compressed tar ({tar_err:#}); falling back to RAW",
                        path.display()
                    );
                }
            }
        }

        // `.zip` archives are treated as logical-evidence file trees, exactly
        // like `.tar`. Gated on the extension (not a `PK\x03\x04` signature
        // probe) so OOXML / jar / apk / epub — all valid ZIPs — are NOT hijacked
        // away from their own native-type viewers; only files the user named
        // `.zip` open as zip evidence. A `.zip`-named file that is not a usable
        // archive falls back to the RAW probe.
        if lower_extension(path).as_deref() == Some("zip") {
            match ZipImage::open_cancellable(
                path,
                cache,
                &|label, done, total| {
                    progress(OpenProgress::Decompressing {
                        label: label.to_owned(),
                        done,
                        total,
                    });
                },
                cancel,
            ) {
                Ok(zip) => {
                    return Ok(Self::Zip {
                        image: Arc::new(zip),
                    });
                }
                Err(zip_err) => {
                    anyhow::ensure!(!cancel.is_cancelled(), "image load cancelled");
                    tracing::debug!(
                        "zip probe: {} is not a usable zip archive ({zip_err:#}); falling back to RAW",
                        path.display()
                    );
                }
            }
        }

        if lower_extension(path).as_deref() == Some("7z") {
            match SevenzImage::open(path) {
                Ok(sevenz) => {
                    return Ok(Self::Sevenz {
                        image: Arc::new(sevenz),
                    });
                }
                Err(sevenz_err) => {
                    tracing::debug!(
                        "7z probe: {} is not a usable 7z archive ({sevenz_err:#}); falling back to RAW",
                        path.display()
                    );
                }
            }
        }

        // AccessData AD1 logical images open by their first segment. A file
        // named `.ad1` that is not AD1 at all falls back to RAW; a real AD1
        // that cannot be read (encrypted, segments missing, corrupt) is an
        // error rather than an opaque raw blob.
        if lower_extension(path).as_deref() == Some("ad1") {
            match Ad1Image::open(path) {
                Ok(ad1) => {
                    return Ok(Self::Ad1 {
                        image: Arc::new(ad1),
                    });
                }
                Err(ad1_err) if ad1_image::is_not_ad1(&ad1_err) => {
                    tracing::debug!(
                        "ad1 probe: {} is not an AD1 image ({ad1_err:#}); falling back to RAW",
                        path.display()
                    );
                }
                Err(ad1_err) => return Err(ad1_err),
            }
        } else if is_later_ad1_segment(path)? {
            anyhow::bail!(
                "这是 AD1 镜像的后续分段，请打开第一个分段（.ad1）：{}",
                path.display()
            );
        }

        if should_open_as_ewf(path)? {
            let reader = EwfHandle::open(path)
                .with_context(|| format!("failed to open EWF image: {}", path.display()))?;
            return Ok(Self::Ewf {
                size_bytes: reader.media_size(),
                sector_size: reader.bytes_per_sector(),
                reader: Box::new(FairMutex::new(reader)),
            });
        }

        // RAW has no signature. Recognize numbered families before probing
        // embedded guest bytes as another container, while preserving EWF's
        // explicit extension/signature priority. Require the first member so
        // case identity cannot vary with the selected member.
        let raw_segments = smraw::discover(path)
            .with_context(|| format!("failed to discover RAW segments: {}", path.display()))?;
        if raw_segments.numbered {
            let first = raw_segments
                .paths
                .first()
                .context("RAW segment list is empty")?;
            if crate::core::casework::case::evidence_path_key(
                &std::fs::canonicalize(first)?.to_string_lossy(),
            ) != crate::core::casework::case::evidence_path_key(
                &std::fs::canonicalize(path)?.to_string_lossy(),
            ) {
                anyhow::bail!(
                    "这是 RAW 镜像的后续分段，请打开第一个分段：{}",
                    first.display()
                );
            }
            let reader = smraw::RawImage::open_segments(raw_segments.paths)
                .with_context(|| format!("failed to open split RAW image: {}", path.display()))?;
            return Ok(Self::SplitRaw { reader });
        }

        // Preserve EWF and numbered RAW priority before interpreting guest
        // bytes as a container signature (a RAW member can begin with HDS).
        let mut signature = [0; 16];
        let sparse = File::open(path)?.read(&mut signature)? == signature.len()
            && phdi::is_sparse_signature(&signature);
        if sparse {
            if path
                .parent()
                .is_some_and(|parent| parent.join("DiskDescriptor.xml").is_file())
            {
                anyhow::bail!(
                    "请打开 Parallels DiskDescriptor.xml 或 .hdd 目录，以保留分段与差分父链：{}",
                    path.display()
                );
            }
            let reader = phdi::PhdiImage::open(path)
                .with_context(|| format!("failed to open Parallels image: {}", path.display()))?;
            return Ok(Self::Phdi { reader });
        }

        if should_open_as_pmfx(path)? {
            let reader = PmfxImage::open(path)
                .with_context(|| format!("failed to open PMFX image: {}", path.display()))?;
            return Ok(Self::Pmfx {
                size_bytes: reader.media_size(),
                sector_size: reader.sector_size(),
                reader: FairMutex::new(reader),
            });
        }

        let extension = lower_extension(path);
        if matches!(extension.as_deref(), Some("qcow" | "qcow2")) || has_qcow2_signature(path)? {
            let reader = qcow2::Qcow2::open_path(path)
                .with_context(|| format!("failed to parse QCOW image: {}", path.display()))?;
            let size_bytes = reader.guest_size();
            return Ok(Self::Qcow2 { reader, size_bytes });
        }

        if extension.as_deref() == Some("vdi") || has_vdi_signature(path)? {
            let file = File::open(path)
                .with_context(|| format!("failed to open VDI image: {}", path.display()))?;
            let reader = vdi::VdiDisk::open(file)
                .with_context(|| format!("failed to parse VDI image: {}", path.display()))?;
            let size_bytes = reader.disk_size();
            let sector_size = usize::try_from(reader.sector_size())
                .context("VDI sector size does not fit this platform")?;
            return Ok(Self::Vdi {
                reader,
                size_bytes,
                sector_size,
            });
        }

        if extension.as_deref() == Some("vmdk") {
            let vmdk_path = companion_descriptor_for_flat_extent(path).unwrap_or_else(|| {
                if is_flat_vmdk_extent_path(path) {
                    tracing::info!(
                        "VMDK flat extent {} has no matching descriptor; opening as a raw sector stream",
                        path.display()
                    );
                }
                path.to_path_buf()
            });

            if is_flat_vmdk_extent_path(path) && vmdk_path == path {
                return open_raw_mmap(path, ImageFormat::Vmdk);
            }

            match VmdkHandle::open(&vmdk_path) {
                Ok(reader) => {
                    return Ok(Self::Vmdk {
                        size_bytes: reader.media_size(),
                        reader: FairMutex::new(reader),
                    });
                }
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("failed to open VMDK image: {}", path.display()));
                }
            }
        }

        let is_vhdx = extension.as_deref() == Some("vhdx") || has_vhdx_signature(path)?;
        if matches!(extension.as_deref(), Some("vhd" | "vhdx"))
            || is_vhdx
            || has_vhd_signature(path)?
        {
            let (format, reader) = if is_vhdx {
                (ImageFormat::Vhdx, VhdHandle::open_vhdx(path)?)
            } else {
                (ImageFormat::Vhd, VhdHandle::open_vhd(path)?)
            };
            return Ok(Self::VhdFamily {
                size_bytes: reader.media_size(),
                sector_size: reader.bytes_per_sector(),
                reader: FairMutex::new(reader),
                format,
            });
        }
        // Signature-based DMG detection: real UDIF containers always carry
        // a `koly` magic 512 bytes before EOF. The `.dmg` extension is not a
        // reliable proxy — Apple's first-generation DiskCopy images and
        // some macOS install images ship a `.dmg` extension over what is
        // really a raw HFS+/HFSX partition. Probing the trailer keeps those
        // raw images on the regular RAW path so partition detection picks
        // up the HFS+ volume header at offset 0x400 directly.
        if is_dmg_path(path)? {
            return open_dmg(path);
        }
        let file = File::open(path)
            .with_context(|| format!("failed to open image: {}", path.display()))?;
        let mmap = unsafe { Mmap::map(&file)? };
        let is_android_backup =
            mmap.starts_with(b"ANDROID BACKUP\n") || mmap.starts_with(b"MIUI BACKUP\n");
        if extension.as_deref() == Some("ab") && !is_android_backup {
            anyhow::bail!("invalid Android Backup magic: {}", path.display());
        }
        if is_android_backup {
            return Ok(Self::Raw {
                size_bytes: mmap.len() as u64,
                mmap,
                format: ImageFormat::AndroidBackup,
            });
        }
        if mmap.get(..4)
            == Some(crate::core::evidence::partitions::xiaomi_firmware::XIAOMI_FIRMWARE_MAGIC)
        {
            return Ok(Self::Raw {
                size_bytes: mmap.len() as u64,
                mmap,
                format: ImageFormat::XiaomiFirmware,
            });
        }

        Ok(Self::Raw {
            size_bytes: mmap.len() as u64,
            mmap,
            format: ImageFormat::Raw,
        })
    }
}
