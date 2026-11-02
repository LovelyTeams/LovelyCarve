//! Expanded request objects for the common metadata and directory queries.
use serde_json::{json, Value};

pub(super) fn for_type(name: Option<&str>) -> Option<Value> {
    let name = name?;
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
}
