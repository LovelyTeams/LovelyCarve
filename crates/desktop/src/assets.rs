//! The application's asset source: GPUI Kit's default component icons, the
//! Lucide catalog icons this app uses and the bundled file type icons.
use std::borrow::Cow;

use gpui_kit::{
    assets::{icon_assets, Assets as ComponentAssets},
    AssetSource, Result, SharedString,
};

// The catalog icons the views name that GPUI Kit's default component set
// lacks. An icon missing here still compiles, but renders blank and logs
// "could not find asset" on every frame; the test below keeps the list exact.
icon_assets!(pub ExtraIcons, [
    Archive,
    ArrowUpRight,
    Boxes,
    Briefcase,
    ChartColumn,
    CircleDashed,
    CircleStop,
    Clock,
    ClockArrowLeft,
    Columns3,
    Container,
    Database,
    Download,
    Drone,
    FileSearch,
    Files,
    FingerprintPattern,
    FolderSearch,
    FolderTree,
    Funnel,
    HardDriveDownload,
    Hash,
    KeyRound,
    Layers,
    Link,
    ListTodo,
    Locate,
    Lock,
    LockOpen,
    Monitor,
    Package,
    Phone,
    Puzzle,
    ScanSearch,
    Send,
    Server,
    Share2,
    ShieldAlert,
    ShieldCheck,
    Smartphone,
    Table2,
    Trash,
    Wifi,
    Wrench,
]);

// The file type icons in `assets/file_icons/`, served under the prefix that
// `file_icon` builds. A missing file fails the build; a file left out of the
// list fails the test below.
macro_rules! file_icons {
    ($($name:literal),* $(,)?) => {
        const FILE_ICONS: &[(&str, &[u8])] = &[$((
            concat!("lovelycarve/file_icons/", $name, ".svg"),
            include_bytes!(concat!("../assets/file_icons/", $name, ".svg")),
        )),*];
    };
}

file_icons![
    "archive",
    "astro",
    "audio",
    "ballerina",
    "binary",
    "book",
    "bun",
    "c",
    "cairo",
    "chevron_down",
    "chevron_right",
    "code",
    "coffeescript",
    "cpp",
    "css",
    "dart",
    "database",
    "diff",
    "docker",
    "editorconfig",
    "elixir",
    "elm",
    "envelope",
    "erlang",
    "eslint",
    "file",
    "folder",
    "folder_open",
    "font",
    "fsharp",
    "git",
    "gitlab",
    "gleam",
    "go",
    "graphql",
    "haskell",
    "hcl",
    "helm",
    "heroku",
    "html",
    "image",
    "info",
    "java",
    "javascript",
    "julia",
    "jupyter",
    "kdl",
    "kotlin",
    "lock",
    "lua",
    "luau",
    "metal",
    "nim",
    "nix",
    "ocaml",
    "odin",
    "php",
    "prettier",
    "prisma",
    "puppet",
    "python",
    "r",
    "react",
    "roc",
    "ruby",
    "rust",
    "sass",
    "scala",
    "settings",
    "surrealql",
    "swift",
    "tcl",
    "terminal",
    "terraform",
    "toml",
    "typescript",
    "v",
    "video",
    "vue",
    "vyper",
    "wgsl",
    "yaml",
    "zig",
];

/// The asset path of the bundled file type icon `name`.
pub fn file_icon(name: &str) -> SharedString {
    format!("lovelycarve/file_icons/{name}.svg").into()
}

/// The names of the bundled file type icons.
#[cfg(test)]
pub fn file_icon_names() -> impl Iterator<Item = &'static str> {
    FILE_ICONS.iter().filter_map(|(path, _)| {
        path.strip_prefix("lovelycarve/file_icons/")?
            .strip_suffix(".svg")
    })
}

/// Serves the selected catalog icons first, then GPUI Kit's defaults. The
/// order matters: the default source errors on paths it does not know.
pub struct AppAssets;

