use super::*;

fn manifest(body: &str) -> Result<PluginManifest> {
    PluginManifest::parse(&format!(
        r#"id = "org.test.plugin"
name = "Test"
version = "1"
protocol = 2
library = "test"
{body}"#
    ))
}

#[test]
fn repository_packages_stay_valid() {
    let plugins = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("plugins");
    let linuxinfo = PluginManifest::load(&plugins.join("linuxinfo").join("plugin.toml"))
        .expect("linuxinfo manifest");
    assert_eq!(linuxinfo.id, "org.lovelycarve.linuxinfo");
    let base = linuxinfo.analyzer("linuxbaseinfo").expect("base analyzer");
    assert_eq!(base.scope, AnalyzerScope::System);
    assert!(base.applies_to_family("linux"));
    assert!(!base.applies_to_family("windows"));
    assert_eq!(base.provides, vec!["timezone", "hostname", "users"]);
    assert_eq!(base.options[0].kind, OptionKind::Multi);
    assert_eq!(base.options[0].default_json(), serde_json::json!([]));
    assert_eq!(linuxinfo.memory_limit().expect("memory"), 1 << 30);

    let vehicle = PluginManifest::load(&plugins.join("vehicle").join("plugin.toml"))
        .expect("vehicle manifest");
    assert_eq!(vehicle.analyzers.len(), 4);
    assert!(vehicle
        .analyzers
        .iter()
        .all(|analyzer| analyzer.scope == AnalyzerScope::Evidence && !analyzer.inputs.is_empty()));
}

#[test]
fn lpp_v1_manifests_explain_the_rebuild() {
    let Err(PluginError::Manifest(message)) = PluginManifest::parse(
        r#"id = "org.test.legacy"
name = "Legacy"
version = "1"
protocol_min = 1
protocol_max = 1
[library]
path = "legacy"
"#,
    ) else {
        panic!("v1 manifest must be rejected");
    };
    assert!(message.contains("LPP v1"), "{message}");
    assert!(manifest("").is_ok());
    let Err(PluginError::Manifest(message)) = PluginManifest::parse(
        r#"id = "org.test.future"
name = "Future"
version = "1"
protocol = 3
library = "future"
"#,
    ) else {
        panic!("protocol 3 must be rejected");
    };
    assert!(message.contains("protocol 3"), "{message}");
}

#[test]
fn analyzer_fields_must_fit_their_scope() {
    for body in [
        // Platforms are for systems.
        "[[analyzer]]\nid = \"a\"\nname = \"A\"\nscope = \"evidence\"\nplatforms = [\"linux\"]",
        // Unknown family.
        "[[analyzer]]\nid = \"a\"\nname = \"A\"\nscope = \"system\"\nplatforms = [\"beos\"]",
        // Files runs take no inputs and never start on their own.
        "[[analyzer]]\nid = \"a\"\nname = \"A\"\nscope = \"files\"\ninputs = [\"etc/passwd\"]",
        "[[analyzer]]\nid = \"a\"\nname = \"A\"\nscope = \"files\"\ntrigger = \"auto\"",
        // Extensions are for files runs.
        "[[analyzer]]\nid = \"a\"\nname = \"A\"\nscope = \"system\"\nextensions = [\"db\"]",
        // Inputs must be valid patterns.
        "[[analyzer]]\nid = \"a\"\nname = \"A\"\nscope = \"system\"\ninputs = [\"../etc\"]",
        // Analyzer ids follow the SDK rule.
        "[[analyzer]]\nid = \"Bad-Id\"\nname = \"A\"\nscope = \"system\"",
        // Duplicate analyzers.
        "[[analyzer]]\nid = \"a\"\nname = \"A\"\nscope = \"system\"\n[[analyzer]]\nid = \"a\"\nname = \"B\"\nscope = \"system\"",
        // Options need choices that match their defaults.
        "[[analyzer]]\nid = \"a\"\nname = \"A\"\nscope = \"system\"\n[[analyzer.option]]\nid = \"items\"\ntype = \"multi\"\nlabel = \"子项\"",
        "[[analyzer]]\nid = \"a\"\nname = \"A\"\nscope = \"system\"\n[[analyzer.option]]\nid = \"mode\"\ntype = \"choice\"\nlabel = \"模式\"\nchoices = [{ id = \"fast\", label = \"快\" }]\ndefault = \"slow\"",
        // Typos are errors.
        "[[analyzer]]\nid = \"a\"\nname = \"A\"\nscope = \"system\"\ntriger = \"auto\"",
        "memory = \"1TiB\"",
        "permissions = [\"network\", \"network\"]",
    ] {
        assert!(
            matches!(manifest(body), Err(PluginError::Manifest(_))),
            "{body} must be rejected"
        );
    }
}

