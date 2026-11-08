use std::fmt::Debug;

use crate::array::FromU64;

#[derive(Copy, Clone)]
pub(crate) struct ColumnAttributes(u64);

impl ColumnAttributes {
    const INDEXED: u64 = 1 << 0;
    const UNIQUE: u64 = 1 << 1;
    const RESERVED: u64 = 1 << 2;
    const STRONG_LINKS: u64 = 1 << 3;
    const NULLABLE: u64 = 1 << 4;

    pub(crate) fn new(attributes: u64) -> Self {
        Self(attributes)
    }

    pub(crate) fn is_indexed(&self) -> bool {
        self.0 & Self::INDEXED != 0
    }

    pub(crate) fn is_unique(&self) -> bool {
        self.0 & Self::UNIQUE != 0
    }

    pub(crate) fn is_reserved(&self) -> bool {
        self.0 & Self::RESERVED != 0
    }

    pub(crate) fn is_strong_links(&self) -> bool {
        self.0 & Self::STRONG_LINKS != 0
    }

    pub(crate) fn is_nullable(&self) -> bool {
        self.0 & Self::NULLABLE != 0
    }
}

impl Debug for ColumnAttributes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("ColumnAttributes");
        if self.is_indexed() {
            s.field("indexed", &true);
        }
        if self.is_unique() {
            s.field("unique", &true);
        }
        if self.is_reserved() {
            s.field("reserved", &true);
        }
        if self.is_strong_links() {
            s.field("strong_links", &true);
        }
        if self.is_nullable() {
            s.field("nullable", &true);
        }
        s.finish()
    }
}

impl FromU64 for ColumnAttributes {
    fn from_u64(attributes: u64) -> Self {
        Self::new(attributes)
    }
}
