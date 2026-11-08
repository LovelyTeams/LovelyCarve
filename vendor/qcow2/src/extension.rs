use std::borrow::Cow;
use std::fmt::Debug;
use std::io::{ErrorKind, Read};

use byteorder::ReadBytesExt;

use super::feature::{FeatureKind, FEATURE_KIND_COUNT};
use super::{Error, Result};

/// Parses the payload of a bounded header extension; must consume it fully.
pub trait Extension: Debug {
    fn read(&mut self, io: &mut dyn Read) -> Result<()>;
}

#[derive(Debug)]
pub struct FeatureName {
    kind: u8,
    bit: u8,
    name: String,
}
#[derive(Debug, Default)]
pub struct FeatureNameTable(Vec<FeatureName>);
impl FeatureNameTable {
    pub fn name(&self, kind: FeatureKind, bit: u8) -> Cow<'_, str> {
        for n in &self.0 {
            if n.kind == kind as u8 && n.bit == bit {
                return Cow::Borrowed(&n.name);
            }
        }
        Cow::Owned(format!("bit {} of {:?}", bit, kind))
    }
}
impl Extension for FeatureNameTable {
    fn read(&mut self, io: &mut dyn Read) -> Result<()> {
        loop {
            match io.read_u8() {
                Err(ref e) if e.kind() == ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(Error::Io(e)),
                Ok(kind) => {
                    if kind >= FEATURE_KIND_COUNT as u8 {
                        return Err(Error::FileFormat(
                            "unknown feature type in feature name table".to_owned(),
                        ));
                    }

                    let bit = io.read_u8()?;
                    if bit > 63 {
                        return Err(Error::FileFormat(
                            "bit number too high in feature name table".to_owned(),
                        ));
                    }

                    let mut buf = [0; 46];
                    io.read_exact(&mut buf)?;
                    // Remove trailing zero bytes from name.
                    let chars = buf
                        .iter()
                        .take_while(|&&c| c != 0)
                        .copied()
                        .collect::<Vec<_>>();
                    // Error on non-ASCII characters, are those supported?
                    match chars.iter().find(|c| !c.is_ascii()) {
                        None => {}
                        Some(_) => {
                            return Err(Error::FileFormat(
                                "unsafe characters in feature name table".to_owned(),
                            ))
                        }
                    }
                    // This can't fail!
                    let name = String::from_utf8(chars)
                        .map_err(|_| Error::FileFormat("invalid feature name".into()))?;
                    self.0.push(FeatureName { kind, bit, name });
                }
            }
        }
        Ok(())
    }
}
