//! A plugin package directory: `plugin.toml`, the library, the
//! `schema.json` the build exports from the library (`lc-plugin-host
//! --describe`) and translations in `locales/<locale>.toml`.
//!
//! `schema.json` lets the UI show record types before the first run and
//! pins what the library may report: every handshake compares the
//! library's `plugin.hello` with it ([`PluginDefinition::verify_hello`]),
//! so a package whose library and schema drifted apart is refused instead
//! of writing records under a schema nobody reviewed.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use lovelycarve_plugin::{
    protocol::Hello,
    schema::{schema_hash, RecordSchema},
    wire::PROTOCOL_VERSION,
};

use super::{fingerprint::fingerprint_directory, PluginError, PluginManifest, Result};

pub const MANIFEST_FILE: &str = "plugin.toml";
pub const SCHEMA_FILE: &str = "schema.json";
pub const LOCALES_DIRECTORY: &str = "locales";

/// Largest `schema.json` and locale file read.
const MAX_METADATA_FILE: u64 = 4 * 1024 * 1024;
const MAX_LOCALE_KEY: usize = 256;
const MAX_LOCALE_TEXT: usize = 4096;

/// Translations by locale (`en`), each by stable key (`record.linux.user`).
pub type Locales = BTreeMap<String, BTreeMap<String, String>>;

/// A discovered package.
#[derive(Clone, Debug)]
pub struct PluginDefinition {
    pub directory: PathBuf,
    pub manifest: PluginManifest,
    /// The library's `plugin.hello` exported at build time.
    pub described: Option<Arc<Hello>>,
    pub locales: Arc<Locales>,
    /// Installed by the user (can be uninstalled), not bundled.
    pub removable: bool,
    /// Display-only content hash, computed on first request instead of during
    /// app startup. Shared across clones so it is computed at most once.
    fingerprint: Arc<OnceLock<String>>,
}

impl PluginDefinition {
    /// Loads the package in `directory`.
    pub fn load(directory: &Path, removable: bool) -> Result<Self> {
        let manifest = PluginManifest::load(&directory.join(MANIFEST_FILE))?;
        let described = read_schema(directory)?;
        let definition = Self {
            directory: directory.to_owned(),
            manifest,
            described: described.map(Arc::new),
            locales: Arc::new(read_locales(directory)?),
            removable,
            fingerprint: Arc::default(),
        };
        if let Some(described) = &definition.described {
            definition.check_described(described)?;
        }
        Ok(definition)
    }

    /// A definition without package files (tests).
    pub fn from_manifest(directory: PathBuf, manifest: PluginManifest) -> Self {
        Self {
            directory,
            manifest,
            described: None,
            locales: Arc::default(),
            removable: false,
            fingerprint: Arc::default(),
        }
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    /// Record schemas from `schema.json` (empty without one).
    pub fn record_schemas(&self) -> &[RecordSchema] {
        self.described
            .as_deref()
            .map_or(&[], |described| described.records.as_slice())
    }

    /// Directory content fingerprint for display. Blocking (walks and hashes
    /// the package directory on first call). A directory that cannot be
    /// fingerprinted reports `unavailable: <reason>` rather than hiding the
    /// plugin.
    pub fn fingerprint(&self) -> String {
        self.fingerprint
            .get_or_init(|| match fingerprint_directory(&self.directory) {
                Ok(fingerprint) => fingerprint,
                Err(error) => {
                    tracing::debug!(
                        plugin_id = %self.manifest.id,
                        "plugin fingerprint unavailable: {error}"
                    );
                    format!("unavailable: {error}")
                }
            })
            .clone()
    }

    /// Checks a library's `plugin.hello` against the manifest and the
    /// exported schema.
    pub fn verify_hello(&self, hello: &Hello) -> Result<()> {
        self.check_described(hello)?;
        if let Some(described) = &self.described {
            if described.schema_hash != hello.schema_hash {
                return Err(PluginError::Handshake(format!(
                    "the library's record schemas ({}) differ from the package's {SCHEMA_FILE} \
                     ({}); rebuild the package",
                    hello.schema_hash, described.schema_hash
                )));
            }
        }
        Ok(())
    }

    fn check_described(&self, hello: &Hello) -> Result<()> {
        let mismatch = |message: String| Err(PluginError::Handshake(message));
        if hello.protocol != PROTOCOL_VERSION {
            return mismatch(format!(
                "the library speaks LPP {}; LovelyCarve speaks {PROTOCOL_VERSION}",
                hello.protocol
            ));
        }
        if hello.plugin != self.manifest.id {
            return mismatch(format!(
                "manifest id {} does not match the library's {}",
                self.manifest.id, hello.plugin
            ));
        }
        let analyzers: HashSet<&str> = hello.analyzers.iter().map(String::as_str).collect();
        if let Some(missing) = self
            .manifest
            .analyzers
            .iter()
            .find(|analyzer| !analyzers.contains(analyzer.id.as_str()))
        {
            return mismatch(format!(
                "the library does not implement declared analyzer {}",
                missing.id
            ));
        }
        let filesystems: HashSet<&str> = hello.filesystems.iter().map(String::as_str).collect();
        if let Some(missing) = self
            .manifest
            .filesystems
            .iter()
            .find(|filesystem| !filesystems.contains(filesystem.id.as_str()))
        {
            return mismatch(format!(
                "the library does not implement declared filesystem {}",
                missing.id
            ));
        }
        let mut types = HashSet::new();
        for schema in &hello.records {
            schema.validate().map_err(|message| {
                PluginError::Handshake(format!("record schema {}: {message}", schema.record_type))
            })?;
            if !types.insert(schema.record_type.as_str()) {
                return mismatch(format!(
                    "record type {} is described twice",
                    schema.record_type
                ));
            }
        }
        if schema_hash(&hello.records) != hello.schema_hash {
            return mismatch("the schema hash does not match the record schemas".to_owned());
        }
        Ok(())
    }
}

fn read_schema(directory: &Path) -> Result<Option<Hello>> {
    let path = directory.join(SCHEMA_FILE);
    let Some(bytes) = read_bounded(&path)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| PluginError::Manifest(format!("{}: {error}", path.display())))
}

