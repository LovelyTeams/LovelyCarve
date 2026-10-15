//! Cooperative cancellation and input-byte progress for archive materialisation.
use std::io::{self, Read};
use tokio_util::sync::CancellationToken;

pub(crate) struct LoadReader<'a, R> {
    pub input: R,
    pub cancel: &'a CancellationToken,
    pub progress: &'a dyn Fn(u64),
    pub consumed: u64,
}

impl<R: Read> Read for LoadReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            // Interrupted is retried automatically by several decoder adapters.
            return Err(io::Error::other("image load cancelled"));
        }
        let read = self.input.read(buffer)?;
        self.consumed = self.consumed.saturating_add(read as u64);
        (self.progress)(self.consumed);
        Ok(read)
    }
}
