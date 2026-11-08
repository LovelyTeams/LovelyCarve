use std::fmt::Debug;
use std::sync::Arc;

use tracing::{debug, info, instrument, warn};

use crate::RealmFileError;
use crate::array::{Array, ArrayStringShort, FromU64, IntegerArray, RefOrTaggedValue};
use crate::column::{
    Column, create_backlink_column, create_bool_column, create_bool_null_column,
    create_double_column, create_float_column, create_int_column, create_int_null_column,
    create_link_column, create_linklist_column, create_string_column, create_subtable_column,
    create_timestamp_column,
};
use crate::spec::ColumnType;
use crate::table::column::ColumnAttributes;
use crate::traits::ArrayLike;

/// The header of a table.
///
/// The header contains information about the columns in a table, such as their names, types, and attributes.
#[derive(Debug)]
pub(crate) struct TableHeader {
    columns: Vec<Box<dyn Column>>,
}

impl TableHeader {
    #[instrument(level = "debug")]
    fn from_parts(
        data_array: &Array,
        column_types: Vec<ColumnType>,
        mut column_names: Vec<String>,
        column_attributes: Vec<ColumnAttributes>,
        sub_spec_array: Option<Array>,
    ) -> crate::RealmResult<Self> {
        // NOTE: The same does not apply for column names, as backlinks don't have a name.
        assert_eq!(
            column_types.len(),
            column_attributes.len(),
            "number of column types and column attributes should match"
        );

        let mut columns = Vec::with_capacity(column_types.len());
        let mut data_array_index = 0;
        let mut sub_spec_index = 0;

        // Reverse the column names so we can do a low-cost pop for each column that has a name.
        column_names.reverse();

        for (i, column_type) in column_types.into_iter().enumerate() {
            let attributes = column_attributes[i];
            let data_ref = data_array.get_ref(data_array_index).ok_or_else(|| {
                RealmFileError::InvalidRealmFile {
                    reason: format!("failed to find data entry for column {i}"),
                }
            })?;

            debug!(
                "column type {i}: {column_type:?} has data array index {data_array_index} with ref {data_ref:?}"
            );

            let index_ref = if attributes.is_indexed() {
                Some(data_array.get_ref(data_array_index + 1).ok_or_else(|| {
                    RealmFileError::InvalidRealmFile {
                        reason: format!("failed to find index entry for column {i}"),
                    }
                })?)
            } else {
                None
            };

            let column = match column_type {
                ColumnType::Int => {
                    if attributes.is_nullable() {
                        create_int_null_column(
                            Arc::clone(&data_array.node.realm),
                            data_ref,
                            index_ref,
                            attributes,
                            column_names.pop().unwrap(),
                        )?
                    } else {
                        create_int_column(
                            Arc::clone(&data_array.node.realm),
                            data_ref,
                            index_ref,
                            attributes,
                            column_names.pop().unwrap(),
                        )?
                    }
                }
                ColumnType::Bool => {
                    if attributes.is_nullable() {
                        create_bool_null_column(
                            Arc::clone(&data_array.node.realm),
                            data_ref,
                            index_ref,
                            attributes,
                            column_names.pop().unwrap(),
                        )?
                    } else {
                        create_bool_column(
                            Arc::clone(&data_array.node.realm),
                            data_ref,
                            index_ref,
                            attributes,
                            column_names.pop().unwrap(),
                        )?
                    }
                }
                ColumnType::String => create_string_column(
                    Arc::clone(&data_array.node.realm),
                    data_ref,
                    index_ref,
                    attributes,
                    column_names.pop().unwrap(),
                )?,
                ColumnType::OldStringEnum
                | ColumnType::Binary
                | ColumnType::OldMixed
                | ColumnType::OldDateTime
                | ColumnType::Reserved4 => {
                    return Err(RealmFileError::Unsupported {
                        reason: format!("Column {i} has unsupported Realm type {column_type:?}"),
                    });
                }
                ColumnType::Table => {
                    let other_table_header_ref = sub_spec_array
                        .as_ref()
                        .ok_or_else(|| RealmFileError::InvalidRealmFile {
                            reason: "Expected sub-spec array for table column".to_string(),
                        })?
                        .get_ref(sub_spec_index)
                        .unwrap();
                    let name = column_names.pop().unwrap();

                    create_subtable_column(
                        Arc::clone(&data_array.node.realm),
                        other_table_header_ref,
                        data_ref,
                        attributes,
                        name,
                    )?
                }
                ColumnType::Timestamp => create_timestamp_column(
                    Arc::clone(&data_array.node.realm),
                    data_ref,
                    index_ref,
                    attributes,
                    column_names.pop().unwrap(),
                )?,
                ColumnType::Float => create_float_column(
                    Arc::clone(&data_array.node.realm),
                    data_ref,
                    attributes,
                    column_names.pop().unwrap(),
                )?,
                ColumnType::Double => create_double_column(
                    Arc::clone(&data_array.node.realm),
                    data_ref,
                    attributes,
                    column_names.pop().unwrap(),
                )?,
                ColumnType::Link => {
                    let target_table_index = Self::get_sub_spec_index_value(
                        sub_spec_array.as_ref().ok_or_else(|| {
                            RealmFileError::InvalidRealmFile {
                                reason: "Expected sub-spec array for link column".to_string(),
                            }
                        })?,
                        sub_spec_index,
                    )?;

                    create_link_column(
                        Arc::clone(&data_array.node.realm),
                        data_ref,
                        attributes,
                        target_table_index,
                        column_names.pop().unwrap(),
                    )?
                }
                ColumnType::LinkList => {
                    let target_table_index = Self::get_sub_spec_index_value(
                        sub_spec_array
                            .as_ref()
                            .ok_or(RealmFileError::InvalidRealmFile {
                                reason: "Expected sub-spec array for link column".to_string(),
                            })?,
                        sub_spec_index,
                    )?;

                    create_linklist_column(
                        Arc::clone(&data_array.node.realm),
                        data_ref,
                        attributes,
                        target_table_index,
                        column_names.pop().unwrap(),
                    )?
                }
                ColumnType::BackLink => {
                    let sub_spec_array =
                        sub_spec_array
                            .as_ref()
                            .ok_or(RealmFileError::InvalidRealmFile {
                                reason: "Expected sub-spec array for backlink column".to_string(),
                            })?;
                    let target_table_index =
                        Self::get_sub_spec_index_value(sub_spec_array, sub_spec_index)?;
                    let target_table_column_index =
                        Self::get_sub_spec_index_value(sub_spec_array, sub_spec_index + 1)?;
                    create_backlink_column(
                        Arc::clone(&data_array.node.realm),
                        data_ref,
                        attributes,
                        target_table_index,
                        target_table_column_index,
                    )?
                }
            };

            tracing::info!("Created column {column:?}");

            columns.push(column);

            data_array_index += 1;
            if attributes.is_indexed() {
                // Indexed columns have an additional data array, so we need to increment the data
                // index. In other words, for column with data index N, with attribute is_indexed,
                // there's an index entry at N+1 in the data array.
                data_array_index += 1;
            }

            if column_type.has_sub_spec() {
                sub_spec_index += column_type.sub_spec_entries_count();
            }
        }

        Ok(Self { columns })
    }

