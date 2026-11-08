use std::fmt::Debug;
use std::sync::Arc;

use tracing::{instrument, warn};

use crate::array::{Array, RealmRef};
use crate::realm::{Realm, RealmNode};
use crate::traits::{ArrayLike, Node, NodeWithContext};
use crate::utils;

#[derive(Debug, Clone)]
pub(crate) struct SmallBlobsArray {
    lengths: Array,
    blobs: RealmNode,
    null: Option<Array>,
}

impl NodeWithContext<()> for SmallBlobsArray {
    fn from_ref_with_context(realm: Arc<Realm>, ref_: RealmRef, _: ()) -> crate::RealmResult<Self>
    where
        Self: Sized,
    {
        let array = Array::from_ref(realm, ref_)?;

        let size = array.node.header.size as usize;
        assert!(size >= 2, "SmallBlobsArray size must be at least 2");
        assert!(size <= 3, "SmallBlobsArray size must be at most 3");

        let lengths_array: Array = array.get_node(0)?.unwrap();
        let blobs: RealmNode = array.get_node(1)?.unwrap();
        let null_array: Option<Array> = if size == 3 { array.get_node(2)? } else { None };

        if let Some(null_array) = &null_array {
            assert_eq!(lengths_array.size(), null_array.size());
        }

        Ok(Self {
            lengths: lengths_array,
            blobs,
            null: null_array,
        })
    }
}

impl ArrayLike<Option<Vec<u8>>> for SmallBlobsArray {
    #[instrument(level = "debug")]
    fn get(&self, index: usize) -> crate::RealmResult<Option<Vec<u8>>> {
        if let Some(null_array) = &self.null {
            let is_null = null_array.get(index);
            assert!(
                is_null == 0 || is_null == 1,
                "Invalid null value: {is_null}"
            );
            if is_null == 0 {
                return Ok(None);
            }
        }

        let begin = if index == 0 {
            0
        } else {
            self.lengths.get(index - 1) as usize
        };
        let end = self.lengths.get(index) as usize;

        assert!(
            end > begin,
            "Invalid blob length: end ({end}) <= begin ({begin})"
        );

        assert!(
            end <= self.blobs.payload().len(),
            "Blob end index out of bounds: {end} >= {}",
            self.blobs.payload().len()
        );

        Ok(Some(self.blobs.payload()[begin..end].to_vec()))
    }

    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        _: (),
    ) -> crate::RealmResult<Option<Vec<u8>>> {
        // No real way to do this without basically reconstructing the constructor. Might as well just call that and use `get` to get the bytes.
        let array = SmallBlobsArray::from_ref(realm, ref_)?;
        array.get(index)
    }

    fn is_null(&self, index: usize) -> crate::RealmResult<bool> {
        if let Some(nulls) = &self.null {
            Ok(nulls.get(index) == 0)
        } else {
            Ok(false)
        }
    }

    fn size(&self) -> usize {
        self.lengths.size()
    }
}

impl ArrayLike<Option<String>> for SmallBlobsArray {
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
        if let Some(nulls) = &self.null {
            Ok(nulls.get(index) == 0)
        } else {
            Ok(false)
        }
    }

    fn size(&self) -> usize {
        self.lengths.size()
    }
}

impl ArrayLike<String> for SmallBlobsArray {
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
        if let Some(nulls) = &self.null {
            Ok(nulls.get(index) == 0)
        } else {
            Ok(false)
        }
    }

    fn size(&self) -> usize {
        self.lengths.size()
    }
}
