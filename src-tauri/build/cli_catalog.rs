//! Generate the CLI from the backend command registry and the reviewed schema
//! in `src/cli/commands.toml`: a new backend command fails the build until it
//! is mapped to a command, and every command follows the naming rules.
use std::{collections::BTreeMap, error::Error, fs, path::PathBuf};

use syn::{parse::Parser, punctuated::Punctuated, visit::Visit, Token};

#[path = "cli_catalog/emit.rs"]
mod emit;
#[path = "cli_catalog/manifest.rs"]
mod manifest;
#[path = "cli_catalog/operation.rs"]
mod operation;
#[path = "cli_catalog/resolve.rs"]
mod resolve;
#[path = "cli_catalog/source.rs"]
mod source;

const MANIFEST: &str = "src/cli/commands.toml";

#[derive(Default)]
struct Registry(Vec<syn::Path>);
impl<'ast> Visit<'ast> for Registry {
    fn visit_macro(&mut self, invocation: &'ast syn::Macro) {
        if invocation
            .path
            .segments
            .last()
            .is_some_and(|part| part.ident == "generate_handler")
        {
            match Punctuated::<syn::Path, Token![,]>::parse_terminated
                .parse2(invocation.tokens.clone())
            {
                Ok(paths) => self.0.extend(paths),
                Err(error) => panic!("invalid command registry: {error}"),
            }
        }
        syn::visit::visit_macro(self, invocation);
    }
}

/// Registered operations by name, and the source files they were read from.
type Operations = (BTreeMap<String, operation::Operation>, Vec<PathBuf>);

fn operations() -> Result<Operations, Box<dyn Error>> {
    let mut registry = Registry::default();
    registry.visit_file(&syn::parse_file(&fs::read_to_string("src/lib.rs")?)?);
    if registry.0.is_empty() {
        return Err("empty backend command registry in src/lib.rs".into());
    }
    let mut sources = source::Sources::new("src/commands.rs");
    let mut operations = BTreeMap::new();
    for path in registry.0 {
        let parts: Vec<_> = path
            .segments
            .iter()
            .map(|part| part.ident.to_string())
            .collect();
        if parts.len() < 3 || parts[0] != "commands" {
            return Err(format!("unsupported command path: {}", parts.join("::")).into());
        }
        let name = parts[parts.len() - 1].clone();
        let function = sources.function(&parts[1..parts.len() - 1], &name)?;
        let operation = operation::from_fn(path, &function)?;
        if operations.insert(name.clone(), operation).is_some() {
            return Err(format!("duplicate registered command: {name}").into());
        }
    }
    let files = sources.files().cloned().collect();
    Ok((operations, files))
}

pub fn generate() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed={MANIFEST}");
    for file in [
        "build/cli_catalog.rs",
        "build/cli_catalog/emit.rs",
        "build/cli_catalog/manifest.rs",
        "build/cli_catalog/operation.rs",
        "build/cli_catalog/resolve.rs",
        "build/cli_catalog/source.rs",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }
    let (operations, files) = operations()?;
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    let manifest = manifest::parse(&fs::read_to_string(MANIFEST)?)?;
    let resolved = resolve::resolve(&manifest, &operations).map_err(|errors| {
        format!(
            "{MANIFEST} does not match the backend command registry:\n  {}",
            errors.join("\n  ")
        )
    })?;
    let version = std::env::var("CARGO_PKG_VERSION")?;
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR missing")?);
    fs::write(out.join("cli_dispatch.rs"), emit::dispatch(&resolved))?;
    fs::write(
        out.join("cli_catalog.json"),
        serde_json::to_vec(&emit::catalog(&manifest, &resolved, &version))?,
    )?;
    Ok(())
}
