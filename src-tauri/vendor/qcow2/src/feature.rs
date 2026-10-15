use super::extension::FeatureNameTable;

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Wire discriminants used when validating the feature-name table.
pub enum FeatureKind {
    Incompatible = 0,
    Compatible = 1,
    Autoclear = 2,
}
pub const FEATURE_KIND_COUNT: usize = 3;

// We can't use bitflags, since there may be unknown bits.
pub struct Feature {
    bits: u64,
    kind: FeatureKind,
    names: &'static [&'static str],
}

impl Feature {
    pub fn new(kind: FeatureKind, names: &'static [&'static str]) -> Self {
        Feature {
            bits: 0,
            kind,
            names,
        }
    }

    pub fn set(&mut self, bits: u64) {
        self.bits = bits
    }
    #[allow(dead_code)]
    pub fn enable(&mut self, bit: u64) {
        self.bits |= bit
    }
    #[allow(dead_code)]
    pub fn disable(&mut self, bit: u64) {
        self.bits &= !bit
    }

    pub fn bits(&self) -> u64 {
        self.bits
    }

    // Show a nice representation of a feature set.
    pub fn to_string(&self, table: &FeatureNameTable) -> String {
        let known = self.names.len();
        let mut pos = 0;
        let mut bits = self.bits;
        let mut descs = Vec::new();

        while bits > 0 {
            let trailing = bits.trailing_zeros();
            if trailing > 0 {
                bits >>= trailing;
                pos += trailing;
                continue;
            }

            if (pos as usize) < known {
                descs.push(self.names[pos as usize].to_owned());
            } else {
                descs.push(table.name(self.kind, pos as u8).into_owned());
            }
            bits >>= 1;
            pos += 1;
        }

        descs.join(" | ")
    }
}
