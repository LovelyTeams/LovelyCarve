use crate::array::FromU64;

/// The type of value contained in a column of a Realm table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[allow(unused)]
pub(crate) enum ColumnType {
    Int = 0,
    Bool = 1,
    String = 2,
    OldStringEnum = 3, // double refs
    Binary = 4,
    Table = 5,
    OldMixed = 6,
    OldDateTime = 7,
    Timestamp = 8,
    Float = 9,
    Double = 10,
    Reserved4 = 11, // Decimal
    Link = 12,
    LinkList = 13,
    BackLink = 14,
}

impl FromU64 for ColumnType {
    fn from_u64(value: u64) -> Self {
        unsafe { std::mem::transmute(value as u8) }
    }
}

impl ColumnType {
    pub(crate) fn has_sub_spec(&self) -> bool {
        matches!(
            self,
            ColumnType::Table | ColumnType::Link | ColumnType::LinkList | ColumnType::BackLink
        )
    }

    pub(crate) fn sub_spec_entries_count(&self) -> usize {
        match self {
            ColumnType::Table | ColumnType::Link | ColumnType::LinkList => 1,
            ColumnType::BackLink => 2,
            _ => 0,
        }
    }
}
