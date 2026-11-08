use crate::array::{Array, RealmRef};
use crate::column::{Column, ColumnImpl, ColumnType};
use crate::realm::Realm;
use crate::table::ColumnAttributes;
use crate::traits::{ArrayLike, Node, NodeWithContext};
use crate::utils::read_array_value;
use crate::value::Link;
use std::sync::Arc;

pub(crate) struct LinkColumnType;

#[derive(Debug, Copy, Clone)]
pub(crate) struct LinkColumnContext {
    target_table_index: usize,
}

impl ColumnType for LinkColumnType {
    type Value = Option<Link>;
    type LeafType = LinkLeaf;
    type LeafContext = LinkColumnContext;
}

#[derive(Debug)]
pub(crate) struct LinkLeaf {
    root: Array,
    context: LinkColumnContext,
}

impl NodeWithContext<LinkColumnContext> for LinkLeaf {
    fn from_ref_with_context(
        realm: Arc<Realm>,
        ref_: RealmRef,
        context: LinkColumnContext,
    ) -> crate::RealmResult<Self>
    where
        Self: Sized,
    {
        let root = Array::from_ref(realm, ref_)?;
        Ok(Self { root, context })
    }
}

impl ArrayLike<Option<Link>, LinkColumnContext> for LinkLeaf {
    fn get(&self, index: usize) -> crate::RealmResult<Option<Link>> {
        let value = self.root.get(index);
        if value == 0 {
            return Ok(None);
        }

        Ok(Some(Link::new(
            self.context.target_table_index,
            value as usize - 1,
        )))
    }

    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        context: LinkColumnContext,
    ) -> crate::RealmResult<Option<Link>> {
        let header = realm.header(ref_)?;
        let payload = realm.payload(ref_, header.payload_len());

        match read_array_value(payload, header.width(), index) {
            0 => Ok(None),
            value => Ok(Some(Link::new(
                context.target_table_index,
                value as usize - 1,
            ))),
        }
    }

    fn is_null(&self, index: usize) -> crate::RealmResult<bool> {
        Ok(self.root.get(index) == 0)
    }

    fn size(&self) -> usize {
        self.root.node.header.size as usize
    }
}

// Factory function for link columns
pub(crate) fn create_link_column(
    realm: Arc<Realm>,
    ref_: RealmRef,
    attributes: ColumnAttributes,
    target_table_index: usize,
    name: String,
) -> crate::RealmResult<Box<dyn Column>> {
    Ok(Box::new(LinkColumn::new(
        realm,
        ref_,
        // Link columns cannot be indexed
        None,
        attributes,
        Some(name),
        LinkColumnContext { target_table_index },
    )?))
}

pub(crate) type LinkColumn = ColumnImpl<LinkColumnType>;
