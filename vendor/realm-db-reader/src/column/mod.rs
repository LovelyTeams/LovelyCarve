use std::fmt::Debug;

use crate::array::RealmRef;
pub(crate) use crate::column::backlink::create_backlink_column;
pub(crate) use crate::column::bool::create_bool_column;
pub(crate) use crate::column::bool_optional::create_bool_null_column;
use crate::column::bptree::BpTree;
pub(crate) use crate::column::double::create_double_column;
pub(crate) use crate::column::float::create_float_column;
pub(crate) use crate::column::integer::create_int_column;
pub(crate) use crate::column::integer_optional::create_int_null_column;
pub(crate) use crate::column::link::create_link_column;
pub(crate) use crate::column::linklist::create_linklist_column;
pub(crate) use crate::column::string::create_string_column;
pub(crate) use crate::column::subtable::create_subtable_column;
pub(crate) use crate::column::timestamp::create_timestamp_column;
use crate::index::Index;
use crate::realm::Realm;
use crate::table::ColumnAttributes;
use crate::traits::{ArrayLike, Node, NodeWithContext};
use crate::value::Value;
use std::sync::Arc;

mod backlink;
mod bool;
mod bool_optional;
mod bptree;
mod double;
mod float;
mod integer;
mod integer_optional;
mod link;
mod linklist;
mod string;
mod subtable;
mod timestamp;

/// A column for a table.
pub trait Column: Debug + Send {
    /// Get the value for this column for the row with the given index.
    fn get(&self, index: usize) -> crate::RealmResult<Value>;

    /// Check whether the value at the given index is null. Note that some
    /// column types are never null, see [`Value`] for details.
    fn is_null(&self, index: usize) -> crate::RealmResult<bool>;

    /// Get the total number of values in this column.
    fn count(&self) -> crate::RealmResult<usize>;

    /// Get whether this column is nullable. Note that some column types are
    /// never null, see [`Value`] for details.
    fn nullable(&self) -> bool;

    /// Is this column indexed? If so, you can use
    /// [`table.find_row_from_indexed_column`](crate::Table::find_row_from_indexed_column)
    /// to find rows by a known value based on this column.
    fn is_indexed(&self) -> bool;

    /// Look up a value for this column in the index.
    ///
    /// Panics if this column is not indexed.
    fn get_row_number_by_index(&self, lookup_value: &Value) -> crate::RealmResult<Option<usize>>;

    /// Get the name of this column. All columns except backlinks are named.
    fn name(&self) -> Option<&str>;
}

/// The definition of a column type, which includes the value type, leaf type, and B+Tree type.
pub(crate) trait ColumnType {
    type Value: Into<Value>;
    type LeafContext: Copy + Debug;
    type LeafType: ArrayLike<Self::Value, Self::LeafContext>;
}

struct ColumnImpl<T: ColumnType> {
    tree: BpTree<T>,
    index: Option<Index>,
    attributes: ColumnAttributes,
    name: Option<String>,
}

impl<T: ColumnType> Debug for ColumnImpl<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColumnImpl")
            .field("tree", &self.tree)
            .field("attributes", &self.attributes)
            .field("name", &self.name)
            .finish()
    }
}

impl<T: ColumnType + Send> Column for ColumnImpl<T>
where
    Value: From<T::Value>,
    <T as ColumnType>::LeafContext: Send,
    <T as ColumnType>::LeafType: Send,
{
    fn get(&self, index: usize) -> crate::RealmResult<Value> {
        Ok(Value::from(self.tree.get(index)?))
    }

    fn is_null(&self, index: usize) -> crate::RealmResult<bool> {
        self.tree.is_null(index)
    }

    fn count(&self) -> crate::RealmResult<usize> {
        self.tree.count()
    }

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
        self.name.as_deref()
    }
}

impl<T: ColumnType> ColumnImpl<T> {
    pub(crate) fn new(
        realm: Arc<Realm>,
        data_ref: RealmRef,
        index_ref: Option<RealmRef>,
        attributes: ColumnAttributes,
        name: Option<String>,
        context: T::LeafContext,
    ) -> crate::RealmResult<Self> {
        let tree = BpTree::from_ref_with_context(Arc::clone(&realm), data_ref, context)?;
        let index = index_ref
            .map(|ref_| Index::from_ref(realm, ref_))
            .transpose()?;

        Ok(Self {
            tree,
            index,
            attributes,
            name,
        })
    }
}
