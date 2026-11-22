//! Native components: dynamic libraries beside the executables that carry
//! large dependencies the executables only need for some operations (the
//! `src-tauri/components` workspace; ONNX Runtime for [`inference`]). The
//! C ABI is `lovelycarve-component-abi`.
//!
//! A component is loaded on first use and stays loaded for the rest of the
//! process. Only the executable's own directory is searched, plus the Cargo
//! profile directory above test and example binaries; the current directory
//! and `PATH` never are, so a folder of evidence cannot plant a library. The component's own dependencies resolve
//! from its directory and the system directories.

pub mod inference;

use std::ffi::OsString;
use std::mem::MaybeUninit;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use lovelycarve_component_abi as abi;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::core::common::error::Failure;

/// A loaded component and its function table.
pub(crate) struct Component {
    api: abi::ComponentApiV1,
    path: PathBuf,
}

// SAFETY: the component ABI requires every function to be callable
// concurrently from any thread, and the library is never unloaded.
unsafe impl Send for Component {}
unsafe impl Sync for Component {}

impl Component {
    /// Loads `library` (its name without platform prefix or suffix) from the
    /// search directories of this executable.
    pub(crate) fn load(library: &str) -> anyhow::Result<Self> {
        let file_name = libloading::library_filename(library);
        let directories = search_directories()?;
        match find_library(&file_name, &directories) {
            Some(path) => Self::open(&path),
            None => Err(missing_library(&file_name, &directories)),
        }
    }

    fn open(path: &Path) -> anyhow::Result<Self> {
        // SAFETY: loading runs the library's initialisers; components are
        // built from this repository and shipped beside the executable.
        let library = unsafe { open_library(path) }
            .with_context(|| format!("加载组件 {} 失败", path.display()))?;
        // SAFETY: the ABI fixes the entry point's signature.
        let entry = unsafe { library.get::<abi::EntryFn>(abi::ENTRY_SYMBOL) }
            .map(|symbol| *symbol)
            .with_context(|| {
                format!(
                    "{} 不是 LovelyCarve 组件（缺少 lovelycarve_component_v1 入口）",
                    path.display()
                )
            })?;
        let mut api = MaybeUninit::<abi::ComponentApiV1>::uninit();
        // SAFETY: `api` is writable memory for one table.
        let status = unsafe { entry(api.as_mut_ptr()) };
        if status != abi::STATUS_OK {
            bail!(Failure::unavailable(format!(
                "组件 {} 初始化失败（ABI 状态 {status}）",
                path.display()
            )));
        }
        // SAFETY: every ABI version starts with these two fields, so they are
        // initialised whatever layout the component wrote.
        let (abi_version, struct_size) = unsafe {
            let table = api.as_ptr();
            (
                std::ptr::addr_of!((*table).abi_version).read(),
                std::ptr::addr_of!((*table).struct_size).read(),
            )
        };
        if abi_version != abi::ABI_VERSION || struct_size < abi::COMPONENT_API_SIZE {
            bail!(
                Failure::unavailable(format!("组件 {} 实现 ABI v{abi_version}（{struct_size} 字节），本程序需要 v{}；请使用与主程序同一次构建的组件", path.display(), abi::ABI_VERSION))
            );
        }
        // SAFETY: a v1 entry point that succeeded wrote the whole v1 table.
        let api = unsafe { api.assume_init() };
        // Calls may still run on other threads when a caller gives up, so a
        // component is never unloaded.
        std::mem::forget(library);
        Ok(Self {
            api,
            path: path.to_path_buf(),
        })
    }

