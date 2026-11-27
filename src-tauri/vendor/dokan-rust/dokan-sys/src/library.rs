//! Run-time loading of Dokany's user-mode library.
//!
//! Upstream builds `dokan2.dll` from bundled Dokany sources and links it into
//! every program, which then refuses to start without the DLL and only builds
//! for x86 and x64. This copy loads the library when it is first needed,
//! like MemProcFS: first the `dokan2.dll` that Dokany installed in System32,
//! so library and driver come from one installation on every architecture,
//! then one beside the executable, then the caller's fallback files. The API
//! is resolved once per process and the library is never unloaded, because
//! Dokany may call back on its own threads until the process exits.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libloading::os::windows::{
	Library as RawLibrary, LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
	LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use winapi::{
	shared::minwindef::{DWORD, HMODULE},
	um::libloaderapi::GetModuleFileNameW,
};

use crate::Api;

/// The file name of Dokany 2's user-mode library.
pub const LIBRARY_NAME: &str = "dokan2.dll";

/// The `dokan2.dll` this process loaded and its resolved functions.
pub struct Library {
	path: PathBuf,
	pub(crate) api: Api,
}

impl Library {
	/// The file the library was loaded from.
	pub fn path(&self) -> &Path {
		&self.path
	}

	/// The library's Dokany version without dots, such as `231` for 2.3.1.
	pub fn version(&self) -> u32 {
		// SAFETY: DokanVersion has no preconditions.
		unsafe { (self.api.DokanVersion)() }
	}

	/// The installed driver's Dokany version without dots, or `0` when no
	/// Dokany driver answers.
	pub fn driver_version(&self) -> u32 {
		// SAFETY: DokanDriverVersion has no preconditions.
		unsafe { (self.api.DokanDriverVersion)() }
	}
}

/// Why no usable `dokan2.dll` could be loaded.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
	/// No searched location held a loadable `dokan2.dll`.
	#[error("dokan2.dll is not installed or cannot be loaded ({})", .attempts.join("; "))]
	Unavailable {
		/// Each location tried, with the reason it failed.
		attempts: Vec<String>,
	},
	/// A `dokan2.dll` loaded but does not export the Dokany 2 API.
	#[error("{} does not export the Dokany 2 function {symbol}", .path.display())]
	Incomplete { path: PathBuf, symbol: &'static str },
}

static LIBRARY: OnceLock<Library> = OnceLock::new();

/// The library a successful [`load`] installed, if any.
pub fn loaded() -> Option<&'static Library> {
	LIBRARY.get()
}

/// Loads `dokan2.dll` for this process, or returns the library already
/// loaded. Searches System32, the executable's directory, then each existing
/// file of `fallbacks` in order. A failed search is not remembered, so a
/// later call finds a library installed in the meantime.
pub fn load(fallbacks: &[PathBuf]) -> Result<&'static Library, LoadError> {
	if let Some(library) = LIBRARY.get() {
		return Ok(library);
	}
	let library = open(fallbacks)?;
	// When a concurrent caller won, this load only added a reference to the
	// module it installed.
	Ok(LIBRARY.get_or_init(|| library))
}

/// The library the functions call into, loading it with the default search
/// when no caller has.
pub(crate) fn current() -> Option<&'static Library> {
	LIBRARY.get().or_else(|| load(&[]).ok())
}

fn open(fallbacks: &[PathBuf]) -> Result<Library, LoadError> {
	let mut attempts = Vec::new();
	let installed = [
		("System32", LOAD_LIBRARY_SEARCH_SYSTEM32),
		(
			"the executable's directory",
			LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
		),
	];
	for (location, flags) in installed {
		// SAFETY: loading runs the library's initialisers. The search is
		// restricted to System32 and the executable's directory, so neither
		// the current directory nor PATH can plant a library.
		match unsafe { RawLibrary::load_with_flags(LIBRARY_NAME, flags) } {
			Ok(raw) => return Library::resolve(raw),
			Err(error) => attempts.push(format!("{location}: {error}")),
		}
	}
	for path in fallbacks.iter().filter(|path| path.is_file()) {
		// SAFETY: as above; the file's own dependencies resolve from its
		// directory and System32.
		let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
		match unsafe { RawLibrary::load_with_flags(path, flags) } {
			Ok(raw) => return Library::resolve(raw),
			Err(error) => attempts.push(format!("{}: {error}", path.display())),
		}
	}
	Err(LoadError::Unavailable { attempts })
}

impl Library {
	fn resolve(raw: RawLibrary) -> Result<Self, LoadError> {
		// SAFETY: the module is a file named dokan2.dll loaded as Dokany's
		// library, whose exports have the declared signatures.
		let resolved = unsafe { Api::resolve(&raw) };
		let module = raw.into_raw();
		let path = module_path(module);
		match resolved {
			// The module stays loaded for the rest of the process: the
			// resolved functions point into it.
			Ok(api) => Ok(Self { path, api }),
			Err(symbol) => {
				// SAFETY: `module` came from `into_raw` above and is released
				// exactly once.
				drop(unsafe { RawLibrary::from_raw(module) });
				Err(LoadError::Incomplete { path, symbol })
			}
		}
	}
}

/// The file `module` was loaded from, or the bare library name when Windows
/// cannot say.
fn module_path(module: isize) -> PathBuf {
	let mut buffer = vec![0u16; 260];
	while buffer.len() <= 32_768 {
		// SAFETY: `buffer` is writable for its length and `module` is a loaded
		// module.
		let length = unsafe {
			GetModuleFileNameW(module as HMODULE, buffer.as_mut_ptr(), buffer.len() as DWORD)
		} as usize;
		if length == 0 {
			break;
		}
		if length < buffer.len() {
			return PathBuf::from(OsString::from_wide(&buffer[..length]));
		}
		// The name was truncated to the buffer.
		buffer.resize(buffer.len() * 2, 0);
	}
	PathBuf::from(LIBRARY_NAME)
}
