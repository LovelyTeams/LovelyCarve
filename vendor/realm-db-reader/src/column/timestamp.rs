use crate::array::{Array, RealmRef};
use crate::column::integer::IntColumnType;
use crate::column::integer_optional::IntNullableColumnType;
use crate::column::{BpTree, Column};
use crate::index::Index;
use crate::realm::Realm;
use crate::table::ColumnAttributes;
use crate::traits::Node;
use crate::value::Value;
use chrono::DateTime;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct TimestampColumn {
    seconds: BpTree<IntNullableColumnType>,
    nanoseconds: BpTree<IntColumnType>,
    index: Option<Index>,
    attributes: ColumnAttributes,
    name: String,
}

impl TimestampColumn {
    pub(crate) fn new(
        realm: Arc<Realm>,
        data_ref: RealmRef,
        index_ref: Option<RealmRef>,
        attributes: ColumnAttributes,
        name: String,
    ) -> crate::RealmResult<Self> {
        let array = Array::from_ref(Arc::clone(&realm), data_ref)?;
        let index = index_ref
            .map(|ref_| Index::from_ref(Arc::clone(&realm), ref_))
            .transpose()?;
        let seconds = array.get_node(0)?.unwrap();
        let nanoseconds = array.get_node(1)?.unwrap();

        Ok(Self {
            seconds,
            nanoseconds,
            index,
            attributes,
            name,
        })
    }
}

impl Column for TimestampColumn {
    fn get(&self, index: usize) -> crate::RealmResult<Value> {
        // Get seconds value
        let seconds = match self.seconds.get(index)? {
            Some(seconds) => seconds,
            None => return Ok(Value::None),
        };

        // If seconds is 0, the timestamp is null
        if seconds == 0 {
            return Ok(Value::Timestamp(Default::default()));
        }

        // Get nanoseconds value
        let nanoseconds = self.nanoseconds.get(index)?;

        // Convert to DateTime
        let seconds = i64::from_le_bytes(seconds.to_le_bytes());
        Ok(DateTime::from_timestamp(seconds, nanoseconds as u32).into())
    }

    fn is_null(&self, index: usize) -> crate::RealmResult<bool> {
        self.seconds.is_null(index)
    }

    fn count(&self) -> crate::RealmResult<usize> {
        self.seconds.count()
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
        Some(&self.name)
    }
}

// Factory function for timestamp columns
pub(crate) fn create_timestamp_column(
    realm: Arc<Realm>,
    data_ref: RealmRef,
    index_ref: Option<RealmRef>,
    attributes: ColumnAttributes,
    name: String,
) -> crate::RealmResult<Box<dyn Column>> {
    Ok(Box::new(TimestampColumn::new(
        realm, data_ref, index_ref, attributes, name,
    )?))
}
