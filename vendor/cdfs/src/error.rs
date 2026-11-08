// SPDX-License-Identifier: (MIT OR Apache-2.0)

use std::{
    io,
    num::{ParseIntError, TryFromIntError},
    str,
};

use thiserror::Error;

/// The master error structure.
#[derive(Error, Debug)]
pub enum ISOError {
    /// I/O error while trying to read the filesystem image.
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// String value that was expected to fit into a UTF-8 shape, in fact did not.  This is not the
    /// greatest design decision as nothing in ISO 9660 / ECMA-119 should be encoded in UTF-8.
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] str::Utf8Error),

    /// `encoding-rs` ran into one or more errors parsing the string.  Joliet only.
    #[error("UTF-16 / UCS-2 conversion error")]
    Utf16,

    /// The filesystem contained error(s) and could not be parsed.
    #[error("Invalid ISO9660: {0}")]
    InvalidFs(&'static str),

    /// A [`String`] that was supposed to contain a numeric value did not.  Currently this error only occurs in the file identifier parsing code.
    #[error("Int parse error: {0}")]
    ParseInt(#[from] ParseIntError),

    /// An error trying to go from one size of integer to another.
    #[error("Integer conversion error (over/underflow): {0}")]
    TryFromInt(#[from] TryFromIntError),

    /// The buffer or block did not have enough data.  Presumably the filesystem is corrupt.
    ///
    /// # See Also
    ///
    /// [`BLOCK_SIZE`](crate::BLOCK_SIZE)
    #[error("Wanted to read '{}' bytes, got '{0}' bytes", crate::BLOCK_SIZE)]
    ReadSize(usize),

    /// A `nom` parser failed. Most likely the filesystem is either corrupt.  Enabling the
    /// `verbose-error` feature will replace this variant with the `VerboseNom` variant and take
    /// advantage of nom's `VerboseError` type.
    #[cfg(not(feature = "verbose-error"))]
    #[error("Parse error: {0:?}")]
    Nom(nom::error::ErrorKind),

    /// A `nom` parser failed. Most likely the filesystem is either corrupt.  This is the verbose
    /// variant.  If you don't need the extra context that nom's `VerboseError` type brings,
    /// disabling the `verbose-error` feature will utilize the standard nom error type.
    #[cfg(feature = "verbose-error")]
    #[error("Parse error: {0:?}")]
    VerboseNom(nom::error::VerboseError<Vec<u8>>),
}

#[cfg(not(feature = "verbose-error"))]
impl From<nom::Err<nom::error::Error<&[u8]>>> for ISOError {
    fn from(err: nom::Err<nom::error::Error<&[u8]>>) -> ISOError {
        ISOError::Nom(match err {
            nom::Err::Error(e) | nom::Err::Failure(e) => e.code,
            nom::Err::Incomplete(_) => panic!(), // XXX
        })
    }
}

#[cfg(feature = "verbose-error")]
impl From<nom::Err<nom::error::VerboseError<&[u8]>>> for ISOError {
    fn from(err: nom::Err<nom::error::VerboseError<&[u8]>>) -> ISOError {
        ISOError::VerboseNom(match err {
            nom::Err::Error(e) | nom::Err::Failure(e) => nom::error::VerboseError {
                errors: e
                    .errors
                    .into_iter()
                    .map(|(i, e)| (i.to_owned(), e))
                    .collect(),
            },
            nom::Err::Incomplete(_) => panic!(), // XXX
        })
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "verbose-error")] {
        pub(crate) type OurNomError<T> = nom::error::VerboseError<T>;
    } else {
        pub(crate) type OurNomError<T> = nom::error::Error<T>;
    }
}

pub(crate) type NomRes<T, U> = nom::IResult<T, U, OurNomError<T>>;
