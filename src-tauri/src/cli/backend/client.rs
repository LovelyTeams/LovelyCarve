//! Talking to a case backend without ever starting one implicitly.
use std::{
    io::{BufReader, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    time::Duration,
};

use serde_json::Value;

use super::{
    protocol::{self, Request, Work},
    registry,
};
use crate::cli::{progress::Reporter, CliError, Output};

/// Send `work` to the backend of `case_dir`. Raw bytes go to `sink`,
/// progress events to `reporter`.
pub(super) fn exchange(
    case_dir: &str,
    work: Work,
    reporter: Option<&Reporter>,
    sink: &mut impl Write,
) -> Result<Output, CliError> {
    let descriptor = match registry::read(case_dir) {
        Ok(Some(descriptor)) => descriptor,
        Ok(None) => {
            return Err(CliError::new(
                "caseBackendNotRunning",
                format!("案件 {case_dir} 没有运行中的后台进程；用 case open 启动"),
            ))
        }
        // An unreadable or incompatible record a dead backend left behind.
        Err(error) if registry::clear_stale(case_dir)? => {
            tracing::debug!("stale case backend record {case_dir}: {}", error.message);
            return Err(registry::exited(case_dir));
        }
        Err(error) => return Err(error),
    };
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, descriptor.port));
    let mut stream = match TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
        Ok(stream) => stream,
        Err(_) if registry::clear_stale(case_dir)? => return Err(registry::exited(case_dir)),
        Err(error) => {
            return Err(CliError::new(
                "caseBackendUnavailable",
                format!(
                    "案件后台进程仍在运行但没有应答（{error}）：可能还在打开案件，或已无响应；日志见 {}",
                    registry::log_path(case_dir)?.display()
                ),
            ))
        }
    };
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    if matches!(work, Work::Info) {
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    }
    serde_json::to_writer(
        &mut stream,
        &Request {
            token: descriptor.token,
            work,
            progress: reporter.is_some(),
        },
    )?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    protocol::receive(&mut BufReader::new(stream), sink, reporter)
}

/// The running backend's state.
pub(super) fn info(case_dir: &str) -> Result<Value, CliError> {
    match exchange(case_dir, Work::Info, None, &mut std::io::sink())? {
        Output::Json(value) => Ok(value.value().clone()),
        _ => Err(crate::cli::internal_error("后台进程状态返回了非 JSON 结果")),
    }
}
