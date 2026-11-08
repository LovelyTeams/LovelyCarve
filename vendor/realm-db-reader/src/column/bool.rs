use crate::array::{RealmRef, ScalarArray};
use crate::column::{Column, ColumnImpl, ColumnType};
use crate::realm::Realm;
use crate::table::ColumnAttributes;
use std::sync::Arc;

// Boolean column type implementation
#[derive(Debug, Clone)]
pub(crate) struct BoolColumnType;

impl ColumnType for BoolColumnType {
    type Value = bool;
    type LeafType = ScalarArray;
    type LeafContext = ();
}

// Factory function for boolean columns
pub(crate) fn create_bool_column(
    realm: Arc<Realm>,
    data_ref: RealmRef,
    index_ref: Option<RealmRef>,
    attributes: ColumnAttributes,
    name: String,
) -> crate::RealmResult<Box<dyn Column>> {
    Ok(Box::new(BoolColumn::new(
        realm,
        data_ref,
        index_ref,
        attributes,
        Some(name),
        (),
    )?))
}

// Type alias for convenience
pub(crate) type BoolColumn = ColumnImpl<BoolColumnType>;
