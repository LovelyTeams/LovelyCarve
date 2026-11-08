use std::fmt::Debug;
use std::sync::Arc;

use crate::array::{Array, RealmRef};
use crate::realm::Realm;
use crate::traits::{ArrayLike, Node, NodeWithContext};
use crate::utils::read_array_value;

pub(crate) trait FromU64 {
    fn from_u64(value: u64) -> Self;
}

#[derive(Debug, Clone)]
pub(crate) struct IntegerArray {
    array: Array,
}

impl NodeWithContext<()> for IntegerArray {
    fn from_ref_with_context(realm: Arc<Realm>, ref_: RealmRef, _: ()) -> crate::RealmResult<Self>
    where
        Self: Sized,
    {
        let array = Array::from_ref(realm, ref_)?;

        Ok(Self::from_array(array))
    }
}

impl ArrayLike<u64> for IntegerArray {
    fn get(&self, index: usize) -> crate::RealmResult<u64> {
        Ok(self.array.get(index))
    }

    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        _: (),
    ) -> crate::RealmResult<u64> {
        let header = realm.header(ref_)?;
        let width = header.width();

        Ok(read_array_value(
            realm.payload(ref_, header.payload_len()),
            width,
            index,
        ))
    }

    fn is_null(&self, _: usize) -> crate::RealmResult<bool> {
        Ok(false)
    }

    fn size(&self) -> usize {
        self.array.node.header.size as usize
    }
}

impl ArrayLike<i64> for IntegerArray {
    fn get(&self, index: usize) -> crate::RealmResult<i64> {
        let value = self.array.get(index);

        Ok(i64::from_le_bytes(value.to_le_bytes()))
    }

    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        _: (),
    ) -> crate::RealmResult<i64> {
        let header = realm.header(ref_)?;
        let width = header.width();

        let value = read_array_value(realm.payload(ref_, header.payload_len()), width, index);
        Ok(i64::from_le_bytes(value.to_le_bytes()))
    }

    fn is_null(&self, _: usize) -> crate::RealmResult<bool> {
        Ok(false)
    }

    fn size(&self) -> usize {
        self.array.node.header.size as usize
    }
}

impl ArrayLike<Option<i64>> for IntegerArray {
    fn get(&self, index: usize) -> crate::RealmResult<Option<i64>> {
        let value = self.array.get(index + 1);
        let null_value = self.array.get(0);

        Ok(if value == null_value {
            None
        } else {
            Some(i64::from_le_bytes(value.to_le_bytes()))
        })
    }

    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        _: (),
    ) -> crate::RealmResult<Option<i64>> {
        let header = realm.header(ref_)?;
        let width = header.width();

        let value = read_array_value(realm.payload(ref_, header.payload_len()), width, index + 1);
        let null_value = read_array_value(realm.payload(ref_, header.payload_len()), width, 0);

        Ok(if value == null_value {
            None
        } else {
            Some(i64::from_le_bytes(value.to_le_bytes()))
        })
    }

    fn is_null(&self, index: usize) -> crate::RealmResult<bool> {
        let value = self.array.get(index + 1);
        let null_value = self.array.get(0);

        Ok(value == null_value)
    }

    fn size(&self) -> usize {
        self.array.node.header.size as usize
    }
}

impl IntegerArray {
    pub(crate) fn from_array(array: Array) -> Self {
        Self { array }
    }
}

impl IntegerArray {
    pub(crate) fn get_integers(&self) -> Vec<u64> {
        (0..self.array.node.header.size as usize)
            .map(|i| self.array.get(i))
            .collect()
    }
}
