//! The desktop protocol for an install request from the plugin release page.

use serde::Serialize;

use crate::core::{casework::evidence_link::SCHEME, extensions::plugin::is_valid_plugin_id};

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PluginInstallLink {
    plugin_id: String,
    version: String,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("插件安装链接无效")]
pub(super) struct InvalidInstallLink;

impl PluginInstallLink {
    /// Recognizes the action even if its inputs are invalid, so it can be
    /// delivered to a desktop that understands this action.
    pub(super) fn matches(text: &str) -> bool {
        let Some((scheme, address)) = text.trim().split_once("://") else {
            return false;
        };
        let host = address.split(['/', '?', '#']).next();
        scheme.eq_ignore_ascii_case(SCHEME)
            && host.is_some_and(|host| host.eq_ignore_ascii_case("install-plugin"))
    }

    pub(super) fn parse(text: &str) -> Result<Self, InvalidInstallLink> {
        if !Self::matches(text) || text.len() > 2048 {
            return Err(InvalidInstallLink);
        }
        let url = reqwest::Url::parse(text).map_err(|_| InvalidInstallLink)?;
        if !matches!(url.path(), "" | "/") || url.fragment().is_some() {
            return Err(InvalidInstallLink);
        }
        let mut plugin_id = None;
        let mut version = None;
        for (name, value) in url.query_pairs() {
            match name.as_ref() {
                "id" if plugin_id.is_none() && is_valid_plugin_id(&value) => {
                    plugin_id = Some(value.into_owned());
                }
                "version" if version.is_none() => {
                    semver::Version::parse(&value).map_err(|_| InvalidInstallLink)?;
                    version = Some(value.into_owned());
                }
                _ => return Err(InvalidInstallLink),
            }
        }
        Ok(Self {
            plugin_id: plugin_id.ok_or(InvalidInstallLink)?,
            version: version.ok_or(InvalidInstallLink)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_install_link_carries_only_release_identity() {
        let link = PluginInstallLink::parse(
            "LOVELYCARVE://INSTALL-PLUGIN/?id=org.example.demo&version=2.0.1-rc.1%2Bbuild",
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(link).unwrap(),
            serde_json::json!({"pluginId": "org.example.demo", "version": "2.0.1-rc.1+build"})
        );
    }

    #[test]
    fn plugin_install_link_rejects_untrusted_or_ambiguous_fields() {
        let base = "lovelycarve://install-plugin?id=org.example.demo&version=1.0.0";
        for invalid in [
            "https://install-plugin?id=p&version=1.0.0",
            "lovelycarve://open?case=1",
            "lovelycarve://other@install-plugin?id=p&version=1.0.0",
            "lovelycarve://install-plugin:9?id=p&version=1.0.0",
            "lovelycarve://install-plugin/file?id=p&version=1.0.0",
            "lovelycarve://install-plugin?id=p",
            "lovelycarve://install-plugin?id=%2fapp%2fplugin&version=1.0.0",
            "lovelycarve://install-plugin?id=p&version=latest",
            &format!("{base}&id=another"),
            &format!("{base}&version=2.0.0"),
            &format!("{base}&source=https://example.org"),
            &format!("{base}#fragment"),
        ] {
            assert_eq!(PluginInstallLink::parse(invalid), Err(InvalidInstallLink));
        }
    }
}
