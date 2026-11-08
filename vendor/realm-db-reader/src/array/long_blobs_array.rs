use std::fmt::Debug;
use std::sync::Arc;

use tracing::{instrument, warn};

use crate::array::{Array, RealmRef};
use crate::realm::{Realm, RealmNode};
use crate::traits::{ArrayLike, Node, NodeWithContext};
use crate::utils::{self, read_array_value};

#[derive(Debug, Clone)]
pub(crate) struct LongBlobsArray {
    array: Array,
}

impl NodeWithContext<()> for LongBlobsArray {
    #[instrument(level = "debug")]
    fn from_ref_with_context(realm: Arc<Realm>, ref_: RealmRef, _: ()) -> crate::RealmResult<Self>
    where
        Self: Sized,
    {
        let array = Array::from_ref(realm, ref_)?;

        assert!(
            array.node.header.has_refs(),
            "LongBlobsArray must have references"
        );
        assert!(
            array.node.header.context_flag(),
            "LongBlobsArray must have context flag set"
        );

        Ok(Self { array })
    }
}

impl LongBlobsArray {
    fn element_is_null(&self, index: usize) -> crate::RealmResult<bool> {
        Ok(self
            .array
            .get_node::<RealmNode>(index)?
            .map(|node| node.header.size == 0)
            .unwrap_or(true))
    }

    fn item_bytes(realm: Arc<Realm>, ref_: RealmRef) -> crate::RealmResult<Option<Vec<u8>>> {
        let item: RealmNode = RealmNode::from_ref(Arc::clone(&realm), ref_)?;
        let payload = item.payload();
        let size = item.header.size as usize;

        if size == 0 {
            return Ok(None);
        }

        assert!(
            size <= payload.len(),
            "LongBlobsArray: size ({size}) is greater than payload length ({})",
            payload.len()
        );

        // The payload is owned by item.node, which is dropped at the end of this function.
        // Returning a reference to its data is invalid. Instead, return an owned Vec<u8>.
        Ok(Some(payload[..size].to_vec()))
    }
}

impl ArrayLike<Option<Vec<u8>>> for LongBlobsArray {
    #[instrument(level = "debug")]
    fn get(&self, index: usize) -> crate::RealmResult<Option<Vec<u8>>> {
        let Some(ref_) = self.array.get_ref(index) else {
            warn!("get: index={index} returned NULL");
            return Ok(None);
        };

        Self::item_bytes(Arc::clone(&self.array.node.realm), ref_)
    }

    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        _: (),
    ) -> crate::RealmResult<Option<Vec<u8>>> {
        let header = realm.header(ref_)?;

        assert!(
            index < header.size as usize,
            "LongBlobsArray: index ({index}) is out of bounds"
        );

        let item_ref = read_array_value(
            realm.payload(ref_, header.payload_len()),
            header.width(),
            index,
        );
        if item_ref == 0 {
            return Ok(None);
        }

        let item_ref = RealmRef::new(item_ref as usize);
        Self::item_bytes(Arc::clone(&realm), item_ref)
    }

    fn is_null(&self, index: usize) -> crate::RealmResult<bool> {
        self.element_is_null(index)
    }

    fn size(&self) -> usize {
        self.array.node.header.size as usize
    }
}

impl ArrayLike<Option<String>> for LongBlobsArray {
    fn get(&self, index: usize) -> crate::RealmResult<Option<String>> {
        let bytes = <Self as ArrayLike<Option<Vec<u8>>>>::get(self, index)?;

        Ok(bytes.map(utils::string_from_bytes))
    }

    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        context: (),
    ) -> crate::RealmResult<Option<String>>
    where
        Self: Sized,
    {
        let bytes = <Self as ArrayLike<Option<Vec<u8>>>>::get_direct(realm, ref_, index, context)?;

        Ok(bytes.map(utils::string_from_bytes))
    }

    fn is_null(&self, index: usize) -> crate::RealmResult<bool> {
        self.element_is_null(index)
    }

    fn size(&self) -> usize {
        self.array.node.header.size as usize
    }
}

impl ArrayLike<String> for LongBlobsArray {
    fn get(&self, index: usize) -> crate::RealmResult<String> {
        <Self as ArrayLike<Option<String>>>::get(self, index).map(|s| s.unwrap_or_default())
    }

    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        context: (),
    ) -> crate::RealmResult<String>
    where
        Self: Sized,
    {
        <Self as ArrayLike<Option<String>>>::get_direct(realm, ref_, index, context)
            .map(|s| s.unwrap_or_default())
    }

    fn is_null(&self, index: usize) -> crate::RealmResult<bool> {
        self.element_is_null(index)
    }

    fn size(&self) -> usize {
        self.array.node.header.size as usize
    }
}
