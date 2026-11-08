//! A read-only implementation of reading
//! [Realm](https://github.com/realm/realm-swift?tab=readme-ov-file#about-realm-database)
//! database files in Rust.
//!
//! Note that not all features, and in particular all column types, are
//! supported. But for the cases where you only need to _read_ data from a Realm
//! file in Rust, within the current limitations of this library, it may be a
//! more ergonomic alternative than interacting with the [C++
//! SDK](https://github.com/realm/realm-cpp).
//!
//! # Loading a Realm file
//!
//! ```no_run
//! use realm_db_reader::{Group, Realm};
//!
//! let realm = Realm::open("my-database.realm").unwrap();
//! let group = realm.into_group().unwrap();
//! ```
//!
//! At this point, you have a [`Group`] instance, which is the root of the Realm
//! database. You can use it to access the tables and columns within the loaded file.
//!
//! # Reading data from tables
//!
//! ```no_run
//! # use realm_db_reader::{Group, Realm};
//! # let realm = Realm::open("my-database.realm").unwrap();
//! # let group = realm.into_group().unwrap();
//! # let my_objects_table_index = 0;
//! let table = group.get_table_by_name("col_MyObjects").unwrap();
//! // or:
//! let table = group.get_table(my_objects_table_index).unwrap();
//!
//! // Tables allow you to load all rows, a given row based on its index, or
//! // look up a row by an indexed column.
//! // Here, we'll check the total number of rows in the table, and load the
//! // middle one.
//! let row_count = table.row_count().unwrap();
//! let middle_row = table.get_row(row_count / 2).unwrap();
//!
//! let row = table.get_row(0).unwrap();
//! dbg!(row);
//! ```
//!
//! As mentioned, if the table you're interacting with has an indexed column,
//! you can find a row by a known value:
//!
//! ```no_run
//! # use realm_db_reader::{Group, Realm};
//! # let realm = Realm::open("my-database.realm").unwrap();
//! # let group = realm.into_group().unwrap();
//! # let some_id = "";
//! let table = group.get_table_by_name("col_MyObjects").unwrap();
//!
//! let row = table.find_row_from_indexed_column("id", &some_id.into()).unwrap();
//! dbg!(row);
//! ```
//!
//! Here, `row`, will be the _first row_ matching the given value. If no rows
//! match, [`Option::None`] will be returned.
//!
//! # Mapping rows to structs
//!
//! [`Row`] values are relatively low-level, so you may want to map them to your
//! struct for convenience. You can use the [`realm_model`] macro for this.
//!
//! ```no_run
//! # use realm_db_reader::{realm_model, Group, Realm};
//! # let realm = Realm::open("my-database.realm").unwrap();
//! # let group = realm.into_group().unwrap();
//! # let table = group.get_table(0).unwrap();
//! # let row = table.get_row(0).unwrap();
//!
//! struct MyObject {
//!   id: String,
//!   name: String,
//! }
//!
//! realm_model!(MyObject => id, name);
//!
//! let my_object: MyObject = row.try_into().unwrap();
//! ```
//!
//! Check [the macro documentation](realm_model) for more details.

mod array;
mod column;
mod error;
mod group;
mod index;
mod model;
mod realm;
mod spec;
mod table;
mod traits;
mod utils;
mod value;

// Export public types.
pub use column::Column;
pub use error::{RealmFileError, RealmResult, TableError, TableResult, ValueError, ValueResult};
pub use group::Group;
pub use realm::Realm;
pub use table::{Row, Table};
pub use value::{Backlink, Link, Value};
