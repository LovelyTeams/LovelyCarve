use tracing::{instrument, warn};

use crate::array::{Array, ArrayStringShort};
use crate::error::TableError;
use crate::table::Table;
use crate::traits::ArrayLike;

/// The group is the central root of a Realm database. It contains all the
/// tables and their names.
///
/// The main way to interact with the Realm database, is by opening the
/// [`Realm`](crate::Realm::open), and calling
/// [`realm.into_group`][`crate::Realm::into_group`]. The resulting [`Group`]
/// can then be used to access tables.
///
/// ```no_run
/// use realm_db_reader::{Realm, Group};
///
/// let realm = Realm::open("example.realm").unwrap();
/// let group = realm.into_group().unwrap();
///
/// let table = group.get_table(0).unwrap();
/// let row = table.get_row(0).unwrap();
/// ```
#[derive(Debug)]
pub struct Group {
    tables_array: Array,
    table_names: Vec<String>,
}

impl Group {
    #[instrument(level = "debug")]
    pub(crate) fn build(array: Array) -> crate::RealmResult<Self> {
        let table_names = {
            let array: ArrayStringShort = array.get_node(0)?.unwrap();
            array.get_all()?
        };

        let tables_array = array.get_node(1)?.unwrap();

        Ok(Self {
            tables_array,
            table_names,
        })
    }
}

impl Group {
    /// Get the [`Table`] with the given number (starting from 0).
    ///
    /// Panics if the table number is out of bounds.
    #[instrument(level = "debug", skip(self), fields(table_names = ?self.table_names))]
    pub fn get_table(&self, table_number: usize) -> crate::TableResult<Table> {
        let table_array = self.tables_array.get_node(table_number)?.unwrap();

        let table = Table::build(table_array, table_number)?;

        Ok(table)
    }

    /// Get the [`Table`] with the given name.
    ///
    /// Panics if the table name is not found.
    #[instrument(level = "debug", skip(self), fields(table_names = ?self.table_names))]
    pub fn get_table_by_name(&self, name: &str) -> crate::TableResult<Table> {
        let table_number = self
            .table_names
            .iter()
            .position(|n| n == name)
            .ok_or_else(|| TableError::TableNotFound {
                name: name.to_string(),
            })?;

        self.get_table(table_number)
    }

    /// Get the number of tables in the group.
    pub fn table_count(&self) -> usize {
        self.table_names.len()
    }

    /// Get the name of the table at the given index.
    ///
    /// Panics if the index is out of bounds.
    pub fn get_table_name(&self, index: usize) -> &str {
        &self.table_names[index]
    }

    /// Get the names of all tables in the group.
    pub fn get_table_names(&self) -> &[String] {
        &self.table_names
    }
}
