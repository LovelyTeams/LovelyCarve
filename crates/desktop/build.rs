//! Embed the application icon into `LovelyCarve.exe` on Windows. Explorer
//! shows the executable's first icon, and GPUI registers icon resource 1 as
//! its window class icon, which the taskbar and Alt+Tab show. GPUI Kit's own
//! build script already embeds the Windows manifest, so this resource script
//! holds the icon only.

fn main() {
    println!("cargo:rerun-if-changed=assets/icons/app.rc");
    println!("cargo:rerun-if-changed=assets/icons/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let embedded = embed_resource::compile(
        "assets/icons/app.rc",
        embed_resource::ParamsIncludeDirs(["assets/icons"]),
    )
    .manifest_required();
    if let Err(error) = embedded {
        panic!("failed to embed the application icon: {error}");
    }
}
