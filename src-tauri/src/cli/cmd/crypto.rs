//! Encrypted volumes: what a partition or container is encrypted with, and
//! one `unlock` for BitLocker, LUKS, VeraCrypt, FileVault and iOS backups.
//! The disk schemes share one backend unlock and differ only in credentials.
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    cli::{
        browsing::LastLink,
        exec::Exec,
        types::{parse_address, parse_host_path, parse_secret, Address, HostPath, Secret},
        workspace::Target,
        CliError, Output,
    },
    core::evidence::crypto::{
        encrypted_image::{EncryptionScheme, UnlockCredential, UnlockOptions, UnlockTarget},
        filevault::InspectOptions,
    },
};

/// The partition a root entry stands for: `part:N`, or `unsupported:N` for
/// a partition whose contents are not readable yet (a locked container).
pub fn partition_number(node_id: &str) -> Option<u8> {
    node_id
        .strip_prefix("part:")
        .or_else(|| node_id.strip_prefix("unsupported:"))?
        .parse()
        .ok()
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Scheme {
    Bitlocker,
    Luks,
    Veracrypt,
    Filevault,
    Ios,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptionArgs {
    /// 分区（/<别名>/part:N）或整份检材（/<别名>）
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    /// FileVault：本地 EncryptedRoot.plist(.wipekey)
    #[arg(long, value_parser = parse_host_path, value_name = "文件")]
    pub plist: Option<HostPath>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlockArgs {
    /// 分区（/<别名>/part:N），或整份加密检材（/<别名>：VeraCrypt 容器、BitLocker To Go、iOS 备份）
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    /// 加密类型；默认按分区类型识别（VeraCrypt 没有签名，须指定）
    #[arg(long, value_enum)]
    pub scheme: Option<Scheme>,
    /// 密码（@文件、- 或在终端中输入）
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_parser = parse_secret, value_name = "@文件")]
    pub password: Option<Secret>,
    /// BitLocker 48 位恢复密码
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_parser = parse_secret, value_name = "@文件")]
    pub recovery_password: Option<Secret>,
    /// BitLocker 启动密钥（.BEK 文件）
    #[arg(long, value_parser = parse_host_path, value_name = "文件")]
    pub startup_key: Option<HostPath>,
    /// BitLocker FVEK（十六进制，即 BitLocker 的主密钥）；原始 FVEK 没有认证，须核验解密结果
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_parser = parse_secret, value_name = "@文件")]
    pub fvek: Option<Secret>,
    /// BitLocker Elephant Diffuser 的 tweak 密钥（十六进制，配合 --fvek）
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_parser = parse_secret, value_name = "@文件")]
    pub tweak: Option<Secret>,
    /// BitLocker VMK（十六进制）
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_parser = parse_secret, value_name = "@文件")]
    pub vmk: Option<Secret>,
    /// BitLocker：在这个内存镜像中搜索 VMK 并用它解锁
    #[arg(long, value_parser = parse_host_path, value_name = "内存镜像")]
    pub vmk_from: Option<HostPath>,
    /// LUKS、FileVault（以及 BitLocker FVEK）：已恢复的主密钥（十六进制）
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_parser = parse_secret, value_name = "@文件")]
    pub master_key: Option<Secret>,
    /// VeraCrypt PIM（0–1000）
    #[arg(long, default_value_t = 0)]
    pub pim: u32,
    /// VeraCrypt 隐藏卷
    #[arg(long)]
    pub hidden: bool,
    /// FileVault：逻辑卷序号（encryption 输出的 index）
    #[arg(long, value_name = "序号")]
    pub logical_volume: Option<usize>,
    /// FileVault：本地 EncryptedRoot.plist(.wipekey)
    #[arg(long, value_parser = parse_host_path, value_name = "文件")]
    pub plist: Option<HostPath>,
    /// 加密区域相对检材的字节偏移（不按分区选择时）
    #[arg(long)]
    pub offset: Option<u64>,
    /// 加密区域的字节长度；默认到检材末尾。VeraCrypt 区域须为检材扇区大小的整数倍
    #[arg(long)]
    pub length: Option<u64>,
    /// 解密结果作为新的检材，而不是在原检材中直接可读（给 --offset 时总是新检材）
    #[arg(long)]
    pub new_image: bool,
    /// 新检材的别名
    #[arg(long = "as", value_name = "别名")]
    pub alias: Option<String>,
}

