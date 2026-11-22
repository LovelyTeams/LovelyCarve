//! Private, authenticated loopback transport. A request carries the parsed
//! command itself; byte results stay binary, in bounded chunks that keep
//! large reads out of JSON and out of a single buffer.
use std::io::{BufRead, Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::{cmd::Command, exec::RequestOptions, CliError, Json, Origin, Output};

pub(super) const CHUNK: usize = 8 * 1024 * 1024;
pub(super) const VERSION: u32 = 5;
const MAX_REQUEST: u64 = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Request {
    pub token: String,
    pub work: Work,
    /// Send progress events while the command runs.
    #[serde(default)]
    pub progress: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(super) enum Work {
    Run {
        command: Box<Command>,
        options: RequestOptions,
    },
    Info,
    Close,
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
        return Err(CliError::invalid("后台进程请求超过 16 MiB"));
    }
    if length == 0 {
        return Err(CliError::invalid("后台进程请求为空"));
    }
    Ok(serde_json::from_str(&line)?)
}

pub(super) fn header(writer: &mut impl Write, header: &Header) -> Result<(), CliError> {
    serde_json::to_writer(&mut *writer, header)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

pub(super) fn chunk(writer: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    let length = u32::try_from(bytes.len()).map_err(|_| std::io::Error::other("数据块过大"))?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(bytes)
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

/// Send a JSON, byte or empty result; streamed results are written by the
/// server, which holds the evidence they come from.
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
        Ok(Output::Stream(_)) => header(
            writer,
            &Header::Error {
                error: crate::cli::internal_error("流式结果未由服务端写出").into(),
            },
        )?,
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
            "caseBackendDisconnected",
            "案件后台进程的连接已中断；命令可能已执行，请检查状态后再决定是否重试",
        ));
    }
    Ok(serde_json::from_str(&line)?)
}

/// Read a response: JSON is returned, raw bytes are copied to `sink`, and
/// progress events go to `reporter`.
pub(super) fn receive(
    reader: &mut impl BufRead,
    sink: &mut impl Write,
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
                    return Err(CliError::invalid(
                        "后台进程的 JSON 结果超过 512 MiB，请分页读取",
                    ));
                }
                let mut data = String::new();
                if reader.take(bytes).read_to_string(&mut data)? as u64 != bytes {
                    return Err(CliError::new(
                        "caseBackendDisconnected",
                        "后台进程的 JSON 结果未完整传输",
                    ));
                }
                return Ok(Output::Json(Json::from_text(data)));
            }
            Header::Done => return Ok(Output::Written),
            Header::Error { error } => return Err(error.into()),
            Header::Stream => {
                copy_stream(reader, sink)?;
                return Ok(Output::Written);
            }
        }
    }
}

fn copy_stream(reader: &mut impl BufRead, writer: &mut impl Write) -> Result<(), CliError> {
    loop {
        let mut length = [0; 4];
        reader.read_exact(&mut length)?;
        let length = u32::from_le_bytes(length) as usize;
        if length == 0 {
            break;
        }
        if length > CHUNK {
            return Err(CliError::invalid("后台进程的数据块超过 8 MiB"));
        }
        let mut bytes = vec![0; length];
        reader.read_exact(&mut bytes)?;
        writer.write_all(&bytes)?;
    }
    match read_header(reader)? {
        Header::Done => {
            writer.flush()?;
            Ok(())
        }
        Header::Error { error } => Err(error.into()),
        Header::Json { .. } | Header::Stream | Header::Event { .. } => {
            Err(CliError::invalid("后台进程的数据流结束标记无效"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_streams_preserve_boundaries_and_report_partial_failure() {
        let mut wire = Vec::new();
        chunk(&mut wire, &[0xe4, 0xb8]).unwrap();
        chunk(&mut wire, &[0xad, 0xff]).unwrap();
        end_stream(&mut wire, Ok(())).unwrap();
        let mut bytes = Vec::new();
        copy_stream(&mut wire.as_slice(), &mut bytes).unwrap();
        assert_eq!(bytes, [0xe4, 0xb8, 0xad, 0xff]);
        wire.clear();
        chunk(&mut wire, b"partial").unwrap();
        end_stream(&mut wire, Err(CliError::invalid("source short read"))).unwrap();
        assert!(copy_stream(&mut wire.as_slice(), &mut Vec::new()).is_err());
        wire.truncate(3);
        assert!(copy_stream(&mut wire.as_slice(), &mut Vec::new()).is_err());
    }
}
