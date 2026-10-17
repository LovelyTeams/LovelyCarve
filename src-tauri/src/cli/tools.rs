//! `tools` (hidden): every command as a Model Context Protocol tool
//! definition, so an MCP server can wrap this program. Arguments and options
//! become the input schema's properties under their command-line names; the
//! evidence scope comes from `LOVELYCARVE_IMAGE`, as in any script.
use serde::{ser::SerializeMap, Serialize, Serializer};
use serde_json::Value;

use super::catalog::{Catalog, Command};

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
struct InputSchema<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    properties: Properties<'a>,
    required: Vec<&'a str>,
}

#[derive(Serialize)]
struct Property<'a> {
    #[serde(rename = "type")]
    kind: &'a Value,
    description: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<&'a Value>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    values: Option<&'a Value>,
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
    for meta in command.arguments.iter().chain(&command.options) {
        let Some(name) = meta["name"].as_str() else {
            continue;
        };
        properties.push((
            name,
            Property {
                kind: &meta["type"],
                description: &meta["description"],
                default: meta.get("default"),
                values: meta
                    .get("enum")
                    .filter(|values| values.as_array().is_some_and(|values| !values.is_empty())),
            },
        ));
        if meta["required"] == true {
            required.push(name);
        }
    }
    Tool {
        name: tool_name(&command.command),
        description: &command.description,
        input_schema: InputSchema {
            kind: "object",
            properties: Properties(properties),
            required,
        },
    }
}

/// `{version, tools}` for every command in the catalogue.
pub fn definitions(catalog: &Catalog) -> impl Serialize + '_ {
    Tools {
        version: "1.0",
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
        assert_eq!(tools["version"], "1.0");
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
        assert_eq!(
            open["inputSchema"]["properties"]["dry-run"]["default"],
            false
        );
        let text = serde_json::to_string(&definitions(catalog)).unwrap();
        assert!(text.starts_with(r#"{"version":"1.0","tools":[{"name":"#));
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
    }
}
