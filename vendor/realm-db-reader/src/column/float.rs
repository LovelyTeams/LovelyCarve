use std::sync::Arc;

use crate::array::{RealmRef, ScalarArray};
use crate::column::{Column, ColumnImpl, ColumnType};
use crate::realm::Realm;
use crate::table::ColumnAttributes;

// Float column type implementation
#[derive(Debug, Clone)]
pub(crate) struct FloatColumnType;

impl ColumnType for FloatColumnType {
    type Value = f32;
    type LeafType = ScalarArray;
    type LeafContext = ();
}

// Factory function for float columns
pub(crate) fn create_float_column(
    realm: Arc<Realm>,
    data_ref: RealmRef,
    attributes: ColumnAttributes,
    name: String,
) -> crate::RealmResult<Box<dyn Column>> {
    Ok(Box::new(FloatColumn::new(
        realm,
        data_ref,
        // Float columns are not indexed
        None,
        attributes,
        Some(name),
        (),
    )?))
}

// Type alias for convenience
pub(crate) type FloatColumn = ColumnImpl<FloatColumnType>;
