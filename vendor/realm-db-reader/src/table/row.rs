use std::{borrow::Cow, collections::HashMap};

use crate::value::{Backlink, Value};

/// A single row in a Realm table. This allows you to either extract [`Value`]s
/// manually, or use [`realm_model!`](`crate::realm_model`) to convert them into
/// your own structs.
#[derive(Debug, Clone)]
pub struct Row<'a> {
    values: HashMap<Cow<'a, str>, Value>,
    backlinks: Vec<Backlink>,
}

impl<'a> Row<'a> {
    pub(crate) fn new(mut row: Vec<Value>, column_names: Vec<Cow<'a, str>>) -> Self {
        let backlinks = row
            .extract_if(.., |v| matches!(v, Value::BackLink(_)))
            .map(|v| {
                v.try_into()
                    .expect("already matched the right value variant")
            })
            .collect();
        let values = column_names
            .into_iter()
            .enumerate()
            .rev()
            .map(|(index, name)| (name, row.remove(index)))
            .collect();

        Self { values, backlinks }
    }

    /// Returns an iterator over the column names and values in this row.
    pub fn entries(&self) -> impl Iterator<Item = (&Cow<'a, str>, &Value)> {
        self.values.iter()
    }

    /// Returns an iterator over the values in this row.
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.values.values()
    }

    /// Get the value of a column by its name. Returns `None` if the column does
    /// not exist.
    pub fn get(&self, column_name: &str) -> Option<&Value> {
        self.values.get(column_name)
    }

    /// Take the value of a column by its name. Returns `None` if the column
    /// does not exist. This method consumes the value, removing it from the
    /// row. It is used by [`realm_model`](crate::realm_model) to transfer the
    /// backlinks to your custom struct.
    pub fn take(&mut self, column_name: &str) -> Option<Value> {
        self.values.remove(column_name)
    }

    /// Returns an iterator over the [`Backlink`]s in this row.
    pub fn backlinks(&self) -> impl Iterator<Item = &Backlink> {
        self.backlinks.iter()
    }

    /// Take the [`Backlink`]s in this row. This method consumes the backlinks,
    /// removing them from the row. It is used by
    /// [`realm_model`](crate::realm_model) to transfer the backlinks to your
    /// custom struct.
    pub fn take_backlinks(&mut self) -> Vec<Backlink> {
        std::mem::take(&mut self.backlinks)
    }

    /// Check if the row has a field with the given name.
    pub fn has_field(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }
}

impl Row<'_> {
    /// Convert this row into an owned row.
    ///
    /// By default, when you [load a row](`crate::Table::get_row`), the names of
    /// the columns are borrowed from the columns in the originating table. This
    /// can be inconvenient for lifetime reasons, so this method allows you to
    /// sever that connection, by cloning the column names.
    ///
    /// Note that this is only necessary if you want to interact with the row
    /// manually. If you use [`realm_model!`](crate::realm_model), the column
    /// names are no longer used.
    pub fn into_owned(self) -> Row<'static> {
        let values = self
            .values
            .into_iter()
            .map(|(k, v)| (k.into_owned().into(), v))
            .collect();

        Row {
            values,
            backlinks: self.backlinks,
        }
    }
}
