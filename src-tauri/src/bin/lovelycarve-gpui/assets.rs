//! The application's asset source: GPUI Kit's default component icons plus
//! the Lucide catalog icons this app uses.
use std::borrow::Cow;

use gpui_kit::{
    assets::{icon_assets, Assets as ComponentAssets},
    AssetSource, Result, SharedString,
};

icon_assets!(pub ExtraIcons, [
    Archive,
    ArrowDown,
    ArrowUpRight,
    Binary,
    BookOpenText,
    Box,
    Boxes,
    Briefcase,
    CaseSensitive,
    ChartColumn,
    CircleDashed,
    CircleStop,
    ClipboardCopy,
    Clock,
    ClockArrowLeft,
    Container,
    Crosshair,
    Database,
    Disc,
    Download,
    Drone,
    FileArchive,
    FileCode,
    FileCog,
    FileImage,
    FileLock,
    FileMusic,
    FilePlay,
    FileQuestionMark,
    FileSearch,
    FileSpreadsheet,
    FileText,
    FileX,
    Files,
    FingerprintPattern,
    FolderPlus,
    FolderSearch,
    FolderTree,
    Funnel,
    Globe,
    HardDriveDownload,
    Hash,
    Info,
    KeyRound,
    Layers,
    LayoutGrid,
    Link,
    Link2,
    ListFilter,
    ListTodo,
    ListTree,
    Locate,
    Lock,
    LockOpen,
    Mail,
    Monitor,
    Package,
    Phone,
    Play,
    Puzzle,
    Regex,
    RotateCcw,
    Save,
    Scan,
    ScanSearch,
    ScrollText,
    Send,
    Server,
    Share2,
    ShieldAlert,
    ShieldCheck,
    SlidersHorizontal,
    Smartphone,
    SquareCheck,
    Table2,
    TextSearch,
    Trash,
    Upload,
    Usb,
    WholeWord,
    Wifi,
    Wrench,
]);

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
            BRAND_MARK => Some(include_bytes!("../../../../src/assets/app-icon.png")),
            BRAND_TILE => Some(include_bytes!(
                "../../../../src/assets/app-icon-background.png"
            )),
            _ => None,
        };
        if let Some(bytes) = brand {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        ComponentAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = ComponentAssets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
