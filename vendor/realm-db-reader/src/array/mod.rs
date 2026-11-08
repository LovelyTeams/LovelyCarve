mod array_string;
mod array_string_short;
mod integer_array;
mod long_blobs_array;
mod scalar_array;
mod small_blobs_array;

pub(crate) use array_string::ArrayString;
pub(crate) use array_string_short::ArrayStringShort;
pub(crate) use integer_array::{FromU64, IntegerArray};
pub(crate) use long_blobs_array::LongBlobsArray;
pub(crate) use scalar_array::ScalarArray;
pub(crate) use small_blobs_array::SmallBlobsArray;

use std::fmt::Debug;
use std::ops::Add;
use std::sync::Arc;

use tracing::{debug, instrument};

use crate::realm::{Realm, RealmNode};
use crate::traits::Node;
use crate::utils::read_array_value;

#[derive(Copy, Clone, Hash, PartialEq, Eq)]
pub(crate) struct RealmRef(usize);

impl Debug for RealmRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{:X}", self.0)
    }
}

impl RealmRef {
    pub(crate) fn new(ref_: usize) -> Self {
        assert!(ref_ % 8 == 0, "RealmRef must be a multiple of 8");

        Self(ref_)
    }

    pub(crate) fn to_offset(self) -> usize {
        self.0
    }
}

impl Add<usize> for RealmRef {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        Self(self.0 + rhs)
    }
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum RefOrTaggedValue {
    Ref(RealmRef),
    TaggedValue(u64),
}

impl RefOrTaggedValue {
    pub(crate) fn from_raw(value: u64) -> Self {
        if value & 1 == 0 {
            Self::Ref(RealmRef(value as usize))
        } else {
            Self::TaggedValue(value >> 1)
        }
    }
}

/// Basic array. It only supports fetching u64 values.
#[derive(Debug, Clone)]
pub(crate) struct Array {
    pub(crate) node: RealmNode,
    width: u8,
}

impl Node for Array {
    fn from_ref(realm: Arc<Realm>, ref_: RealmRef) -> crate::RealmResult<Self> {
        let node = RealmNode::from_ref(Arc::clone(&realm), ref_)?;
        let width = node.header.width();

        Ok(Self { node, width })
    }
}

impl Array {
    #[instrument(level = "debug")]
    pub(crate) fn get(&self, index: usize) -> u64 {
        assert!(
            index < self.node.header.size as usize,
            "Index out of bounds: {index} >= {}",
            self.node.header.size
        );

        self.get_direct(self.width, index)
    }

    #[instrument(level = "debug")]
    pub(crate) fn get_ref(&self, index: usize) -> Option<RealmRef> {
        assert!(
            index < self.node.header.size as usize,
            "Index out of bounds: {index} >= {}",
            self.node.header.size
        );

        let ref_ = self.get_direct(self.width, index);

        if ref_ == 0 {
            return None;
        }

        assert!(ref_ % 8 == 0);

        Some(RealmRef(ref_ as usize))
    }

    #[instrument(level = "debug")]
    pub(crate) fn get_ref_or_tagged_value(&self, index: usize) -> Option<RefOrTaggedValue> {
        assert!(
            index < self.node.header.size as usize,
            "Index out of bounds: {index} >= {}",
            self.node.header.size
        );

        let value = self.get_direct(self.width, index);

        if value == 0 {
            return None;
        }

        Some(RefOrTaggedValue::from_raw(value))
    }

    #[instrument(level = "debug")]
    pub(crate) fn get_node<N>(&self, index: usize) -> crate::RealmResult<Option<N>>
    where
        N: Node,
    {
        let Some(ref_) = self.get_ref(index) else {
            return Ok(None);
        };

        debug!(
            "get_node: offset={ref_:?} payload=0x{}",
            hex::encode(self.node.payload())
        );

        N::from_ref(self.node.realm.clone(), ref_).map(Some)
    }

    pub(crate) fn back(&self) -> u64 {
        let size = self.node.header.size as usize;
        if size == 0 {
            return 0;
        }

        read_array_value(self.node.payload(), self.width, size - 1)
    }

    #[instrument(level = "debug")]
    fn get_direct(&self, width: u8, index: usize) -> u64 {
        read_array_value(self.node.payload(), self.width, index)
    }

    pub(crate) fn size(&self) -> usize {
        self.node.header.size as usize
    }
}
