//! File and directory glyphs shared by the browser and preview.
use gpui_kit::{
    assets::IconName as Glyph,
    component::{ActiveTheme as _, Icon, IconName},
    px, App, Styled,
};
use lovelycarve_lib::core::{
    catalog::flat_query::flat_ext_of,
    filesystem::tree::fs_tree::{FsNode, NodeKind},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileIcon {
    Folder,
    Volume,
    Container,
    Snapshot,
    Stream,
    Unallocated,
    Link,
    Image,
    Video,
    Audio,
    Document,
    Spreadsheet,
    Code,
    Archive,
    Executable,
    Database,
    Mail,
    File,
}

impl FileIcon {
    fn special(id: &str) -> Option<Self> {
        if id.starts_with("refs-snap:") {
            Some(Self::Snapshot)
        } else if id.starts_with("ntfs-ads:") {
            Some(Self::Stream)
        } else if id.starts_with("unalloc:") || id.starts_with("diskgap:") {
            Some(Self::Unallocated)
        } else if id.strip_prefix("host-docker:").is_some_and(|path| {
            path.is_empty()
                || path
                    .strip_suffix(':')
                    .is_some_and(|name| !name.contains(':'))
        }) {
            Some(Self::Container)
        } else if id.strip_prefix("part:").is_some_and(numeric)
            || id
                .strip_prefix("lvm:")
                .and_then(|id| id.split_once(':'))
                .is_some_and(|(partition, volume)| numeric(partition) && numeric(volume))
        {
            Some(Self::Volume)
        } else {
            None
        }
    }

    fn for_node(node: &FsNode) -> Self {
        Self::special(&node.id).unwrap_or_else(|| match node.kind {
            NodeKind::Dir => Self::Folder,
            NodeKind::Symlink => Self::Link,
            NodeKind::Unallocated => Self::Unallocated,
            NodeKind::File => Self::for_filename(&node.name, &flat_ext_of(node)),
        })
    }

    fn for_filename(name: &str, suffix: &str) -> Self {
        // Conventional names take priority, following Zed's stem-before-suffix
        // convention. Type hints stay name-only, with no evidence byte reads.
        let stem = name.trim_start_matches('.');
        if [
            "Makefile",
            "Dockerfile",
            "Containerfile",
            "gitignore",
            "gitattributes",
            "editorconfig",
            "env",
        ]
        .iter()
        .any(|alias| stem.eq_ignore_ascii_case(alias))
        {
            return Self::Code;
        }
        if ["README", "LICENSE", "CHANGELOG"]
            .iter()
            .any(|alias| name.eq_ignore_ascii_case(alias))
        {
            return Self::Document;
        }

        match suffix {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "avif" | "svg" | "tif"
            | "tiff" | "heic" => Self::Image,
            "mp4" | "webm" | "mov" | "mkv" | "avi" | "m4v" | "wmv" => Self::Video,
            "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" | "wma" => Self::Audio,
            "md" | "markdown" | "mdown" | "mkd" | "pdf" | "ofd" | "doc" | "docx" | "odt"
            | "rtf" | "ppt" | "pptx" | "odp" | "pages" | "key" | "wps" => Self::Document,
            "csv" | "tsv" | "xls" | "xlsx" | "xlsm" | "xlsb" | "ods" | "et" => Self::Spreadsheet,
            "zip" | "rar" | "7z" | "gz" | "bz2" | "xz" | "zst" | "lz4" | "tar" | "tgz" | "tbz2"
            | "txz" | "cab" | "wim" | "arj" | "jar" | "war" | "apk" | "ipa" | "a" | "ar"
            | "lib" => Self::Archive,
            "dd" | "img" | "raw" | "iso" | "e01" | "ex01" | "aff" | "aff4" | "ad1" | "vhd"
            | "vhdx" | "vmdk" | "vdi" | "qcow" | "qcow2" | "dmg" => Self::Volume,
            "exe" | "dll" | "sys" | "com" | "scr" | "msi" | "drv" | "ocx" | "cpl" | "efi"
            | "mui" | "elf" | "so" | "dylib" | "bin" | "o" | "ko" => Self::Executable,
            "db" | "db3" | "sqlite" | "sqlite3" | "mdb" | "accdb" | "edb" | "frm" | "ibd"
            | "realm" | "bcp" => Self::Database,
            "eml" | "msg" | "pst" | "ost" | "mbox" => Self::Mail,
            "txt" | "text" | "log" | "conf" | "cfg" | "ini" | "env" | "properties" | "json"
            | "json5" | "jsonc" | "xml" | "yaml" | "yml" | "toml" | "plist" | "js" | "mjs"
            | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" | "vue" | "svelte" | "py" | "pyi"
            | "rb" | "php" | "pl" | "lua" | "r" | "rs" | "go" | "c" | "h" | "cc" | "cpp"
            | "cxx" | "hpp" | "hxx" | "cs" | "java" | "kt" | "kts" | "swift" | "m" | "mm"
            | "scala" | "clj" | "ex" | "exs" | "erl" | "hs" | "dart" | "sh" | "bash" | "zsh"
            | "fish" | "ps1" | "psm1" | "bat" | "cmd" | "sql" | "graphql" | "gql" | "proto"
            | "html" | "htm" | "css" | "scss" | "sass" | "less" | "styl" | "dockerfile"
            | "makefile" | "cmake" | "gradle" | "gitignore" | "gitattributes" | "editorconfig" => {
                Self::Code
            }
            _ => Self::File,
        }
    }

    fn render(self, expanded: bool, cx: &App) -> Icon {
        let theme = cx.theme();
        let (icon, color) = match self {
            Self::Folder => (
                Icon::new(if expanded {
                    IconName::FolderOpen
                } else {
                    IconName::Folder
                }),
                theme.warning,
            ),
            Self::Volume => (Icon::new(IconName::HardDrive), theme.info),
            Self::Container => (Icon::new(Glyph::Container), theme.info),
            Self::Snapshot => (Icon::new(Glyph::ClockArrowLeft), theme.warning),
            Self::Stream => (Icon::new(Glyph::Layers), theme.warning),
            Self::Unallocated => (Icon::new(Glyph::CircleDashed), theme.muted_foreground),
            Self::Link => (Icon::new(Glyph::Link), theme.muted_foreground),
            Self::Image => (Icon::new(Glyph::FileImage), theme.success),
            Self::Video => (Icon::new(Glyph::FilePlay), theme.info),
            Self::Audio => (Icon::new(Glyph::FileMusic), theme.info),
            Self::Document => (Icon::new(Glyph::FileText), theme.primary),
            Self::Spreadsheet => (Icon::new(Glyph::FileSpreadsheet), theme.success),
            Self::Code => (Icon::new(Glyph::FileCode), theme.accent_foreground),
            Self::Archive => (Icon::new(Glyph::FileArchive), theme.warning),
            Self::Executable => (Icon::new(Glyph::FileCog), theme.danger),
            Self::Database => (Icon::new(Glyph::Database), theme.warning),
            Self::Mail => (Icon::new(Glyph::Mail), theme.info),
            Self::File => (Icon::new(IconName::File), theme.muted_foreground),
        };
        icon.size(px(16.)).text_color(color)
    }
}

fn numeric(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn node_icon(node: &FsNode, cx: &App) -> Icon {
    FileIcon::for_node(node).render(false, cx)
}

pub(super) fn directory_icon(id: &str, expanded: bool, cx: &App) -> Icon {
    FileIcon::special(id)
        .unwrap_or(FileIcon::Folder)
        .render(expanded, cx)
}

#[cfg(test)]
mod tests {
    use super::FileIcon;

    #[test]
    fn file_icon_recognizes_special_names_and_foreign_formats() {
        assert_eq!(FileIcon::for_filename(".GITIGNORE", ""), FileIcon::Code);
        assert_eq!(FileIcon::for_filename("Dockerfile", ""), FileIcon::Code);
        assert_eq!(FileIcon::for_filename("README", ""), FileIcon::Document);
        assert_eq!(
            FileIcon::for_filename("capture.E01", "e01"),
            FileIcon::Volume
        );
        assert_eq!(
            FileIcon::for_filename("backup.tar.gz", "gz"),
            FileIcon::Archive
        );
        assert_eq!(
            FileIcon::for_filename("report.csv", "csv"),
            FileIcon::Spreadsheet
        );
        assert_eq!(
            FileIcon::for_filename("entry.unknown", "unknown"),
            FileIcon::File
        );
    }

    #[test]
    fn special_nodes_do_not_rely_on_the_display_name() {
        assert_eq!(FileIcon::special("part:0"), Some(FileIcon::Volume));
        assert_eq!(FileIcon::special("lvm:0:1"), Some(FileIcon::Volume));
        assert_eq!(FileIcon::special("part:0:entry"), None);
        assert_eq!(
            FileIcon::special("host-docker:example:"),
            Some(FileIcon::Container)
        );
        assert_eq!(FileIcon::special("host-docker:example:root"), None);
        assert_eq!(FileIcon::special("ntfs-ads:1"), Some(FileIcon::Stream));
        assert_eq!(FileIcon::special("refs-snap:1"), Some(FileIcon::Snapshot));
        assert_eq!(FileIcon::special("diskgap:0"), Some(FileIcon::Unallocated));
    }
}
