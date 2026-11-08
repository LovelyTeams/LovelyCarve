//! Require the zlib end marker and Adler-32 before reporting EOF.
//! A short compressed backup must never publish a seemingly complete TAR tree.

use std::io::{self, BufRead, Read};

use flate2::{Decompress, FlushDecompress, Status};

pub(super) struct ValidatedZlib<R> {
    input: R,
    decoder: Decompress,
    finished: bool,
}

impl<R: BufRead> ValidatedZlib<R> {
    pub(super) fn new(input: R) -> Self {
        Self {
            input,
            decoder: Decompress::new(true),
            finished: false,
        }
    }
}

impl<R: BufRead> Read for ValidatedZlib<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.finished {
            return Ok(0);
        }
        loop {
            let input = self.input.fill_buf()?;
            let eof = input.is_empty();
            let before_in = self.decoder.total_in();
            let before_out = self.decoder.total_out();
            let status = self
                .decoder
                .decompress(
                    input,
                    output,
                    if eof {
                        FlushDecompress::Finish
                    } else {
                        FlushDecompress::None
                    },
                )
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let consumed = (self.decoder.total_in() - before_in) as usize;
            let produced = (self.decoder.total_out() - before_out) as usize;
            self.input.consume(consumed);
            self.finished = status == Status::StreamEnd;
            if produced > 0 || self.finished {
                return Ok(produced);
            }
            if consumed == 0 {
                return Err(io::Error::new(
                    if eof {
                        io::ErrorKind::UnexpectedEof
                    } else {
                        io::ErrorKind::InvalidData
                    },
                    if eof {
                        "truncated Android Backup zlib stream"
                    } else {
                        "stalled Android Backup zlib stream"
                    },
                ));
            }
        }
    }
}

#[cfg(test)]
#[path = "zlib/tests.rs"]
mod tests;
