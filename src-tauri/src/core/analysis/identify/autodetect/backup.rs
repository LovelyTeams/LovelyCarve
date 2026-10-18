//! 归档/备份文件识别与备份类型到平台、厂商的映射。
//!
//! 按三种线索判备份:
//!
//! 1. **特征文件名**(精确条目名):`huaweicheck.json`、`thunderforensic.json`、
//!    `kunlunmobile.json`、`backupinfo.ini`、`backupHistoryInfo.xml`、
//!    `SmartSwitchBackupInf.bk`、`com.tencent.mm.tar/.zip`、
//!    `com.tencent.mobileqq.tar/.zip`、`weixin_resource.zip`、`info.xml`。
//! 2. **条目名关键词**:厂商词 `huawei/xiaomi/meizu/coolpad/samsung/vivo/miui…`
//!    与大类词 `ios/android/backup/HUAWEI/compressfile`;跳过扩展名表内的条目。
//! 3. **字节魔数**:adb 备份 `ANDROID BACKUP`,小米 `.bak` 是带
//!    私有前缀的同一格式。
//!
//! 结论的 `backup_type` 使用备份类型枚举词(写入 `evidence.type`),工具/
//! 厂商的人读名放在 `tool`;具体工具没有独立枚举词时归 `backup` 大类。

use serde::{Deserialize, Serialize};

use super::EvidencePlatform;

/// 归档检测跳过的条目扩展名(不参与关键词比对)。
pub const SKIP_EXTENSIONS: [&str; 17] = [
    ".apk", ".bat", ".cmd", ".com", ".css", ".exe", ".gif", ".htm", ".jpg", ".log", ".mjs", ".pdf",
    ".png", ".svg", ".tar", ".xml", ".zip",
];

/// 随包资源 `huaweicheck.json`(38 字节)的默认内容:华为生态品牌白名单——
/// 荣耀、中国移动/天翼定制机、PTAC、JRD、鼎桥。
pub const HUAWEI_CHECK_BRANDS: [&str; 6] = ["HONOR", "CMDC", "TIANYI", "PTAC", "JRD", "tdtech"];

/// 厂商关键词。`miui` 归小米。
pub const BRAND_KEYWORDS: [&str; 12] = [
    "huawei",
    "xiaomi",
    "meizu",
    "coolpad",
    "samsung",
    "vivo",
    "miui",
    "oppo",
    "nubia",
    "infinix",
    "transsion",
    "oneplus",
];

/// 大类枚举词 `backup`:没有专属枚举词的备份都归此类。
pub const BACKUP_TYPE_GENERIC: &str = "backup";

/// adb 备份文件头魔数(含换行,`android.app.backup` 格式第一行)。
pub const ANDROID_BACKUP_MAGIC: &[u8] = b"ANDROID BACKUP\n";

/// 小米 `.bak` 私有前缀的搜索窗口:魔数不在偏移 0 但落在窗口内即判小米包装。
const XIAOMI_BAK_SCAN_WINDOW: usize = 64 * 1024;

/// 关键词走查的条目上限,防止超大归档拖慢首任务。
pub const KEYWORD_SCAN_LIMIT: usize = 4096;

/// 一条特征文件签名。
#[derive(Clone, Copy, Debug)]
struct Signature {
    entry: &'static str,
    tool: &'static str,
    backup_type: &'static str,
    platform: EvidencePlatform,
    manufactor: Option<&'static str>,
}

