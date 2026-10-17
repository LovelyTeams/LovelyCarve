//! The documentation `docs search` and `docs get` read offline: the user
//! guides and every format guide under `../docs`, embedded at build time.
use std::{error::Error, fs, path::Path};

/// User guides shipped with the CLI, relative to `docs/`; every page under
/// `docs/formats/` is added to them.
const GUIDES: &[&str] = &["USAGE.md", "CLI.md", "IOS_BACKUPS.md", "LINUXINFO.md"];

pub fn emit(manifest_dir: &Path, out: &Path) -> Result<(), Box<dyn Error>> {
    let root = manifest_dir.join("..").join("docs");
    let formats = root.join("formats");
    println!("cargo:rerun-if-changed={}", formats.display());
    let mut pages: Vec<String> = GUIDES.iter().map(|page| (*page).to_owned()).collect();
    let mut format_pages = Vec::new();
    for entry in fs::read_dir(&formats)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name.ends_with(".md") {
            format_pages.push(format!("formats/{name}"));
        }
    }
    format_pages.sort();
    pages.extend(format_pages);
    let mut source =
        String::from("/// Embedded pages: the path under `docs/` and the Markdown.\npub static PAGES: &[(&str, &str)] = &[\n");
    for page in &pages {
        let file = root.join(page);
        if !file.is_file() {
            return Err(format!("missing documentation page docs/{page}").into());
        }
        println!("cargo:rerun-if-changed={}", file.display());
        let file = file.to_str().ok_or("documentation path is not UTF-8")?;
        source.push_str(&format!("    ({page:?}, include_str!({file:?})),\n"));
    }
    source.push_str("];\n");
    fs::write(out.join("cli_docs.rs"), source)?;
    Ok(())
}
