//! `timeline`: the case timeline of one evidence item — build, status,
//! events, bucket counts and CSV export.
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    cli::{
        exec::Exec,
        types::{parse_host_path, HostPath},
        CliError, Output,
    },
    core::timeline::timeline_store::{TimelineQuery, TimelineUnit},
};

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineArgs {
    #[command(subcommand)]
    pub command: Option<TimelineCommand>,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimelineCommand {
    /// 构建状态、时间范围与事件类型（默认）
    Status(EvidenceOption),
    /// 构建或重建时间线
    Build(EvidenceOption),
    /// 分页列出事件
    Ls(EventQuery),
    /// 按时间桶统计事件数
    Stats(StatsQuery),
    /// 把事件导出为 CSV
    Export(ExportQuery),
}

#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceOption {
    /// 检材别名；只有一份检材时可省略
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
}

#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Filters {
    /// 检材别名；只有一份检材时可省略
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
    /// 起始时间：RFC 3339、YYYY-MM-DD（UTC）或 Unix 秒
    #[arg(long, value_name = "时间")]
    pub from: Option<String>,
    /// 结束时间：RFC 3339、YYYY-MM-DD（含当天，UTC）或 Unix 秒
    #[arg(long, value_name = "时间")]
    pub to: Option<String>,
    /// 来源应用（timeline status 列出的 fromApp，可重复）
    #[arg(long = "app", value_name = "应用")]
    pub apps: Vec<String>,
    /// 事件类型编号（可重复）
    #[arg(long = "type", value_name = "编号")]
    pub types: Vec<i32>,
    /// 只看包含此文本的事件
    #[arg(long, value_name = "文本")]
    pub text: Option<String>,
    /// 包含已删除来源的事件
    #[arg(long)]
    pub deleted: bool,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventQuery {
    #[command(flatten)]
    pub filters: Filters,
    /// 跳过的事件数
    #[arg(long, default_value_t = 0)]
    pub offset: u64,
    /// 每页事件数
    #[arg(long, default_value_t = 200)]
    pub limit: u64,
    /// 从新到旧
    #[arg(long)]
    pub desc: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unit {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsQuery {
    #[command(flatten)]
    pub filters: Filters,
    /// 时间桶
    #[arg(long, value_enum, default_value_t = Unit::Day)]
    pub by: Unit,
    /// 时间桶所用的 UTC 偏移（小时）
    #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
    pub utc_offset: i32,
    /// 最多返回的桶数
    #[arg(long)]
    pub limit: Option<u64>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportQuery {
    #[command(flatten)]
    pub filters: Filters,
    /// 输出的 CSV 文件（必须不存在）
    #[arg(short = 'o', long, value_parser = parse_host_path, value_name = "文件")]
    pub output: HostPath,
}

pub fn path(args: &TimelineArgs) -> &'static str {
    match &args.command {
        None | Some(TimelineCommand::Status(_)) => "timeline status",
        Some(TimelineCommand::Build(_)) => "timeline build",
        Some(TimelineCommand::Ls(_)) => "timeline ls",
        Some(TimelineCommand::Stats(_)) => "timeline stats",
        Some(TimelineCommand::Export(_)) => "timeline export",
    }
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

/// Unix seconds from RFC 3339, a UTC date, or seconds; a date as `--to`
/// covers that whole day.
pub fn parse_time(text: &str, end_of_day: bool) -> Result<i64, CliError> {
    if let Ok(seconds) = text.parse::<i64>() {
        return Ok(seconds);
    }
    if let Ok(time) = chrono::DateTime::parse_from_rfc3339(text) {
        return Ok(time.timestamp());
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        let time = if end_of_day {
            date.and_hms_opt(23, 59, 59)
        } else {
            date.and_hms_opt(0, 0, 0)
        };
        if let Some(time) = time {
            return Ok(time.and_utc().timestamp());
        }
    }
    Err(CliError::new(
        "usage",
        format!("无效时间：{text}（用 2024-05-01T08:00:00+08:00、2024-05-01 或 Unix 秒）"),
    ))
}

fn query(
    filters: &Filters,
    unit: TimelineUnit,
    offset_hours: i32,
) -> Result<TimelineQuery, CliError> {
    Ok(TimelineQuery {
        start: filters
            .from
            .as_deref()
            .map(|text| parse_time(text, false))
            .transpose()?,
        end: filters
            .to
            .as_deref()
            .map(|text| parse_time(text, true))
            .transpose()?,
        time_zone_hours: offset_hours,
        time_unit: unit,
        from_apps: filters.apps.clone(),
        data_types: filters.types.clone(),
        pairs: Vec::new(),
        text: filters.text.clone(),
        show_delete: filters.deleted,
    })
}

pub async fn run(exec: &Exec, args: TimelineArgs) -> Result<Output, CliError> {
    match args
        .command
        .unwrap_or(TimelineCommand::Status(EvidenceOption::default()))
    {
        TimelineCommand::Status(option) => {
            let evidence = exec.resolve_default(option.evidence.as_deref()).await?;
            let status = crate::commands::timeline::timeline::timeline_status(
                exec.state(),
                evidence.image_id.clone(),
            )
            .await
            .map_err(operation("timeline_status"))?;
            let range = crate::commands::timeline::timeline::timeline_range(
                exec.state(),
                evidence.image_id.clone(),
            )
            .await
            .map_err(operation("timeline_range"))?;
            let types = crate::commands::timeline::timeline::timeline_types(
                exec.state(),
                evidence.image_id.clone(),
            )
            .await
            .map_err(operation("timeline_types"))?;
            Ok(Output::json(json!({
                "evidence": evidence.alias,
                "status": serde_json::to_value(crate::cli::wire::Camel(status))?,
                "range": serde_json::to_value(crate::cli::wire::Camel(range))?,
                "types": serde_json::to_value(crate::cli::wire::Camel(types))?,
            })))
        }
        TimelineCommand::Build(option) => {
            let evidence = exec.resolve_default(option.evidence.as_deref()).await?;
            crate::cli::json_output(
                crate::commands::timeline::timeline::timeline_start(
                    exec.host.clone(),
                    exec.state(),
                    evidence.image_id,
                )
                .await
                .map_err(operation("timeline_start"))?,
            )
        }
        TimelineCommand::Ls(events) => {
            let evidence = exec
                .resolve_default(events.filters.evidence.as_deref())
                .await?;
            let page = crate::commands::timeline::timeline::timeline_datas(
                exec.state(),
                evidence.image_id,
                query(&events.filters, TimelineUnit::Day, 0)?,
                Some(events.offset),
                Some(events.limit.clamp(1, 10_000)),
                Some(if events.desc { "desc" } else { "asc" }.to_owned()),
            )
            .await
            .map_err(operation("timeline_datas"))?;
            crate::cli::json_output(page)
        }
        TimelineCommand::Stats(stats) => {
            let evidence = exec
                .resolve_default(stats.filters.evidence.as_deref())
                .await?;
            let unit = match stats.by {
                Unit::Year => TimelineUnit::Year,
                Unit::Month => TimelineUnit::Month,
                Unit::Day => TimelineUnit::Day,
                Unit::Hour => TimelineUnit::Hour,
                Unit::Minute => TimelineUnit::Minute,
                Unit::Second => TimelineUnit::Second,
            };
            let buckets = crate::commands::timeline::timeline::timeline_count(
                exec.state(),
                evidence.image_id,
                query(&stats.filters, unit, stats.utc_offset)?,
                stats.limit,
                None,
            )
            .await
            .map_err(operation("timeline_count"))?;
            Ok(Output::json(
                json!({ "items": serde_json::to_value(crate::cli::wire::Camel(buckets))? }),
            ))
        }
        TimelineCommand::Export(export) => {
            if export.output.as_path().exists() {
                return Err(CliError::new(
                    "alreadyExists",
                    format!("{} 已存在：导出从不覆盖已有文件", export.output.text()),
                ));
            }
            let evidence = exec
                .resolve_default(export.filters.evidence.as_deref())
                .await?;
            crate::cli::json_output(
                crate::commands::timeline::timeline::timeline_export(
                    exec.state(),
                    evidence.image_id,
                    query(&export.filters, TimelineUnit::Day, 0)?,
                    export.output.text(),
                )
                .await
                .map_err(operation("timeline_export"))?,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_take_rfc3339_dates_and_seconds() {
        assert_eq!(parse_time("1700000000", false).unwrap(), 1_700_000_000);
        assert_eq!(
            parse_time("2024-01-01T08:00:00+08:00", false).unwrap(),
            1_704_067_200
        );
        assert_eq!(parse_time("2024-01-01", false).unwrap(), 1_704_067_200);
        assert_eq!(parse_time("2024-01-01", true).unwrap(), 1_704_153_599);
        assert!(parse_time("yesterday", false).is_err());
    }
}
