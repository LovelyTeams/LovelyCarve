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
        // Override tauri-build's default Windows application manifest with one
        // that requests Administrator elevation at launch — but only in release
        // builds. lovelycarve needs admin for raw block-device reads
        // (`\\.\PhysicalDriveN`), live memory capture (BitLocker VMK scan), and
        // Dokan mount control. Shipping the elevated manifest in the installer
        // means a single UAC prompt at first launch and no `ERROR_ACCESS_DENIED`
        // surprises afterwards.
        //
        // Debug builds (`cargo run`, `bun run tauri dev`) keep the default
        // `asInvoker` manifest so the development loop doesn't trigger a UAC
        // prompt on every restart — devs can right-click → Run as administrator
        // when they need to test the privileged code paths.
        //
        // We have to inline the full manifest XML (rather than appending the
        // trustInfo block) because tauri-build's `app_manifest` setter replaces
        // the default outright — the Common-Controls dependency must be
        // preserved here too.
        let mut attributes = tauri_build::Attributes::new();
        #[cfg(target_os = "windows")]
        {
            let is_release = std::env::var("PROFILE").as_deref() == Ok("release");
            if is_release {
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
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#,
                );
                attributes = attributes.windows_attributes(windows);
            }
            // Re-run when the active profile flips so the manifest stays in sync.
            println!("cargo:rerun-if-env-changed=PROFILE");
        }
        tauri_build::try_build(attributes).expect("failed to run tauri-build");
    }
}
