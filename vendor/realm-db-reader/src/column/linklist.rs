use crate::array::{Array, IntegerArray, RealmRef, RefOrTaggedValue};
use crate::column::{Column, ColumnImpl, ColumnType};
use crate::realm::Realm;
use crate::table::ColumnAttributes;
use crate::traits::{ArrayLike, Node, NodeWithContext};
use crate::utils::read_array_value;
use crate::value::Link;
use std::sync::Arc;

pub(crate) struct LinkListColumnType;

#[derive(Debug, Copy, Clone)]
pub(crate) struct LinkListColumnContext {
    target_table_index: usize,
}

impl ColumnType for LinkListColumnType {
    type Value = Vec<Link>;
    type LeafType = LinkListLeaf;
    type LeafContext = LinkListColumnContext;
}

#[derive(Debug)]
pub(crate) struct LinkListLeaf {
    root: Array,
    context: LinkListColumnContext,
}

impl NodeWithContext<LinkListColumnContext> for LinkListLeaf {
    fn from_ref_with_context(
        realm: Arc<Realm>,
        ref_: RealmRef,
        context: LinkListColumnContext,
    ) -> crate::RealmResult<Self>
    where
        Self: Sized,
    {
        let root = Array::from_ref(realm, ref_)?;
        Ok(Self { root, context })
    }
}

impl ArrayLike<Vec<Link>, LinkListColumnContext> for LinkListLeaf {
    fn get(&self, index: usize) -> crate::RealmResult<Vec<Link>> {
        let sub_array = match self.root.get_ref_or_tagged_value(index) {
            Some(RefOrTaggedValue::Ref(ref_)) => {
                Array::from_ref(Arc::clone(&self.root.node.realm), ref_)?
            }
            _ => return Ok(vec![]),
        };

        Ok(Self::get_from_sub_array(sub_array, self.context))
    }

    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        context: LinkListColumnContext,
    ) -> crate::RealmResult<Vec<Link>> {
        let header = realm.header(ref_)?;
        let payload = realm.payload(ref_, header.payload_len());

        let sub_array = match read_array_value(payload, header.width(), index) {
            0 => return Ok(vec![]),
            n => match RefOrTaggedValue::from_raw(n) {
                RefOrTaggedValue::Ref(ref_) => Array::from_ref(Arc::clone(&realm), ref_)?,
                _ => return Ok(vec![]),
            },
        };

        Ok(Self::get_from_sub_array(sub_array, context))
    }

    fn is_null(&self, _: usize) -> crate::RealmResult<bool> {
        Ok(false)
    }

    fn size(&self) -> usize {
        self.root.node.header.size as usize
    }
}

impl LinkListLeaf {
    fn get_from_sub_array(sub_array: Array, context: LinkListColumnContext) -> Vec<Link> {
        assert!(!sub_array.node.header.is_inner_bptree());

        IntegerArray::from_array(sub_array)
            .get_integers()
            .into_iter()
            .map(|x| Link::new(context.target_table_index, x as usize))
            .collect::<Vec<_>>()
    }
}

// Factory function for link list columns
pub(crate) fn create_linklist_column(
    realm: Arc<Realm>,
    ref_: RealmRef,
    attributes: ColumnAttributes,
    target_table_index: usize,
    name: String,
) -> crate::RealmResult<Box<dyn Column>> {
    Ok(Box::new(LinkListColumn::new(
        realm,
        ref_,
        // Link list columns cannot be indexed
        None,
        attributes,
        Some(name),
        LinkListColumnContext { target_table_index },
    )?))
}

pub(crate) type LinkListColumn = ColumnImpl<LinkListColumnType>;
