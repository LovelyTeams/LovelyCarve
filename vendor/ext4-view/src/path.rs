// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::dir_entry::{DirEntryName, DirEntryNameError};
use crate::format::{BytesDisplay, format_bytes_debug};
use alloc::string::String;
use alloc::vec::Vec;
use core::error::Error;
use core::fmt::{self, Debug, Display, Formatter};
use core::str::{self, Utf8Error};

/// Error returned when [`Path`] or [`PathBuf`] construction fails.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PathError {
    /// Path contains a component longer than 255 bytes.
    ComponentTooLong,

    /// Path contains a null byte.
    ContainsNull,

    /// Path cannot be created due to encoding.
    ///
    /// This error only occurs on non-Unix targets, where
    /// [`std::os::unix::ffi::OsStrExt`] is not available. On non-Unix
    /// targets, converting an [`OsStr`] or [`std::path::Path`] to
    /// [`ext4_view::Path`] requires first converting the input to a
    /// `&str`, which will fail if the input is not valid UTF-8.
    ///
    /// [`OsStr`]: std::ffi::OsStr
    /// [`ext4_view::Path`]: Path
    Encoding,
}

impl Display for PathError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::ComponentTooLong => {
                write!(f, "path contains a component longer than 255 bytes")
            }
            Self::ContainsNull => write!(f, "path contains a null byte"),
            Self::Encoding => {
                write!(f, "path cannot be created due to encoding")
            }
        }
    }
}

impl Error for PathError {}

/// Reference path type.
///
/// Paths are mostly arbitrary sequences of bytes, with two restrictions:
/// * The path cannot contain any null bytes.
/// * Each component of the path must be no longer than 255 bytes.
#[derive(Clone, Copy, Eq, Ord, PartialOrd, Hash)]
pub struct Path<'a>(
    // Use `&[u8]` rather than `[u8]` so that we don't have to use any
    // unsafe code. Unfortunately that means we can't impl `Deref` to
    // convert from `PathBuf` to `Path`.
    &'a [u8],
);

impl<'a> Path<'a> {
    /// Unix path separator.
    pub const SEPARATOR: u8 = b'/';

    /// Root path, equivalent to `/`.
    pub const ROOT: Path<'static> = Path(&[Self::SEPARATOR]);

    /// Create a new `Path`.
    ///
    /// This panics if the input is invalid, use [`Path::try_from`] if
    /// error handling is desired.
    ///
    /// # Panics
    ///
    /// Panics if the path contains any null bytes or if a component of
    /// the path is longer than 255 bytes.
    #[track_caller]
    pub fn new<P>(p: &'a P) -> Self
    where
        P: AsRef<[u8]> + ?Sized,
    {
        Self::try_from(p.as_ref()).unwrap()
    }

    /// Get whether the path is absolute (starts with `/`).
    #[must_use]
    pub fn is_absolute(self) -> bool {
        if self.0.is_empty() {
            false
        } else {
            self.0[0] == Self::SEPARATOR
        }
    }

    /// Get an object that implements [`Display`] to allow conveniently
    /// printing paths that may or may not be valid UTF-8. Non-UTF-8
    /// characters will be replaced with '�'.
    ///
    /// [`Display`]: core::fmt::Display
    pub fn display(self) -> BytesDisplay<'a> {
        BytesDisplay(self.0)
    }

    /// Create a new `PathBuf` joining `self` with `path`.
    ///
    /// This will add a separator if needed. Note that if the argument
    /// is an absolute path, the returned value will be equal to `path`.
    ///
    /// # Panics
    ///
    /// Panics if the argument is not a valid path.
    #[must_use]
    pub fn join(self, path: impl AsRef<[u8]>) -> PathBuf {
        PathBuf::from(self).join(path)
    }

    /// Get an iterator over each [`Component`] in the path.
    #[must_use]
    pub fn components(self) -> Components<'a> {
        Components {
            path: self,
            offset: 0,
        }
    }

    /// Convert to a `&str` if the path is valid UTF-8.
    pub fn to_str(self) -> Result<&'a str, Utf8Error> {
        str::from_utf8(self.0)
    }
}

impl<'a> AsRef<[u8]> for Path<'a> {
    fn as_ref(&self) -> &'a [u8] {
        self.0
    }
}

