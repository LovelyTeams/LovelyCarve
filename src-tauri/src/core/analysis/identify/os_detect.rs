//! Operating-system identification over a read-only filesystem view.
//!
//! Provides detectors for Windows, Linux, macOS, VMware ESXi, MikroTik
//! RouterOS, OpenWrt, DingTalk smart devices and DJI aircraft. Each probes a
//! filesystem through a thin [`OsFingerprint`] adapter that exposes
//! `read` / `exists` / `is_dir` over a path. The volume layer in
//! `core::analysis::identify::os_detect_adapters` wraps every supported filesystem (NTFS,
//! Ext4, FAT, exFAT, HFS+, APFS, XFS, Btrfs, UFS, SquashFS) into one of
//! these adapters.
//!
//! The dispatcher [`detect`] runs the detectors as a short-circuit
//! chain: as soon as one matches it returns. Callers that want every
//! match (because a single partition can host multiple stacked indicators
//! — e.g. a Linux rootfs that also has an OpenWrt EFI marker) can use
//! [`detect_all`].

use std::collections::BTreeMap;
use std::io::Cursor;

use notatin::cell_value::CellValue;
use notatin::filter::FilterBuilder;
use notatin::parser::ParserIterator;
use notatin::parser_builder::ParserBuilder;
use plist::Value;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OsFamily {
    Windows,
    Linux,
    MacOs,
    Esxi,
    MikroTik,
    OpenWrt,
    DingTalk,
    Dji,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DetectedOs {
    pub family: OsFamily,
    pub product_name: String,
    pub version: Option<String>,
    pub edition: Option<String>,
    pub architecture: Option<String>,
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

impl DetectedOs {
    fn new(family: OsFamily, product_name: impl Into<String>) -> Self {
        Self {
            family,
            product_name: product_name.into(),
            version: None,
            edition: None,
            architecture: None,
            extra: BTreeMap::new(),
        }
    }

    fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    fn with_architecture(mut self, arch: impl Into<String>) -> Self {
        self.architecture = Some(arch.into());
        self
    }

    fn with_extra(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }
}

/// Read-only filesystem façade exposed to the detectors. Implementations
/// live in `core::analysis::identify::os_detect_adapters` (one per filesystem). Path
/// arguments are POSIX-style (`/Windows/System32/...`) and matching is
/// expected to be case-insensitive on Windows-host filesystems (NTFS,
/// FAT, exFAT) — adapters take care of that internally.
pub trait OsFingerprint {
    fn read(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>>;
    fn exists(&self, path: &str) -> anyhow::Result<bool>;
    fn is_dir(&self, path: &str) -> anyhow::Result<bool>;

    /// Read a complete small fingerprint file, rejecting oversized content.
    /// I/O adapters should override this to enforce the cap before allocating
    /// or reading the entire file; the default supports in-memory adapters.
    fn read_small(&self, path: &str, cap: usize) -> anyhow::Result<Option<Vec<u8>>> {
        let content = self.read(path)?;
        anyhow::ensure!(
            content.as_ref().is_none_or(|bytes| bytes.len() <= cap),
            "fingerprint file exceeds {cap} bytes: {path}"
        );
        Ok(content)
    }
}

/// Short-circuit dispatcher. Returns at most one detection — the first
/// detector in the standard order that matches.
pub fn detect(fs: &dyn OsFingerprint) -> Vec<DetectedOs> {
    run_chain(fs, true)
}

/// Run every detector and collect every match. Used by callers that
/// want to surface stacked indicators (e.g. Linux + OpenWrt EFI markers
/// on the same partition). Detector errors are logged and skipped.
pub fn detect_all(fs: &dyn OsFingerprint) -> Vec<DetectedOs> {
    run_chain(fs, false)
}

/// One detector entry in [`run_chain`]: a stable name (used for tracing
/// when the detector errors out) plus a function that runs the probe.
type Detector = (
    &'static str,
    fn(&dyn OsFingerprint) -> anyhow::Result<Option<DetectedOs>>,
);

fn run_chain(fs: &dyn OsFingerprint, short_circuit: bool) -> Vec<DetectedOs> {
    let detectors: [Detector; 8] = [
        ("windows", detect_windows),
        ("dji", |fs| Ok(super::dji::detect(fs)?.map(|hit| hit.os))),
        ("linux", detect_linux),
        ("esxi", detect_esxi),
        ("mikrotik", detect_mikrotik),
        ("dingtalk", detect_dingtalk),
        ("macos", detect_macos),
        ("openwrt", detect_openwrt),
    ];
    let mut hits = Vec::new();
    for (name, detector) in detectors {
        match detector(fs) {
            Ok(Some(os)) => {
                hits.push(os);
                if short_circuit {
                    break;
                }
            }
            Ok(None) => {}
            Err(err) => tracing::debug!("os_detect.{name} failed: {err}"),
        }
    }
    hits
}

const WIN_HIVE_PATH: &str = "/Windows/System32/config/SOFTWARE";
const WIN_SYSWOW64_PATH: &str = "/Windows/SysWOW64";
const WIN_KEY_PATH: &str = "Microsoft\\Windows NT\\CurrentVersion";

fn detect_windows(fs: &dyn OsFingerprint) -> anyhow::Result<Option<DetectedOs>> {
    let Some(hive_bytes) = fs.read(WIN_HIVE_PATH)? else {
        return Ok(None);
    };
    let (product_name, current_build) = read_software_hive(&hive_bytes)?;
    let Some(product) = product_name else {
        return Ok(None);
    };
    let display_name = match current_build.as_deref().and_then(|s| s.parse::<u32>().ok()) {
        Some(build) if build >= 22000 => "Windows 11".to_owned(),
        _ => product,
    };
    let mut os = DetectedOs::new(OsFamily::Windows, display_name);
    if let Some(build) = current_build {
        os = os.with_extra("current_build", build);
    }
    if fs.is_dir(WIN_SYSWOW64_PATH)? {
        os = os.with_architecture("x64");
    }
    Ok(Some(os))
}

fn read_software_hive(bytes: &[u8]) -> anyhow::Result<(Option<String>, Option<String>)> {
    let filter = FilterBuilder::new()
        .add_key_path(WIN_KEY_PATH)
        .return_child_keys(false)
        .build()
        .map_err(|err| anyhow::anyhow!("notatin filter: {err}"))?;
    let cursor = Cursor::new(bytes.to_vec());
    let mut builder = ParserBuilder::from_file(cursor);
    builder.with_filter(filter);
    let parser = builder
        .build()
        .map_err(|err| anyhow::anyhow!("notatin parse: {err}"))?;

    let mut product_name = None;
    let mut current_build = None;
    let needle = WIN_KEY_PATH.to_ascii_lowercase();
    for key in ParserIterator::new(&parser).iter() {
        if !key.path.to_ascii_lowercase().ends_with(&needle) {
            continue;
        }
        for value in key.value_iter() {
            let (content, _logs) = value.get_content();
            if let CellValue::String(text) = content {
                match value.get_pretty_name().to_ascii_lowercase().as_str() {
                    "productname" => product_name = Some(text),
                    "currentbuild" => current_build = Some(text),
                    _ => {}
                }
            }
        }
    }
    Ok((product_name, current_build))
}

fn detect_linux(fs: &dyn OsFingerprint) -> anyhow::Result<Option<DetectedOs>> {
    if let Some(content) = fs.read("/etc/os-release")? {
        if let Some(os) = parse_keyed(&content, "NAME", "VERSION") {
            return Ok(Some(os));
        }
    }
    if let Some(content) = fs.read("/etc/lsb-release")? {
        if let Some(os) = parse_keyed(&content, "DISTRIB_ID", "DISTRIB_RELEASE") {
            return Ok(Some(os));
        }
    }
    if let Some(content) = fs.read("/etc/redhat-release")? {
        let text = decode_utf8_trim(&content);
        if !text.is_empty() {
            return Ok(Some(DetectedOs::new(OsFamily::Linux, text)));
        }
    }
    if let Some(content) = fs.read("/etc/debian_version")? {
        let text = decode_utf8_trim(&content);
        if !text.is_empty() {
            return Ok(Some(
                DetectedOs::new(OsFamily::Linux, "Debian GNU/Linux").with_version(text),
            ));
        }
    }
    if fs.is_dir("/@rootfs")? {
        return Ok(Some(DetectedOs::new(
            OsFamily::Linux,
            "Linux (Btrfs subvolume)",
        )));
    }
    Ok(None)
}

fn parse_keyed(content: &[u8], name_key: &str, version_key: &str) -> Option<DetectedOs> {
    let text = String::from_utf8_lossy(content);
    let mut name = None;
    let mut version = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim_matches('"').trim_matches('\'').trim();
        if value.is_empty() {
            continue;
        }
        if key.eq_ignore_ascii_case(name_key) {
            name = Some(value.to_owned());
        } else if key.eq_ignore_ascii_case(version_key) {
            version = Some(value.to_owned());
        }
    }
    let name = name?;
    let mut os = DetectedOs::new(OsFamily::Linux, name);
    if let Some(v) = version {
        os = os.with_version(v);
    }
    Some(os)
}

fn decode_utf8_trim(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}

const MAC_PLIST_PATH: &str = "/System/Library/CoreServices/SystemVersion.plist";

fn detect_macos(fs: &dyn OsFingerprint) -> anyhow::Result<Option<DetectedOs>> {
    let Some(bytes) = fs.read(MAC_PLIST_PATH)? else {
        return Ok(None);
    };
    let value = Value::from_reader(Cursor::new(bytes))
        .map_err(|err| anyhow::anyhow!("plist parse: {err}"))?;
    let Some(dict) = value.as_dictionary() else {
        return Ok(None);
    };
    let product = dict
        .get("ProductName")
        .and_then(Value::as_string)
        .map(str::to_owned);
    let version = dict
        .get("ProductVersion")
        .and_then(Value::as_string)
        .map(str::to_owned);
    let Some(product) = product else {
        return Ok(None);
    };
    let mut os = DetectedOs::new(OsFamily::MacOs, product);
    if let Some(v) = version {
        os = os.with_version(v);
    }
    Ok(Some(os))
}

const ESXI_BOOT_CFG: &str = "/boot.cfg";
const ESXI_RESOURCE_VERSION: &str = "/resourceGroups/version";

fn detect_esxi(fs: &dyn OsFingerprint) -> anyhow::Result<Option<DetectedOs>> {
    let Some(content) = fs.read(ESXI_BOOT_CFG)? else {
        return Ok(None);
    };
    let text = String::from_utf8_lossy(&content);
    let (mut title, mut build) = (None, None);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("title=") {
            title = Some(rest.trim().to_owned());
        } else if let Some(rest) = line.strip_prefix("build=") {
            build = Some(rest.trim().to_owned());
        }
    }
    let title = title.unwrap_or_default();
    if !title.contains("VMware ESXi") {
        return Ok(None);
    }
    let mut os = DetectedOs::new(OsFamily::Esxi, "VMware ESXi");
    if !title.is_empty() {
        os = os.with_extra("boot_title", title);
    }
    if let Some(build) = build {
        os = os.with_version(build);
    }
    if let Some(version_bytes) = fs.read(ESXI_RESOURCE_VERSION)? {
        let resource_version = decode_utf8_trim(&version_bytes);
        if !resource_version.is_empty() {
            os = os.with_extra("resource_version", resource_version);
        }
    }
    Ok(Some(os))
}

fn detect_mikrotik(fs: &dyn OsFingerprint) -> anyhow::Result<Option<DetectedOs>> {
    if let Some(version) = fs.read("/boot/VERSION")? {
        let text = decode_utf8_trim(&version);
        if !text.is_empty() {
            return Ok(Some(
                DetectedOs::new(OsFamily::MikroTik, "MikroTik RouterOS").with_version(text),
            ));
        }
    }
    if let Some(issue) = fs.read("/etc/issue")? {
        let text = String::from_utf8_lossy(&issue);
        if text.contains("MikroTik") || text.contains("RouterOS") {
            return Ok(Some(DetectedOs::new(
                OsFamily::MikroTik,
                "MikroTik RouterOS",
            )));
        }
    }
    Ok(None)
}

fn detect_openwrt(fs: &dyn OsFingerprint) -> anyhow::Result<Option<DetectedOs>> {
    let aarch64 = fs.exists("/efi/openwrt/bootaa64.efi")?;
    let vmlinux = fs.exists("/efi/openwrt/vmlinux")?;
    let grub = fs.exists("/efi/openwrt/grub.cfg")?;
    if !(aarch64 || vmlinux || grub) {
        return Ok(None);
    }
    let mut os = DetectedOs::new(OsFamily::OpenWrt, "OpenWrt");
    if aarch64 {
        os = os.with_architecture("arm64");
    }
    Ok(Some(os))
}

const DTALK_INFO: &str = "/dingtalk/dtalkinfo";

fn detect_dingtalk(fs: &dyn OsFingerprint) -> anyhow::Result<Option<DetectedOs>> {
    let Some(bytes) = fs.read(DTALK_INFO)? else {
        return Ok(None);
    };
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|err| anyhow::anyhow!("dtalkinfo json: {err}"))?;
    let brand = value
        .get("DevBrand")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let version = value
        .get("DevSoftversion")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let Some(brand) = brand else {
        return Ok(None);
    };
    let mut os = DetectedOs::new(OsFamily::DingTalk, brand);
    if let Some(v) = version {
        os = os.with_version(v);
    }
    Ok(Some(os))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory filesystem used by detector unit tests. Paths are
    /// normalised to a leading slash and forward separators on insert
    /// and on every query, so callers can mix `/etc/...`, `etc/...`, or
    /// `\etc\...` interchangeably.
    #[derive(Default)]
    pub(in crate::core) struct MockFingerprint {
        files: BTreeMap<String, Vec<u8>>,
        dirs: BTreeMap<String, ()>,
    }

    impl MockFingerprint {
        fn new() -> Self {
            Self::default()
        }

        fn with_file(mut self, path: &str, contents: impl Into<Vec<u8>>) -> Self {
            let path = normalize(path);
            if let Some(parent) = parent_of(&path) {
                self.dirs.insert(parent.to_owned(), ());
            }
            self.files.insert(path, contents.into());
            self
        }

        #[allow(dead_code)]
        fn with_dir(mut self, path: &str) -> Self {
            self.dirs.insert(normalize(path), ());
            self
        }
    }

    impl OsFingerprint for MockFingerprint {
        fn read(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(self.files.get(&normalize(path)).cloned())
        }

        fn exists(&self, path: &str) -> anyhow::Result<bool> {
            let key = normalize(path);
            Ok(self.files.contains_key(&key) || self.dirs.contains_key(&key))
        }

        fn is_dir(&self, path: &str) -> anyhow::Result<bool> {
            Ok(self.dirs.contains_key(&normalize(path)))
        }
    }

    fn normalize(path: &str) -> String {
        let translated = path.replace('\\', "/");
        let trimmed = translated.trim_matches('/');
        let mut out = String::with_capacity(trimmed.len() + 1);
        out.push('/');
        out.push_str(trimmed);
        out
    }

    fn parent_of(path: &str) -> Option<&str> {
        let trimmed = path.trim_end_matches('/');
        let idx = trimmed.rfind('/')?;
        Some(&trimmed[..idx.max(1)])
    }

    #[test]
    fn linux_detects_ubuntu_via_os_release() {
        let fs = MockFingerprint::new().with_file(
            "/etc/os-release",
            b"NAME=\"Ubuntu\"\nVERSION=\"22.04.3 LTS (Jammy Jellyfish)\"\n".to_vec(),
        );
        let hits = detect(&fs);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].family, OsFamily::Linux);
        assert_eq!(hits[0].product_name, "Ubuntu");
        assert_eq!(
            hits[0].version.as_deref(),
            Some("22.04.3 LTS (Jammy Jellyfish)")
        );
    }

    #[test]
    fn linux_detects_centos_via_redhat_release() {
        let fs = MockFingerprint::new().with_file(
            "/etc/redhat-release",
            b"CentOS Linux release 7.9.2009 (Core)\n".to_vec(),
        );
        let hits = detect(&fs);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].product_name, "CentOS Linux release 7.9.2009 (Core)");
    }

    #[test]
    fn linux_detects_debian_via_version_file() {
        let fs = MockFingerprint::new().with_file("/etc/debian_version", b"12.4\n".to_vec());
        let hits = detect(&fs);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].product_name, "Debian GNU/Linux");
        assert_eq!(hits[0].version.as_deref(), Some("12.4"));
    }

    #[test]
    fn macos_detects_from_xml_plist() {
        let plist_xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>ProductName</key>
    <string>macOS</string>
    <key>ProductVersion</key>
    <string>14.5</string>
</dict>
</plist>
"#;
        let fs = MockFingerprint::new().with_file(MAC_PLIST_PATH, plist_xml.to_vec());
        let hits = detect(&fs);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].family, OsFamily::MacOs);
        assert_eq!(hits[0].product_name, "macOS");
        assert_eq!(hits[0].version.as_deref(), Some("14.5"));
    }

    #[test]
    fn esxi_detects_from_boot_cfg() {
        let fs = MockFingerprint::new().with_file(
            "/boot.cfg",
            b"title=Loading VMware ESXi\nbuild=8.0.0-21495797\nupdated=1\n".to_vec(),
        );
        let hits = detect(&fs);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].family, OsFamily::Esxi);
        assert_eq!(hits[0].version.as_deref(), Some("8.0.0-21495797"));
    }

    #[test]
    fn esxi_ignores_non_esxi_boot_cfg() {
        let fs = MockFingerprint::new().with_file(
            "/boot.cfg",
            b"title=Linux Kernel\nbuild=irrelevant\n".to_vec(),
        );
        let hits = detect(&fs);
        assert!(hits.is_empty());
    }

    #[test]
    fn mikrotik_detects_via_boot_version() {
        let fs = MockFingerprint::new().with_file("/boot/VERSION", b"7.14.3\n".to_vec());
        let hits = detect(&fs);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].family, OsFamily::MikroTik);
        assert_eq!(hits[0].version.as_deref(), Some("7.14.3"));
    }

    #[test]
    fn mikrotik_detects_via_etc_issue() {
        let fs =
            MockFingerprint::new().with_file("/etc/issue", b"MikroTik RouterOS \\r \\l\n".to_vec());
        let hits = detect(&fs);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].family, OsFamily::MikroTik);
    }

    #[test]
    fn openwrt_detects_arm64() {
        let fs = MockFingerprint::new().with_file("/efi/openwrt/bootaa64.efi", b"".to_vec());
        let hits = detect(&fs);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].family, OsFamily::OpenWrt);
        assert_eq!(hits[0].architecture.as_deref(), Some("arm64"));
    }

    #[test]
    fn openwrt_detects_generic_via_grub() {
        let fs = MockFingerprint::new().with_file("/efi/openwrt/grub.cfg", b"".to_vec());
        let hits = detect(&fs);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].architecture, None);
    }

    #[test]
    fn dingtalk_detects_device() {
        let fs = MockFingerprint::new().with_file(
            "/dingtalk/dtalkinfo",
            br#"{"DevBrand":"DingTalk Mini","DevSoftversion":"3.2.1"}"#.to_vec(),
        );
        let hits = detect(&fs);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].family, OsFamily::DingTalk);
        assert_eq!(hits[0].product_name, "DingTalk Mini");
        assert_eq!(hits[0].version.as_deref(), Some("3.2.1"));
    }

    #[test]
    fn short_circuit_stops_at_first_match() {
        let fs = MockFingerprint::new()
            .with_file(
                "/etc/os-release",
                b"NAME=\"Debian GNU/Linux\"\nVERSION=\"12 (bookworm)\"\n".to_vec(),
            )
            .with_file("/efi/openwrt/grub.cfg", b"# secondary marker\n".to_vec());
        let hits = detect(&fs);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].family, OsFamily::Linux);
    }

    #[test]
    fn detect_all_collects_every_match() {
        let fs = MockFingerprint::new()
            .with_file("/etc/os-release", b"NAME=\"Debian GNU/Linux\"\n".to_vec())
            .with_file("/efi/openwrt/grub.cfg", b"".to_vec());
        let hits = detect_all(&fs);
        assert_eq!(hits.len(), 2);
        let families: Vec<OsFamily> = hits.iter().map(|h| h.family).collect();
        assert!(families.contains(&OsFamily::Linux));
        assert!(families.contains(&OsFamily::OpenWrt));
    }

    #[test]
    fn no_fingerprint_returns_nothing() {
        let fs = MockFingerprint::new();
        assert!(detect(&fs).is_empty());
    }

    /// Regression: a detector that does not call `with_extra` (e.g. the
    /// Linux detector for typical `/etc/os-release` images) must still
    /// produce JSON with `extra: {}` and `version: null` etc., not omit
    /// those fields. The frontend's `Object.entries(detection.extra)`
    /// throws on `undefined`, which crashes React without an error
    /// boundary and produces a white screen.
    #[test]
    fn detected_os_serializes_all_fields_even_when_empty() {
        let fs = MockFingerprint::new().with_file("/etc/os-release", b"NAME=\"Ubuntu\"\n".to_vec());
        let hits = detect(&fs);
        let json = serde_json::to_value(&hits[0]).expect("serialize");
        assert!(json.get("extra").is_some(), "extra must be present");
        assert!(json["extra"].is_object());
        assert!(json.get("version").is_some(), "version must be present");
        assert!(json.get("edition").is_some(), "edition must be present");
        assert!(
            json.get("architecture").is_some(),
            "architecture must be present"
        );
    }
}
