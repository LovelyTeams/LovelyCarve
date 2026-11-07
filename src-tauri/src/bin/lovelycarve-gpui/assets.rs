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
    FileImage,
    FileLock,
    FileQuestionMark,
    FileSearch,
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

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
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
