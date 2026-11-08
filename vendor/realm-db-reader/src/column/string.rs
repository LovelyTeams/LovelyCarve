use crate::array::{Array, ArrayString, LongBlobsArray, RealmRef, SmallBlobsArray};
use crate::column::Column;
use crate::column::bptree::BpTreeNode;
use crate::index::Index;
use crate::realm::Realm;
use crate::table::ColumnAttributes;
use crate::traits::{ArrayLike, Node};
use crate::value::Value;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct StringColumn {
    root: Array,
    index: Option<Index>,
    attributes: ColumnAttributes,
    name: String,
}

impl StringColumn {
    pub(crate) fn new(
        realm: Arc<Realm>,
        data_ref: RealmRef,
        index_ref: Option<RealmRef>,
        attributes: ColumnAttributes,
        name: String,
    ) -> crate::RealmResult<Self> {
        let root = Array::from_ref(Arc::clone(&realm), data_ref)?;
        let index = index_ref
            .map(|ref_| Index::from_ref(realm, ref_))
            .transpose()?;

        Ok(StringColumn {
            root,
            index,
            attributes,
            name,
        })
    }
}

impl Column for StringColumn {
    /// Get the value for this column for the row with the given index.
    fn get(&self, index: usize) -> crate::RealmResult<Value> {
        if self.root_is_leaf() {
            return Ok(if self.nullable() {
                ArrayString::<Option<String>>::get_inner(
                    &self.root.node.header,
                    Arc::clone(&self.root.node.realm),
                    self.root.node.ref_,
                )?
                .get(index)?
                .into()
            } else {
                ArrayString::<String>::get_inner(
                    &self.root.node.header,
                    Arc::clone(&self.root.node.realm),
                    self.root.node.ref_,
                )?
                .get(index)?
                .into()
            });
        }

        // Non-leaf root
        let (leaf_ref, index_in_leaf) = BpTreeNode::new(&self.root).get_bptree_leaf(index)?;
        let leaf_header = self.root.node.realm.header(leaf_ref)?;

        Ok(if self.nullable() {
            ArrayString::<Option<String>>::get_inner(
                &leaf_header,
                Arc::clone(&self.root.node.realm),
                leaf_ref,
            )?
            .get(index_in_leaf)?
            .into()
        } else {
            ArrayString::<String>::get_inner(
                &leaf_header,
                Arc::clone(&self.root.node.realm),
                leaf_ref,
            )?
            .get(index_in_leaf)?
            .into()
        })
    }

    fn is_null(&self, index: usize) -> crate::RealmResult<bool> {
        Ok(self.nullable() && self.get(index)?.is_none())
    }

    /// Get the total number of values in this column.
    fn count(&self) -> crate::RealmResult<usize> {
        if self.root_is_leaf() {
            let long_strings = self.root.node.header.has_refs();
            if !long_strings {
                return Ok(self.root.node.header.size as usize);
            }

            let is_big = self.root.node.header.context_flag();
            if !is_big {
                // Small strings
                let small_blobs_array = SmallBlobsArray::from_ref(
                    Arc::clone(&self.root.node.realm),
                    self.root.node.ref_,
                )?;
                return Ok(<SmallBlobsArray as ArrayLike<String>>::size(
                    &small_blobs_array,
                ));
            }

            // Long strings
            let long_blobs_array =
                LongBlobsArray::from_ref(Arc::clone(&self.root.node.realm), self.root.node.ref_)?;
            return Ok(<LongBlobsArray as ArrayLike<String>>::size(
                &long_blobs_array,
            ));
        }

        // Non-leaf root
        Ok(BpTreeNode::new(&self.root).get_bptree_size())
    }

    /// Get whether this column is nullable.
    fn nullable(&self) -> bool {
        self.attributes.is_nullable()
    }

    fn is_indexed(&self) -> bool {
        self.attributes.is_indexed()
    }

    fn get_row_number_by_index(&self, lookup_value: &Value) -> crate::RealmResult<Option<usize>> {
        let Some(index) = &self.index else {
            panic!("Column {:?} is not indexed", self.name());
        };

        index.find_first(lookup_value)
    }

    fn name(&self) -> Option<&str> {
        Some(&self.name)
    }
}

impl StringColumn {
    fn root_is_leaf(&self) -> bool {
        !self.root.node.header.is_inner_bptree()
    }
}

// Factory function for string columns
pub(crate) fn create_string_column(
    realm: Arc<Realm>,
    data_ref: RealmRef,
    index_ref: Option<RealmRef>,
    attributes: ColumnAttributes,
    name: String,
) -> crate::RealmResult<Box<dyn Column>> {
    Ok(Box::new(StringColumn::new(
        realm, data_ref, index_ref, attributes, name,
    )?))
}
