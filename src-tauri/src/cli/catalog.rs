//! Machine-readable command discovery; generated from the registered Rust handlers.
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct Parameter {
    pub name: String,
    pub flag: String,
    pub r#type: String,
    pub required: bool,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct Command {
    pub name: String,
    pub group: String,
    pub description: String,
    pub parameters: Vec<Parameter>,
    #[serde(default)]
    pub parallel: bool,
}
pub fn commands() -> Result<Vec<Command>, serde_json::Error> {
    let mut commands: Vec<Command> =
        serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/cli_catalog.json")))?;
    for command in &mut commands {
        command.parallel = super::session::concurrent(&command.name);
    }
    Ok(commands)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_registered_desktop_command_has_a_cli_entry() {
        let commands = commands().unwrap();
        let source = include_str!("../lib.rs");
        let registry = source
            .split("tauri::generate_handler![")
            .nth(1)
            .unwrap()
            .split("])")
            .next()
            .unwrap();
        let mut registered: Vec<_> = registry
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix("commands::")
                    .map(|path| path.trim_end_matches(',').rsplit("::").next().unwrap())
            })
            .collect();
        let mut exposed: Vec<_> = commands
            .iter()
            .map(|command| command.name.as_str())
            .collect();
        registered.sort_unstable();
        exposed.sort_unstable();
        assert_eq!(exposed, registered);
        assert!(commands
            .iter()
            .find(|command| command.name == "fs_children")
            .unwrap()
            .parameters
            .iter()
            .any(|parameter| parameter.name == "imageId"));
    }
}
