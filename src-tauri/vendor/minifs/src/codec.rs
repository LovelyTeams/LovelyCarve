//! LZMA-alone decoding with strict source EOF, EOS and output-length checks.
use anyhow::{ensure, Context, Result};
use std::io::{self, Read};

struct Input<'a, F> {
    remaining: &'a [u8],
    check: &'a mut F,
    consumed: usize,
}
impl<F: FnMut() -> Result<()>> Read for Input<'_, F> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.consumed >= 65536 {
            (self.check)().map_err(io::Error::other)?;
            self.consumed = 0;
        }
        let count = output.len().min(self.remaining.len());
        if count == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        output[..count].copy_from_slice(&self.remaining[..count]);
        self.remaining = &self.remaining[count..];
        self.consumed += count;
        Ok(count)
    }
}

pub(super) fn decode(
    bytes: &[u8],
    expected: usize,
    dictionary_limit: u32,
    check: &mut impl FnMut() -> Result<()>,
) -> Result<Vec<u8>> {
    let header = bytes.get(..13).context("truncated LZMA-alone header")?;
    let properties = header[0] as u32;
    ensure!(properties < 225, "invalid LZMA properties");
    let dictionary = u32::from_le_bytes(header[1..5].try_into()?);
    ensure!(
        dictionary.max(4096) <= dictionary_limit,
        "MINIFS LZMA dictionary limit"
    );
    let declared = u64::from_le_bytes(header[5..13].try_into()?);
    ensure!(
        declared == u64::MAX || declared == expected as u64,
        "MINIFS LZMA length mismatch"
    );
    let literal_context = properties % 9;
    let literal_position = properties / 9 % 5;
    let position = properties / 45;
    // Always decode through EOS, even with a known LZMA-alone output length.
    // The bounded pull source refuses implicit bytes beyond the compressed span.
    let input = Input {
        remaining: &bytes[13..],
        check: &mut *check,
        consumed: 65536,
    };
    let mut reader = lzma_rust2::LzmaReader::new(
        input,
        u64::MAX,
        literal_context,
        literal_position,
        position,
        dictionary.max(4096),
        None,
    )?;
    let mut output = Vec::with_capacity(expected);
    let mut buffer = [0; 65536];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        ensure!(
            output
                .len()
                .checked_add(count)
                .is_some_and(|length| length <= expected),
            "MINIFS decoded chunk exceeds declared length"
        );
        output.extend_from_slice(&buffer[..count]);
        (reader.inner_mut().check)()?;
    }
    ensure!(output.len() == expected, "MINIFS short decoded chunk");
    ensure!(
        reader.into_inner().remaining.is_empty(),
        "MINIFS trailing compressed bytes"
    );
    // The pull decoder's relaxed EOS handling does not verify the final range
    // code. Its bounded source proves that every byte really exists; the push
    // decoder additionally checks the range-code state. Require agreement.
    check()?;
    let mut decoder = lzma_rust2::LzmaStream::new(
        u64::MAX,
        literal_context,
        literal_position,
        position,
        dictionary.max(4096),
        None,
    )?;
    let mut verified = vec![
        0;
        expected
            .checked_add(1)
            .context("MINIFS verification length overflow")?
    ];
    let result = decoder.process(&bytes[13..], &mut verified, lzma_rust2::Action::Finish)?;
    ensure!(
        result.status == lzma_rust2::Status::StreamEnd
            && result.bytes_consumed == bytes.len() - 13
            && result.bytes_produced == expected
            && decoder.unused_input().is_empty()
            && verified[..expected] == output,
        "MINIFS invalid LZMA final state"
    );
    check()?;
    Ok(output)
}
