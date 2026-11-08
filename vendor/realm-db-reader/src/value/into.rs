use std::any::type_name;
use std::error::Error;

use chrono::{DateTime, Utc};

use crate::error::ValueError;
use crate::table::Row;
use crate::value::{ARRAY_VALUE_KEY, Backlink, Link, Value};

macro_rules! value_try_into {
    (Option<$target:ty>, $source:ident) => {
        impl TryFrom<Value> for Option<$target> {
            type Error = ValueError;

            fn try_from(value: Value) -> Result<Self, Self::Error> {
                match value {
                    Value::$source(val) => Ok(Some(val)),
                    Value::None => Ok(None),
                    value => Err(ValueError::UnexpectedType {
                        expected: stringify!($target),
                        found: value,
                    }),
                }
            }
        }
    };

    ($target:ty, $source:ident) => {
        impl TryFrom<Value> for $target {
            type Error = ValueError;

            fn try_from(value: Value) -> Result<Self, Self::Error> {
                match value {
                    Value::$source(val) => Ok(val),
                    value => Err(ValueError::UnexpectedType {
                        expected: stringify!($target),
                        found: value,
                    }),
                }
            }
        }

        impl<'a> TryFrom<Row<'a>> for $target {
            type Error = ValueError;

            fn try_from(mut value: Row<'a>) -> Result<Self, Self::Error> {
                let Some(value) = value.take(ARRAY_VALUE_KEY) else {
                    return Err(ValueError::ExpectedArrayRow {
                        field: ARRAY_VALUE_KEY,
                        found: value.into_owned(),
                    });
                };

                value.try_into()
            }
        }
    };
}

value_try_into!(String, String);
value_try_into!(Option<String>, String);
value_try_into!(i64, Int);
value_try_into!(Option<i64>, Int);
value_try_into!(bool, Bool);
value_try_into!(f32, Float);
value_try_into!(f64, Double);
value_try_into!(DateTime<Utc>, Timestamp);
value_try_into!(Option<DateTime<Utc>>, Timestamp);
value_try_into!(Backlink, BackLink);
value_try_into!(Link, Link);
value_try_into!(Option<Link>, Link);

impl<'a, T> TryFrom<Value> for Vec<T>
where
    T: TryFrom<Row<'a>>,
    T::Error: Error + 'static,
{
    type Error = ValueError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Table(rows) => {
                let mut result = Vec::with_capacity(rows.len());
                for row in rows {
                    result.push(row.try_into().map_err(|e| ValueError::VecConversionError {
                        element_type: type_name::<T>(),
                        source: Box::new(e),
                    })?);
                }

                Ok(result)
            }
            value => Err(ValueError::ExpectedTable { found: value }),
        }
    }
}
