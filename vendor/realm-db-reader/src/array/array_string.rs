use std::fmt::Debug;
use std::sync::Arc;

use tracing::instrument;

use crate::array::long_blobs_array::LongBlobsArray;
use crate::array::small_blobs_array::SmallBlobsArray;
use crate::array::{ArrayStringShort, RealmRef};
use crate::realm::{NodeHeader, Realm};
use crate::traits::{ArrayLike, Node, NodeWithContext};

pub(crate) struct ArrayString<T> {
    size: usize,
    inner: Box<dyn ArrayLike<T>>,
}

impl<T> Debug for ArrayString<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArrayString")
            .field("size", &self.size)
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T> NodeWithContext<()> for ArrayString<T>
where
    ArrayStringShort: ArrayLike<T>,
    SmallBlobsArray: ArrayLike<T>,
    LongBlobsArray: ArrayLike<T>,
{
    fn from_ref_with_context(realm: Arc<Realm>, ref_: RealmRef, _: ()) -> crate::RealmResult<Self>
    where
        Self: Sized,
    {
        let header = realm.header(ref_)?;
        let inner = Self::get_inner(&header, realm, ref_)?;

        Ok(Self {
            size: header.size as usize,
            inner,
        })
    }
}

impl ArrayLike<String> for ArrayString<String> {
    #[instrument(level = "debug")]
    fn get(&self, index: usize) -> crate::RealmResult<String> {
        self.inner.get(index)
    }

    #[instrument(level = "debug")]
    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        context: (),
    ) -> crate::RealmResult<String> {
        let header = realm.header(ref_)?;

        match (header.has_refs(), header.context_flag()) {
            (false, _) => ArrayStringShort::get_direct(realm, ref_, index, context),
            (true, false) => SmallBlobsArray::get_direct(realm, ref_, index, context),
            (true, true) => LongBlobsArray::get_direct(realm, ref_, index, context),
        }
    }

    fn is_null(&self, index: usize) -> crate::RealmResult<bool> {
        self.inner.is_null(index)
    }

    fn size(&self) -> usize {
        self.inner.size()
    }
}

impl ArrayLike<Option<String>> for ArrayString<Option<String>> {
    #[instrument(level = "debug")]
    fn get(&self, index: usize) -> crate::RealmResult<Option<String>> {
        self.inner.get(index)
    }

    #[instrument(level = "debug")]
    fn get_direct(
        realm: Arc<Realm>,
        ref_: RealmRef,
        index: usize,
        context: (),
    ) -> crate::RealmResult<Option<String>> {
        let header = realm.header(ref_)?;

        match (header.has_refs(), header.context_flag()) {
            (false, _) => ArrayStringShort::get_direct(realm, ref_, index, context),
            (true, false) => SmallBlobsArray::get_direct(realm, ref_, index, context),
            (true, true) => LongBlobsArray::get_direct(realm, ref_, index, context),
        }
    }

    fn is_null(&self, index: usize) -> crate::RealmResult<bool> {
        self.inner.is_null(index)
    }

    fn size(&self) -> usize {
        self.inner.size()
    }
}

impl<T> ArrayString<T>
where
    ArrayStringShort: ArrayLike<T>,
    SmallBlobsArray: ArrayLike<T>,
    LongBlobsArray: ArrayLike<T>,
{
    #[instrument(level = "debug")]
    pub(crate) fn get_inner(
        header: &NodeHeader,
        realm: Arc<Realm>,
        ref_: RealmRef,
    ) -> crate::RealmResult<Box<dyn ArrayLike<T>>> {
        Ok(match (header.has_refs(), header.context_flag()) {
            (false, _) => Box::new(ArrayStringShort::from_ref(realm, ref_)?),
            (true, false) => Box::new(SmallBlobsArray::from_ref(realm, ref_)?),
            (true, true) => Box::new(LongBlobsArray::from_ref(realm, ref_)?),
        })
    }
}