fn read_locales(directory: &Path) -> Result<Locales> {
    let mut locales = Locales::new();
    let root = directory.join(LOCALES_DIRECTORY);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(locales),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("toml") {
            continue;
        }
        let Some(locale) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !valid_locale(locale) {
            return Err(PluginError::Manifest(format!(
                "{}: locale files are named like en.toml or zh-CN.toml",
                path.display()
            )));
        }
        let Some(bytes) = read_bounded(&path)? else {
            continue;
        };
        let text = String::from_utf8(bytes)
            .map_err(|_| PluginError::Manifest(format!("{} is not UTF-8", path.display())))?;
        let table: toml::Table = toml::from_str(&text)
            .map_err(|error| PluginError::Manifest(format!("{}: {error}", path.display())))?;
        let mut catalog = BTreeMap::new();
        for (key, value) in table {
            let toml::Value::String(text) = value else {
                return Err(PluginError::Manifest(format!(
                    "{}: {key} must be a string",
                    path.display()
                )));
            };
            if key.len() > MAX_LOCALE_KEY || text.len() > MAX_LOCALE_TEXT {
                return Err(PluginError::Manifest(format!(
                    "{}: {key} is too long",
                    path.display()
                )));
            }
            catalog.insert(key, text);
        }
        locales.insert(locale.to_owned(), catalog);
    }
    Ok(locales)
}

/// `en`, `zh-CN`, `pt-BR`.
fn valid_locale(locale: &str) -> bool {
    let mut parts = locale.split('-');
    let language = parts.next().unwrap_or_default();
    let region = parts.next();
    parts.next().is_none()
        && (2..=3).contains(&language.len())
        && language.bytes().all(|byte| byte.is_ascii_lowercase())
        && region.is_none_or(|region| {
            region.len() == 2 && region.bytes().all(|byte| byte.is_ascii_uppercase())
        })
}

fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.len() > MAX_METADATA_FILE {
        return Err(PluginError::Manifest(format!(
            "{} is larger than {MAX_METADATA_FILE} bytes",
            path.display()
        )));
    }
    Ok(Some(fs::read(path)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_package(directory: &Path, hello: Option<&Hello>) {
        fs::create_dir_all(directory.join(LOCALES_DIRECTORY)).expect("package");
        fs::write(
            directory.join(MANIFEST_FILE),
            r#"id = "org.test.package"
name = "测试"
version = "1"
protocol = 2
library = "test"
[[analyzer]]
id = "scan"
name = "扫描"
scope = "evidence"
"#,
        )
        .expect("manifest");
        fs::write(
            directory.join(LOCALES_DIRECTORY).join("en.toml"),
            "\"analyzer.scan\" = \"Scan\"\n",
        )
        .expect("locale");
        if let Some(hello) = hello {
            fs::write(
                directory.join(SCHEMA_FILE),
                serde_json::to_vec(hello).expect("schema"),
            )
            .expect("schema file");
        }
    }

    fn hello(records: Vec<RecordSchema>) -> Hello {
        Hello {
            protocol: PROTOCOL_VERSION,
            plugin: "org.test.package".to_owned(),
            sdk: "0.2.0".to_owned(),
            analyzers: vec!["scan".to_owned()],
            filesystems: Vec::new(),
            schema_hash: schema_hash(&records),
            records,
        }
    }

    fn user_schema(label: &str) -> RecordSchema {
        RecordSchema::new("test.user", label).field(lovelycarve_plugin::FieldSchema::new(
            "name",
            "用户名",
            lovelycarve_plugin::FieldKind::Text,
        ))
    }

    #[test]
    fn packages_carry_schema_and_locales_and_pin_the_library() {
        let directory = tempfile::tempdir().expect("package");
        let described = hello(vec![user_schema("用户")]);
        write_package(directory.path(), Some(&described));
        let definition = PluginDefinition::load(directory.path(), true).expect("package");
        assert_eq!(definition.record_schemas().len(), 1);
        assert_eq!(definition.locales["en"]["analyzer.scan"], "Scan");
        definition.verify_hello(&described).expect("same library");

        let drifted = hello(vec![user_schema("账户")]);
        assert!(matches!(
            definition.verify_hello(&drifted),
            Err(PluginError::Handshake(message)) if message.contains(SCHEMA_FILE)
        ));
        let mut missing = described.clone();
        missing.analyzers.clear();
        assert!(definition.verify_hello(&missing).is_err());
        let mut forged = described.clone();
        forged.schema_hash = "fnv1a64:0000000000000000".to_owned();
        assert!(definition.verify_hello(&forged).is_err());
    }

    #[test]
    fn packages_without_a_schema_load_and_verify_at_the_handshake() {
        let directory = tempfile::tempdir().expect("package");
        write_package(directory.path(), None);
        let definition = PluginDefinition::load(directory.path(), false).expect("package");
        assert!(definition.record_schemas().is_empty());
        definition
            .verify_hello(&hello(vec![user_schema("用户")]))
            .expect("hello");
    }

    #[test]
    fn locale_names_are_language_tags() {
        assert!(valid_locale("en"));
        assert!(valid_locale("zh-CN"));
        assert!(!valid_locale("EN"));
        assert!(!valid_locale("zh_CN"));
        assert!(!valid_locale("../en"));
    }
}