/// 特征文件表。顺序即优先级:越前越具体(华为 OTG/取证导出包 → 厂商
/// 备份 → 手机助手应用包 → HiSuite info.xml → iTunes 清单)。
const SIGNATURES: &[Signature] = &[
    Signature {
        entry: "huaweicheck.json",
        tool: "华为 OTG 备份",
        backup_type: "huaweiotgbackuptotar",
        platform: EvidencePlatform::Android,
        manufactor: Some("huawei"),
    },
    Signature {
        entry: "thunderforensic.json",
        tool: "迅雷取证导出包",
        backup_type: BACKUP_TYPE_GENERIC,
        platform: EvidencePlatform::Android,
        manufactor: None,
    },
    Signature {
        entry: "kunlunmobile.json",
        tool: "昆仑手机取证导出包",
        backup_type: BACKUP_TYPE_GENERIC,
        platform: EvidencePlatform::Android,
        manufactor: None,
    },
    Signature {
        entry: "KunlunMobile.json",
        tool: "昆仑手机取证导出包",
        backup_type: BACKUP_TYPE_GENERIC,
        platform: EvidencePlatform::Android,
        manufactor: None,
    },
    Signature {
        entry: "backupinfo.ini",
        tool: "酷派备份",
        backup_type: BACKUP_TYPE_GENERIC,
        platform: EvidencePlatform::Android,
        manufactor: Some("coolpad"),
    },
    Signature {
        entry: "backupHistoryInfo.xml",
        tool: "vivo 备份",
        backup_type: BACKUP_TYPE_GENERIC,
        platform: EvidencePlatform::Android,
        manufactor: Some("vivo"),
    },
    Signature {
        entry: "SmartSwitchBackupInf.bk",
        tool: "三星 Smart Switch 备份",
        backup_type: "samsungmachinegtool",
        platform: EvidencePlatform::Android,
        manufactor: Some("samsung"),
    },
    Signature {
        entry: "com.tencent.mm.tar",
        tool: "微信手机助手备份包",
        backup_type: "wechatbackupgtool",
        platform: EvidencePlatform::Android,
        manufactor: None,
    },
    Signature {
        entry: "com.tencent.mm.zip",
        tool: "微信手机助手备份包",
        backup_type: "wechatbackupgtool",
        platform: EvidencePlatform::Android,
        manufactor: None,
    },
    Signature {
        entry: "weixin_resource.zip",
        tool: "微信资源备份包",
        backup_type: "wechatbackup2gtool",
        platform: EvidencePlatform::Android,
        manufactor: None,
    },
    Signature {
        entry: "com.tencent.mobileqq.tar",
        tool: "QQ 手机助手备份包",
        backup_type: BACKUP_TYPE_GENERIC,
        platform: EvidencePlatform::Android,
        manufactor: None,
    },
    Signature {
        entry: "com.tencent.mobileqq.zip",
        tool: "QQ 手机助手备份包",
        backup_type: BACKUP_TYPE_GENERIC,
        platform: EvidencePlatform::Android,
        manufactor: None,
    },
    Signature {
        entry: "info.xml",
        tool: "华为 HiSuite 备份",
        backup_type: BACKUP_TYPE_GENERIC,
        platform: EvidencePlatform::Android,
        manufactor: Some("huawei"),
    },
    Signature {
        entry: "Manifest.plist",
        tool: "iTunes/MobileSync 备份",
        backup_type: "iosbackup",
        platform: EvidencePlatform::Ios,
        manufactor: None,
    },
    Signature {
        entry: "Manifest.db",
        tool: "iTunes/MobileSync 备份",
        backup_type: "iosbackup",
        platform: EvidencePlatform::Ios,
        manufactor: None,
    },
];

/// 酷派备份的第二个配置文件;与 `backupinfo.ini` 同时出现时作为佐证记入特征。
const COOLPAD_COMPANION: &str = "backup_config.json";

/// 备份识别结论。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackupConclusion {
    /// 备份类型枚举词,回写 `evidence.type`。
    pub backup_type: String,
    /// 人读的工具/厂商备份名。
    pub tool: String,
    pub platform: EvidencePlatform,
    pub manufactor: Option<String>,
    /// 命中的特征文件、关键词或魔数,按发现顺序。
    pub features: Vec<String>,
    /// 加密备份且未预填密码。
    pub needs_password: bool,
}

/// 条目扩展名是否在跳过表中(大小写不敏感)。
pub fn is_skipped_entry(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SKIP_EXTENSIONS
        .iter()
        .any(|extension| lower.ends_with(extension))
}