pub const BRAND_MARK: &str = "lovelycarve/brand.png";
pub const BRAND_TILE: &str = "lovelycarve/brand-tile.png";
pub const EVIDENCE_TRAY_LIGHT: &str = "lovelycarve/evidence-tray-light.svg";
pub const EVIDENCE_TRAY_DARK: &str = "lovelycarve/evidence-tray-dark.svg";
pub const EVIDENCE_SHEET_LIGHT: &str = "lovelycarve/evidence-sheet-light.svg";
pub const EVIDENCE_SHEET_DARK: &str = "lovelycarve/evidence-sheet-dark.svg";
pub const EVIDENCE_SECTOR: &str = "lovelycarve/evidence-sector.svg";

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let artwork = match path {
            EVIDENCE_TRAY_LIGHT | EVIDENCE_TRAY_DARK => {
                Some(include_str!("workspace/evidence-tray.svg"))
            }
            EVIDENCE_SHEET_LIGHT | EVIDENCE_SHEET_DARK => {
                Some(include_str!("workspace/evidence-sheet.svg"))
            }
            EVIDENCE_SECTOR => {
                return Ok(Some(Cow::Borrowed(include_bytes!(
                    "workspace/evidence-sector.svg"
                ))));
            }
            _ => None,
        };
        if let Some(artwork) = artwork {
            let dark = matches!(path, EVIDENCE_TRAY_DARK | EVIDENCE_SHEET_DARK);
            return Ok(Some(Cow::Owned(crate::theme::evidence_artwork(
                artwork, dark,
            ))));
        }
        let brand: Option<&[u8]> = match path {
            BRAND_MARK => Some(include_bytes!("../assets/app-icon.png")),
            BRAND_TILE => Some(include_bytes!("../assets/app-icon-background.png")),
            _ => None,
        };
        if let Some(bytes) = brand {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if let Some((_, bytes)) = FILE_ICONS.iter().find(|(file, _)| *file == path) {
            return Ok(Some(Cow::Borrowed(*bytes)));
        }
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        ComponentAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = ComponentAssets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.extend(
            FILE_ICONS
                .iter()
                .map(|(file, _)| *file)
                .filter(|file| file.starts_with(path))
                .map(SharedString::from),
        );
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::{AppAssets, ComponentAssets, ExtraIcons};
    use gpui_kit::{assets::IconName, AssetSource as _};
    use std::{collections::BTreeSet, fs, path::PathBuf};

    fn view_sources() -> Vec<String> {
        let mut pending = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut sources = Vec::new();
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    sources.push(fs::read_to_string(&path).unwrap());
                }
            }
        }
        sources
    }

    /// The asset paths of the icons the sources name through `IconName` or
    /// an alias of it (`type Alias = …IconName;`, `IconName as Alias`). A
    /// name outside the catalog comes back as a placeholder that never loads.
    fn named_icon_paths(sources: &[String]) -> BTreeSet<String> {
        let identifier_byte = |byte: &u8| byte.is_ascii_alphanumeric() || *byte == b'_';
        let mut aliases = BTreeSet::from(["IconName"]);
        for line in sources.iter().flat_map(|source| source.lines()) {
            let line = line.trim();
            let declared = line
                .strip_prefix("type ")
                .filter(|declaration| declaration.ends_with("IconName;"))
                .and_then(|declaration| declaration.split_once('='))
                .map(|(alias, _)| alias.trim());
            let renamed = line
                .split_once("IconName as ")
                .map(|(_, alias)| alias.trim_end_matches([',', ';', '}']).trim());
            aliases.extend(declared.into_iter().chain(renamed).filter(|alias| {
                !alias.is_empty() && alias.bytes().all(|byte| identifier_byte(&byte))
            }));
        }
        let mut names = BTreeSet::new();
        for source in sources {
            let bytes = source.as_bytes();
            for (separator, _) in source.match_indices("::") {
                let start = bytes[..separator]
                    .iter()
                    .rposition(|byte| !identifier_byte(byte))
                    .map_or(0, |position| position + 1);
                let name_start = separator + 2;
                let end = bytes[name_start..]
                    .iter()
                    .position(|byte| !identifier_byte(byte))
                    .map_or(bytes.len(), |position| name_start + position);
                let name = &source[name_start..end];
                if aliases.contains(&source[start..separator])
                    && name.starts_with(|first: char| first.is_ascii_uppercase())
                    && name != "ALL"
                {
                    names.insert(name);
                }
            }
        }
        names
            .into_iter()
            .map(|name| {
                IconName::ALL
                    .iter()
                    .find(|icon| format!("{icon:?}") == name)
                    .map_or_else(
                        || format!("<unknown icon {name}>"),
                        |icon| icon.path().to_string(),
                    )
            })
            .collect()
    }

    /// Every icon the views name must load, and `ExtraIcons` must hold just
    /// the named ones GPUI Kit's default component icons do not cover.
    #[test]
    fn extra_icons_match_the_icons_the_views_name() {
        let named = named_icon_paths(&view_sources());
        let unserved: Vec<&str> = named
            .iter()
            .map(String::as_str)
            .filter(|path| !matches!(AppAssets.load(path), Ok(Some(_))))
            .collect();
        let unneeded: Vec<String> = ExtraIcons
            .list("")
            .unwrap()
            .into_iter()
            .map(|path| path.to_string())
            .filter(|path| {
                !named.contains(path) || matches!(ComponentAssets.load(path), Ok(Some(_)))
            })
            .collect();
        assert_eq!(
            (unserved, unneeded),
            (Vec::<&str>::new(), Vec::<String>::new())
        );
    }

    #[test]
    fn served_file_icons_are_the_svg_files_in_their_directory() {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/file_icons");
        let mut on_disk: Vec<String> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "svg"))
            .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        on_disk.sort();
        let mut served: Vec<String> = super::file_icon_names().map(str::to_owned).collect();
        served.sort();
        assert_eq!(served, on_disk);
        for name in &served {
            assert!(
                matches!(AppAssets.load(&super::file_icon(name)), Ok(Some(_))),
                "{name}"
            );
        }
    }
}
