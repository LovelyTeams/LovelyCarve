use crate::{header, input::Input, invalid, Member, Options};
use crc32fast::Hasher;
use flate2::{Decompress, FlushDecompress, Status};
use std::io::{self, BufRead, Read, Seek};

struct ActiveMember {
    metadata: Member,
    inflater: Decompress,
    checksum: Hasher,
}

/// Streaming concatenated-member reader. Only trailer-validated members are
/// published through `members()`. An error permanently poisons this decoder.
/// Read to EOF or call `finish()` before declaring a complete successful read.
pub struct GzipDecoder<R> {
    pub(crate) input: Input<R>,
    active: Option<ActiveMember>,
    pub(crate) members: Vec<Member>,
    metadata_bytes: usize,
    total_output: u64,
    finished: bool,
    failed: bool,
}

impl<R: Read> GzipDecoder<R> {
    pub fn new(reader: R) -> Self {
        Self::with_options(reader, Options::default())
    }

    pub fn with_options(reader: R, options: Options) -> Self {
        Self {
            input: Input::new(reader, options),
            active: None,
            members: Vec::new(),
            metadata_bytes: 0,
            total_output: 0,
            finished: false,
            failed: false,
        }
    }

    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// Total actual output so far, independent of 32-bit trailer ISIZE values.
    pub fn uncompressed_size(&self) -> u64 {
        self.total_output
    }

    pub fn compressed_offset(&self) -> u64 {
        self.input.offset
    }

    /// True only after an EOF read has validated all trailers and the file end.
    pub fn is_complete(&self) -> bool {
        self.finished && !self.failed
    }

    pub fn has_failed(&self) -> bool {
        self.failed
    }

    /// Drain unread content with a fixed-size buffer and validate the full file.
    pub fn finish(&mut self) -> io::Result<()> {
        let mut buffer = [0; 64 * 1024];
        while self.read(&mut buffer)? != 0 {}
        Ok(())
    }

    fn read_inner(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.input.options.check_cancelled()?;
        if self.failed {
            return Err(invalid(
                "GZIP decoder cannot continue after an earlier error",
            ));
        }
        if self.finished {
            return Ok(0);
        }
        loop {
            self.input.options.check_cancelled()?;
            if self.active.is_none() {
                if self.input.at_end()? {
                    if self.members.is_empty() {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "GZIP file contains no members",
                        ));
                    }
                    self.finished = true;
                    return Ok(0);
                }
                if self.members.len() >= self.input.options.max_members {
                    return Err(invalid("GZIP member count limit exceeded"));
                }
                let remaining = self
                    .input
                    .options
                    .max_metadata_bytes
                    .checked_sub(self.metadata_bytes)
                    .ok_or_else(|| invalid("GZIP metadata limit exceeded"))?;
                let (metadata, bytes) =
                    header::read(&mut self.input, self.total_output, remaining)?;
                self.metadata_bytes += bytes;
                self.active = Some(ActiveMember {
                    metadata,
                    inflater: Decompress::new(false),
                    checksum: Hasher::new(),
                });
            }

            let active = self
                .active
                .as_mut()
                .ok_or_else(|| invalid("missing active GZIP member"))?;
            let input_before = active.inflater.total_in();
            let output_before = active.inflater.total_out();
            let available = match self.input.reader.fill_buf() {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            // Cap work per call, so cancellation latency does not depend on a
            // caller passing a very large destination buffer.
            let output_length = output.len().min(64 * 1024);
            let status = active
                .inflater
                .decompress(
                    available,
                    &mut output[..output_length],
                    FlushDecompress::None,
                )
                .map_err(|error| {
                    invalid(format!(
                        "invalid DEFLATE in GZIP member at source offset {}: {error}",
                        active.metadata.header_offset
                    ))
                })?;
            let consumed = usize::try_from(active.inflater.total_in() - input_before)
                .map_err(|_| invalid("GZIP compressed count overflow"))?;
            let produced = usize::try_from(active.inflater.total_out() - output_before)
                .map_err(|_| invalid("GZIP output count overflow"))?;
            self.input.consume(consumed)?;
            self.total_output = self
                .total_output
                .checked_add(produced as u64)
                .ok_or_else(|| invalid("GZIP uncompressed size overflow"))?;
            if self.total_output > self.input.options.max_uncompressed_bytes {
                return Err(invalid("GZIP uncompressed byte limit exceeded"));
            }
            active.checksum.update(&output[..produced]);
            if status == Status::StreamEnd {
                let mut active = self
                    .active
                    .take()
                    .ok_or_else(|| invalid("missing completed GZIP member"))?;
                let metadata = &mut active.metadata;
                metadata.trailer_offset = self.input.offset;
                metadata.compressed_data_size =
                    metadata.trailer_offset - metadata.compressed_data_offset;
                metadata.uncompressed_size = active.inflater.total_out();
                let mut trailer = [0; 8];
                self.input.exact(&mut trailer)?;
                metadata.end_offset = self.input.offset;
                metadata.crc32 =
                    u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
                metadata.isize =
                    u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
                if metadata.crc32 != active.checksum.finalize() {
                    return Err(invalid(format!(
                        "GZIP data CRC mismatch at source offset {}",
                        metadata.trailer_offset
                    )));
                }
                if metadata.isize != metadata.uncompressed_size as u32 {
                    return Err(invalid(format!(
                        "GZIP ISIZE mismatch at source offset {}",
                        metadata.trailer_offset + 4
                    )));
                }
                self.members.try_reserve(1).map_err(io::Error::other)?;
                self.members.push(active.metadata);
            } else if consumed == 0 && produced == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!(
                        "truncated or stalled DEFLATE at source offset {}",
                        self.input.offset
                    ),
                ));
            }
            if produced != 0 {
                return Ok(produced);
            }
        }
    }
}

impl<R: Read> Read for GzipDecoder<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let result = self.read_inner(output);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

impl<R: Read + Seek> GzipDecoder<R> {
    pub(crate) fn restart(&mut self, source_offset: u64, logical_offset: u64) -> io::Result<()> {
        if self.failed {
            return Err(invalid("GZIP decoder cannot seek after an earlier error"));
        }
        self.input.seek(source_offset)?;
        self.active = None;
        self.members.clear();
        self.metadata_bytes = 0;
        self.total_output = logical_offset;
        self.finished = false;
        Ok(())
    }
}
