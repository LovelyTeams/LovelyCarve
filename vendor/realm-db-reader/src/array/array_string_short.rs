use std::fmt::Debug;
use std::sync::Arc;

use crate::array::RealmRef;
use crate::realm::{Realm, RealmNode};
use crate::traits::{ArrayLike, Node, NodeWithContext};
use std::str;
use tracing::{debug, instrument};

#[derive(Debug, Clone)]
pub(crate) struct ArrayStringShort {
    node: RealmNode,
}

impl NodeWithContext<()> for ArrayStringShort {
    fn from_ref_with_context(realm: Arc<Realm>, ref_: RealmRef, _: ()) -> crate::RealmResult<Self>
    where
        Self: Sized,
    {
        let node = RealmNode::from_ref(realm, ref_)?;

        Ok(Self { node })
    }
}

impl ArrayLike<Option<String>> for ArrayStringShort {
    #[instrument(level = "debug")]
    fn get(&self, index: usize) -> crate::RealmResult<Option<String>> {
        Ok(Self::get_static(&self.node, index).map(|s| s.to_string()))
    }

    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        _: (),
    ) -> crate::RealmResult<Option<String>> {
        let node = RealmNode::from_ref(realm, ref_)?;

        Ok(Self::get_static(&node, index).map(|s| s.to_string()))
    }

    fn is_null(&self, index: usize) -> crate::RealmResult<bool> {
        let width = self.node.header.width();
        if width == 0 {
            return Ok(true);
        }

        // Every element has an indicator of the number of zeroes ('\0') in its
        // last byte. Read only that last byte.
        let width_byte_index = index * width as usize + width as usize - 1;
        let zeroes = self.node.payload()[width_byte_index];

        // The element is null if all bytes are zeroes (equal to the width).
        Ok(zeroes == width)
    }

    fn size(&self) -> usize {
        self.node.header.size as usize
    }
}

impl ArrayLike<String> for ArrayStringShort {
    fn get(&self, index: usize) -> crate::RealmResult<String> {
        <Self as ArrayLike<Option<String>>>::get(self, index).map(|s| s.unwrap_or_default())
    }

    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        context: (),
    ) -> crate::RealmResult<String> {
        <Self as ArrayLike<Option<String>>>::get_direct(realm, ref_, index, context)
            .map(|s| s.unwrap_or_default())
    }

    fn is_null(&self, _: usize) -> crate::RealmResult<bool> {
        // Implementing for `String`, so we always return false.
        Ok(false)
    }

    fn size(&self) -> usize {
        self.node.header.size as usize
    }
}

impl ArrayStringShort {
    #[instrument(level = "debug")]
    fn get_static(node: &RealmNode, index: usize) -> Option<&str> {
        let width = node.header.width() as usize;
        if width == 0 {
            debug!("get: width is 0, returning None");
            return None;
        }

        let element_data = &node.payload()[index * width..(index + 1) * width];
        let zeroes = element_data[width - 1] as usize;
        if zeroes == width {
            return None;
        }

        debug!(
            "get: index={index} width={width} zeroes={zeroes} element_data=0x{}",
            hex::encode(element_data)
        );

        // e.g. width = 4, zeroes = 1, element_data = [xx, xx, 00, 01]
        Some(unsafe { str::from_utf8_unchecked(&element_data[..width - 1 - zeroes]) })
    }
}