/// adb 备份 / 小米 `.bak` 魔数判定。`head` 为文件起始若干字节(至少覆盖
/// [`XIAOMI_BAK_SCAN_WINDOW`] 才能识别小米包装)。
pub fn detect_android_backup_header(head: &[u8]) -> Option<BackupConclusion> {
    let window = &head[..head.len().min(XIAOMI_BAK_SCAN_WINDOW)];
    let offset = memchr::memmem::find(window, ANDROID_BACKUP_MAGIC)?;
    let header = &window[offset..];
    let mut lines = header.split(|byte| *byte == b'\n').skip(1);
    let version = lines
        .next()
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .unwrap_or_default();
    let compressed = lines.next().map(|line| line == b"1").unwrap_or(false);
    let encryption = lines
        .next()
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .unwrap_or_default();
    let mut features = vec![
        format!("ANDROID BACKUP @0x{offset:x}"),
        format!("version={version}"),
        format!("compressed={}", u8::from(compressed)),
        format!("encryption={encryption}"),
    ];
    let (tool, manufactor) = if offset == 0 {
        ("ADB 备份（.ab）", None)
    } else {
        features.push("xiaomi bak prefix".to_owned());
        ("小米备份（.bak）", Some("xiaomi".to_owned()))
    };
    Some(BackupConclusion {
        backup_type: BACKUP_TYPE_GENERIC.to_owned(),
        tool: tool.to_owned(),
        platform: EvidencePlatform::Android,
        manufactor,
        features,
        needs_password: !encryption.is_empty() && encryption != "none",
    })
}

/// 归档检测:`entries` 为归档内条目相对路径(`/` 分隔),
/// `huaweicheck` 为归档内 `huaweicheck.json` 的文本(有则读)。
pub fn detect_archive_backup(
    entries: &[String],
    huaweicheck: Option<&str>,
) -> Option<BackupConclusion> {
    let mut features = Vec::new();
    let keywords = scan_keywords(entries, &mut features);

    if let Some((signature, matched_entry)) = match_signature(entries) {
        features.insert(0, matched_entry);
        if signature.entry == "backupinfo.ini" {
            if let Some(companion) = entries
                .iter()
                .find(|entry| file_name(entry) == COOLPAD_COMPANION)
            {
                features.insert(1, companion.clone());
            }
        }
        if signature.entry == "huaweicheck.json" {
            let brands: Vec<&str> = huaweicheck
                .map(parse_huawei_check)
                .unwrap_or_else(|| HUAWEI_CHECK_BRANDS.to_vec());
            features.insert(1, format!("huaweicheck.json: {}", brands.join(",")));
        }
        let platform = keywords.platform_override(signature.platform);
        let manufactor = signature
            .manufactor
            .map(str::to_owned)
            .or_else(|| keywords.manufactor.clone());
        return Some(BackupConclusion {
            backup_type: signature.backup_type.to_owned(),
            tool: signature.tool.to_owned(),
            platform,
            manufactor,
            features,
            needs_password: false,
        });
    }

    if !keywords.is_backup() {
        return None;
    }
    Some(BackupConclusion {
        backup_type: BACKUP_TYPE_GENERIC.to_owned(),
        tool: "关键词匹配备份".to_owned(),
        platform: keywords.platform_override(EvidencePlatform::Android),
        manufactor: keywords.manufactor,
        features,
        needs_password: false,
    })
}

fn match_signature(entries: &[String]) -> Option<(&'static Signature, String)> {
    for signature in SIGNATURES {
        if let Some(entry) = entries
            .iter()
            .find(|entry| file_name(entry) == signature.entry)
        {
            return Some((signature, entry.clone()));
        }
    }
    None
}

fn file_name(entry: &str) -> &str {
    entry.rsplit('/').next().unwrap_or(entry)
}