#[test]
fn files_analyzers_match_by_extension_or_mime() {
    let manifest = manifest(
        "[[analyzer]]\nid = \"sqlite\"\nname = \"SQLite\"\nscope = \"files\"\nextensions = [\"db\", \"sqlite\"]\nmime_types = [\"application/vnd.sqlite3\"]",
    )
    .expect("manifest");
    let analyzer = manifest.analyzer("sqlite").expect("analyzer");
    assert!(analyzer.accepts_file("History.DB", None));
    assert!(analyzer.accepts_file("blob", Some("application/vnd.sqlite3")));
    assert!(!analyzer.accepts_file("notes.txt", Some("text/plain")));
    assert_eq!(analyzer.trigger, TaskTrigger::Manually);
}

#[test]
fn option_values_are_checked_against_their_kind() {
    let manifest = manifest(
        "[[analyzer]]\nid = \"a\"\nname = \"A\"\nscope = \"system\"\n[[analyzer.option]]\nid = \"items\"\ntype = \"multi\"\nlabel = \"子项\"\nchoices = [{ id = \"users\", label = \"用户\" }, { id = \"groups\", label = \"组\" }]",
    )
    .expect("manifest");
    let option = &manifest.analyzer("a").expect("analyzer").options[0];
    assert!(option.check(&serde_json::json!(["users"])).is_ok());
    assert!(option.check(&serde_json::json!([])).is_ok());
    assert!(option.check(&serde_json::json!(["shadow"])).is_err());
    assert!(option.check(&serde_json::json!("users")).is_err());
}

#[test]
fn library_paths_resolve_to_platform_file_names() {
    let library = |path: &str| {
        PluginManifest::parse(&format!(
            "id = \"org.test.library\"\nname = \"L\"\nversion = \"1\"\nprotocol = 2\nlibrary = \"{path}\""
        ))
    };
    let directory = Path::new("plugins").join("demo");
    assert_eq!(
        library("demo").expect("demo").library_path(&directory),
        directory.join(format!("{DLL_PREFIX}demo{DLL_SUFFIX}"))
    );
    assert_eq!(
        library("bin/demo").expect("bin").library_path(&directory),
        directory
            .join("bin")
            .join(format!("{DLL_PREFIX}demo{DLL_SUFFIX}"))
    );
    assert_eq!(
        library("custom.DLL").expect("dll").library_path(&directory),
        directory.join("custom.DLL")
    );
    for escaping in ["../demo", "", "/abs/demo"] {
        assert!(library(escaping).is_err(), "{escaping:?} must be rejected");
    }
}

#[test]
fn sizes_parse_binary_and_decimal_units() {
    assert_eq!(parse_size("512MiB"), Some(512 << 20));
    assert_eq!(parse_size("2GiB"), Some(2 << 30));
    assert_eq!(parse_size("2048MB"), Some(2_048_000_000));
    assert_eq!(parse_size("1073741824"), Some(1 << 30));
    assert_eq!(parse_size("lots"), None);
}
