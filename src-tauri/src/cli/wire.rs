//! CLI wire serialization: camelCase struct fields, verbatim map keys and values.
//! Evidence-owned column names, plugin fields and arbitrary JSON are maps;
//! their spelling must never be rewritten as part of the transport contract.
use serde::{ser::*, Serialize, Serializer};

pub(super) struct Camel<T>(pub T);

impl<T: Serialize> Serialize for Camel<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(Wire(serializer))
    }
}

fn camel(name: &str) -> String {
    let mut output = String::with_capacity(name.len());
    let mut upper = false;
    for character in name.chars() {
        if character == '_' {
            upper = true;
        } else if upper {
            output.extend(character.to_uppercase());
            upper = false;
        } else {
            output.push(character);
        }
    }
    output
}

struct Wire<S>(S);

/// These command responses consist entirely of backend-owned fields but use
/// serde(flatten), which erases the struct/map distinction. Convert only their
/// outer records; nested evidence maps still pass through unchanged.
pub(super) fn flattened_output<T: Serialize>(value: &T) -> Result<super::Output, super::CliError> {
    fn record(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(fields) => serde_json::Value::Object(
                fields
                    .into_iter()
                    .map(|(key, value)| (camel(&key), value))
                    .collect(),
            ),
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.into_iter().map(record).collect())
            }
            value => value,
        }
    }
    Ok(super::Output::json(record(serde_json::to_value(Camel(
        value,
    ))?)))
}
struct Nested<S>(S);
struct Fields<S>(S);
struct Variant<S> {
    map: S,
    variant: &'static str,
    fields: serde_json::Map<String, serde_json::Value>,
}

macro_rules! scalar {
    ($($method:ident($kind:ty)),* $(,)?) => {$(
        fn $method(self, value: $kind) -> Result<Self::Ok, Self::Error> {
            self.0.$method(value)
        }
    )*};
}

impl<S: Serializer> Serializer for Wire<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = Nested<S::SerializeSeq>;
    type SerializeTuple = Nested<S::SerializeTuple>;
    type SerializeTupleStruct = Nested<S::SerializeTupleStruct>;
    type SerializeTupleVariant = Nested<S::SerializeTupleVariant>;
    type SerializeMap = Nested<S::SerializeMap>;
    type SerializeStruct = Fields<S::SerializeMap>;
    type SerializeStructVariant = Variant<S::SerializeMap>;

    scalar! {
        serialize_bool(bool), serialize_i8(i8), serialize_i16(i16), serialize_i32(i32),
        serialize_i64(i64), serialize_i128(i128), serialize_u8(u8), serialize_u16(u16),
        serialize_u32(u32), serialize_u64(u64), serialize_u128(u128), serialize_f32(f32),
        serialize_f64(f64), serialize_char(char), serialize_str(&str), serialize_bytes(&[u8]),
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_none()
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit()
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_some(&Camel(value))
    }
    fn serialize_unit_struct(self, name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit_struct(name)
    }
    fn serialize_unit_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit_variant(name, index, variant)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_newtype_struct(name, &Camel(value))
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.0
            .serialize_newtype_variant(name, index, variant, &Camel(value))
    }
    fn serialize_seq(self, length: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.0.serialize_seq(length).map(Nested)
    }
    fn serialize_tuple(self, length: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.0.serialize_tuple(length).map(Nested)
    }
    fn serialize_tuple_struct(
        self,
        name: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.0.serialize_tuple_struct(name, length).map(Nested)
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.0
            .serialize_tuple_variant(name, index, variant, length)
            .map(Nested)
    }
    fn serialize_map(self, length: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.0.serialize_map(length).map(Nested)
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        length: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.0.serialize_map(Some(length)).map(Fields)
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _length: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Ok(Variant {
            map: self.0.serialize_map(Some(1))?,
            variant,
            fields: serde_json::Map::new(),
        })
    }
    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

impl<S: SerializeSeq> SerializeSeq for Nested<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.0.serialize_element(&Camel(value))
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

impl<S: SerializeTuple> SerializeTuple for Nested<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.0.serialize_element(&Camel(value))
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

impl<S: SerializeTupleStruct> SerializeTupleStruct for Nested<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.0.serialize_field(&Camel(value))
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

impl<S: SerializeTupleVariant> SerializeTupleVariant for Nested<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.0.serialize_field(&Camel(value))
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

impl<S: SerializeMap> SerializeMap for Nested<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Self::Error> {
        self.0.serialize_key(key)
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.0.serialize_value(&Camel(value))
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

impl<S: SerializeMap> SerializeStruct for Fields<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.0.serialize_entry(&camel(key), &Camel(value))
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

impl<S: SerializeMap> SerializeStructVariant for Variant<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.fields.insert(
            camel(key),
            serde_json::to_value(Camel(value)).map_err(S::Error::custom)?,
        );
        Ok(())
    }
    fn end(mut self) -> Result<Self::Ok, Self::Error> {
        self.map.serialize_entry(self.variant, &self.fields)?;
        self.map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn flattened_transport_records_keep_nested_evidence_keys() {
        #[derive(Serialize)]
        struct Faults {
            synthetic_bytes: u64,
            bad_regions: Vec<Region>,
        }
        #[derive(Serialize)]
        struct Region {
            start_offset: u64,
        }
        #[derive(Serialize)]
        struct Export {
            bytes_written: u64,
            #[serde(flatten)]
            faults: Faults,
            metadata: BTreeMap<String, String>,
        }
        let output = flattened_output(&vec![Export {
            bytes_written: 5,
            faults: Faults {
                synthetic_bytes: 1,
                bad_regions: vec![Region { start_offset: 4 }],
            },
            metadata: BTreeMap::from([("original_key".into(), "original_value".into())]),
        }])
        .unwrap();
        let super::super::Output::Json(output) = output else {
            panic!("JSON response");
        };
        assert_eq!(
            output.value(),
            &json!([{"bytesWritten": 5, "syntheticBytes": 1,
            "badRegions": [{"startOffset": 4}], "metadata": {"original_key": "original_value"}}])
        );
    }

    #[test]
    fn struct_fields_are_camel_case_but_evidence_maps_and_enum_values_are_exact() {
        #[derive(Serialize)]
        struct Node {
            image_id: String,
            next_cursor: Option<String>,
            evidence_columns: BTreeMap<String, String>,
        }
        #[derive(Serialize)]
        enum Record {
            SourceData { node_id: String, values: Vec<Node> },
        }
        let record = Record::SourceData {
            node_id: "source_id".into(),
            values: vec![Node {
                image_id: "image_id".into(),
                next_cursor: None,
                evidence_columns: BTreeMap::from([("user_name".into(), "raw_value".into())]),
            }],
        };
        assert_eq!(
            serde_json::to_value(Camel(record)).unwrap(),
            json!({"SourceData": {
                "nodeId": "source_id", "values": [{"imageId": "image_id", "nextCursor": null,
                    "evidenceColumns": {"user_name": "raw_value"}}]
            }})
        );
        assert_eq!(
            serde_json::to_value(Camel(json!({"raw_key": {"nested_key": 1}}))).unwrap(),
            json!({"raw_key": {"nested_key": 1}})
        );
    }
}
