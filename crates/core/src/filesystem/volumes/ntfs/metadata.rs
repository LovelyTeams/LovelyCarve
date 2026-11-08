//! Data stream enumeration and file metadata projection.
use super::*;

impl<'a> NtfsVolume<'a> {
    /// Enumerates every `$DATA` attribute on the given MFT record, returning
    /// one [`NtfsDataStream`] per stream — the unnamed default stream first,
    /// followed by every named alternate (ADS) sorted by name. The default
    /// stream is omitted if the record has no body (e.g. directories with no
    /// ADS); ADS-bearing directories surface their named streams here too.
    pub fn list_data_streams(&self, record_number: u64) -> Result<Vec<NtfsDataStream>> {
        let mut fs = self.reader();
        let file = self
            .ntfs
            .file(&mut fs, record_number)
            .map_err(|err| anyhow!("NTFS file record {record_number}: {err}"))?;
        let mut out = Vec::new();
        let mut seen_names = HashSet::<String>::new();
        let mut iter = file.attributes();
        while let Some(item) = iter.next(&mut fs) {
            let item = match item {
                Ok(item) => item,
                Err(err) => {
                    tracing::debug!("NTFS attribute walk skipped: {err}");
                    continue;
                }
            };
            let attribute = match item.to_attribute() {
                Ok(attr) => attr,
                Err(err) => {
                    tracing::debug!("NTFS to_attribute skipped: {err}");
                    continue;
                }
            };
            match attribute.ty() {
                Ok(NtfsAttributeType::Data) => {}
                Ok(_) => continue,
                Err(err) => {
                    tracing::debug!("NTFS attribute ty error: {err}");
                    continue;
                }
            }
            let name = match attribute.name() {
                Ok(n) => n.to_string_lossy(),
                Err(err) => {
                    tracing::debug!("NTFS attribute name error: {err}");
                    continue;
                }
            };
            // Non-resident attributes can be split across multiple records via
            // `$ATTRIBUTE_LIST`; the iterator yields each piece but
            // value_length() returns the full canonical size on every piece —
            // keep only the first occurrence of each name.
            if !seen_names.insert(name.clone()) {
                continue;
            }
            out.push(NtfsDataStream {
                name,
                size: attribute.value_length(),
                resident: attribute.is_resident(),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(idx) = out.iter().position(|s| s.name.is_empty()) {
            let default = out.remove(idx);
            out.insert(0, default);
        }
        Ok(out)
    }

    pub fn file_stat(&self, record_number: u64, node_id: &str) -> Result<FsStat> {
        let mut fs = self.reader();
        let file = self
            .ntfs
            .file(&mut fs, record_number)
            .map_err(|err| anyhow!("NTFS file record {record_number}: {err}"))?;
        let _info: Option<NtfsStandardInformation> = file.info().ok();

        let mut compression: Option<&'static str> = None;
        let (size, sample) = if file.is_directory() {
            (0, Vec::new())
        } else if let Some(data_result) = file.data(&mut fs, "") {
            let data_item = data_result.map_err(|err| anyhow!("NTFS $DATA attribute: {err}"))?;
            let data_attribute = data_item
                .to_attribute()
                .map_err(|err| anyhow!("NTFS $DATA to_attribute: {err}"))?;
            let len = data_attribute.value_length();
            let flags = data_attribute.flags();
            if flags.contains(NtfsAttributeFlags::ENCRYPTED) {
                compression = Some("encrypted (EFS)");
                (len, Vec::new())
            } else {
                let mut value = super::logical_stream::LogicalStream::new(
                    &data_attribute,
                    &file,
                    &mut fs,
                    self.ntfs.cluster_size(),
                    record_number,
                )?;
                let want = len.min(SAMPLE_BYTES) as usize;
                let mut sample = vec![0u8; want];
                let mut filled = 0;
                while filled < sample.len() {
                    let read = value
                        .read(&mut fs, &mut sample[filled..])
                        .map_err(|err| anyhow!("NTFS stat read: {err}"))?;
                    if read == 0 {
                        break;
                    }
                    filled += read;
                }
                sample.truncate(filled);
                (len, sample)
            }
        } else {
            (0, Vec::new())
        };

        let mime = match compression {
            Some(label) => label.to_string(),
            None => detect_mime_bytes(&sample).to_string(),
        };

        Ok(FsStat {
            node_id: node_id.to_string(),
            size,
            inode: Some(record_number),
            mode: None,
            mime,
            magic_hex: if sample.is_empty() {
                None
            } else {
                Some(
                    sample
                        .iter()
                        .take(16)
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            },
            entropy: if sample.is_empty() {
                None
            } else {
                Some(shannon_entropy(&sample))
            },
            data_runs: Vec::<FsDataRun>::new(),
        })
    }

    /// Stat a named ADS of `record_number` as if it were a standalone file —
    /// used by the ADS pseudo-file (`ntfs-ads:` id). Mirrors [`Self::file_stat`]
    /// but samples MIME / magic / entropy from the named stream's bytes and
    /// sizes from the stream. `data_runs` is empty (matching the default-stream
    /// `file_stat` path, which also leaves them empty). LZNT1 streams are sampled
    /// after decoding; EFS streams keep their encryption label and no sample.
    pub fn named_stream_stat(
        &self,
        record_number: u64,
        stream_name: &str,
        node_id: &str,
    ) -> Result<FsStat> {
        if stream_name.is_empty() {
            anyhow::bail!("named-stream stat requires a non-empty stream name");
        }
        let mut fs = self.reader();
        let file = self
            .ntfs
            .file(&mut fs, record_number)
            .map_err(|err| anyhow!("NTFS file record {record_number}: {err}"))?;
        let data_item = file
            .data(&mut fs, stream_name)
            .ok_or_else(|| missing_stream_error(record_number, stream_name))?
            .map_err(|err| anyhow!("NTFS $DATA attribute: {err}"))?;
        let data_attribute = data_item
            .to_attribute()
            .map_err(|err| anyhow!("NTFS $DATA to_attribute: {err}"))?;
        let len = data_attribute.value_length();
        let flags = data_attribute.flags();

        let mut compression: Option<&'static str> = None;
        let sample = if flags.contains(NtfsAttributeFlags::ENCRYPTED) {
            compression = Some("encrypted (EFS)");
            Vec::new()
        } else {
            let mut value = super::logical_stream::LogicalStream::new(
                &data_attribute,
                &file,
                &mut fs,
                self.ntfs.cluster_size(),
                record_number,
            )?;
            let want = len.min(SAMPLE_BYTES) as usize;
            let mut sample = vec![0u8; want];
            let mut filled = 0;
            while filled < sample.len() {
                let read = value
                    .read(&mut fs, &mut sample[filled..])
                    .map_err(|err| anyhow!("NTFS stat read: {err}"))?;
                if read == 0 {
                    break;
                }
                filled += read;
            }
            sample.truncate(filled);
            sample
        };

        let mime = match compression {
            Some(label) => label.to_string(),
            None => detect_mime_bytes(&sample).to_string(),
        };

        Ok(FsStat {
            node_id: node_id.to_string(),
            size: len,
            inode: Some(record_number),
            mode: None,
            mime,
            magic_hex: if sample.is_empty() {
                None
            } else {
                Some(
                    sample
                        .iter()
                        .take(16)
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            },
            entropy: if sample.is_empty() {
                None
            } else {
                Some(shannon_entropy(&sample))
            },
            data_runs: Vec::<FsDataRun>::new(),
        })
    }
}
