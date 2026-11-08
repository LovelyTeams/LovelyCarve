//! Every executable that embeds the backend (the CLI, the desktop app and
//! their tests) loads Dokan, OpenSSL and the app-local VC++ runtime from
//! `binaries/` at the repository root. Stage them into the profile directory
//! so `cargo run`, the `just` recipes and a double-clicked debug build start
//! instead of failing with "DLL not found".
fn main() {
    const LIBRARIES: [&str; 6] = [
        "dokan2.dll",
        "libcrypto-4-x64.dll",
        "msvcp140.dll",
        "msvcp140_1.dll",
        "vcruntime140.dll",
        "vcruntime140_1.dll",
    ];
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
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
    let source_dir = std::path::Path::new(&manifest_dir).join("../../binaries");
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
                "cargo:warning=could not stage {} beside the executables: {error}",
                source.display()
            );
        }
    }
}
