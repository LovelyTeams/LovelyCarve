//! Native callback dispatch and state ownership.
use super::*;

impl<'c, 'h: 'c> FileSystemHandler<'c, 'h> for LovelyCarveDokanFs {
    type Context = OpenContext;

    fn create_file(
        &'h self,
        file_name: &U16CStr,
        _security_context: &IO_SECURITY_CONTEXT,
        desired_access: ACCESS_MASK,
        _file_attributes: u32,
        _share_access: u32,
        create_disposition: u32,
        create_options: u32,
        _info: &mut OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<CreateFileInfo<Self::Context>> {
        if wants_write(desired_access, create_disposition, create_options) {
            return Err(STATUS_ACCESS_DENIED);
        }
        let node = self.resolve(file_name)?;
        if node.kind == NodeKind::Dir {
            if create_options & FILE_NON_DIRECTORY_FILE != 0 {
                return Err(STATUS_FILE_IS_A_DIRECTORY);
            }
            if !matches!(create_disposition, FILE_OPEN | FILE_OPEN_IF) {
                return Err(STATUS_INVALID_PARAMETER);
            }
        } else {
            if create_options & FILE_DIRECTORY_FILE != 0 {
                return Err(STATUS_NOT_A_DIRECTORY);
            }
            if create_disposition == FILE_CREATE {
                return Err(STATUS_OBJECT_NAME_COLLISION);
            }
        }
        Ok(CreateFileInfo {
            is_dir: node.kind == NodeKind::Dir,
            context: OpenContext {
                node,
                cache: Arc::new(Mutex::new(None)),
            },
            new_file_created: false,
        })
    }

    fn read_file(
        &'h self,
        _file_name: &U16CStr,
        offset: i64,
        buffer: &mut [u8],
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<u32> {
        if offset < 0 {
            return Err(STATUS_INVALID_PARAMETER);
        }
        if context.node.kind == NodeKind::Dir {
            return Err(STATUS_FILE_IS_A_DIRECTORY);
        }
        let offset = offset as u64;
        if offset >= context.node.size || buffer.is_empty() {
            return Ok(0);
        }
        read_node_range_cached(&self.image, &context.node, &context.cache, offset, buffer)
            .map_err(to_status)
    }

    fn write_file(
        &'h self,
        _file_name: &U16CStr,
        _offset: i64,
        _buffer: &[u8],
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<u32> {
        Err(STATUS_ACCESS_DENIED)
    }

    fn get_file_information(
        &'h self,
        _file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<FileInfo> {
        Ok(self.file_info(&context.node))
    }

    fn find_files(
        &'h self,
        file_name: &U16CStr,
        mut fill_find_data: impl FnMut(&FindData) -> dokan::FillDataResult,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        let path = normalize_dokan_path(&file_name.to_string_lossy());
        fill_self_entries(&path, &context.node, &mut fill_find_data)?;
        for child in self.children_for(&path, &context.node)? {
            let info = self.file_info(&child);
            let display = sanitize_for_windows(&child.name);
            let find = FindData {
                attributes: info.attributes,
                creation_time: info.creation_time,
                last_access_time: info.last_access_time,
                last_write_time: info.last_write_time,
                file_size: info.file_size,
                file_name: U16CString::from_str(display.as_ref())
                    .map_err(|_| STATUS_NO_SUCH_FILE)?,
            };
            fill_find_data(&find).map_err(Into::<NTSTATUS>::into)?;
        }
        Ok(())
    }

    fn set_file_attributes(
        &'h self,
        _file_name: &U16CStr,
        _file_attributes: u32,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Err(STATUS_ACCESS_DENIED)
    }

    fn set_file_time(
        &'h self,
        _file_name: &U16CStr,
        _creation_time: dokan::FileTimeOperation,
        _last_access_time: dokan::FileTimeOperation,
        _last_write_time: dokan::FileTimeOperation,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Err(STATUS_ACCESS_DENIED)
    }

    fn delete_file(
        &'h self,
        _file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Err(STATUS_ACCESS_DENIED)
    }

    fn delete_directory(
        &'h self,
        _file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Err(STATUS_ACCESS_DENIED)
    }

    fn move_file(
        &'h self,
        _file_name: &U16CStr,
        _new_file_name: &U16CStr,
        _replace_if_existing: bool,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Err(STATUS_ACCESS_DENIED)
    }

    fn set_end_of_file(
        &'h self,
        _file_name: &U16CStr,
        _offset: i64,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Err(STATUS_ACCESS_DENIED)
    }

    fn set_allocation_size(
        &'h self,
        _file_name: &U16CStr,
        _alloc_size: i64,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Err(STATUS_ACCESS_DENIED)
    }

    fn get_disk_free_space(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<DiskSpaceInfo> {
        // Mount Manager rejects volumes that report a 1-byte size — for
        // partition-backed mounts `root.size` is the partition byte count
        // and we land in the right ballpark, but the docker synthetic
        // root carries `size = 0` (it's a directory, not a block device).
        // Fall back to the underlying image's byte count so Windows sees
        // a believable capacity.
        let byte_count = if self.root.size > 0 {
            self.root.size
        } else {
            self.image.len().max(1)
        };
        Ok(DiskSpaceInfo {
            byte_count,
            free_byte_count: 0,
            available_byte_count: 0,
        })
    }

    fn get_volume_information(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<VolumeInfo> {
        Ok(VolumeInfo {
            name: U16CString::from_str(&self.volume_name).map_err(|_| STATUS_INVALID_PARAMETER)?,
            serial_number: stable_index(&self.root_node_id) as u32,
            max_component_length: 255,
            fs_flags: FILE_CASE_PRESERVED_NAMES | FILE_UNICODE_ON_DISK | FILE_READ_ONLY_VOLUME,
            fs_name: U16CString::from_str(&self.fs_name).map_err(|_| STATUS_INVALID_PARAMETER)?,
        })
    }

    fn mounted(
        &'h self,
        mount_point: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<()> {
        tracing::info!(
            "Dokan read-only mount ready at {}",
            mount_point.to_string_lossy()
        );
        Ok(())
    }

    fn unmounted(&'h self, _info: &OperationInfo<'c, 'h, Self>) -> OperationResult<()> {
        tracing::info!("Dokan read-only mount unmounted");
        Ok(())
    }

    fn flush_file_buffers(
        &'h self,
        _file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Ok(())
    }

    fn lock_file(
        &'h self,
        _file_name: &U16CStr,
        _offset: i64,
        _length: i64,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Ok(())
    }

    fn unlock_file(
        &'h self,
        _file_name: &U16CStr,
        _offset: i64,
        _length: i64,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Ok(())
    }

    fn set_file_security(
        &'h self,
        _file_name: &U16CStr,
        _security_information: u32,
        _security_descriptor: winapi::um::winnt::PSECURITY_DESCRIPTOR,
        _buffer_length: u32,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<()> {
        Err(STATUS_ACCESS_DENIED)
    }

    fn get_file_security(
        &'h self,
        _file_name: &U16CStr,
        _security_information: u32,
        security_descriptor: PSECURITY_DESCRIPTOR,
        buffer_length: u32,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) -> OperationResult<u32> {
        let descriptor_len = READONLY_SECURITY_DESCRIPTOR.len() as u32;
        if !security_descriptor.is_null() && buffer_length >= descriptor_len {
            unsafe {
                READONLY_SECURITY_DESCRIPTOR
                    .as_ptr()
                    .copy_to_nonoverlapping(
                        security_descriptor.cast::<u8>(),
                        READONLY_SECURITY_DESCRIPTOR.len(),
                    );
            }
        }
        Ok(descriptor_len)
    }

    fn find_streams(
        &'h self,
        _file_name: &U16CStr,
        mut fill_find_stream_data: impl FnMut(&dokan::FindStreamData) -> dokan::FillDataResult,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        // PowerShell `Get-Item -Stream *` and forensic tools expect to see
        // every $DATA attribute of an NTFS file (the unnamed default stream
        // plus every Alternate Data Stream). Non-NTFS filesystems don't have
        // streams, so we tell Windows the operation is unsupported and let
        // it skip the query rather than reporting an empty (and misleading)
        // list.
        if !matches!(self.fs_kind, Some(PartitionKind::Ntfs)) {
            return Err(STATUS_NOT_IMPLEMENTED);
        }
        if context.node.kind != NodeKind::File && context.node.kind != NodeKind::Dir {
            return Err(STATUS_INVALID_PARAMETER);
        }
        let streams = fs_tree::list_data_streams(&self.image, &context.node.id)
            .map_err(|err| {
                tracing::debug!("Dokan find_streams list_data_streams failed: {err:#}");
                err
            })
            .unwrap_or_default();
        for stream in streams {
            let label = if stream.name.is_empty() {
                "::$DATA".to_string()
            } else {
                format!(":{}:$DATA", stream.name)
            };
            let find = dokan::FindStreamData {
                size: stream.size.min(i64::MAX as u64) as i64,
                name: U16CString::from_str(label).map_err(|_| STATUS_INVALID_PARAMETER)?,
            };
            fill_find_stream_data(&find).map_err(Into::<NTSTATUS>::into)?;
        }
        Ok(())
    }
}

pub(super) fn wants_write(
    desired_access: ACCESS_MASK,
    create_disposition: u32,
    create_options: u32,
) -> bool {
    let write_bits = GENERIC_WRITE
        | GENERIC_ALL
        | FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | FILE_WRITE_EA
        | FILE_WRITE_ATTRIBUTES
        | DELETE;
    desired_access & write_bits != 0
        || matches!(
            create_disposition,
            FILE_SUPERSEDE | FILE_OVERWRITE | FILE_OVERWRITE_IF
        )
        || create_options & FILE_DELETE_ON_CLOSE != 0
}

pub(super) fn fill_self_entries(
    path: &str,
    node: &FsNode,
    fill_find_data: &mut impl FnMut(&FindData) -> dokan::FillDataResult,
) -> OperationResult<()> {
    let info = file_info_for_node(node);
    fill_find_data(&FindData {
        attributes: info.attributes,
        creation_time: info.creation_time,
        last_access_time: info.last_access_time,
        last_write_time: info.last_write_time,
        file_size: 0,
        file_name: U16CString::from_str(".").map_err(|_| STATUS_NO_SUCH_FILE)?,
    })
    .map_err(Into::<NTSTATUS>::into)?;

    if path != "\\" {
        fill_find_data(&FindData {
            attributes: info.attributes,
            creation_time: info.creation_time,
            last_access_time: info.last_access_time,
            last_write_time: info.last_write_time,
            file_size: 0,
            file_name: U16CString::from_str("..").map_err(|_| STATUS_NO_SUCH_FILE)?,
        })
        .map_err(Into::<NTSTATUS>::into)?;
    }
    Ok(())
}

pub(super) fn file_info_for_node(node: &FsNode) -> FileInfo {
    let attributes = match node.kind {
        NodeKind::Dir => FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_READONLY,
        NodeKind::File | NodeKind::Symlink => FILE_ATTRIBUTE_NORMAL | FILE_ATTRIBUTE_READONLY,
        NodeKind::Unallocated => FILE_ATTRIBUTE_READONLY,
    };
    let created = parse_time(node.timestamps.created.as_deref())
        .or_else(|| parse_time(node.timestamps.birth.as_deref()))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let accessed =
        parse_time(node.timestamps.accessed.as_deref()).unwrap_or(SystemTime::UNIX_EPOCH);
    let modified =
        parse_time(node.timestamps.modified.as_deref()).unwrap_or(SystemTime::UNIX_EPOCH);
    FileInfo {
        attributes,
        creation_time: created,
        last_access_time: accessed,
        last_write_time: modified,
        file_size: if node.kind == NodeKind::Dir {
            0
        } else {
            node.size
        },
        number_of_links: 1,
        file_index: node.inode.unwrap_or_else(|| stable_index(&node.id)),
    }
}

pub(super) fn parse_time(value: Option<&str>) -> Option<SystemTime> {
    let value = value?;
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| SystemTime::from(dt.with_timezone(&chrono::Utc)))
}

pub(super) fn stable_index(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub(super) fn to_status(err: anyhow::Error) -> NTSTATUS {
    tracing::warn!("Dokan filesystem operation failed: {err:#}");
    STATUS_ACCESS_DENIED
}