    fn get_sub_spec_index_value(
        sub_spec_array: &Array,
        sub_spec_index: usize,
    ) -> crate::RealmResult<usize> {
        match sub_spec_array.get_ref_or_tagged_value(sub_spec_index) {
            Some(RefOrTaggedValue::Ref(_)) => Err(RealmFileError::InvalidRealmFile {
                reason: "Expected tagged integer for link column".to_string(),
            }),
            Some(RefOrTaggedValue::TaggedValue(target_table_index)) => {
                Ok(target_table_index as usize)
            }
            _ => Err(RealmFileError::InvalidRealmFile {
                reason: "Expected tagged integer for link column".to_string(),
            }),
        }
    }

    pub(crate) fn column_count(&self) -> usize {
        self.columns.len()
    }

    pub(crate) fn get_columns(&self) -> &[Box<dyn Column>] {
        &self.columns
    }

    pub(crate) fn get_column(&self, index: usize) -> Option<&dyn Column> {
        self.columns.get(index).map(|c| c.as_ref())
    }
}

impl TableHeader {
    #[instrument(level = "debug")]
    pub(crate) fn build(header_array: &Array, data_array: &Array) -> crate::RealmResult<Self> {
        let column_types = {
            let array: IntegerArray = header_array.get_node(0)?.unwrap();
            array
                .get_integers()
                .into_iter()
                .map(ColumnType::from_u64)
                .collect::<Vec<_>>()
        };

        info!("column_types: {:?}", column_types);

        let column_names = {
            let array: ArrayStringShort = header_array.get_node(1)?.unwrap();
            array.get_all()?
        };

        info!("column_names: {:?}", column_names);

        let column_attributes = {
            let array: IntegerArray = header_array.get_node(2)?.unwrap();
            array
                .get_integers()
                .into_iter()
                .map(ColumnAttributes::from_u64)
                .collect::<Vec<_>>()
        };

        info!("column_attributes: {:?}", column_attributes);

        let sub_spec_array = if header_array.node.header.size > 3 {
            header_array.get_node(3)?
        } else {
            None
        };

        Self::from_parts(
            data_array,
            column_types,
            column_names,
            column_attributes,
            sub_spec_array,
        )
    }
}
