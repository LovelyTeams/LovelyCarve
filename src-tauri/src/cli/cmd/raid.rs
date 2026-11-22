//! `raid` (assembling RAID and LVM from member evidence) and `vss` (NTFS
//! shadow copies). What they assemble or open becomes evidence with its own
//! alias.
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    cli::{
        browsing::LastLink,
        exec::Exec,
        types::{parse_address, Address},
        CliError, Output,
    },
    core::common::ids::ImageId,
};

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RaidArgs {
    #[command(subcommand)]
    pub command: RaidCommand,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RaidCommand {
    /// 识别并组装已打开检材中的 mdadm、LVM 与固件 RAID；无元数据的等大成员盘按 RAID0/5 推定
    Detect(DetectArgs),
    /// 按给定几何手动组装 RAID
    Assemble(AssembleArgs),
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectArgs {
    /// 只检测这些成员检材（别名，可重复）；默认全部已打开的源检材
    #[arg(long = "member", value_name = "别名")]
    pub members: Vec<String>,
    /// 允许降级组装的 md 阵列（detect 输出的 arrays[].setUuid，可重复）
    #[arg(long = "degraded", value_name = "UUID")]
    pub degraded: Vec<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssembleArgs {
    /// 成员检材别名，按列序逗号分隔；缺失的列写 -
    #[arg(long, value_delimiter = ',', required = true, value_name = "别名")]
    pub members: Vec<String>,
    /// RAID 级别：-1 linear、0、1、4、5、6、10
    #[arg(long, allow_hyphen_values = true)]
    pub level: i32,
    /// RAID5/6 布局号；RAID10 为 near | far<<8 | offset<<16
    #[arg(long, default_value_t = 0)]
    pub layout: u32,
    /// 条带大小（扇区）
    #[arg(long, default_value_t = 128)]
    pub chunk_sectors: u32,
    /// 成员上的数据起始偏移（扇区）
    #[arg(long, default_value_t = 0)]
    pub data_offset_sectors: u64,
    /// 按校验或镜像重建、不读取的列（可重复）
    #[arg(long = "missing", value_name = "列")]
    pub missing: Vec<usize>,
    /// 组装出的检材的别名
    #[arg(long = "as", value_name = "别名")]
    pub alias: Option<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VssArgs {
    #[command(subcommand)]
    pub command: VssCommand,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VssCommand {
    /// 列出检材中 NTFS 卷的卷影副本
    Ls(VssList),
    /// 把一个卷影副本打开为只读检材
    Open(VssOpen),
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VssList {
    /// 检材（/<别名>）或其中的 NTFS 分区（/<别名>/part:N）
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VssOpen {
    /// NTFS 分区（/<别名>/part:N）
    #[arg(value_parser = parse_address, value_name = "分区")]
    pub path: Address,
    /// 卷影副本序号（vss ls 的 index）
    #[arg(value_name = "序号")]
    pub index: usize,
    /// 打开的卷影副本的别名
    #[arg(long = "as", value_name = "别名")]
    pub alias: Option<String>,
}

pub fn raid_path(args: &RaidArgs) -> &'static str {
    match args.command {
        RaidCommand::Detect(_) => "raid detect",
        RaidCommand::Assemble(_) => "raid assemble",
    }
}

pub fn vss_path(args: &VssArgs) -> &'static str {
    match args.command {
        VssCommand::Ls(_) => "vss ls",
        VssCommand::Open(_) => "vss open",
    }
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

/// Images loaded now that were not loaded before.
fn added(
    before: &[ImageId],
    after: &[crate::core::evidence::image::ImageInfo],
) -> Vec<(ImageId, String)> {
    after
        .iter()
        .filter(|info| !before.iter().any(|id| id.as_str() == info.id))
        .map(|info| (ImageId::from(info.id.clone()), info.path.clone()))
        .collect()
}

pub async fn raid(exec: &Exec, args: RaidArgs) -> Result<Output, CliError> {
    let before: Vec<ImageId> = crate::commands::evidence::image::image_list(exec.state())
        .await
        .map_err(operation("image_list"))?
        .into_iter()
        .map(|info| ImageId::from(info.id))
        .collect();
    match args.command {
        RaidCommand::Detect(detect) => {
            let mut members = Vec::new();
            for alias in &detect.members {
                members.push(exec.resolve(alias.trim_start_matches('/')).await?.image_id);
            }
            if members.is_empty() {
                // Every source evidence item takes part; open case evidence first.
                for item in exec.evidence().await? {
                    exec.resolve(&item.alias).await?;
                }
            }
            let stack = crate::commands::evidence::image::raid_auto_detect(
                exec.host.clone(),
                exec.state(),
                (!detect.degraded.is_empty()).then_some(detect.degraded),
                (!members.is_empty()).then_some(members),
            )
            .await
            .map_err(operation("raid_auto_detect"))?;
            let after = crate::commands::evidence::image::image_list(exec.state())
                .await
                .map_err(operation("image_list"))?;
            let mut assembled = Vec::new();
            for (image_id, path) in added(&before, &after) {
                let preferred = path.trim_start_matches("assembled://").to_owned();
                let alias = exec.adopt(&image_id, &preferred).await?;
                assembled.push(
                    json!({"path": format!("/{alias}"), "imageId": image_id, "source": path}),
                );
            }
            let mut value = serde_json::to_value(crate::cli::wire::Camel(stack))?;
            value["assembled"] = json!(assembled);
            Ok(Output::json(value))
        }
        RaidCommand::Assemble(assemble) => {
            let mut member_ids = Vec::new();
            for alias in &assemble.members {
                if alias == "-" || alias.is_empty() {
                    member_ids.push(String::new());
                } else {
                    member_ids.push(
                        exec.resolve(alias.trim_start_matches('/'))
                            .await?
                            .image_id
                            .to_string(),
                    );
                }
            }
            let params = serde_json::from_value(json!({
                "member_image_ids": member_ids,
                "level": assemble.level,
                "layout": assemble.layout,
                "chunk_sectors": assemble.chunk_sectors,
                "data_offset_sectors": assemble.data_offset_sectors,
                "missing_slots": assemble.missing,
            }))?;
            let result =
                crate::commands::evidence::image::raid_manual_assemble(exec.state(), params)
                    .await
                    .map_err(operation("raid_manual_assemble"))?;
            let image_id = ImageId::from(result.image_id.clone());
            let alias = exec
                .adopt(&image_id, assemble.alias.as_deref().unwrap_or("raid"))
                .await?;
            let mut value = serde_json::to_value(crate::cli::wire::Camel(result))?;
            value["path"] = json!(format!("/{alias}"));
            Ok(Output::json(value))
        }
    }
}

pub async fn vss(exec: &Exec, args: VssArgs) -> Result<Output, CliError> {
    match args.command {
        VssCommand::Ls(list) => {
            let target = exec.target(&list.path, LastLink::Follow).await?;
            let only = target
                .location
                .first()
                .and_then(|(_, id)| super::crypto::partition_number(id));
            let volumes = crate::commands::evidence::shadow_copy::shadow_copy_list(
                exec.state(),
                target.image_id().clone(),
            )
            .await
            .map_err(operation("shadow_copy_list"))?;
            let mut items = Vec::new();
            for volume in volumes {
                if only.is_some_and(|number| number != volume.partition_id) {
                    continue;
                }
                let mut value = serde_json::to_value(crate::cli::wire::Camel(&volume))?;
                value["path"] = json!(format!(
                    "/{}/part:{}",
                    target.evidence.alias, volume.partition_id
                ));
                items.push(value);
            }
            Ok(Output::json(json!({ "items": items })))
        }
        VssCommand::Open(open) => {
            let target = exec.target(&open.path, LastLink::Follow).await?;
            let number = target
                .location
                .first()
                .and_then(|(_, id)| super::crypto::partition_number(id))
                .ok_or_else(|| CliError::new("usage", "需要 NTFS 分区：/<别名>/part:N"))?;
            let info = crate::commands::evidence::shadow_copy::shadow_copy_open(
                exec.state(),
                target.image_id().clone(),
                number,
                open.index,
                Some(super::task_id("vss open")),
            )
            .await
            .map_err(operation("shadow_copy_open"))?;
            let image_id = ImageId::from(info.id.clone());
            let preferred = open
                .alias
                .clone()
                .unwrap_or_else(|| format!("{}-vss{}", target.evidence.alias, open.index));
            let alias = exec.adopt(&image_id, &preferred).await?;
            let mut value = serde_json::to_value(crate::cli::wire::Camel(info))?;
            value["path"] = json!(format!("/{alias}"));
            Ok(Output::json(value))
        }
    }
}
