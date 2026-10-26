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
        let attributes = tauri_build::Attributes::new();
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