/// 解析 `huaweicheck.json`:一行一个品牌,忽略空行。
pub fn parse_huawei_check(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

/// 关键词走查结果。
#[derive(Debug, Default)]
struct KeywordHits {
    manufactor: Option<String>,
    saw_ios: bool,
    saw_android: bool,
    saw_harmony: bool,
    saw_hmos: bool,
    saw_backup: bool,
}

impl KeywordHits {
    fn is_backup(&self) -> bool {
        self.saw_backup || self.manufactor.is_some()
    }

    /// 关键词能把平台从默认值改写:`ios` 优先于 `android`,鸿蒙词再覆盖。
    fn platform_override(&self, default: EvidencePlatform) -> EvidencePlatform {
        if self.saw_hmos {
            EvidencePlatform::HuaweiHmos
        } else if self.saw_harmony {
            EvidencePlatform::Harmony
        } else if self.saw_ios && !self.saw_android {
            EvidencePlatform::Ios
        } else if self.saw_android && default == EvidencePlatform::Ios {
            EvidencePlatform::Android
        } else {
            default
        }
    }
}

fn scan_keywords(entries: &[String], features: &mut Vec<String>) -> KeywordHits {
    let mut hits = KeywordHits::default();
    for entry in entries.iter().take(KEYWORD_SCAN_LIMIT) {
        if is_skipped_entry(entry) {
            continue;
        }
        let lower = entry.to_ascii_lowercase();
        if hits.manufactor.is_none() {
            if let Some(brand) = BRAND_KEYWORDS.iter().find(|brand| lower.contains(*brand)) {
                hits.manufactor = Some(normalize_brand(brand).to_owned());
                features.push(format!("keyword:{brand} ← {entry}"));
            }
        }
        // `HUAWEI` 比对是大小写敏感的独立词,单独记一笔。
        if entry.contains("HUAWEI") && hits.manufactor.is_none() {
            hits.manufactor = Some("huawei".to_owned());
            features.push(format!("keyword:HUAWEI ← {entry}"));
        }
        if !hits.saw_ios && lower.contains("ios") {
            hits.saw_ios = true;
            features.push(format!("keyword:ios ← {entry}"));
        }
        if !hits.saw_android && lower.contains("android") {
            hits.saw_android = true;
            features.push(format!("keyword:android ← {entry}"));
        }
        if !hits.saw_hmos && (lower.contains("huaweihmos") || lower.contains("hmos")) {
            hits.saw_hmos = true;
            features.push(format!("keyword:hmos ← {entry}"));
        }
        if !hits.saw_harmony && lower.contains("harmony") {
            hits.saw_harmony = true;
            features.push(format!("keyword:harmony ← {entry}"));
        }
        if !hits.saw_backup && (lower.contains("backup") || lower.contains("compressfile")) {
            hits.saw_backup = true;
            features.push(format!("keyword:backup ← {entry}"));
        }
    }
    hits
}

/// 厂商词归一:`miui` 是小米系统名。
pub fn normalize_brand(brand: &str) -> &str {
    match brand {
        "miui" => "xiaomi",
        other => other,
    }
}

/// 备份/导出类型枚举词 → (平台, 厂商)。厂商取
/// 枚举词内含的厂商关键词。未知词返回 `None`。
pub fn resolve_backup_type(token: &str) -> Option<(EvidencePlatform, Option<&'static str>)> {
    const ANDROID_TOKENS: [&str; 26] = [
        "androiddatamining",
        "androidselectexport",
        "androidquickcollection",
        "androidquicklyexportmtp",
        "androidElePrirunas",
        "androidhuaweiclone",
        "androidhuaweiclonegtool",
        "androidmobileextra",
        "androidmeizumachinegtool",
        "androidnubiamachinegtool",
        "androidvivoexchangegtool",
        "androidvivoexchangewifigtool",
        "androidhuaweiexchange",
        "androidhuaweiexchangewifigtool",
        "androidxiaomiexchange",
        "oneplusexchangegtool",
        "oneplusexchangemtp",
        "transsionmachinegtool",
        "transsionmachinegtoolmtp",
        "samsungmachinegtool",
        "xiaomimachinegtool",
        "wechatbackupgtool",
        "wechatbackup2gtool",
        "wechatFriendListgtool",
        "otgbackuptotargtool",
        "huaweiotgbackuptotar",
    ];
    const HUAWEI_ENCRYPTED_TOKENS: [&str; 3] = [
        "huaweiencryptation",
        "huaweiencryptationV2",
        "huaweiencryptationV3",
    ];
    const IOS_TOKENS: [&str; 7] = [
        "iosbackup",
        "iosbackup_backuptime",
        "iosbackuploggtool",
        "iosjailbreakextract",
        "iosjailbreakextractgtool",
        "ioscloud",
        "mobilebackup",
    ];
    let platform = if token == "android"
        || token == "runasroot"
        || token == BACKUP_TYPE_GENERIC
        || token == "info.xml"
        || ANDROID_TOKENS.contains(&token)
        || HUAWEI_ENCRYPTED_TOKENS.contains(&token)
    {
        EvidencePlatform::Android
    } else if token == "ios" || token == "jailbreak" || IOS_TOKENS.contains(&token) {
        EvidencePlatform::Ios
    } else if token == "harmony" || token == "harmonyhuaweiexchange" {
        EvidencePlatform::Harmony
    } else if token == "huaweihmos" {
        EvidencePlatform::HuaweiHmos
    } else if token == "othermobile" {
        EvidencePlatform::OtherMobile
    } else {
        return None;
    };
    let lower = token.to_ascii_lowercase();
    let manufactor = BRAND_KEYWORDS
        .iter()
        .find(|brand| lower.contains(*brand))
        .map(|brand| normalize_brand(brand));
    Some((platform, manufactor))
}

/// 平台的人读短名。
pub fn gen_friend_platform_name(platform: Option<EvidencePlatform>) -> String {
    match platform {
        Some(EvidencePlatform::Android) => "Android",
        Some(EvidencePlatform::Ios) => "iOS",
        Some(EvidencePlatform::Windows) => "Windows",
        Some(EvidencePlatform::MacOs) => "macOS",
        Some(EvidencePlatform::Linux) => "Linux",
        Some(EvidencePlatform::Harmony) => "HarmonyOS",
        Some(EvidencePlatform::HuaweiHmos) => "HarmonyOS (HMOS)",
        Some(EvidencePlatform::OtherMobile) => "其他移动设备",
        None => "未识别",
    }
    .to_owned()
}

/// 平台显示名:短名 + 厂商 + 备份工具,以 `·` 连接。
pub fn gen_display_platform_name(
    platform: Option<EvidencePlatform>,
    manufactor: Option<&str>,
    backup: Option<&BackupConclusion>,
) -> String {
    let mut parts = vec![gen_friend_platform_name(platform)];
    if let Some(manufactor) = manufactor.filter(|value| !value.is_empty()) {
        parts.push(capitalize(manufactor));
    }
    if let Some(backup) = backup {
        parts.push(backup.tool.clone());
    }
    parts.join(" · ")
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(list: &[&str]) -> Vec<String> {
        list.iter().map(|entry| (*entry).to_owned()).collect()
    }

    #[test]
    fn skip_table_is_case_insensitive() {
        assert!(is_skipped_entry("photos/IMG_0001.JPG"));
        assert!(is_skipped_entry("app.apk"));
        assert!(is_skipped_entry("com.tencent.mm.zip"));
        assert!(!is_skipped_entry("backupinfo.ini"));
        assert!(!is_skipped_entry("SmartSwitchBackupInf.bk"));
    }

    #[test]
    fn adb_backup_header_at_offset_zero() {
        let head = b"ANDROID BACKUP\n5\n1\nnone\n\x78\x9c...".to_vec();
        let conclusion = detect_android_backup_header(&head).expect("adb backup");
        assert_eq!(
            conclusion,
            BackupConclusion {
                backup_type: "backup".into(),
                tool: "ADB 备份（.ab）".into(),
                platform: EvidencePlatform::Android,
                manufactor: None,
                features: vec![
                    "ANDROID BACKUP @0x0".into(),
                    "version=5".into(),
                    "compressed=1".into(),
                    "encryption=none".into(),
                ],
                needs_password: false,
            }
        );
    }

    #[test]
    fn encrypted_adb_backup_needs_password() {
        let head = b"ANDROID BACKUP\n5\n1\nAES-256\n".to_vec();
        let conclusion = detect_android_backup_header(&head).expect("adb backup");
        assert!(conclusion.needs_password);
    }

    #[test]
    fn xiaomi_bak_prefix_is_recognised() {
        let mut head = vec![0u8; 512];
        head.extend_from_slice(b"MIUI-BACKUP-PREFIX");
        head.extend_from_slice(b"ANDROID BACKUP\n5\n0\nnone\n");
        let conclusion = detect_android_backup_header(&head).expect("xiaomi bak");
        assert_eq!(conclusion.tool, "小米备份（.bak）");
        assert_eq!(conclusion.manufactor.as_deref(), Some("xiaomi"));
        assert!(conclusion
            .features
            .contains(&"xiaomi bak prefix".to_owned()));
        assert_eq!(conclusion.features[0], "ANDROID BACKUP @0x212");
    }

    #[test]
    fn no_magic_means_no_backup() {
        assert_eq!(detect_android_backup_header(b"PK\x03\x04"), None);
        assert_eq!(detect_android_backup_header(&[]), None);
    }

    #[test]
    fn huawei_otg_signature_reads_whitelist() {
        let conclusion = detect_archive_backup(
            &entries(&["huaweicheck.json", "data/backup.tar"]),
            Some("HONOR\nCMDC\n\nTIANYI\n"),
        )
        .expect("huawei otg");
        assert_eq!(conclusion.backup_type, "huaweiotgbackuptotar");
        assert_eq!(conclusion.manufactor.as_deref(), Some("huawei"));
        assert_eq!(conclusion.platform, EvidencePlatform::Android);
        assert_eq!(conclusion.features[0], "huaweicheck.json");
        assert_eq!(
            conclusion.features[1],
            "huaweicheck.json: HONOR,CMDC,TIANYI"
        );
    }

    #[test]
    fn huawei_otg_without_file_content_uses_packed_whitelist() {
        let conclusion =
            detect_archive_backup(&entries(&["huaweicheck.json"]), None).expect("huawei otg");
        assert_eq!(
            conclusion.features[1],
            "huaweicheck.json: HONOR,CMDC,TIANYI,PTAC,JRD,tdtech"
        );
    }

    #[test]
    fn coolpad_signature_records_companion() {
        let conclusion = detect_archive_backup(
            &entries(&[
                "backupinfo.ini",
                "backup_config.json",
                "Contact/contact.vcf",
            ]),
            None,
        )
        .expect("coolpad");
        assert_eq!(conclusion.backup_type, "backup");
        assert_eq!(conclusion.tool, "酷派备份");
        assert_eq!(conclusion.manufactor.as_deref(), Some("coolpad"));
        assert_eq!(
            &conclusion.features[..2],
            &["backupinfo.ini".to_owned(), "backup_config.json".to_owned()]
        );
    }

    #[test]
    fn vendor_signatures_map_to_documented_tokens() {
        let vivo = detect_archive_backup(&entries(&["backupHistoryInfo.xml"]), None).expect("vivo");
        assert_eq!(
            (vivo.backup_type.as_str(), vivo.manufactor.as_deref()),
            ("backup", Some("vivo"))
        );
        let samsung =
            detect_archive_backup(&entries(&["SmartSwitchBackupInf.bk"]), None).expect("samsung");
        assert_eq!(
            (samsung.backup_type.as_str(), samsung.manufactor.as_deref()),
            ("samsungmachinegtool", Some("samsung"))
        );
        let wechat =
            detect_archive_backup(&entries(&["com.tencent.mm.zip"]), None).expect("wechat");
        assert_eq!(wechat.backup_type, "wechatbackupgtool");
        let resource =
            detect_archive_backup(&entries(&["weixin_resource.zip"]), None).expect("wx res");
        assert_eq!(resource.backup_type, "wechatbackup2gtool");
        let hisuite =
            detect_archive_backup(&entries(&["info.xml", "media.db"]), None).expect("hisuite");
        assert_eq!(
            (hisuite.backup_type.as_str(), hisuite.manufactor.as_deref()),
            ("backup", Some("huawei"))
        );
        let itunes = detect_archive_backup(&entries(&["Manifest.plist", "Info.plist"]), None)
            .expect("itunes");
        assert_eq!(
            (itunes.backup_type.as_str(), itunes.platform),
            ("iosbackup", EvidencePlatform::Ios)
        );
    }

    #[test]
    fn nested_signature_is_matched_by_file_name() {
        let conclusion =
            detect_archive_backup(&entries(&["export/backup/SmartSwitchBackupInf.bk"]), None)
                .expect("nested");
        assert_eq!(
            conclusion.features[0],
            "export/backup/SmartSwitchBackupInf.bk"
        );
    }

    #[test]
    fn keyword_only_backup_needs_backup_or_brand_word() {
        let plain = detect_archive_backup(&entries(&["docs/readme.txt", "photos/a.jpg"]), None);
        assert_eq!(plain, None);

        let branded =
            detect_archive_backup(&entries(&["MIUI/backup/AllBackup/contacts.bak"]), None)
                .expect("keyword backup");
        assert_eq!(branded.backup_type, "backup");
        assert_eq!(branded.manufactor.as_deref(), Some("xiaomi"));
        assert_eq!(branded.platform, EvidencePlatform::Android);
        assert!(branded
            .features
            .iter()
            .any(|f| f.starts_with("keyword:miui")));
        assert!(branded
            .features
            .iter()
            .any(|f| f.starts_with("keyword:backup")));
    }

    #[test]
    fn keyword_ios_overrides_platform_and_skip_table_applies() {
        // 跳过表:.zip 条目不参与关键词比对,所以只剩 txt 触发。
        let conclusion = detect_archive_backup(
            &entries(&["android_backup.zip", "ios_backup/notes.txt"]),
            None,
        )
        .expect("ios keyword");
        assert_eq!(conclusion.platform, EvidencePlatform::Ios);
        assert!(!conclusion
            .features
            .iter()
            .any(|f| f.starts_with("keyword:android")));
    }

    #[test]
    fn upper_case_huawei_word_is_a_separate_probe() {
        let conclusion =
            detect_archive_backup(&entries(&["HUAWEI P40/backup.db"]), None).expect("hw");
        // 小写 contains 已先命中 huawei,大写探针不重复登记。
        assert_eq!(conclusion.manufactor.as_deref(), Some("huawei"));
        assert_eq!(
            conclusion
                .features
                .iter()
                .filter(|f| f.starts_with("keyword:huawei") || f.starts_with("keyword:HUAWEI"))
                .count(),
            1
        );
    }

    #[test]
    fn harmony_keywords_switch_platform() {
        let harmony =
            detect_archive_backup(&entries(&["harmony_backup/x.db"]), None).expect("harmony");
        assert_eq!(harmony.platform, EvidencePlatform::Harmony);
        let hmos =
            detect_archive_backup(&entries(&["huaweihmos_backup/x.db"]), None).expect("hmos");
        assert_eq!(hmos.platform, EvidencePlatform::HuaweiHmos);
        assert_eq!(hmos.manufactor.as_deref(), Some("huawei"));
    }

    #[test]
    fn backup_type_mapping_covers_documented_enum() {
        assert_eq!(
            resolve_backup_type("huaweiencryptationV3"),
            Some((EvidencePlatform::Android, Some("huawei")))
        );
        assert_eq!(
            resolve_backup_type("androidvivoexchangewifigtool"),
            Some((EvidencePlatform::Android, Some("vivo")))
        );
        assert_eq!(
            resolve_backup_type("iosjailbreakextractgtool"),
            Some((EvidencePlatform::Ios, None))
        );
        assert_eq!(
            resolve_backup_type("harmonyhuaweiexchange"),
            Some((EvidencePlatform::Harmony, Some("huawei")))
        );
        assert_eq!(
            resolve_backup_type("huaweihmos"),
            Some((EvidencePlatform::HuaweiHmos, Some("huawei")))
        );
        assert_eq!(
            resolve_backup_type("othermobile"),
            Some((EvidencePlatform::OtherMobile, None))
        );
        assert_eq!(
            resolve_backup_type("transsionmachinegtoolmtp"),
            Some((EvidencePlatform::Android, Some("transsion")))
        );
        assert_eq!(resolve_backup_type("windowsexport"), None);
    }

    #[test]
    fn display_names_join_platform_manufactor_and_tool() {
        assert_eq!(gen_friend_platform_name(None), "未识别");
        assert_eq!(
            gen_friend_platform_name(Some(EvidencePlatform::MacOs)),
            "macOS"
        );
        let backup = detect_archive_backup(&entries(&["huaweicheck.json"]), None).expect("otg");
        assert_eq!(
            gen_display_platform_name(
                Some(EvidencePlatform::Android),
                Some("huawei"),
                Some(&backup)
            ),
            "Android · Huawei · 华为 OTG 备份"
        );
        assert_eq!(
            gen_display_platform_name(Some(EvidencePlatform::Windows), None, None),
            "Windows"
        );
    }
}
