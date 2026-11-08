//! File and directory icons shared by the file list, the directory tree and
//! the detail view, all drawn in the muted foreground. A file's icon follows
//! `file_types`: its whole name first, then the text after each dot from the
//! longest down, then the part before the first dot. Evidence structures such
//! as partitions, streams and unallocated space keep their own glyphs.
mod file_types;

use std::{collections::HashMap, sync::LazyLock};

use gpui_kit::{
    assets::IconName as Glyph,
    component::{ActiveTheme as _, Icon},
    px, App, Styled,
};
use lovelycarve_backend::core::{
    catalog::flat_query::flat_ext_of,
    filesystem::tree::fs_tree::{FsNode, NodeKind},
};

use super::source::source_language;
use crate::assets::file_icon;

/// What an entry draws ahead of its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Artwork {
    /// A bundled file type icon, by name.
    File(&'static str),
    /// A catalog glyph, for what the file type icons do not cover.
    Glyph(Glyph),
}

const FOLDER: Artwork = Artwork::File("folder");
const FOLDER_OPEN: Artwork = Artwork::File("folder_open");
const CHEVRON_COLLAPSED: Artwork = Artwork::File("chevron_right");
const CHEVRON_EXPANDED: Artwork = Artwork::File("chevron_down");
/// Text the source preview can highlight that no association names.
const SOURCE: Artwork = Artwork::File("code");
const DEFAULT: Artwork = Artwork::File("file");

/// The file type tables, keyed by lowercase name.
struct Associations {
    stems: HashMap<String, &'static str>,
    suffixes: HashMap<String, &'static str>,
    artwork: HashMap<&'static str, Artwork>,
}

