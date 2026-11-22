//! Generate the CLI's `rpc` dispatch from the backend command registry: every
//! command registered in `src/lib.rs` can be called by name with its IPC
//! parameters, so the CLI reaches every desktop feature without a hand-written
//! command for each one.
use std::{collections::BTreeMap, error::Error, fs, path::PathBuf};

use syn::{parse::Parser, punctuated::Punctuated, visit::Visit, Token};

#[path = "cli_rpc/emit.rs"]
mod emit;
#[path = "cli_rpc/operation.rs"]
mod operation;
#[path = "cli_rpc/source.rs"]
mod source;

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
    for file in [
        "build/cli_rpc.rs",
        "build/cli_rpc/emit.rs",
        "build/cli_rpc/operation.rs",
        "build/cli_rpc/source.rs",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }
    let (operations, files) = operations()?;
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR missing")?);
    fs::write(out.join("cli_rpc_dispatch.rs"), emit::dispatch(&operations))?;
    fs::write(
        out.join("cli_rpc_operations.json"),
        serde_json::to_vec(&emit::operations(&operations))?,
    )?;
    Ok(())
}
