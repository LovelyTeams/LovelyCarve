//! Private, authenticated loopback transport. Byte results stay binary and
//! bounded chunks keep large file reads out of JSON and out of a single buffer.
use std::io::{BufRead, Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::cli::{CliError, Json, Origin, Output};

pub(super) const CHUNK: usize = 8 * 1024 * 1024;
pub(super) const VERSION: u32 = 2;
const MAX_REQUEST: u64 = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Request {
    pub token: String,
    pub command: String,
    #[serde(default)]
    pub args: Map<String, Value>,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub progress: bool,
    #[serde(default)]
    pub initial_image: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(super) enum Header {
    Event { event: crate::cli::progress::Event },
    Json { bytes: u64 },
    Stream,
    Done,
    Error { error: Failure },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Failure {
    code: String,
    message: String,
    details: Option<Value>,
    exit_code: i32,
    backend: bool,
    operation: Option<String>,
}

impl From<CliError> for Failure {
    fn from(error: CliError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            details: error.details,
            exit_code: error.exit_code,
            backend: matches!(error.origin, Origin::Backend(_)),
            operation: match error.origin {
                Origin::Backend(operation) => operation,
                Origin::Cli => None,
            },
        }
    }
}

impl From<Failure> for CliError {
    fn from(error: Failure) -> Self {
        Self {
            code: error.code,
            message: error.message,
            details: error.details,
            exit_code: error.exit_code,
            reported: false,
            origin: if error.backend {
                Origin::Backend(error.operation)
            } else {
                Origin::Cli
            },
        }
    }
}

pub(super) fn read_request(reader: &mut impl BufRead) -> Result<Request, CliError> {
    let mut line = String::new();
    let length = reader.take(MAX_REQUEST + 1).read_line(&mut line)?;
    if length as u64 > MAX_REQUEST {
        return Err(CliError::invalid("会话请求超过 16 MiB"));
    }
    if length == 0 {
        return Err(CliError::invalid("会话请求为空"));
    }
    Ok(serde_json::from_str(&line)?)
}

pub(super) fn header(writer: &mut impl Write, header: &Header) -> Result<(), CliError> {
    serde_json::to_writer(&mut *writer, header)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

pub(super) fn chunk(writer: &mut impl Write, bytes: &[u8]) -> Result<(), CliError> {
    let length = u32::try_from(bytes.len()).map_err(|_| CliError::invalid("会话数据块过大"))?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(bytes)?;
    Ok(())
}

pub(super) fn end_stream(
    writer: &mut impl Write,
    result: Result<(), CliError>,
) -> Result<(), CliError> {
    chunk(writer, &[])?;
    header(
        writer,
        &match result {
            Ok(()) => Header::Done,
            Err(error) => Header::Error {
                error: error.into(),
            },
        },
    )
}

pub(super) fn send(
    writer: &mut impl Write,
    output: Result<Output, CliError>,
) -> Result<(), CliError> {
    match output {
        Ok(Output::Json(json)) => {
            let text = json.text();
            header(
                writer,
                &Header::Json {
                    bytes: text.len() as u64,
                },
            )?;
            writer.write_all(text.as_bytes())?;
            writer.flush()?;
        }
        Ok(Output::Bytes(bytes)) => {
            header(writer, &Header::Stream)?;
            for bytes in bytes.chunks(CHUNK) {
                chunk(writer, bytes)?;
            }
            end_stream(writer, Ok(()))?;
        }
        Ok(Output::Written) => header(writer, &Header::Done)?,
        Err(error) => header(
            writer,
            &Header::Error {
                error: error.into(),
            },
        )?,
    }
    Ok(())
}

fn read_header(reader: &mut impl BufRead) -> Result<Header, CliError> {
    let mut line = String::new();
    let length = reader.take(1024 * 1024).read_line(&mut line)?;
    if length == 0 || !line.ends_with('\n') {
        return Err(CliError::new(
            "sessionDisconnected",
            "常驻会话连接已中断；命令可能已执行，请检查会话状态后再决定是否重试",
        ));
    }
    Ok(serde_json::from_str(&line)?)
}

pub(super) fn receive(reader: &mut impl BufRead, text: bool) -> Result<Output, CliError> {
    receive_observed(reader, text, None)
}

pub(super) fn receive_observed(
    reader: &mut impl BufRead,
    text: bool,
    reporter: Option<&crate::cli::progress::Reporter>,
) -> Result<Output, CliError> {
    loop {
        match read_header(reader)? {
            Header::Event { event } => {
                if let Some(reporter) = reporter {
                    reporter
                        .event(&event.event, event.payload)
                        .map_err(crate::cli::internal_error)?;
                }
            }
            Header::Json { bytes } => {
                if bytes > 512 * 1024 * 1024 {
                    return Err(CliError::invalid("会话 JSON 结果超过 512 MiB，请分页读取"));
                }
                let mut data = String::new();
                if reader.take(bytes).read_to_string(&mut data)? as u64 != bytes {
                    return Err(CliError::new(
                        "sessionDisconnected",
                        "会话 JSON 结果未完整传输",
                    ));
                }
                return Ok(Output::Json(Json {
                    text: Some(data),
                    value: std::sync::OnceLock::new(),
                }));
            }
            Header::Done => return Ok(Output::Written),
            Header::Error { error } => return Err(error.into()),
            Header::Stream => {
                copy_stream(reader, &mut std::io::stdout().lock(), text)?;
                return Ok(Output::Written);
            }
        }
    }
}

fn copy_stream(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    text: bool,
) -> Result<(), CliError> {
    let mut decoder = crate::cli::native::Utf8Stream::default();
    loop {
        let mut length = [0; 4];
        reader.read_exact(&mut length)?;
        let length = u32::from_le_bytes(length) as usize;
        if length == 0 {
            break;
        }
        if length > CHUNK {
            return Err(CliError::invalid("会话数据块超过 8 MiB"));
        }
        let mut bytes = vec![0; length];
        reader.read_exact(&mut bytes)?;
        if text {
            let mut decoded = String::new();
            decoder.push(&bytes, &mut decoded);
            writer.write_all(decoded.as_bytes())?;
        } else {
            writer.write_all(&bytes)?;
        }
    }
    match read_header(reader)? {
        Header::Done => {
            let mut tail = String::new();
            decoder.finish(&mut tail);
            writer.write_all(tail.as_bytes())?;
            writer.flush()?;
            Ok(())
        }
        Header::Error { error } => Err(error.into()),
        Header::Json { .. } | Header::Stream | Header::Event { .. } => {
            Err(CliError::invalid("会话流结束标记无效"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_and_text_streams_preserve_boundaries_and_report_partial_failure() {
        let mut wire = Vec::new();
        chunk(&mut wire, &[0xe4, 0xb8]).unwrap();
        chunk(&mut wire, &[0xad, 0xff]).unwrap();
        end_stream(&mut wire, Ok(())).unwrap();
        let mut bytes = Vec::new();
        copy_stream(&mut wire.as_slice(), &mut bytes, false).unwrap();
        assert_eq!(bytes, [0xe4, 0xb8, 0xad, 0xff]);
        let mut text = Vec::new();
        copy_stream(&mut wire.as_slice(), &mut text, true).unwrap();
        assert_eq!(text, "中�".as_bytes());
        wire.clear();
        chunk(&mut wire, b"partial").unwrap();
        end_stream(&mut wire, Err(CliError::invalid("source short read"))).unwrap();
        assert!(copy_stream(&mut wire.as_slice(), &mut Vec::new(), false).is_err());
        wire.truncate(3);
        assert!(copy_stream(&mut wire.as_slice(), &mut Vec::new(), false).is_err());
    }
}