pub fn secrets_mut(args: &mut UnlockArgs) -> Vec<(&'static str, &mut Secret)> {
    let mut secrets = Vec::new();
    for (name, secret) in [
        ("password", args.password.as_mut()),
        ("recovery-password", args.recovery_password.as_mut()),
        ("fvek", args.fvek.as_mut()),
        ("tweak", args.tweak.as_mut()),
        ("vmk", args.vmk.as_mut()),
        ("master-key", args.master_key.as_mut()),
    ] {
        if let Some(secret) = secret {
            secrets.push((name, secret));
        }
    }
    secrets
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

fn value(secret: &Option<Secret>) -> Result<Option<String>, CliError> {
    secret
        .as_ref()
        .map(|secret| secret.value().map(str::to_owned))
        .transpose()
}

/// The partition the target names, and its kind.
async fn partition(
    exec: &Exec,
    target: &Target,
) -> Result<(Option<u8>, Option<String>, Value), CliError> {
    let info =
        crate::commands::evidence::image::image_info(exec.state(), target.image_id().clone())
            .await
            .map_err(operation("image_info"))?;
    let info = serde_json::to_value(crate::cli::wire::Camel(info))?;
    let number = target
        .location
        .first()
        .and_then(|(_, id)| partition_number(id));
    let kind = number.and_then(|number| {
        info["partitions"].as_array().and_then(|partitions| {
            partitions
                .iter()
                .find(|partition| partition["id"] == number)
                .and_then(|partition| partition["kind"].as_str().map(str::to_owned))
        })
    });
    Ok((number, kind, info))
}

/// The scheme an unlocked partition was opened with, from `image_info`.
fn unlocked_scheme(info: &Value, number: u8) -> Option<String> {
    info["unlockedPartitions"]
        .as_array()?
        .iter()
        .find(|unlocked| unlocked["id"] == number)
        .and_then(|unlocked| unlocked["scheme"].as_str().map(str::to_owned))
}

/// Whether `kind` names a partition still locked behind an encryption header.
fn locked_scheme(kind: Option<&str>) -> Option<&str> {
    kind.filter(|kind| matches!(*kind, "bitlocker" | "luks" | "filevault"))
}

pub async fn encryption(exec: &Exec, args: EncryptionArgs) -> Result<Output, CliError> {
    let target = exec.target(&args.path, LastLink::Follow).await?;
    let (number, kind, info) = partition(exec, &target).await?;
    let unlocked = number.and_then(|number| unlocked_scheme(&info, number));
    let scheme = unlocked
        .clone()
        .or_else(|| locked_scheme(kind.as_deref()).map(str::to_owned));
    let is_unlocked = unlocked.is_some();
    let mut value = match (number, scheme.as_deref()) {
        (Some(number), Some("bitlocker")) => {
            // The metadata stays readable from the source after an unlock.
            let metadata = crate::commands::evidence::encrypted_volume::bitlocker_inspect(
                exec.state(),
                target.image_id().clone(),
                number,
            )
            .await
            .map_err(operation("bitlocker_inspect"))?;
            json!({"scheme": "bitlocker", "unlocked": is_unlocked,
                "metadata": serde_json::to_value(crate::cli::wire::Camel(metadata))?})
        }
        (Some(number), Some("filevault")) => {
            let inventory = crate::commands::evidence::encrypted_volume::filevault_inspect(
                exec.state(),
                target.image_id().clone(),
                InspectOptions {
                    partition_id: Some(number),
                    offset: None,
                    length: None,
                    encrypted_root_plist_path: args.plist.as_ref().map(HostPath::text),
                },
                Some(super::task_id("encryption")),
            )
            .await
            .map_err(operation("filevault_inspect"))?;
            json!({"scheme": "filevault", "unlocked": is_unlocked,
                "inventory": serde_json::to_value(crate::cli::wire::Camel(inventory))?})
        }
        (Some(_), Some(scheme)) => json!({"scheme": scheme, "unlocked": is_unlocked}),
        (Some(_), None) => json!({"scheme": null, "partitionKind": kind}),
        (None, _) if !info["iosBackup"].is_null() => {
            json!({"scheme": "ios", "iosBackup": info["iosBackup"]})
        }
        (None, _) => json!({"scheme": null, "sourceFormat": info["sourceFormat"],
            "note": "整份检材没有可识别的加密签名；VeraCrypt 容器（大小须为扇区的整数倍）用 unlock --scheme veracrypt"}),
    };
    value["path"] = json!(target.address());
    Ok(Output::json(value))
}

pub async fn unlock(exec: &Exec, args: UnlockArgs) -> Result<Output, CliError> {
    let target = exec.target(&args.path, LastLink::Follow).await?;
    let (number, kind, info) = partition(exec, &target).await?;
    let scheme = match (args.scheme, kind.as_deref()) {
        (Some(scheme), _) => scheme,
        (None, Some("bitlocker")) => Scheme::Bitlocker,
        (None, Some("luks")) => Scheme::Luks,
        (None, Some("filevault")) => Scheme::Filevault,
        (None, _) if number.is_none() && !info["iosBackup"].is_null() => Scheme::Ios,
        (None, kind) => {
            return Err(CliError::new(
                "usage",
                format!(
                "无法识别 {} 的加密类型{}；用 --scheme bitlocker|luks|veracrypt|filevault|ios 指定",
                target.address(),
                kind.map(|kind| format!("（分区类型 {kind}）"))
                    .unwrap_or_default()
            ),
            ))
        }
    };
    let scheme = match scheme {
        Scheme::Ios => return ios_backup(exec, &target, &args).await,
        Scheme::Bitlocker => EncryptionScheme::Bitlocker,
        Scheme::Luks => EncryptionScheme::Luks,
        Scheme::Veracrypt => EncryptionScheme::Veracrypt,
        Scheme::Filevault => EncryptionScheme::Filevault,
    };
    if let Some(memory) = &args.vmk_from {
        return vmk_from_memory(exec, &target, number, scheme, memory, &args).await;
    }
    let credential = credential(&args, scheme)?;
    Ok(Output::json(
        volume(exec, &target, number, scheme, credential, &args).await?,
    ))
}

async fn ios_backup(exec: &Exec, target: &Target, args: &UnlockArgs) -> Result<Output, CliError> {
    let password =
        value(&args.password)?.ok_or_else(|| CliError::new("usage", "iOS 备份需要 --password"))?;
    let unlocked = crate::commands::evidence::ios_backup::ios_backup_unlock(
        exec.state(),
        target.image_id().clone(),
        password,
        Some(super::task_id("unlock")),
    )
    .await
    .map_err(operation("ios_backup_unlock"))?;
    let image_id = crate::core::common::ids::ImageId::from(unlocked.id.clone());
    let alias = if image_id == *target.image_id() {
        target.evidence.alias.clone()
    } else {
        exec.adopt(
            &image_id,
            args.alias
                .as_deref()
                .unwrap_or(&format!("{}-dec", target.evidence.alias)),
        )
        .await?
    };
    Ok(Output::json(json!({
        "scheme": "ios",
        "path": format!("/{alias}"),
        "image": serde_json::to_value(crate::cli::wire::Camel(unlocked))?,
    })))
}

/// The one credential the options give; none is BitLocker's clear key.
fn credential(args: &UnlockArgs, scheme: EncryptionScheme) -> Result<UnlockCredential, CliError> {
    if args.tweak.is_some() && args.fvek.is_none() {
        return Err(CliError::new("usage", "--tweak 只能配合 --fvek"));
    }
    let mut given = Vec::new();
    if let Some(password) = value(&args.password)? {
        given.push(UnlockCredential::Password { password });
    }
    if let Some(recovery_password) = value(&args.recovery_password)? {
        given.push(UnlockCredential::RecoveryPassword { recovery_password });
    }
    if let Some(path) = &args.startup_key {
        given.push(UnlockCredential::StartupKey { path: path.text() });
    }
    if let Some(key_hex) = value(&args.fvek)? {
        given.push(UnlockCredential::MasterKey {
            key_hex,
            tweak_hex: value(&args.tweak)?,
        });
    }
    if let Some(key_hex) = value(&args.vmk)? {
        given.push(UnlockCredential::Vmk { key_hex });
    }
    if let Some(key_hex) = value(&args.master_key)? {
        given.push(UnlockCredential::MasterKey {
            key_hex,
            tweak_hex: None,
        });
    }
    if given.len() > 1 {
        return Err(CliError::new(
            "usage",
            "只能给一种凭据：--password、--recovery-password、--startup-key、--fvek、--vmk、--vmk-from 或 --master-key",
        ));
    }
    match given.pop() {
        Some(credential) => Ok(credential),
        // A suspended BitLocker volume opens with the clear key it keeps.
        None if scheme == EncryptionScheme::Bitlocker => Ok(UnlockCredential::ClearKey),
        None => Err(CliError::new("usage", "需要 --password 或 --master-key")),
    }
}

/// Find VMKs in a memory image and unlock with the first that opens the volume.
async fn vmk_from_memory(
    exec: &Exec,
    target: &Target,
    number: Option<u8>,
    scheme: EncryptionScheme,
    memory: &HostPath,
    args: &UnlockArgs,
) -> Result<Output, CliError> {
    if scheme != EncryptionScheme::Bitlocker {
        return Err(CliError::new("usage", "--vmk-from 只用于 BitLocker"));
    }
    let number =
        number.ok_or_else(|| CliError::new("usage", "--vmk-from 需要分区：/<别名>/part:N"))?;
    let other_secrets = [
        &args.password,
        &args.recovery_password,
        &args.fvek,
        &args.tweak,
        &args.vmk,
        &args.master_key,
    ];
    if other_secrets.iter().any(|secret| secret.is_some()) || args.startup_key.is_some() {
        return Err(CliError::new("usage", "--vmk-from 不能与其他凭据同时使用"));
    }
    let scan = crate::commands::evidence::encrypted_volume::bitlocker_scan_memory(
        exec.host.clone(),
        exec.state(),
        target.image_id().clone(),
        number,
        memory.text(),
        super::task_id("unlock vmk-scan"),
    )
    .await
    .map_err(operation("bitlocker_scan_memory"))?;
    let mut candidates: Vec<&_> = scan.candidates.iter().collect();
    candidates.sort_by_key(|candidate| candidate.recovery_key.is_none());
    for candidate in candidates {
        let credential = UnlockCredential::Vmk {
            key_hex: candidate.vmk_hex.clone(),
        };
        if let Ok(mut value) = volume(exec, target, Some(number), scheme, credential, args).await {
            value["vmkOffset"] = json!(candidate.offset);
            return Ok(Output::json(value));
        }
    }
    Err(CliError::new(
        "unlockFailed",
        format!(
            "内存镜像中找到 {} 个 VMK 候选，没有一个能解锁 /{}/part:{number}",
            scan.candidates.len(),
            target.evidence.alias
        ),
    ))
}

async fn volume(
    exec: &Exec,
    target: &Target,
    number: Option<u8>,
    scheme: EncryptionScheme,
    credential: UnlockCredential,
    args: &UnlockArgs,
) -> Result<Value, CliError> {
    // An encrypted partition, or a whole-image volume such as a VeraCrypt
    // file, becomes readable in the source evidence; a byte range always
    // becomes evidence of its own.
    let in_place = !args.new_image && args.offset.is_none();
    let options = UnlockOptions {
        target: if in_place {
            UnlockTarget::SourcePartition
        } else {
            UnlockTarget::NewImage
        },
        scheme,
        credential,
        partition_id: number,
        offset: args.offset,
        length: args.length,
        hidden: args.hidden,
        pim: args.pim,
        logical_volume_index: args.logical_volume,
        encrypted_root_plist_path: args.plist.as_ref().map(HostPath::text),
    };
    let result = crate::commands::evidence::encrypted_volume::encrypted_volume_unlock(
        exec.host.clone(),
        exec.state(),
        target.image_id().clone(),
        options,
        Some(super::task_id("unlock")),
    )
    .await
    .map_err(operation("encrypted_volume_unlock"))?;
    let image_id = crate::core::common::ids::ImageId::from(result.image.id.clone());
    let path = if image_id == *target.image_id() {
        // The decrypted volume, where the source evidence now shows it.
        match (&result.volume_key, number) {
            (Some(volume), _) => format!("/{}/{volume}", target.evidence.alias),
            (None, Some(number)) => format!("/{}/part:{number}", target.evidence.alias),
            (None, None) => format!("/{}", target.evidence.alias),
        }
    } else {
        let preferred = args
            .alias
            .clone()
            .unwrap_or_else(|| format!("{}-dec", target.evidence.alias));
        format!("/{}", exec.adopt(&image_id, &preferred).await?)
    };
    let mut value = serde_json::to_value(crate::cli::wire::Camel(result))?;
    value["path"] = json!(path);
    Ok(value)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_ids_name_their_number() {
        assert_eq!(partition_number("part:3"), Some(3));
        assert_eq!(partition_number("unsupported:0"), Some(0));
        assert_eq!(partition_number("part:x"), None);
        assert_eq!(partition_number("lvm:0:1"), None);
    }
}
