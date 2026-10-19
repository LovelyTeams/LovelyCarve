//! Plugin → host calls: the `run.*`, `file.*` and `view.*` methods of a run
//! (routed to its [`RunContext`] by the id they carry) and the frozen
//! `source.*` methods filesystem plugins read raw bytes with.
use lovelycarve_plugin::protocol::{
    self, method, EmitBatch, FileHandleId, FileRead, FileReadv, LeaseId, RunProgress, RunWarn,
    ViewFind, ViewList, ViewPath,
};

use super::*;

/// Serves one plugin → host call on a blocking worker.
pub(super) fn dispatch_host_request(
    name: &str,
    params: Value,
    services: &HostRpcServices,
) -> Result<Value> {
    match name {
        method::RUN_EMIT => {
            let batch: EmitBatch = decode(params)?;
            encode(&services.run(&batch.run_id)?.emit(batch)?)
        }
        method::RUN_WARN => {
            let warning: RunWarn = decode(params)?;
            services.run(&warning.run_id)?.warn(warning)?;
            Ok(Value::Nil)
        }
        method::RUN_PROGRESS => {
            let progress: RunProgress = decode(params)?;
            services.run(&progress.run_id)?.progress(progress)?;
            Ok(Value::Nil)
        }
        method::FILE_READ => {
            let read: FileRead = decode(params)?;
            Ok(Value::Binary(services.run(&read.file_id)?.read(&read)?))
        }
        method::FILE_READV => {
            let read: FileReadv = decode(params)?;
            Ok(Value::Array(
                services
                    .run(&read.file_id)?
                    .readv(&read)?
                    .into_iter()
                    .map(Value::Binary)
                    .collect(),
            ))
        }
        method::FILE_MATERIALIZE => {
            let handle: FileHandleId = decode(params)?;
            encode(
                &services
                    .run(&handle.file_id)?
                    .materialize(&handle.file_id)?,
            )
        }
        method::FILE_RELEASE => {
            let lease: LeaseId = decode(params)?;
            services.run(&lease.lease_id)?.release(&lease.lease_id)?;
            Ok(Value::Nil)
        }
        method::VIEW_STAT => {
            let request: ViewPath = decode(params)?;
            encode(&services.run(&request.view_id)?.view_stat(&request)?)
        }
        method::VIEW_LIST => {
            let request: ViewList = decode(params)?;
            encode(&services.run(&request.view_id)?.view_list(&request)?)
        }
        method::VIEW_OPEN => {
            let request: ViewPath = decode(params)?;
            encode(&services.run(&request.view_id)?.view_open(&request)?)
        }
        method::VIEW_FIND => {
            let request: ViewFind = decode(params)?;
            encode(&services.run(&request.view_id)?.view_find(&request)?)
        }
        "source.info" => {
            let source_id = string_param(&params, "source_id")?;
            let info = services.sources.info(source_id)?;
            Ok(Value::Map(vec![
                (Value::from("source_id"), Value::from(info.source_id)),
                (Value::from("size"), Value::from(info.size)),
                (Value::from("sector_size"), Value::from(info.sector_size)),
                (Value::from("read_only"), Value::from(info.read_only)),
            ]))
        }
        "source.read" => {
            let source_id = string_param(&params, "source_id")?;
            let offset = u64_param(&params, "offset")?;
            let length = usize::try_from(u64_param(&params, "length")?)
                .map_err(|_| invalid_params("length does not fit this host"))?;
            Ok(Value::Binary(
                services.sources.read(source_id, offset, length)?,
            ))
        }
        "source.readv" => {
            let source_id = string_param(&params, "source_id")?;
            let ranges = ranges_param(&params)?;
            Ok(Value::Array(
                services
                    .sources
                    .readv(source_id, &ranges)?
                    .into_iter()
                    .map(Value::Binary)
                    .collect(),
            ))
        }
        _ => Err(PluginError::Rpc {
            code: "METHOD_NOT_FOUND".to_owned(),
            message: format!("host method {name} is not available"),
        }),
    }
}

fn decode<T: for<'de> serde::Deserialize<'de>>(params: Value) -> Result<T> {
    protocol::from_value(params).map_err(|error| PluginError::Rpc {
        code: "INVALID_PARAMS".to_owned(),
        message: error.message,
    })
}

fn encode<T: serde::Serialize>(payload: &T) -> Result<Value> {
    protocol::to_value(payload).map_err(|error| PluginError::Rpc {
        code: "HOST_ERROR".to_owned(),
        message: error.message,
    })
}

fn map_param<'a>(params: &'a Value, name: &str) -> Result<&'a Value> {
    let Value::Map(entries) = params else {
        return Err(invalid_params("params must be a map"));
    };
    entries
        .iter()
        .find_map(|(key, value)| (key.as_str() == Some(name)).then_some(value))
        .ok_or_else(|| invalid_params(&format!("missing {name}")))
}

fn string_param<'a>(params: &'a Value, name: &str) -> Result<&'a str> {
    map_param(params, name)?
        .as_str()
        .ok_or_else(|| invalid_params(&format!("{name} must be a string")))
}

fn u64_param(params: &Value, name: &str) -> Result<u64> {
    map_param(params, name)?
        .as_u64()
        .ok_or_else(|| invalid_params(&format!("{name} must be an unsigned integer")))
}

fn ranges_param(params: &Value) -> Result<Vec<(u64, usize)>> {
    let Value::Array(values) = map_param(params, "ranges")? else {
        return Err(invalid_params("ranges must be an array"));
    };
    values
        .iter()
        .map(|value| {
            let Value::Array(range) = value else {
                return Err(invalid_params("each range must be [offset, length]"));
            };
            if range.len() != 2 {
                return Err(invalid_params("each range must contain two integers"));
            }
            let offset = range[0]
                .as_u64()
                .ok_or_else(|| invalid_params("range offset must be unsigned"))?;
            let length = range[1]
                .as_u64()
                .ok_or_else(|| invalid_params("range length must be unsigned"))?;
            let length = usize::try_from(length)
                .map_err(|_| invalid_params("range length does not fit this host"))?;
            Ok((offset, length))
        })
        .collect()
}

fn invalid_params(message: &str) -> PluginError {
    PluginError::Rpc {
        code: "INVALID_PARAMS".to_owned(),
        message: message.to_owned(),
    }
}

pub(super) fn plugin_error_to_rpc(error: PluginError) -> RpcError {
    match error {
        PluginError::Rpc { code, message } => RpcError::new(code, message),
        PluginError::ResourceLimit(message) => RpcError::new("RESOURCE_LIMIT", message),
        error => RpcError::new("HOST_ERROR", error.to_string()),
    }
}
