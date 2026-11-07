#[path = "build/cli_catalog.rs"]
mod cli_catalog;

fn main() {
    export_app_identifier();
    if let Err(error) = cli_catalog::generate() {
        panic!("failed to generate the CLI:\n{error}");
    }
    #[cfg(feature = "desktop")]
    desktop::build();
    #[cfg(not(feature = "desktop"))]
    embed_cli_manifest();
    stage_gpui_runtime();
}

/// The GPUI desktop app has no bundler to place its load-time libraries
/// (Dokan, OpenSSL and the VC++ runtime from `binaries/`, which Tauri copies
/// as bundle resources) beside the executable. Copy them into the profile
/// directory, so `cargo run`, `just dev-gpui` and a double-clicked build all
/// start instead of failing with "DLL not found".
fn stage_gpui_runtime() {
    const LIBRARIES: [&str; 6] = [
        "dokan2.dll",
        "libcrypto-4-x64.dll",
        "msvcp140.dll",
        "msvcp140_1.dll",
        "vcruntime140.dll",
        "vcruntime140_1.dll",
    ];
    if std::env::var_os("CARGO_FEATURE_GPUI").is_none()
        || std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
    {
        return;
    }
    let (Some(manifest_dir), Some(out_dir)) = (
        std::env::var_os("CARGO_MANIFEST_DIR"),
        std::env::var_os("OUT_DIR"),
    ) else {
        return;
    };
    // OUT_DIR is <target>/<profile>/build/<package>-<hash>/out.
    let Some(profile_dir) = std::path::Path::new(&out_dir).ancestors().nth(3) else {
        return;
    };
    let source_dir = std::path::Path::new(&manifest_dir).join("binaries");
    for name in LIBRARIES {
        let source = source_dir.join(name);
        println!("cargo:rerun-if-changed={}", source.display());
        let target = profile_dir.join(name);
        let up_to_date = match (std::fs::metadata(&source), std::fs::metadata(&target)) {
            (Ok(source), Ok(target)) => {
                source.len() == target.len() && source.modified().ok() <= target.modified().ok()
            }
            _ => false,
        };
        if up_to_date {
            continue;
        }
        if let Err(error) = std::fs::copy(&source, &target) {
            println!(
                "cargo:warning=could not stage {} beside the GPUI app: {error}",
                source.display()
            );
        }
    }
}

/// The CLI resolves the same per-user data directory as the desktop app,
/// which Tauri derives from the bundle identifier.
fn export_app_identifier() {
    println!("cargo:rerun-if-changed=tauri.conf.json");
    let config = std::fs::read_to_string("tauri.conf.json").expect("read tauri.conf.json");
    let config: serde_json::Value = serde_json::from_str(&config).expect("parse tauri.conf.json");
    let identifier = config["identifier"]
        .as_str()
        .expect("tauri.conf.json has an identifier");
    println!("cargo:rustc-env=LOVELYCARVE_APP_IDENTIFIER={identifier}");
}

/// Without the desktop runtime, nothing else embeds a Windows manifest: the
/// standalone CLI runs `asInvoker` so pipes and normal image analysis never
/// trigger UAC.
#[cfg(not(feature = "desktop"))]
fn embed_cli_manifest() {
    println!("cargo:rerun-if-changed=cli.manifest");
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !windows_msvc {
        return;
    }
    let manifest =
        std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"))
            .join("cli.manifest");
    println!("cargo:rustc-link-arg-bin=lovelycarve-cli=/MANIFEST:EMBED");
    // cli.manifest already supplies asInvoker. LLD 20 can namespace the level
    // attribute incorrectly when merging it with its generated UAC fragment.
    println!("cargo:rustc-link-arg-bin=lovelycarve-cli=/MANIFESTUAC:NO");
    println!(
        "cargo:rustc-link-arg-bin=lovelycarve-cli=/MANIFESTINPUT:{}",
        manifest.display()
    );
}

#[cfg(feature = "desktop")]
mod desktop {
    pub fn build() {
        // Every profile runs with the caller's privileges. Users can explicitly
        // run as administrator when accessing physical disks or live memory.
        // app_manifest replaces the default XML, so preserve Common-Controls.
        let attributes = tauri_build::Attributes::new().plugin(
            "preview-window",
            tauri_build::InlinedPlugin::new().commands(&["open"]),
        );
        #[cfg(target_os = "windows")]
        let attributes = {
            let windows = tauri_build::WindowsAttributes::new().app_manifest(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#,
            );
            attributes.windows_attributes(windows)
        };
        tauri_build::try_build(attributes).expect("failed to run tauri-build");
    }
}