    /// Where the library was loaded from.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Runs one method on the calling thread and decodes its JSON result.
    pub(crate) fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: &impl Serialize,
        payload: &[u8],
    ) -> anyhow::Result<T> {
        let params = serde_json::to_vec(params)
            .with_context(|| format!("编码组件调用 {method} 的参数失败"))?;
        let mut out = abi::Buffer::empty();
        // SAFETY: every argument borrows live memory for the duration of the
        // call, and `out` is one writable buffer.
        let status = unsafe {
            (self.api.call)(
                abi::Bytes::from_slice(method.as_bytes()),
                abi::Bytes::from_slice(&params),
                abi::Bytes::from_slice(payload),
                &mut out,
            )
        };
        // SAFETY: the component filled `out`, or left it empty.
        let reply = unsafe { out.as_slice() }.to_vec();
        // SAFETY: `out` came from this component and is released once.
        unsafe { (self.api.free_buffer)(&mut out) };
        let message = || String::from_utf8_lossy(&reply).into_owned();
        match status {
            abi::STATUS_OK => serde_json::from_slice(&reply).with_context(|| {
                format!("组件 {} 对 {method} 的应答格式无效", self.path.display())
            }),
            abi::STATUS_ERROR => Err(anyhow!(message())),
            abi::STATUS_INVALID_ARGUMENT => Err(anyhow!(Failure::invalid_input(format!(
                "组件 {} 拒绝了 {method} 调用：{}",
                self.path.display(),
                message()
            )))),
            abi::STATUS_PANIC => Err(anyhow!(
                "组件 {} 处理 {method} 时崩溃：{}",
                self.path.display(),
                message()
            )),
            status => Err(anyhow!(
                "组件 {} 处理 {method} 时返回未知状态 {status}",
                self.path.display()
            )),
        }
    }
}

/// The executable's directory, plus the Cargo profile directory when the
/// executable is a test or example binary below it.
fn search_directories() -> anyhow::Result<Vec<PathBuf>> {
    let executable = std::env::current_exe().context("无法确定本程序的位置，不能加载组件")?;
    let directory = executable
        .parent()
        .with_context(|| format!("{} 没有上级目录", executable.display()))?;
    Ok(component_directories(directory))
}

fn component_directories(executable_directory: &Path) -> Vec<PathBuf> {
    let mut directories = vec![executable_directory.to_path_buf()];
    if let Some(profile) = cargo_profile_directory(executable_directory) {
        directories.push(profile.to_path_buf());
    }
    directories
}

/// The Cargo profile directory above a test or example binary:
/// `<profile>/deps/` and `<profile>/examples/`, or
/// `<profile>/build/<package>/<hash>/out/` with Cargo's newer build-dir layout.
fn cargo_profile_directory(executable_directory: &Path) -> Option<&Path> {
    fn name(path: &Path) -> Option<&str> {
        path.file_name().and_then(|name| name.to_str())
    }
    match name(executable_directory) {
        Some("deps" | "examples") => executable_directory.parent(),
        Some("out") => executable_directory
            .ancestors()
            .nth(3)
            .filter(|build| name(build) == Some("build"))
            .and_then(Path::parent),
        _ => None,
    }
}

fn find_library(file_name: &OsString, directories: &[PathBuf]) -> Option<PathBuf> {
    directories
        .iter()
        .map(|directory| directory.join(file_name))
        .find(|candidate| candidate.is_file())
}

fn missing_library(file_name: &OsString, directories: &[PathBuf]) -> anyhow::Error {
    let searched = directories
        .iter()
        .map(|directory| directory.display().to_string())
        .collect::<Vec<_>>()
        .join("、");
    anyhow!(
        Failure::unavailable(format!("缺少组件 {}：发布包应把它放在主程序旁；开发构建请先运行 `just build-components`（已查找：{searched}）", file_name.to_string_lossy()))
    )
}

#[cfg(windows)]
unsafe fn open_library(path: &Path) -> Result<libloading::Library, libloading::Error> {
    use libloading::os::windows::{
        Library, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    };
    unsafe {
        Library::load_with_flags(
            path,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        )
    }
    .map(Into::into)
}

#[cfg(not(windows))]
unsafe fn open_library(path: &Path) -> Result<libloading::Library, libloading::Error> {
    unsafe { libloading::Library::new(path) }
}

#[cfg(test)]
mod tests;
