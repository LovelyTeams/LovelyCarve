#[path = "build/cli_catalog.rs"]
mod cli_catalog;

fn main() {
    if let Err(error) = cli_catalog::generate() {
        panic!("failed to generate the CLI:\n{error}");
    }
    embed_cli_manifest();
}

/// The CLI embeds its own Windows manifest and runs `asInvoker`, so pipes and
/// normal image analysis never trigger UAC.
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
