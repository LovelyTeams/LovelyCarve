//! Expanded request objects for the common metadata and directory queries.
use serde_json::{json, Value};

/// The typed ids of `core::common::ids`, which travel as plain strings.
fn id_schema(name: &str) -> Option<Value> {
    let (optional, inner) = match name
        .strip_prefix("Option<")
        .and_then(|inner| inner.strip_suffix('>'))
    {
        Some(inner) => (true, inner),
        None => (false, name),
    };
    let (array, inner) = match inner
        .strip_prefix("Vec<")
        .and_then(|inner| inner.strip_suffix('>'))
    {
        Some(inner) => (true, inner),
        None => (false, inner),
    };
    let description = match inner {
        "ImageId" => "检材 ID",
        "NodeId" => "文件树节点 ID",
        "VolumeKey" => "卷标识，如 part:2",
        _ => return None,
    };
    let kind = match (array, optional) {
        (false, false) => json!("string"),
        (false, true) => json!(["string", "null"]),
        (true, false) => json!("array"),
        (true, true) => json!(["array", "null"]),
    };
    Some(if array {
        json!({"type": kind, "items": {"type": "string", "description": description}})
    } else {
        json!({"type": kind, "description": description})
    })
}

pub(super) fn for_type(name: Option<&str>) -> Option<Value> {
    let name = name?;
    if let Some(schema) = id_schema(name) {
        return Some(schema);
    }
    if name == "FileFilter" || name.ends_with("::FileFilter") {
        let mut properties = serde_json::Map::new();
        for key in [
            "namePrefix",
            "nameContains",
            "nameGlob",
            "pathPrefix",
            "pathContains",
        ] {
            properties.insert(key.into(), json!({"type":["string","null"]}));
        }
        properties.insert("nameGlob".into(), json!({"type":["string","null"],"description":"完整文件名通配符，忽略大小写；支持 *、?、[abc]、[^abc]"}));
        for key in ["extensions", "kinds"] {
            properties.insert(
                key.into(),
                json!({"type":"array","items":{"type":"string"}}),
            );
        }
        for key in ["minSize", "maxSize"] {
            properties.insert(key.into(), json!({"type":["integer","null"]}));
        }
        properties.insert(
            "isDelete".into(),
            json!({"enum":[1,2,null],"description":"1 删除，2 正常"}),
        );
        for key in ["limit", "offset"] {
            properties.insert(key.into(), json!({"type":"integer","minimum":0}));
        }
        return Some(json!({"type":"object","additionalProperties":false,"properties":properties}));
    }
    if name.contains("DirectoryQuery") {
        let mut properties = serde_json::Map::new();
        for key in [
            "keyword",
            "nameQuery",
            "pathQuery",
            "extQuery",
            "kindQuery",
            "linkQuery",
            "inodeQuery",
        ] {
            properties.insert(key.into(), json!({"type":["string","null"]}));
        }
        properties.insert("deleted".into(), json!({"type":["boolean","null"]}));
        for key in ["sizeGe", "sizeLe"] {
            properties.insert(key.into(), json!({"type":["integer","null"],"minimum":0}));
        }
        for key in ["extensions", "otherCategoryExts"] {
            properties.insert(
                key.into(),
                json!({"type":"array","items":{"type":"string"}}),
            );
        }
        properties.insert("sizeMod".into(), json!({"type":["object","null"],"required":["divisor","remainder"],"additionalProperties":false,
            "properties":{"divisor":{"type":"integer","minimum":1},"remainder":{"type":"integer","minimum":0}}}));
        for key in ["timeCreated", "timeAccessed", "timeModified"] {
            properties.insert(
                key.into(),
                json!({"type":["object","null"],"additionalProperties":false,
                "properties":{"from":{"type":["string","null"]},"to":{"type":["string","null"]}}}),
            );
        }
        return Some(
            json!({"type":["object","null"],"additionalProperties":false,"properties":{
            "filter":{"type":"object","additionalProperties":false,"properties":properties},
            "sort":{"enum":["kind","name","ext","size","created","accessed","modified","link","inode","path","deleted",null]},
            "descending":{"type":"boolean"}}}),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::catalog::directory_query::DirectoryQuery;
    use crate::core::catalog::filedb::filedb_query::FileFilter;

    #[test]
    fn expanded_filters_match_the_deserialized_contract() {
        let filter = for_type(Some("FileFilter")).unwrap();
        assert_eq!(
            filter["properties"]["isDelete"]["enum"],
            json!([1, 2, null])
        );
        let parsed: FileFilter = serde_json::from_value(
            json!({"nameGlob":"*.conf","pathPrefix":"/root/","limit":10,"offset":0,"isDelete":2}),
        )
        .unwrap();
        assert_eq!(parsed.name_glob.as_deref(), Some("*.conf"));
        assert!(
            serde_json::from_value::<FileFilter>(json!({"name_glob":"wrong spelling"})).is_err()
        );
        let directory = for_type(Some(
            "Option<crate::core::catalog::directory_query::DirectoryQuery>",
        ))
        .unwrap();
        assert_eq!(
            directory["properties"]["filter"]["properties"]["nameQuery"]["type"],
            json!(["string", "null"])
        );
        let query: DirectoryQuery = serde_json::from_value(
            json!({"filter":{"nameQuery":"*.conf"},"sort":"name","descending":false}),
        )
        .unwrap();
        assert_eq!(query.filter.name_query.as_deref(), Some("*.conf"));
    }

    #[test]
    fn typed_ids_are_described_as_strings() {
        assert_eq!(
            [
                for_type(Some("ImageId")),
                for_type(Some("Option<NodeId>")),
                for_type(Some("Option<Vec<ImageId>>")),
                for_type(Some("String")),
            ],
            [
                Some(json!({"type": "string", "description": "检材 ID"})),
                Some(json!({"type": ["string", "null"], "description": "文件树节点 ID"})),
                Some(
                    json!({"type": ["array", "null"], "items": {"type": "string", "description": "检材 ID"}})
                ),
                None,
            ]
        );
    }
}
