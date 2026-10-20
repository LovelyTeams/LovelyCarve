//! Ordinary argv commands forwarded to a running backend without starting one.
use std::{
    io::{BufReader, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    time::Duration,
};

use serde_json::{json, Map, Value};

use super::{protocol, registry};
use crate::cli::{
    argv::{Globals, Invocation},
    output,
    progress::{Options, Reporter},
    prompt, CliError, Output,
};

pub(super) fn exchange(
    name: &str,
    command: &str,
    args: Map<String, Value>,
    force: bool,
    dry_run: bool,
    text: bool,
) -> Result<Output, CliError> {
    exchange_observed(
        name,
        command,
        args,
        Exchange {
            timezone: None,
            force,
            dry_run,
            text,
            initial_image: None,
        },
        None,
    )
}

struct Exchange {
    timezone: Option<String>,
    force: bool,
    dry_run: bool,
    text: bool,
    initial_image: Option<String>,
}
fn exchange_observed(
    name: &str,
    command: &str,
    args: Map<String, Value>,
    options: Exchange,
    reporter: Option<&Reporter>,
) -> Result<Output, CliError> {
    let descriptor = registry::read(name)?;
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, descriptor.port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .map_err(|error| CliError::new("sessionUnavailable", format!("无法连接会话 {name}：{error}；用 sessions get {name} 检查，或 sessions open {name} 重新启动")))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    if command == "@info" {
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    }
    serde_json::to_writer(
        &mut stream,
        &protocol::Request {
            timezone: options.timezone,
            token: descriptor.token,
            command: command.into(),
            args,
            force: options.force,
            dry_run: options.dry_run,
            progress: reporter.is_some(),
            initial_image: options.initial_image,
        },
    )?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    if let Some(reporter) = reporter {
        protocol::receive_observed(&mut BufReader::new(stream), options.text, Some(reporter))
    } else {
        protocol::receive(&mut BufReader::new(stream), options.text)
    }
}

pub(super) fn info(name: &str) -> Result<Value, CliError> {
    match exchange(name, "@info", Map::new(), false, false, false)? {
        Output::Json(value) => Ok(value.value().clone()),
        Output::Bytes(_) | Output::Written => {
            Err(crate::cli::internal_error("会话状态返回非 JSON 结果"))
        }
    }
}

pub(in crate::cli) fn forward(
    name: &str,
    globals: &Globals,
    mut invocation: Invocation,
) -> Result<i32, CliError> {
    let fields = crate::cli::projection::parse(invocation.fields.as_deref())?;
    if let Some(zone) = globals.timezone.as_deref() {
        crate::cli::timezone::validate(zone)?;
    }
    if globals.case.is_some() {
        return Err(CliError::invalid(
            "连接常驻会话时请用 cases open 切换案件；--case 仅用于 sessions open 的首次启动",
        ));
    }
    if invocation.command.native.as_deref().is_some_and(|native| {
        matches!(native, "session" | "shell" | "complete") || native.starts_with("sessions-")
    }) {
        return Err(CliError::invalid(
            "连接常驻会话请用 sessions attach <会话名>，不能嵌套会话",
        ));
    }
    if let (Some(image), Some(parameter)) = (
        &globals.image,
        invocation
            .command
            .params
            .iter()
            .find(|parameter| parameter.scope),
    ) {
        invocation
            .values
            .entry(parameter.name.clone())
            .or_insert(json!(image));
    }
    for field in &invocation.ask {
        let parameter = invocation
            .command
            .params
            .iter()
            .find(|parameter| &parameter.name == field)
            .ok_or_else(|| CliError::invalid(format!("未知参数：{field}")))?;
        let value = prompt::required_field(&parameter.option, &parameter.description, true)?;
        invocation.values.insert(field.clone(), json!(value));
    }
    let reporter = Reporter::new(Options {
        quiet: globals.quiet,
        events: globals.events,
    });
    let mut monitor = reporter.start(&invocation.command.path());
    let initial_image = (invocation.command.ipc_command.as_deref() == Some("raid_auto_detect"))
        .then(|| globals.image.clone())
        .flatten();
    let result = exchange_observed(
        name,
        &invocation.command.path(),
        invocation.values,
        Exchange {
            timezone: globals.timezone.clone(),
            force: invocation.force,
            dry_run: invocation.dry_run,
            text: invocation.text,
            initial_image,
        },
        Some(&reporter),
    );
    monitor.finish(&result);
    let result = crate::cli::projection::apply(result?, &fields)?;
    let (_, done) = invocation.command.category.labels();
    output::print_output(
        result,
        done,
        globals.quiet || globals.events,
        invocation.text,
    )?;
    Ok(0)
}
