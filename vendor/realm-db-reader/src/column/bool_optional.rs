use crate::array::{RealmRef, ScalarArray};
use crate::column::{Column, ColumnImpl, ColumnType};
use crate::realm::Realm;
use crate::table::ColumnAttributes;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct BoolNullableColumnType;

impl ColumnType for BoolNullableColumnType {
    type Value = Option<bool>;
    type LeafType = ScalarArray;
    type LeafContext = ();
}

// Factory function for nullable bool columns
pub(crate) fn create_bool_null_column(
    realm: Arc<Realm>,
    data_ref: RealmRef,
    index_ref: Option<RealmRef>,
    attributes: ColumnAttributes,
    name: String,
) -> crate::RealmResult<Box<dyn Column>> {
    Ok(Box::new(BoolNullColumn::new(
        realm,
        data_ref,
        index_ref,
        attributes,
        Some(name),
        (),
    )?))
}

// Type alias for convenience
pub(crate) type BoolNullColumn = ColumnImpl<BoolNullableColumnType>;
