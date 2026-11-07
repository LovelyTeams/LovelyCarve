//! Interface preferences persisted beside the case registry
//! (`<app data>/gpui-preferences.json`). Unreadable or missing files fall
//! back to the defaults; a failed save is logged and the session keeps going.
use std::path::PathBuf;

use gpui_kit::{
    component::{Theme, ThemeMode},
    px, App, Global, Window,
};
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "gpui-preferences.json";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "跟随系统",
            ThemeChoice::Light => "浅色",
            ThemeChoice::Dark => "深色",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Preferences {
    pub theme: ThemeChoice,
    /// The rem base: every text, spacing and control size follows it.
    pub font_size: f32,
    pub sidebar_collapsed: bool,
    pub welcome_motion: bool,
    #[cfg(all(windows, target_arch = "x86_64"))]
    pub outside_in_directory: Option<PathBuf>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            font_size: 14.0,
            sidebar_collapsed: false,
            welcome_motion: true,
            #[cfg(all(windows, target_arch = "x86_64"))]
            outside_in_directory: None,
        }
    }
}

impl Global for Preferences {}

impl Preferences {
    fn path() -> Option<PathBuf> {
        lovelycarve_lib::host::default_app_data_dir().map(|directory| directory.join(FILE_NAME))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                tracing::warn!("ignore unreadable {}: {error}", path.display());
                Self::default()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                tracing::warn!("read {}: {error}", path.display());
                Self::default()
            }
        }
    }

    fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        let saved = serde_json::to_string_pretty(self)
            .map_err(std::io::Error::other)
            .and_then(|text| {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, text)
            });
        if let Err(error) = saved {
            tracing::warn!("save {}: {error}", path.display());
        }
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Change the preferences, persist them and apply the theme.
    pub fn update(window: &Window, cx: &mut App, edit: impl FnOnce(&mut Self)) {
        let previous = Self::global(cx);
        let mut preferences = previous.clone();
        edit(&mut preferences);
        let appearance_changed =
            preferences.theme != previous.theme || preferences.font_size != previous.font_size;
        preferences.save();
        cx.set_global(preferences);
        if appearance_changed {
            apply_theme(window, cx);
        } else {
            cx.refresh_windows();
        }
    }
}

/// Apply the preferred appearance and base font size to the global theme.
pub fn apply_theme(window: &Window, cx: &mut App) {
    let preferences = cx.global::<Preferences>().clone();
    let mode = match preferences.theme {
        ThemeChoice::Light => ThemeMode::Light,
        ThemeChoice::Dark => ThemeMode::Dark,
        ThemeChoice::System => window.appearance().into(),
    };
    Theme::change(mode, None, cx);
    Theme::update(cx, |theme| {
        crate::theme::apply(theme);
        theme.font_size = px(preferences.font_size);
    });
}

#[cfg(test)]
mod tests {
    use super::{Preferences, ThemeChoice};

    #[test]
    fn older_preferences_enable_welcome_motion_by_default() {
        let mut preferences: Preferences =
            serde_json::from_str(r#"{"theme":"dark","fontSize":15}"#).unwrap();
        assert_eq!(preferences.theme, ThemeChoice::Dark);
        assert!(preferences.welcome_motion);

        preferences.welcome_motion = false;
        let saved = serde_json::to_string(&preferences).unwrap();
        let reloaded: Preferences = serde_json::from_str(&saved).unwrap();
        assert!(!reloaded.welcome_motion);
    }
}
