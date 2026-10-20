//! `tools` (hidden): every command as a Model Context Protocol tool
//! definition, so an MCP server can wrap this program. Arguments and options
//! become the input schema's properties under their API names; the
//! evidence scope comes from `LOVELYCARVE_IMAGE`, as in any script.
use serde::{ser::SerializeMap, Serialize, Serializer};
use serde_json::Value;

use super::catalog::{Catalog, Command, Kind};

#[derive(Serialize)]
struct Tools<'a> {
    version: &'static str,
    tools: Vec<Tool<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Tool<'a> {
    name: String,
    description: &'a str,
    input_schema: InputSchema<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InputSchema<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    properties: Properties<'a>,
    required: Vec<&'a str>,
    additional_properties: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    one_of: Option<Value>,
}

#[derive(Serialize)]
struct Property<'a> {
    #[serde(rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'static str>,
    description: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<&'a Value>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    values: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    items: Option<Value>,
}

/// Properties in declaration order: arguments, then options.
struct Properties<'a>(Vec<(&'a str, Property<'a>)>);

impl Serialize for Properties<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, property) in &self.0 {
            map.serialize_entry(name, property)?;
        }
        map.end()
    }
}

/// `lovelycarve-cli images open` → `lovelycarve_cli_images_open`.
fn tool_name(command: &str) -> String {
    let mut name = String::with_capacity(command.len());
    let mut separator = false;
    for character in command.chars() {
        if character.is_whitespace() || character == '-' {
            separator = true;
        } else {
            if separator && !name.is_empty() {
                name.push('_');
            }
            separator = false;
            name.push(character);
        }
    }
    name
}

fn tool(command: &Command) -> Tool<'_> {
    let mut properties = Vec::new();
    let mut required = Vec::new();
    for param in command.params.iter().filter(|param| param.visible()) {
        properties.push((
            param.name.as_str(),
            Property {
                kind: match param.kind {
                    Kind::String | Kind::Enum => Some("string"),
                    Kind::Number => Some("number"),
                    Kind::Boolean => Some("boolean"),
                    Kind::Array => Some("array"),
                    Kind::Json => None,
                },
                description: &param.description,
                default: param.default.as_ref(),
                values: (param.kind != Kind::Array)
                    .then_some(param.values.as_deref())
                    .flatten(),
                items: (param.kind == Kind::Array).then(|| match &param.values {
                    Some(values) => serde_json::json!({"enum": values}),
                    None => serde_json::json!({}),
                }),
            },
        ));
        if param.required && !param.synthetic {
            required.push(param.name.as_str());
        }
    }
    Tool {
        name: tool_name(&command.command),
        description: &command.description,
        input_schema: InputSchema {
            kind: "object",
            properties: Properties(properties),
            required,
            additional_properties: false,
            one_of: command
                .target
                .as_deref()
                .filter(|_| command.param("path").is_some_and(|param| param.required))
                .map(|target| serde_json::json!([{"required": ["path"]}, {"required": [target]}])),
        },
    }
}

/// `{version, tools}` for every command in the catalogue.
pub fn definitions(catalog: &Catalog) -> impl Serialize + '_ {
    Tools {
        version: "2.0",
        tools: catalog.commands.iter().map(tool).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::catalog::catalog;

    #[test]
    fn every_command_becomes_a_tool_with_its_options() {
        assert_eq!(
            tool_name("lovelycarve-cli files sqlite deleted-records list"),
            "lovelycarve_cli_files_sqlite_deleted_records_list"
        );
        let catalog = catalog().unwrap();
        let tools = serde_json::to_value(definitions(catalog)).unwrap();
        assert_eq!(tools["version"], "2.0");
        assert_eq!(
            tools["tools"].as_array().unwrap().len(),
            catalog.commands.len()
        );
        let open = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "lovelycarve_cli_images_open")
            .unwrap();
        assert_eq!(open["inputSchema"]["type"], "object");
        assert_eq!(open["inputSchema"]["required"], serde_json::json!(["path"]));
        assert!(open["inputSchema"]["properties"].get("imageId").is_none());
        assert!(open["inputSchema"]["properties"]
            .get("qcowPassword")
            .is_some());
        assert!(open["inputSchema"]["properties"]
            .get("qcow-password")
            .is_none());
        let text = serde_json::to_string(&definitions(catalog)).unwrap();
        assert!(text.starts_with(r#"{"version":"2.0","tools":[{"name":"#));
        let export = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "lovelycarve_cli_images_export")
            .unwrap();
        assert_eq!(
            export["inputSchema"]["properties"]["mode"]["enum"],
            serde_json::json!(["source", "unlockedPartitions"])
        );
        let read = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "lovelycarve_cli_files_read")
            .unwrap();
        assert_eq!(
            read["inputSchema"]["oneOf"],
            serde_json::json!([
                {"required": ["path"]}, {"required": ["nodeId"]}
            ])
        );
        assert_eq!(read["inputSchema"]["additionalProperties"], false);
    }
}
