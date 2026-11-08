//! # Subtable column implementation
//!
//! Subtables have multiple layers of indirection:
//!
//! [`crate::column::bptree::BpTree`] -> [`SubtableArrayLeaf`] -> [`Table`]
//!
//! The first layer, an instance of [`crate::column::bptree::BpTree`], just handles the top-level
//! array being a B+Tree. Once it finds the node in the top-level data array, it creates and calls
//! the [`SubtableArrayLeaf`]. At this point, we're either in a sub-array of the B+Tree, or still
//! in the top level data array.
//!
//! There we can just get the row index, and create the [`Table`]. That one receives a reference
//! to the data array for the subtable for the given row, so we can create the
//! [`crate::table::TableHeader`] and fetch all rows from it.

use tracing::instrument;

use crate::array::{Array, RealmRef, RefOrTaggedValue};
use crate::column::{Column, ColumnImpl, ColumnType};
use crate::realm::Realm;
use crate::table::{ColumnAttributes, Row, Table};
use crate::traits::{ArrayLike, Node, NodeWithContext};
use crate::utils::read_array_value;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub(crate) struct SubtableContext {
    header_ref: RealmRef,
}

pub(crate) struct SubtableColumnType;
impl ColumnType for SubtableColumnType {
    type Value = Option<Vec<Row<'static>>>;
    type LeafType = SubtableArrayLeaf;
    type LeafContext = SubtableContext;
}

#[derive(Debug)]
pub(crate) struct SubtableArrayLeaf {
    root: Array,
    header_array: Array,
}

impl NodeWithContext<SubtableContext> for SubtableArrayLeaf {
    #[instrument(level = "debug")]
    fn from_ref_with_context(
        realm: Arc<Realm>,
        ref_: RealmRef,
        context: SubtableContext,
    ) -> crate::RealmResult<Self>
    where
        Self: Sized,
    {
        let root = Array::from_ref(Arc::clone(&realm), ref_)?;
        let header_array = Array::from_ref(realm, context.header_ref)?;

        Ok(SubtableArrayLeaf { root, header_array })
    }
}

impl ArrayLike<Option<Vec<Row<'static>>>, SubtableContext> for SubtableArrayLeaf {
    fn get(&self, index: usize) -> crate::RealmResult<Option<Vec<Row<'static>>>> {
        let Some(data_array) = self.root.get_node(index)? else {
            return Ok(None);
        };

        Ok(Some(
            Table::build_from(&self.header_array, data_array, usize::MAX)?
                .get_rows()?
                .into_iter()
                .map(Row::into_owned)
                .collect(),
        ))
    }

    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        context: SubtableContext,
    ) -> crate::RealmResult<Option<Vec<Row<'static>>>> {
        let header = realm.header(ref_)?;
        let payload = realm.payload(ref_, header.payload_len());

        let data_array = match read_array_value(payload, header.width(), index) {
            0 => return Ok(None),
            n => match RefOrTaggedValue::from_raw(n) {
                RefOrTaggedValue::Ref(ref_) => Array::from_ref(Arc::clone(&realm), ref_)?,
                _ => return Ok(None),
            },
        };
        let header_array = Array::from_ref(realm, context.header_ref)?;

        Ok(Some(
            Table::build_from(&header_array, data_array, usize::MAX)?
                .get_rows()?
                .into_iter()
                .map(|row| Row::into_owned(row))
                .collect(),
        ))
    }

    fn is_null(&self, index: usize) -> crate::RealmResult<bool> {
        Ok(self.root.get_ref(index).is_none())
    }

    fn size(&self) -> usize {
        self.root.node.header.size as usize
    }
}

// Factory function for subtable columns
pub(crate) fn create_subtable_column(
    realm: Arc<Realm>,
    header_ref: RealmRef,
    data_ref: RealmRef,
    attributes: ColumnAttributes,
    name: String,
) -> crate::RealmResult<Box<dyn Column>> {
    Ok(Box::new(SubtableColumn::new(
        realm,
        data_ref,
        // Subtables cannot be indexed.
        None,
        attributes,
        Some(name),
        SubtableContext { header_ref },
    )?))
}

pub(crate) type SubtableColumn = ColumnImpl<SubtableColumnType>;
