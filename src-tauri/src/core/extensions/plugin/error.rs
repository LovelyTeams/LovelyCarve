use thiserror::Error;

pub type Result<T> = std::result::Result<T, PluginError>;

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("plugin I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid LPP frame: {0}")]
    InvalidFrame(String),
    #[error("LPP serialization failed: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),
    #[error("LPP deserialization failed: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),
    #[error("plugin protocol error {code}: {message}")]
    Rpc { code: String, message: String },
    #[error("plugin connection closed")]
    ConnectionClosed,
    #[error("plugin handshake failed: {0}")]
    Handshake(String),
    #[error("invalid plugin manifest: {0}")]
    Manifest(String),
    #[error("plugin runtime unavailable: {0}")]
    Unavailable(String),
    #[error("plugin request timed out")]
    Timeout,
    #[error("plugin resource limit exceeded: {0}")]
    ResourceLimit(String),
    #[error("synchronous plugin RPC was called from an async runtime worker")]
    BlockingOnAsyncRuntime,
    /// A package archive that cannot be inspected or installed.
    #[error(transparent)]
    Package(#[from] PackageRejection),
}

/// Why a package archive was not inspected or installed. The interface
/// shows each kind as its own message ([`PackageRejection::reason`]) with
/// the detail below it.
#[derive(Debug, Error)]
pub enum PackageRejection {
    /// No directory to install packages into.
    #[error("there is no plugin install directory")]
    NoInstallRoot,
    /// Not a readable `.lcpkg`: a broken ZIP, entries that leave the
    /// package, links, too many or too large entries, or no `plugin.toml`.
    #[error("{0}")]
    InvalidArchive(String),
    /// The manifest, `schema.json` or a locale file is invalid.
    #[error("{0}")]
    InvalidPackage(String),
    /// The package has no library for the running platform.
    #[error("the package has no library {library} for this platform")]
    NoLibrary { library: String },
    /// A bundled package with the same id is installed.
    #[error("{plugin_id} is a bundled plugin and cannot be replaced")]
    Bundled { plugin_id: String },
    /// The archive is not the one that was inspected.
    #[error("the package archive changed after it was inspected (SHA-256 {actual}, inspected {expected})")]
    Changed { expected: String, actual: String },
    /// The library did not start, or its `plugin.hello` does not match the
    /// package; the installed version was left in place.
    #[error("the package's library failed its start check: {0}")]
    LibraryRejected(String),
    /// The installed version is still in use (a running task holds it).
    #[error("the installed version is in use (stop its tasks and retry): {0}")]
    InUse(String),
}

impl PackageRejection {
    /// The stable name of the kind, carried to the interface.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::NoInstallRoot => "noInstallRoot",
            Self::InvalidArchive(_) => "invalidArchive",
            Self::InvalidPackage(_) => "invalidPackage",
            Self::NoLibrary { .. } => "noLibrary",
            Self::Bundled { .. } => "bundled",
            Self::Changed { .. } => "changed",
            Self::LibraryRejected(_) => "libraryRejected",
            Self::InUse(_) => "inUse",
        }
    }
}
