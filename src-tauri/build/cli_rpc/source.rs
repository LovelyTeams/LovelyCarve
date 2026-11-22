//! Resolve registered command paths through Rust module declarations.
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use syn::{Item, ItemFn, ItemMod};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone)]
struct Module {
    items: Vec<Item>,
    directory: PathBuf,
    attribute_directory: PathBuf,
}

pub struct Sources {
    files: BTreeMap<PathBuf, Module>,
    entry: PathBuf,
}

impl Sources {
    pub fn new(entry: impl Into<PathBuf>) -> Self {
        Self {
            files: BTreeMap::new(),
            entry: entry.into(),
        }
    }

    fn read(&mut self, path: &Path) -> Result<Module> {
        if let Some(module) = self.files.get(path) {
            return Ok(module.clone());
        }
        let parent = path.parent().ok_or("module file has no parent")?;
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("invalid module filename")?;
        let directory = if matches!(stem, "mod" | "lib" | "main") {
            parent.to_owned()
        } else {
            parent.join(stem)
        };
        let module = Module {
            items: syn::parse_file(&fs::read_to_string(path)?)?.items,
            directory,
            attribute_directory: parent.to_owned(),
        };
        self.files.insert(path.to_owned(), module.clone());
        Ok(module)
    }

    fn child(&mut self, parent: &Module, declaration: &ItemMod) -> Result<Module> {
        let name = declaration.ident.to_string();
        if let Some((_, items)) = &declaration.content {
            let directory = parent.directory.join(&name);
            return Ok(Module {
                items: items.clone(),
                attribute_directory: directory.clone(),
                directory,
            });
        }
        for attribute in &declaration.attrs {
            if attribute.path().is_ident("path") {
                let syn::Meta::NameValue(value) = &attribute.meta else {
                    return Err("invalid module path attribute".into());
                };
                let syn::Expr::Lit(value) = &value.value else {
                    return Err("module path must be a string".into());
                };
                let syn::Lit::Str(value) = &value.lit else {
                    return Err("module path must be a string".into());
                };
                let path = parent.attribute_directory.join(value.value());
                let mut module = self.read(&path)?;
                // An explicit path makes nested declarations relative to the
                // selected file's directory, rather than its filename stem.
                module.directory = module.attribute_directory.clone();
                return Ok(module);
            }
        }
        let flat = parent.directory.join(format!("{name}.rs"));
        let legacy = parent.directory.join(&name).join("mod.rs");
        let path = match (flat.is_file(), legacy.is_file()) {
            (true, false) => flat,
            (false, true) => legacy,
            (true, true) => return Err(format!("ambiguous command module: {name}").into()),
            (false, false) => return Err(format!("command module missing: {name}").into()),
        };
        self.read(&path)
    }

    pub fn function(&mut self, modules: &[String], name: &str) -> Result<ItemFn> {
        let mut module = self.read(&self.entry.clone())?;
        for name in modules {
            let declaration = module
                .items
                .iter()
                .find_map(|item| match item {
                    Item::Mod(declaration) if declaration.ident == name => Some(declaration),
                    _ => None,
                })
                .ok_or_else(|| format!("command module undeclared: {name}"))?;
            module = self.child(&module, declaration)?;
        }
        let function = module
            .items
            .iter()
            .find_map(|item| match item {
                Item::Fn(function) if function.sig.ident == name => Some(function.clone()),
                _ => None,
            })
            .ok_or_else(|| format!("command function missing: {name}"))?;
        if !function.attrs.iter().any(is_command_attribute) {
            return Err(format!("registered function is not a command: {name}").into());
        }
        Ok(function)
    }

    pub fn files(&self) -> impl Iterator<Item = &PathBuf> {
        self.files.keys()
    }
}

/// `#[tauri::command]`, or `#[cfg_attr(feature = "desktop", tauri::command)]`
/// as the Tauri-free command modules write it.
fn is_command_attribute(attribute: &syn::Attribute) -> bool {
    let names_command = |path: &syn::Path| {
        path.segments
            .last()
            .is_some_and(|segment| segment.ident == "command")
    };
    if names_command(attribute.path()) {
        return true;
    }
    if !attribute.path().is_ident("cfg_attr") {
        return false;
    }
    attribute
        .parse_args_with(syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated)
        .is_ok_and(|metas| metas.iter().skip(1).any(|meta| names_command(meta.path())))
}
