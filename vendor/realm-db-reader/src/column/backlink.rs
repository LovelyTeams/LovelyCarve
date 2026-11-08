use crate::array::{Array, IntegerArray, RealmRef, RefOrTaggedValue};
use crate::column::{Column, ColumnImpl, ColumnType};
use crate::realm::Realm;
use crate::table::ColumnAttributes;
use crate::traits::{ArrayLike, Node, NodeWithContext};
use crate::utils::read_array_value;
use crate::value::Backlink;
use std::sync::Arc;

#[derive(Debug, Copy, Clone)]
struct BacklinkContext {
    target_table_number: usize,
    target_table_column_number: usize,
}

struct BacklinkColumnType;

impl ColumnType for BacklinkColumnType {
    type Value = Option<Backlink>;
    type LeafType = BacklinkArray;
    type LeafContext = BacklinkContext;
}

#[derive(Debug)]
struct BacklinkArray {
    root: Array,
    context: BacklinkContext,
}

impl NodeWithContext<BacklinkContext> for BacklinkArray {
    fn from_ref_with_context(
        realm: Arc<Realm>,
        ref_: RealmRef,
        context: BacklinkContext,
    ) -> crate::RealmResult<Self>
    where
        Self: Sized,
    {
        Ok(Self {
            root: Array::from_ref(realm, ref_)?,
            context,
        })
    }
}

impl ArrayLike<Option<Backlink>, BacklinkContext> for BacklinkArray {
    fn get(&self, index: usize) -> crate::RealmResult<Option<Backlink>> {
        let Some(ref_or_tagged) = self.root.get_ref_or_tagged_value(index) else {
            return Ok(None);
        };

        Ok(Some(Self::get_from_ref_or_tagged_value(
            &self.root.node.realm,
            ref_or_tagged,
            self.context,
        )?))
    }

    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        context: BacklinkContext,
    ) -> crate::RealmResult<Option<Backlink>> {
        let header = realm.header(ref_)?;
        let payload = realm.payload(ref_, header.payload_len());

        let ref_or_tagged = match read_array_value(payload, header.width(), index) {
            0 => return Ok(None),
            n => RefOrTaggedValue::from_raw(n),
        };

        Ok(Some(Self::get_from_ref_or_tagged_value(
            &realm,
            ref_or_tagged,
            context,
        )?))
    }

    fn is_null(&self, _: usize) -> crate::RealmResult<bool> {
        Ok(false)
    }

    fn size(&self) -> usize {
        self.root.node.header.size as usize
    }
}

impl BacklinkArray {
    fn get_from_ref_or_tagged_value(
        realm: &Arc<Realm>,
        value: RefOrTaggedValue,
        context: BacklinkContext,
    ) -> crate::RealmResult<Backlink> {
        match value {
            RefOrTaggedValue::Ref(ref_) => {
                let backlink_list = IntegerArray::from_ref(Arc::clone(realm), ref_)?;
                let values = backlink_list
                    .get_integers()
                    .into_iter()
                    .map(|n| n as usize)
                    .collect();
                Ok(Backlink::new(
                    context.target_table_number,
                    context.target_table_column_number,
                    values,
                ))
            }
            RefOrTaggedValue::TaggedValue(value) => Ok(Backlink::new(
                context.target_table_number,
                context.target_table_column_number,
                vec![value as usize],
            )),
        }
    }
}

// Factory function for boolean columns
pub(crate) fn create_backlink_column(
    realm: Arc<Realm>,
    ref_: RealmRef,
    attributes: ColumnAttributes,
    target_table_number: usize,
    target_table_column_number: usize,
) -> crate::RealmResult<Box<dyn Column>> {
    Ok(Box::new(BacklinkColumn::new(
        realm,
        ref_,
        // Backlink columns cannot be indexed.
        None,
        attributes,
        None,
        BacklinkContext {
            target_table_number,
            target_table_column_number,
        },
    )?))
}

type BacklinkColumn = ColumnImpl<BacklinkColumnType>;