impl Debug for Path<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        format_bytes_debug(self.0, f)
    }
}

impl<'a> TryFrom<&'a str> for Path<'a> {
    type Error = PathError;

    fn try_from(s: &'a str) -> Result<Self, PathError> {
        Self::try_from(s.as_bytes())
    }
}

impl<'a> TryFrom<&'a String> for Path<'a> {
    type Error = PathError;

    fn try_from(s: &'a String) -> Result<Self, PathError> {
        Self::try_from(s.as_bytes())
    }
}

impl<'a> TryFrom<&'a [u8]> for Path<'a> {
    type Error = PathError;

    fn try_from(s: &'a [u8]) -> Result<Self, PathError> {
        if s.contains(&0) {
            return Err(PathError::ContainsNull);
        }

        for component in s.split(|b| *b == Path::SEPARATOR) {
            if component.len() > DirEntryName::MAX_LEN {
                return Err(PathError::ComponentTooLong);
            }
        }

        Ok(Self(s))
    }
}

impl<'a, const N: usize> TryFrom<&'a [u8; N]> for Path<'a> {
    type Error = PathError;

    fn try_from(a: &'a [u8; N]) -> Result<Self, PathError> {
        Self::try_from(a.as_slice())
    }
}

impl<'a> TryFrom<&'a PathBuf> for Path<'a> {
    type Error = PathError;

    fn try_from(p: &'a PathBuf) -> Result<Self, PathError> {
        Ok(p.as_path())
    }
}

#[cfg(all(feature = "std", unix))]
impl<'a> TryFrom<&'a std::ffi::OsStr> for Path<'a> {
    type Error = PathError;

    fn try_from(p: &'a std::ffi::OsStr) -> Result<Self, PathError> {
        use std::os::unix::ffi::OsStrExt;

        Self::try_from(p.as_bytes())
    }
}

#[cfg(all(feature = "std", not(unix)))]
impl<'a> TryFrom<&'a std::ffi::OsStr> for Path<'a> {
    type Error = PathError;

    fn try_from(p: &'a std::ffi::OsStr) -> Result<Self, PathError> {
        Self::try_from(p.to_str().ok_or(PathError::Encoding)?)
    }
}

#[cfg(feature = "std")]
impl<'a> TryFrom<&'a std::path::Path> for Path<'a> {
    type Error = PathError;

    fn try_from(p: &'a std::path::Path) -> Result<Self, PathError> {
        Self::try_from(p.as_os_str())
    }
}

impl<T> PartialEq<T> for Path<'_>
where
    T: AsRef<[u8]>,
{
    fn eq(&self, other: &T) -> bool {
        self.0 == other.as_ref()
    }
}

#[cfg(all(feature = "std", unix))]
impl<'a> From<Path<'a>> for &'a std::path::Path {
    fn from(p: Path<'a>) -> &'a std::path::Path {
        use std::os::unix::ffi::OsStrExt;

        let s = std::ffi::OsStr::from_bytes(p.0);
        std::path::Path::new(s)
    }
}

/// Owned path type.
///
/// Paths are mostly arbitrary sequences of bytes, with two restrictions:
/// * The path cannot contain any null bytes.
/// * Each component of the path must be no longer than 255 bytes.
#[derive(Clone, Default, Eq, Ord, PartialOrd, Hash)]
pub struct PathBuf(Vec<u8>);

impl PathBuf {
    /// Create a new `PathBuf`.
    ///
    /// This panics if the input is invalid, use [`Path::try_from`] if
    /// error handling is desired.
    ///
    /// # Panics
    ///
    /// Panics if the path contains any null bytes or if a component of
    /// the path is longer than 255 bytes.
    #[track_caller]
    pub fn new<P>(p: &P) -> Self
    where
        P: AsRef<[u8]> + ?Sized,
    {
        Self::try_from(p.as_ref()).unwrap()
    }

    /// Create empty `PathBuf`.
    #[must_use]
    pub const fn empty() -> Self {
        Self(Vec::new())
    }

