#[doc(hidden)]
#[macro_export]
macro_rules! realm_model_field {
    ($struct:ident, $row:ident, $field:ident = $alias:expr) => {
        $row.take($alias)
            .ok_or_else(|| $crate::ValueError::MissingField {
                field: $alias,
                target_type: stringify!($struct),
                remaining_fields: $row.clone().into_owned(),
            })?
            .try_into()?
    };
    ($struct:ident, $row:ident, $field:ident) => {
        $crate::realm_model_field!($struct, $row, $field = stringify!($field))
    };
}

/// Macro to implement conversion from a Row to a Realm model struct. This allows for easy creation
/// of your own struct instances, based on data retrieved from a Realm database.
///
/// ```rust
/// use realm_db_reader::realm_model;
///
/// struct MyStruct {
///     field1: String,
///     field2: i64,
/// }
///
/// realm_model!(MyStruct => field1, field2);
/// ```
///
/// You may only use types that either are valid Realm values, or can themselves
/// be converted from Realm values. The builtin types are:
///
/// - `String` and `Option<String>`
/// - `i64` and `Option<i64>`
/// - `bool` and `Option<bool>`
/// - `f32`
/// - `f64`
/// - `chrono::DateTime<Utc>` and `Option<chrono::DateTime<Utc>>`
/// - [`Link`](crate::Link), `Option<Link>`, and `Vec<Link>`
///
/// All struct fields must be present, but you may omit columns that you don't
/// need. The types of the fields in your struct should, of course, match the
/// types of the Realm table columns.
///
/// # Renaming fields
///
/// If you want to name a field differently, you can use the `=` syntax to
/// specify an alias:
///
/// ```rust
/// use realm_db_reader::realm_model;
///
/// struct MyStruct {
///     my_struct_field: String,
///     my_other_struct_field: i64,
/// }
///
/// realm_model!(MyStruct => my_struct_field, my_other_struct_field = "realmColumnName");
/// ```
///
/// # Backlinks
///
/// Some tables in Realm can be linked to each other using backlinks. To define
/// a backlink, you can use the `;` syntax to specify the name of your backlink
/// field:
///
/// ```rust
/// use realm_db_reader::{realm_model, Backlink};
///
/// struct MyStruct {
///     field1: String,
///     field2: i64,
///     backlink_field: Vec<Backlink>,
/// }
///
/// realm_model!(MyStruct => field1, field2; backlink_field);
/// ```
///
/// This will create a backlink field in the struct that can be used to retrieve
/// all rows that link to the current row. Backlink fields are unnamed in Realm,
/// which is why they don't follow the same conventions as other fields.
///
/// # Subtables
///
/// In the case where the Realm table contains a subtable, you can refer to this
/// data too:
///
/// ```rust
/// use realm_db_reader::realm_model;
///
/// struct MyStruct {
///     id: String,
///     // A subtable that contains a list of strings.
///     strings: Vec<String>,
///     // A subtable that contains complete data.
///     items: Vec<Item>,
/// }
///
/// realm_model!(MyStruct => id, strings, items);
///
/// struct Item {
///     subtable_row_id: String,
///     subtable_row_content: String,
/// }
///
/// // The aliases are not required here, it's just to illustrate they're
/// // available in subtables too.
/// realm_model!(Item => subtable_row_id = "id", subtable_row_content = "content");
/// ```
#[macro_export]
macro_rules! realm_model {
    ($struct:ident => $($field:ident$(= $alias:expr)?),*$(; $backlinks:ident)?) => {
        impl<'a> ::core::convert::TryFrom<$crate::Row<'a>> for $struct {
            type Error = $crate::ValueError;

            fn try_from(mut row: $crate::Row<'a>) -> $crate::ValueResult<Self> {
                $(
                let $field = $crate::realm_model_field!($struct, row, $field$(= $alias)?);
                )*
                $(
                let $backlinks = row.take_backlinks();
                )?

                Ok(Self {
                    $(
                        $field,
                    )*
                    $(
                        $backlinks,
                    )?
                })
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use crate::value::ARRAY_VALUE_KEY;
    use crate::{Backlink, Link, Row, Value};
    use itertools::*;

    #[test]
    fn test_realm_model() {
        struct MyModel {
            id: String,
            foo: Option<String>,
            bar: Option<chrono::DateTime<chrono::Utc>>,
            baz: i64,
            qux: Option<i64>,
            other: bool,
            items: Vec<String>,
            sub_items: Vec<SubModel>,
        }

        #[derive(Debug, PartialEq)]
        struct SubModel {
            left: i64,
            right: i64,
        }

        realm_model!(MyModel => id, foo, bar, baz, qux, other = "!invalid_rust_alias", items, sub_items = "children");
        realm_model!(SubModel => left, right);

        let foo_values = [Some("hello".to_string()), None];
        let bar_values = [Some(chrono::Utc::now()), None];
        let qux_values = [Some(42), None];

        for (foo_value, bar_value, qux_value) in iproduct!(foo_values, bar_values, qux_values) {
            let values: Vec<Value> = vec![
                "id_value".into(),
                foo_value.clone().into(),
                bar_value.into(),
                "extra_field".into(),
                100.into(),
                qux_value.into(),
                true.into(),
                "extra_field".into(),
                vec![
                    Row::new(vec!["member1".into()], vec![ARRAY_VALUE_KEY.into()]),
                    Row::new(vec!["member2".into()], vec![ARRAY_VALUE_KEY.into()]),
                ]
                .into(),
                vec![
                    Row::new(
                        vec![1.into(), 2.into()],
                        vec!["left".into(), "right".into()],
                    ),
                    Row::new(
                        vec![3.into(), 4.into()],
                        vec!["left".into(), "right".into()],
                    ),
                ]
                .into(),
            ];
            let row = Row::new(
                values,
                vec![
                    "id".into(),
                    "foo".into(),
                    "bar".into(),
                    "some_other_field".into(),
                    "baz".into(),
                    "qux".into(),
                    "!invalid_rust_alias".into(),
                    "another_field".into(),
                    "items".into(),
                    "children".into(),
                ],
            );

            let my_model: MyModel = row.try_into().unwrap();
            assert_eq!(my_model.id, "id_value");
            assert_eq!(my_model.foo, foo_value);
            assert_eq!(my_model.bar, bar_value);
            assert_eq!(my_model.baz, 100);
            assert_eq!(my_model.qux, qux_value);
            assert!(my_model.other);
            assert_eq!(
                my_model.items,
                vec!["member1".to_string(), "member2".to_string()]
            );
            assert_eq!(
                my_model.sub_items,
                vec![
                    SubModel { left: 1, right: 2 },
                    SubModel { left: 3, right: 4 }
                ]
            );
        }
    }

    #[test]
    fn test_model_with_links() {
        struct MyModel {
            id: String,
            link_a: Link,
            // FIXME: This is not supported yet
            // link_b: Vec<Link>,
            optional_link: Option<Link>,
            backlinks: Vec<Backlink>,
        }

        realm_model!(MyModel => id, link_a, optional_link; backlinks);

        let values = vec![
            "123456789".into(),
            "irrelevant_field".into(),
            Link::new(12, 5).into(),
            vec![Link::new(13, 6)].into(),
            Value::None,
            Backlink::new(12, 5, vec![1989]).into(),
        ];
        let row = Row::new(
            values,
            vec![
                "id".into(),
                "other_field".into(),
                "link_a".into(),
                "link_b".into(),
                "optional_link".into(),
                // NOTE: backlinks are unnamed
            ],
        );

        let model: MyModel = row.try_into().unwrap();
        assert_eq!(model.id, "123456789");
        assert_eq!(model.backlinks, vec![Backlink::new(12, 5, vec![1989])]);
        assert_eq!(model.link_a, Link::new(12, 5));
        assert_eq!(model.optional_link, None);
    }
}
