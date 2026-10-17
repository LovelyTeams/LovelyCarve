//! Backend operations as their Rust signatures describe them: the arguments
//! the host injects, the JSON parameters a caller supplies, and the result.
use quote::ToTokens;
use syn::{FnArg, GenericArgument, ItemFn, Pat, PathArguments, ReturnType, Type};

use super::manifest::Kind;

/// What the host supplies to a command instead of the caller.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Injection {
    App,
    State,
    LeaseOwner,
}

pub struct Param {
    /// The camelCase name a caller passes, as Tauri derives it.
    pub name: String,
    pub ident: syn::Ident,
    pub rust_type: String,
    pub kind: Kind,
    pub required: bool,
}

pub enum Input {
    Injected(Injection),
    Param(Param),
}

/// How a command's successful result reaches the caller.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Output {
    Json,
    /// `crate::host::Response`: raw bytes the desktop sends as an ArrayBuffer.
    Response,
    /// `Vec<u8>`: raw bytes the desktop sends as a JSON array.
    Bytes,
}

pub struct Operation {
    pub name: String,
    pub path: syn::Path,
    pub docs: String,
    pub is_async: bool,
    pub returns_result: bool,
    pub output: Output,
    pub inputs: Vec<Input>,
}

impl Operation {
    pub fn params(&self) -> impl Iterator<Item = &Param> {
        self.inputs.iter().filter_map(|input| match input {
            Input::Param(param) => Some(param),
            Input::Injected(_) => None,
        })
    }

    pub fn param(&self, name: &str) -> Option<&Param> {
        self.params().find(|param| param.name == name)
    }
}

pub fn camel_case(name: &str) -> String {
    let mut parts = name.split('_');
    let mut output = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            output.extend(first.to_uppercase());
        }
        output.extend(chars);
    }
    output
}

pub fn from_fn(path: syn::Path, function: &ItemFn) -> Result<Operation, String> {
    let name = function.sig.ident.to_string();
    let mut inputs = Vec::new();
    for argument in &function.sig.inputs {
        let FnArg::Typed(argument) = argument else {
            return Err(format!("{name}: commands take no receiver"));
        };
        let Pat::Ident(pattern) = argument.pat.as_ref() else {
            return Err(format!("{name}: unsupported argument pattern"));
        };
        let Type::Path(kind) = argument.ty.as_ref() else {
            return Err(format!("{name}: unsupported argument type"));
        };
        let last = kind
            .path
            .segments
            .last()
            .ok_or_else(|| format!("{name}: empty argument type"))?
            .ident
            .to_string();
        let injected = match last.as_str() {
            "AppHandle" => Some(Injection::App),
            "State" => Some(Injection::State),
            "LeaseOwner" => Some(Injection::LeaseOwner),
            "WebviewWindow" | "Window" | "Webview" | "Channel" | "Request" => {
                return Err(format!(
                    "{name}: {last} is a Tauri-only argument; use a host type from src/host.rs"
                ))
            }
            _ => None,
        };
        if let Some(injection) = injected {
            inputs.push(Input::Injected(injection));
            continue;
        }
        let (inner, optional) = unwrap_option(argument.ty.as_ref());
        inputs.push(Input::Param(Param {
            name: camel_case(&pattern.ident.to_string()),
            ident: pattern.ident.clone(),
            rust_type: compact(argument.ty.to_token_stream().to_string()),
            kind: kind_of(inner),
            required: !optional,
        }));
    }
    let (returns_result, output) = match &function.sig.output {
        ReturnType::Default => (false, Output::Json),
        ReturnType::Type(_, kind) => {
            let (success, is_result) = unwrap_result(kind);
            (is_result, output_of(success))
        }
    };
    Ok(Operation {
        name,
        path,
        docs: docs(function),
        is_async: function.sig.asyncness.is_some(),
        returns_result,
        output,
        inputs,
    })
}

fn compact(text: String) -> String {
    text.replace(" < ", "<")
        .replace(" >", ">")
        .replace(" :: ", "::")
        .replace(" , ", ", ")
}

fn last_segment(kind: &Type) -> Option<&syn::PathSegment> {
    match kind {
        Type::Path(path) => path.path.segments.last(),
        _ => None,
    }
}

fn first_generic(segment: &syn::PathSegment) -> Option<&Type> {
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.iter().find_map(|argument| match argument {
        GenericArgument::Type(kind) => Some(kind),
        _ => None,
    })
}

fn unwrap_option(kind: &Type) -> (&Type, bool) {
    match last_segment(kind) {
        Some(segment) if segment.ident == "Option" => match first_generic(segment) {
            Some(inner) => (inner, true),
            None => (kind, false),
        },
        _ => (kind, false),
    }
}

fn unwrap_result(kind: &Type) -> (&Type, bool) {
    match last_segment(kind) {
        Some(segment) if segment.ident == "Result" => match first_generic(segment) {
            Some(inner) => (inner, true),
            None => (kind, false),
        },
        _ => (kind, false),
    }
}

fn output_of(kind: &Type) -> Output {
    let Some(segment) = last_segment(kind) else {
        return Output::Json;
    };
    if segment.ident == "Response" {
        return Output::Response;
    }
    if segment.ident == "Vec"
        && first_generic(segment)
            .and_then(last_segment)
            .is_some_and(|item| item.ident == "u8")
    {
        return Output::Bytes;
    }
    Output::Json
}

fn kind_of(kind: &Type) -> Kind {
    let Some(segment) = last_segment(kind) else {
        return Kind::Json;
    };
    match segment.ident.to_string().as_str() {
        "String" | "PathBuf" => Kind::String,
        "bool" => Kind::Boolean,
        "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize" | "f32"
        | "f64" => Kind::Number,
        "Vec"
            if first_generic(segment)
                .and_then(last_segment)
                .is_some_and(|item| item.ident == "String") =>
        {
            Kind::Array
        }
        _ => Kind::Json,
    }
}

fn docs(function: &ItemFn) -> String {
    function
        .attrs
        .iter()
        .filter_map(|attr| {
            if !attr.path().is_ident("doc") {
                return None;
            }
            let syn::Meta::NameValue(value) = &attr.meta else {
                return None;
            };
            let syn::Expr::Lit(value) = &value.value else {
                return None;
            };
            let syn::Lit::Str(value) = &value.lit else {
                return None;
            };
            Some(value.value().trim().to_owned())
        })
        .collect::<Vec<_>>()
        .join(" ")
}