    /// Borrow as a `Path`.
    #[must_use]
    pub fn as_path(&self) -> Path<'_> {
        Path(&self.0)
    }

    /// Get whether the path is absolute (starts with `/`).
    #[must_use]
    pub fn is_absolute(&self) -> bool {
        self.as_path().is_absolute()
    }

    /// Get an object that implements [`Display`] to allow conveniently
    /// printing paths that may or may not be valid UTF-8. Non-UTF-8
    /// characters will be replaced with '�'.
    ///
    /// [`Display`]: core::fmt::Display
    pub fn display(&self) -> BytesDisplay<'_> {
        BytesDisplay(&self.0)
    }

    /// Append to the path.
    ///
    /// This will add a separator if needed. Note that if the argument
    /// is an absolute path, `self` will be replaced with that path.
    ///
    /// # Panics
    ///
    /// Panics if the argument is not a valid path, or if memory cannot
    /// be allocated for the resulting path.
    #[track_caller]
    pub fn push(&mut self, path: impl AsRef<[u8]>) {
        #[track_caller]
        fn inner(this: &mut PathBuf, p: &[u8]) {
            // Panic if the arg is not a valid path.
            let p = Path::try_from(p).expect("push arg must be a valid path");

            // If the arg is absolute, replace `self` with the arg rather
            // than appending.
            if p.is_absolute() {
                this.0.clear();
                this.0.extend(p.0);
                return;
            }

            let add_sep = if let Some(last) = this.0.last() {
                *last != b'/'
            } else {
                false
            };

            if add_sep {
                // OK to unwrap: docstring says panic is allowed for
                // memory allocation failure.
                let len = p.0.len().checked_add(1).unwrap();
                this.0.reserve(len);
                this.0.push(Path::SEPARATOR);
            } else {
                this.0.reserve(p.0.len());
            }

            this.0.extend(p.0);
        }

        inner(self, path.as_ref())
    }

    /// Create a new `PathBuf` joining `self` with `path`.
    ///
    /// This will add a separator if needed. Note that if the argument
    /// is an absolute path, the returned value will be equal to `path`.
    ///
    /// # Panics
    ///
    /// Panics if the argument is not a valid path.
    #[must_use]
    pub fn join(&self, path: impl AsRef<[u8]>) -> Self {
        let mut t = self.clone();
        t.push(path);
        t
    }

    /// Get an iterator over each [`Component`] in the path.
    #[must_use]
    pub fn components(&self) -> Components<'_> {
        self.as_path().components()
    }

    /// Convert to a `&str` if the path is valid UTF-8.
    pub fn to_str(&self) -> Result<&str, Utf8Error> {
        self.as_path().to_str()
    }
}

impl AsRef<[u8]> for PathBuf {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Debug for PathBuf {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.as_path().fmt(f)
    }
}

impl TryFrom<&str> for PathBuf {
    type Error = PathError;

    fn try_from(s: &str) -> Result<Self, PathError> {
        Self::try_from(s.as_bytes().to_vec())
    }
}

impl TryFrom<&String> for PathBuf {
    type Error = PathError;

    fn try_from(s: &String) -> Result<Self, PathError> {
        Self::try_from(s.as_bytes().to_vec())
    }
}

impl TryFrom<String> for PathBuf {
    type Error = PathError;

    fn try_from(s: String) -> Result<Self, PathError> {
        Self::try_from(s.into_bytes())
    }
}

impl TryFrom<&[u8]> for PathBuf {
    type Error = PathError;

    fn try_from(s: &[u8]) -> Result<Self, PathError> {
        Self::try_from(s.to_vec())
    }
}

impl<const N: usize> TryFrom<&[u8; N]> for PathBuf {
    type Error = PathError;

    fn try_from(a: &[u8; N]) -> Result<Self, PathError> {
        Self::try_from(a.as_slice().to_vec())
    }
}

impl TryFrom<Vec<u8>> for PathBuf {
    type Error = PathError;

    fn try_from(s: Vec<u8>) -> Result<Self, PathError> {
        // Validate the input.
        Path::try_from(s.as_slice())?;

        Ok(Self(s))
    }
}

#[cfg(all(feature = "std", unix))]
impl TryFrom<std::ffi::OsString> for PathBuf {
    type Error = PathError;

    fn try_from(p: std::ffi::OsString) -> Result<Self, PathError> {
        use std::os::unix::ffi::OsStringExt;

        Self::try_from(p.into_vec())
    }
}