fn by_name(table: &[(&'static str, &[&str])]) -> HashMap<String, &'static str> {
    table
        .iter()
        .flat_map(|&(key, names)| {
            names
                .iter()
                .map(move |name| (name.to_ascii_lowercase(), key))
        })
        .collect()
}

static ASSOCIATIONS: LazyLock<Associations> = LazyLock::new(|| Associations {
    stems: by_name(file_types::STEMS),
    suffixes: by_name(file_types::SUFFIXES),
    artwork: file_types::ARTWORK.iter().copied().collect(),
});

impl Associations {
    /// The artwork of the key a whole name or a suffix is associated with.
    fn lookup(&self, candidate: &str) -> Option<Artwork> {
        self.stems
            .get(candidate)
            .or_else(|| self.suffixes.get(candidate))
            .and_then(|key| self.artwork.get(key))
            .copied()
    }

    fn file(&self, name: &str) -> Option<Artwork> {
        let name = name.to_ascii_lowercase();
        if let Some(artwork) = self.lookup(&name) {
            return Some(artwork);
        }
        // `a.b.c` tries `b.c`, then `c`, then the stem `a`, so a suffix wins
        // over a known stem: `Dockerfile.rs` is Rust, `Dockerfile.prod` is
        // still a Dockerfile.
        let mut rest = name.as_str();
        let mut stem = None;
        while let Some((prefix, suffix)) = rest.split_once('.') {
            stem.get_or_insert(prefix);
            if let Some(artwork) = self.lookup(suffix) {
                return Some(artwork);
            }
            rest = suffix;
        }
        stem.and_then(|stem| self.lookup(stem))
    }
}

/// Evidence structures, recognised by node id rather than by name.
fn structure(id: &str) -> Option<Artwork> {
    let glyph = if id.starts_with("refs-snap:") {
        Glyph::ClockArrowLeft
    } else if id.starts_with("ntfs-ads:") {
        Glyph::Layers
    } else if id.starts_with("unalloc:") || id.starts_with("diskgap:") {
        Glyph::CircleDashed
    } else if id.strip_prefix("host-docker:").is_some_and(|path| {
        path.is_empty()
            || path
                .strip_suffix(':')
                .is_some_and(|name| !name.contains(':'))
    }) {
        Glyph::Container
    } else if id.strip_prefix("part:").is_some_and(numeric)
        || id
            .strip_prefix("lvm:")
            .and_then(|id| id.split_once(':'))
            .is_some_and(|(partition, volume)| numeric(partition) && numeric(volume))
    {
        Glyph::HardDrive
    } else {
        return None;
    };
    Some(Artwork::Glyph(glyph))
}

fn numeric(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// A file's artwork from its name; text the source preview can highlight
/// falls back to the source icon.
fn file_artwork(name: &str, suffix: &str) -> Artwork {
    ASSOCIATIONS.file(name).unwrap_or_else(|| {
        if source_language(name, suffix).is_some() {
            SOURCE
        } else {
            DEFAULT
        }
    })
}

fn node_artwork(node: &FsNode) -> Artwork {
    structure(&node.id).unwrap_or_else(|| match node.kind {
        NodeKind::Dir => FOLDER,
        NodeKind::Symlink => Artwork::Glyph(Glyph::Link),
        NodeKind::Unallocated => Artwork::Glyph(Glyph::CircleDashed),
        NodeKind::File => file_artwork(&node.name, &flat_ext_of(node)),
    })
}

fn icon(artwork: Artwork, cx: &App) -> Icon {
    match artwork {
        Artwork::File(name) => Icon::empty().path(file_icon(name)),
        Artwork::Glyph(glyph) => Icon::new(glyph),
    }
    .size(px(16.))
    .text_color(cx.theme().muted_foreground)
}

pub(super) fn node_icon(node: &FsNode, cx: &App) -> Icon {
    icon(node_artwork(node), cx)
}

pub(super) fn directory_icon(id: &str, expanded: bool, cx: &App) -> Icon {
    let folder = if expanded { FOLDER_OPEN } else { FOLDER };
    icon(structure(id).unwrap_or(folder), cx)
}

/// The disclosure chevron of a tree row.
pub(super) fn chevron_icon(expanded: bool, cx: &App) -> Icon {
    icon(
        if expanded {
            CHEVRON_EXPANDED
        } else {
            CHEVRON_COLLAPSED
        },
        cx,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use gpui_kit::assets::IconName as Glyph;

    use super::{
        file_artwork, file_types, structure, Artwork, ASSOCIATIONS, CHEVRON_COLLAPSED,
        CHEVRON_EXPANDED, DEFAULT, FOLDER, FOLDER_OPEN, SOURCE,
    };

    #[test]
    fn file_names_resolve_through_their_associations() {
        let cases = [
            ("mod.rs", "rs", Artwork::File("rust")),
            ("Dockerfile.rs", "rs", Artwork::File("rust")),
            ("go.mod", "mod", Artwork::File("go")),
            ("Dockerfile", "", Artwork::File("docker")),
            ("Dockerfile.prod", "prod", Artwork::File("docker")),
            ("eslint.config.js", "js", Artwork::File("eslint")),
            (".GITIGNORE", "", Artwork::File("git")),
            ("IMG_0001.JPG", "jpg", Artwork::File("image")),
            ("backup.tar.gz", "gz", Artwork::File("archive")),
            ("capture.E01", "e01", Artwork::Glyph(Glyph::HardDrive)),
            ("KERNEL32.DLL", "dll", Artwork::File("binary")),
            ("Outlook.pst", "pst", Artwork::File("envelope")),
            ("report.csv", "csv", Artwork::File("database")),
            ("notes.txt", "txt", Artwork::File("book")),
            ("setup.log", "log", Artwork::File("info")),
            ("build.gradle", "gradle", SOURCE),
            ("entry.unknown", "unknown", DEFAULT),
        ];
        let resolved: Vec<_> = cases
            .iter()
            .map(|&(name, suffix, _)| (name, file_artwork(name, suffix)))
            .collect();
        let expected: Vec<_> = cases
            .iter()
            .map(|&(name, _, artwork)| (name, artwork))
            .collect();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn associations_are_unambiguous_and_drawable() {
        let mut conflicts = Vec::new();
        for table in [file_types::STEMS, file_types::SUFFIXES] {
            let mut keys = HashMap::new();
            for &(key, names) in table {
                for name in names {
                    if let Some(other) = keys
                        .insert(name.to_ascii_lowercase(), key)
                        .filter(|other| *other != key)
                    {
                        conflicts.push(format!("{name}: {other} / {key}"));
                    }
                }
            }
        }
        let undrawn: Vec<&str> = file_types::STEMS
            .iter()
            .chain(file_types::SUFFIXES)
            .map(|&(key, _)| key)
            .filter(|key| !ASSOCIATIONS.artwork.contains_key(key))
            .collect();
        assert_eq!(
            (conflicts, undrawn, ASSOCIATIONS.artwork.len()),
            (
                Vec::<String>::new(),
                Vec::<&str>::new(),
                file_types::ARTWORK.len()
            )
        );
    }

    #[test]
    fn the_bundled_file_icons_are_exactly_the_drawn_ones() {
        let drawn: BTreeSet<&str> = file_types::ARTWORK
            .iter()
            .map(|&(_, artwork)| artwork)
            .chain([
                FOLDER,
                FOLDER_OPEN,
                CHEVRON_COLLAPSED,
                CHEVRON_EXPANDED,
                SOURCE,
                DEFAULT,
            ])
            .filter_map(|artwork| match artwork {
                Artwork::File(name) => Some(name),
                Artwork::Glyph(_) => None,
            })
            .collect();
        let bundled: BTreeSet<&str> = crate::assets::file_icon_names().collect();
        assert_eq!(drawn, bundled);
    }

    #[test]
    fn evidence_structures_do_not_rely_on_the_display_name() {
        let cases = [
            ("part:0", Some(Glyph::HardDrive)),
            ("lvm:0:1", Some(Glyph::HardDrive)),
            ("part:0:entry", None),
            ("host-docker:example:", Some(Glyph::Container)),
            ("host-docker:example:root", None),
            ("ntfs-ads:1", Some(Glyph::Layers)),
            ("refs-snap:1", Some(Glyph::ClockArrowLeft)),
            ("diskgap:0", Some(Glyph::CircleDashed)),
        ];
        let resolved: Vec<_> = cases.iter().map(|&(id, _)| (id, structure(id))).collect();
        let expected: Vec<_> = cases
            .iter()
            .map(|&(id, glyph)| (id, glyph.map(Artwork::Glyph)))
            .collect();
        assert_eq!(resolved, expected);
    }
}
