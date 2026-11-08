use crate::array::{IntegerArray, RealmRef};
use crate::column::{Column, ColumnImpl, ColumnType};
use crate::realm::Realm;
use crate::table::ColumnAttributes;
use std::sync::Arc;

// Integer column type implementation
#[derive(Debug, Clone)]
pub(crate) struct IntColumnType;

impl ColumnType for IntColumnType {
    type Value = i64;
    type LeafType = IntegerArray;
    type LeafContext = ();
}

// Factory function for integer columns
pub(crate) fn create_int_column(
    realm: Arc<Realm>,
    data_ref: RealmRef,
    index_ref: Option<RealmRef>,
    attributes: ColumnAttributes,
    name: String,
) -> crate::RealmResult<Box<dyn Column>> {
    Ok(Box::new(IntColumn::new(
        realm,
        data_ref,
        index_ref,
        attributes,
        Some(name),
        (),
    )?))
}

// Type alias for convenience
pub(crate) type IntColumn = ColumnImpl<IntColumnType>;