#[cfg(all(feature = "std", not(unix)))]
impl TryFrom<std::ffi::OsString> for PathBuf {
    type Error = PathError;

    fn try_from(p: std::ffi::OsString) -> Result<Self, PathError> {
        Self::try_from(p.into_string().map_err(|_| PathError::Encoding)?)
    }
}

#[cfg(feature = "std")]
impl TryFrom<std::path::PathBuf> for PathBuf {
    type Error = PathError;

    fn try_from(p: std::path::PathBuf) -> Result<Self, PathError> {
        Self::try_from(p.into_os_string())
    }
}

impl<T> PartialEq<T> for PathBuf
where
    T: AsRef<[u8]>,
{
    fn eq(&self, other: &T) -> bool {
        self.0 == other.as_ref()
    }
}

impl<'a> From<Path<'a>> for PathBuf {
    fn from(p: Path<'a>) -> Self {
        Self(p.0.to_vec())
    }
}

#[cfg(all(feature = "std", unix))]
impl From<PathBuf> for std::path::PathBuf {
    fn from(p: PathBuf) -> Self {
        use std::os::unix::ffi::OsStringExt;

        let s = std::ffi::OsString::from_vec(p.0);
        Self::from(s)
    }
}

/// Component of a [`Path`].
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Component<'a> {
    /// Root directory (`/`), used at the start of an absolute path.
    RootDir,

    /// Current directory (`.`).
    CurDir,

    /// Parent directory (`..`).
    ParentDir,

    /// Directory or file name.
    Normal(DirEntryName<'a>),
}

impl<'a> Component<'a> {
    /// Construct a [`Component::Normal`] from the given `name`.
    pub fn normal<T: AsRef<[u8]> + ?Sized>(name: &'a T) -> Result<Self, DirEntryNameError> {
        Ok(Component::Normal(DirEntryName::try_from(name.as_ref())?))
    }
}

impl Debug for Component<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Component::RootDir => write!(f, "RootDir"),
            Component::CurDir => write!(f, "CurDir"),
            Component::ParentDir => write!(f, "ParentDir"),
            Component::Normal(name) => {
                write!(f, "Normal(")?;
                format_bytes_debug(name.as_ref(), f)?;
                write!(f, ")")
            }
        }
    }
}

impl<T> PartialEq<T> for Component<'_>
where
    T: AsRef<[u8]>,
{
    fn eq(&self, other: &T) -> bool {
        let other = other.as_ref();
        match self {
            Component::RootDir => other == b"/",
            Component::CurDir => other == b".",
            Component::ParentDir => other == b"..",
            Component::Normal(c) => *c == other,
        }
    }
}

/// Iterator over [`Component`]s in a [`Path`].
pub struct Components<'a> {
    path: Path<'a>,
    offset: usize,
}

impl<'a> Iterator for Components<'a> {
    type Item = Component<'a>;

    fn next(&mut self) -> Option<Component<'a>> {
        let path = &self.path.0;

        if self.offset >= path.len() {
            return None;
        }

        if self.offset == 0 && path[0] == Path::SEPARATOR {
            self.offset = 1;
            return Some(Component::RootDir);
        }

        // Coalesce repeated separators like "a//b".
        while self.offset < path.len() && path[self.offset] == Path::SEPARATOR {
            // OK to unwrap: `offset` is less than `path.len()`, which
            // is also a `usize`, so adding `1` cannot fail.
            self.offset = self.offset.checked_add(1).unwrap();
        }
        if self.offset >= path.len() {
            return None;
        }

        let end: usize = if let Some(index) = self
            .path
            .0
            .iter()
            .skip(self.offset)
            .position(|b| *b == Path::SEPARATOR)
        {
            // OK to unwrap: this sum is a valid index within `path`,
            // so it must fit in a `usize`.
            self.offset.checked_add(index).unwrap()
        } else {
            path.len()
        };

        let component = &path[self.offset..end];
        let component = if component == b"." {
            Component::CurDir
        } else if component == b".." {
            Component::ParentDir
        } else {
            // Paths are validated at construction time to ensure each
            // component is of a valid length, so don't need to check
            // that here when constructing `DirEntryName`.
            Component::Normal(DirEntryName(component))
        };

        self.offset = end;
        Some(component)
    }
}
