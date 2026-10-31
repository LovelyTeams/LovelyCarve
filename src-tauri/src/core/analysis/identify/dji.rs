//! DJI aircraft fingerprints shared by OS discovery and evidence platform
//! detection. A vendor property alone also occurs on controllers and cameras:
//! require the firmware property and both flight/navigation services together.
use std::collections::BTreeMap;

use super::os_detect::{DetectedOs, OsFamily, OsFingerprint};

const PROPERTY_CAP: usize = 64 * 1024;
const ROOTS: [(&str, &str, &str); 2] = [
    ("/build.prop", "/bin/dji_flight", "/bin/dji_navigation"),
    (
        "/system/build.prop",
        "/system/bin/dji_flight",
        "/system/bin/dji_navigation",
    ),
];

pub(super) struct Detection {
    pub os: DetectedOs,
    pub markers: [&'static str; 3],
}

pub(super) fn detect(fs: &dyn OsFingerprint) -> anyhow::Result<Option<Detection>> {
    for (properties_path, flight_path, navigation_path) in ROOTS {
        let Some(bytes) = fs.read_small(properties_path, PROPERTY_CAP)? else {
            continue;
        };
        let Some(properties) = parse_properties(&bytes) else {
            continue;
        };
        if !properties
            .get("ro.product.manufacturer")
            .is_some_and(|value| value.eq_ignore_ascii_case("DJI"))
        {
            continue;
        }
        let Some(firmware) = properties
            .get("dji.build.version")
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if !fs.exists(flight_path)?
            || fs.is_dir(flight_path)?
            || !fs.exists(navigation_path)?
            || fs.is_dir(navigation_path)?
        {
            continue;
        }
        let mut extra = BTreeMap::from([
            ("manufacturer".to_owned(), "DJI".to_owned()),
            ("build_properties".to_owned(), properties_path.to_owned()),
        ]);
        for (property, field) in [
            ("ro.product.device", "device"),
            ("ro.product.model", "model"),
            ("ro.build.version.release", "android_version"),
        ] {
            if let Some(value) = properties.get(property).filter(|value| !value.is_empty()) {
                extra.insert(field.to_owned(), value.clone());
            }
        }
        return Ok(Some(Detection {
            os: DetectedOs {
                family: OsFamily::Dji,
                product_name: "DJI drone".to_owned(),
                version: Some(firmware.clone()),
                edition: None,
                architecture: properties
                    .get("ro.product.cpu.abi")
                    .filter(|value| !value.is_empty())
                    .cloned(),
                extra,
            },
            markers: [properties_path, flight_path, navigation_path],
        }));
    }
    Ok(None)
}

fn parse_properties(bytes: &[u8]) -> Option<BTreeMap<String, String>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.contains('\0') {
        return None;
    }
    let mut properties = BTreeMap::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
    {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if !matches!(
            key,
            "ro.product.manufacturer"
                | "dji.build.version"
                | "ro.product.device"
                | "ro.product.model"
                | "ro.build.version.release"
                | "ro.product.cpu.abi"
        ) {
            continue;
        }
        if value.len() > 256 || value.chars().any(char::is_control) {
            return None;
        }
        if properties
            .insert(key.to_owned(), value.to_owned())
            .is_some_and(|previous| previous != value)
        {
            return None;
        }
    }
    Some(properties)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, sync::Arc};

    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::core::{
        analysis::identify::{
            autodetect::{self, EvidencePlatform},
            os_detect, systems,
        },
        filesystem::evidence_session::EvidenceSession,
    };

    const PROPERTIES: &str = "# invented aircraft firmware\nro.product.manufacturer=DJI\ndji.build.version=01.02.03.04\nro.product.device=air_test\nro.product.model=test-aircraft\nro.build.version.release=6.0\nro.product.cpu.abi=armeabi-v7a\n";

    #[derive(Default)]
    struct TestFs {
        files: BTreeMap<String, Vec<u8>>,
        directories: BTreeSet<String>,
    }

    impl TestFs {
        fn aircraft(prefix: &str) -> Self {
            Self {
                files: BTreeMap::from([
                    (
                        format!("{prefix}/build.prop"),
                        PROPERTIES.as_bytes().to_vec(),
                    ),
                    (format!("{prefix}/bin/dji_flight"), b"service".to_vec()),
                    (format!("{prefix}/bin/dji_navigation"), b"service".to_vec()),
                ]),
                ..Self::default()
            }
        }
    }

    impl OsFingerprint for TestFs {
        fn read(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(self.files.get(path).cloned())
        }
        fn exists(&self, path: &str) -> anyhow::Result<bool> {
            Ok(self.files.contains_key(path) || self.directories.contains(path))
        }
        fn is_dir(&self, path: &str) -> anyhow::Result<bool> {
            Ok(self.directories.contains(path))
        }
    }

    #[test]
    fn dji_detects_system_volume_and_extracted_system_tree() {
        for prefix in ["", "/system"] {
            let hit = detect(&TestFs::aircraft(prefix))
                .expect("probe")
                .expect("DJI aircraft");
            assert_eq!(hit.os.family, OsFamily::Dji);
            assert_eq!(hit.os.version.as_deref(), Some("01.02.03.04"));
            assert_eq!(hit.os.extra["device"], "air_test");
            assert_eq!(hit.os.extra["model"], "test-aircraft");
            assert_eq!(hit.os.extra["android_version"], "6.0");
            assert_eq!(hit.os.architecture.as_deref(), Some("armeabi-v7a"));
            assert_eq!(hit.markers[0], format!("{prefix}/build.prop"));
        }
    }

    #[test]
    fn dji_requires_vendor_firmware_and_flight_services_in_the_same_root() {
        for missing in ["/build.prop", "/bin/dji_flight", "/bin/dji_navigation"] {
            let mut fs = TestFs::aircraft("");
            fs.files.remove(missing);
            assert!(detect(&fs).expect("probe").is_none(), "missing {missing}");
        }
        for content in [
            "ro.product.manufacturer=Other\ndji.build.version=1.0\n",
            "# ro.product.manufacturer=DJI\ndji.build.version=1.0\n",
            "ro.product.manufacturer=DJI\ndji.build.version=\n",
            "ro.product.manufacturer=DJI\nro.product.manufacturer=Other\ndji.build.version=1.0\n",
        ] {
            let mut fs = TestFs::aircraft("");
            fs.files
                .insert("/build.prop".to_owned(), content.as_bytes().to_vec());
            assert!(detect(&fs).expect("probe").is_none());
        }
        let mut fs = TestFs::aircraft("");
        fs.files.remove("/bin/dji_navigation");
        fs.files
            .insert("/system/bin/dji_navigation".to_owned(), Vec::new());
        assert!(detect(&fs).expect("probe").is_none());
    }

    #[test]
    fn dji_rejects_a_phone_with_dji_apps_and_directories_named_after_services() {
        let mut fs = TestFs::default();
        fs.files
            .insert("/build.prop".to_owned(), PROPERTIES.as_bytes().to_vec());
        fs.directories.extend(
            [
                "/data/data/dji.go.v4",
                "/bin/dji_flight",
                "/bin/dji_navigation",
            ]
            .map(str::to_owned),
        );
        assert!(detect(&fs).expect("probe").is_none());
    }

    #[test]
    fn dji_handles_crlf_and_rejects_corrupt_or_oversized_properties() {
        let mut fs = TestFs::aircraft("");
        fs.files.insert(
            "/build.prop".to_owned(),
            format!("\u{feff}{}", PROPERTIES.replace('\n', "\r\n")).into_bytes(),
        );
        assert!(detect(&fs).expect("probe").is_some());
        for bytes in [vec![0xff], format!("{PROPERTIES}\0").into_bytes()] {
            fs.files.insert("/build.prop".to_owned(), bytes);
            assert!(detect(&fs).expect("probe").is_none());
        }
        fs.files
            .insert("/build.prop".to_owned(), vec![b'a'; PROPERTY_CAP + 1]);
        assert!(detect(&fs).is_err());
    }

    #[test]
    fn dji_takes_precedence_over_generic_linux_fingerprints() {
        let mut fs = TestFs::aircraft("");
        fs.files.insert(
            "/etc/os-release".to_owned(),
            b"NAME=Linux\nVERSION=1\n".to_vec(),
        );
        assert_eq!(os_detect::detect(&fs)[0].family, OsFamily::Dji);
        let families: Vec<_> = os_detect::detect_all(&fs)
            .into_iter()
            .map(|os| os.family)
            .collect();
        assert_eq!(families, vec![OsFamily::Dji, OsFamily::Linux]);
    }

    #[test]
    fn dji_folder_flows_through_platform_report_and_system_discovery() {
        let directory = tempfile::tempdir().expect("fixture directory");
        std::fs::create_dir_all(directory.path().join("system/bin")).expect("system directory");
        // AOSP 自带的应用名含 backup,不能把飞行器系统树改判成手机备份。
        std::fs::create_dir_all(directory.path().join("system/priv-app/SharedStorageBackup"))
            .expect("AOSP application directory");
        std::fs::write(directory.path().join("system/build.prop"), PROPERTIES).expect("properties");
        for service in ["dji_flight", "dji_navigation"] {
            std::fs::write(
                directory.path().join("system/bin").join(service),
                b"service",
            )
            .expect("service");
        }
        let image = Arc::new(EvidenceSession::open(directory.path()).expect("open folder"));
        let report =
            autodetect::detect_image(&image, &CancellationToken::new()).expect("platform report");
        assert_eq!(report.platform, Some(EvidencePlatform::Drone));
        assert_eq!(report.manufactor.as_deref(), Some("dji"));
        assert_eq!(report.display_platform_name, "无人机 DJI");
        assert!(!report.is_backup);
        assert_eq!(report.evidence_type, autodetect::EVIDENCE_TYPE_ARCHIVE);
        let detected = systems::discover(&image);
        assert_eq!(detected.len(), 1);
        assert_eq!(detected[0].family, "dji");
        assert_eq!(detected[0].version.as_deref(), Some("01.02.03.04"));
        assert!(detected[0].id.starts_with("dji@"));
    }

    #[test]
    #[ignore = "requires DJI_ORACLE_IMAGE pointing to a local DJI aircraft image"]
    fn dji_external_image_detection() {
        let path = std::env::var_os("DJI_ORACLE_IMAGE").expect("set DJI_ORACLE_IMAGE");
        let image = Arc::new(EvidenceSession::open(path).expect("open aircraft image"));
        let report = autodetect::detect_image(&image, &CancellationToken::new())
            .expect("platform detection");
        assert_eq!(report.platform, Some(EvidencePlatform::Drone));
        assert_eq!(report.manufactor.as_deref(), Some("dji"));
        let detected = systems::discover(&image);
        assert!(detected
            .iter()
            .any(|system| system.family == "dji" && system.version.is_some()));
    }
}
