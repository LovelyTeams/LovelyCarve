# Realm Database Reader

This is a Rust library that provides basic functionality for reading data from a
Realm database. For usage and examples, please check the
[documentation](https://docs.rs/realm-db-reader/).

## Supported

This library supports reading `.realm` files of version `9.9`, which are not
encrypted. Support for other versions _may_ be added in the future.

In specific, you can:

- Open a Realm file
- List and open tables in the database
- Read rows in those tables
- Find rows by a known value for indexed columns
- Easily convert rows to a native Rust struct

## Shortcomings

Other than the limitations mentioned above regarding the Realm version, and not
handling encryption, this library does not support:

- Creating or writing Realm databases
- Tables with columns of the following types:
  - Enums
  - Binary
  - Mixed
  - Old datetime (the new datetime column type is supported)
  - Decimal
- Finding _multiple_ rows by a known value for indexed columns

## License

This project is licensed under the MIT License.

Portions of this project are derived from [work covered by the Apache License
2.0](https://github.com/realm/realm-core).

For those portions:
- The original copyright and license notices have been retained.
- Any modifications from the original have been documented in the source code or
  accompanying files.
- A copy of the Apache License 2.0 is provided in APACHE_LICENSE.
- The inclusion of Apache 2.0–licensed code does not alter the terms of the MIT
  License for the rest of the project.
