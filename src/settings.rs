//! The author's preferences: which palette, and which two typefaces. Not document state — a
//! `.dln` never records them, so opening someone else's file does not change how your editor
//! looks — and **imports no `gpui`**, so it is testable like `doc.rs`.
//!
//! One small JSON file in the user's config directory. Loading never fails: a missing,
//! unreadable or garbled file is the defaults, because an editor that will not start over a
//! preferences file is worse than one that forgets a font. Saving is the opposite — it
//! reports — but the view treats a failure as a status message, not an error.
//!
//! **Isolation is the launcher's job**, as it is for the crash copy: the path follows
//! `XDG_CONFIG_HOME`, and `tools/shots.py` points that at a private directory so a screenshot
//! run neither reads nor overwrites the real settings.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which palette is wanted. `System` follows the desktop; the other two override it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePref {
    #[default]
    System,
    Dark,
    Light,
}

impl ThemePref {
    pub const ALL: [ThemePref; 3] = [ThemePref::System, ThemePref::Dark, ThemePref::Light];

    pub fn label(self) -> &'static str {
        match self {
            ThemePref::System => "System",
            ThemePref::Dark => "Dark",
            ThemePref::Light => "Light",
        }
    }
}

/// The face the chrome is set in: menus, labels, the inspector, ids, variable names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiFont {
    #[default]
    Inter,
    Geist,
    Plex,
}

impl UiFont {
    pub const ALL: [UiFont; 3] = [UiFont::Inter, UiFont::Geist, UiFont::Plex];

    /// The name gpui finds the face by: the family the bundled file declares.
    pub fn family(self) -> &'static str {
        match self {
            UiFont::Inter => "Inter",
            UiFont::Geist => "Geist",
            UiFont::Plex => "IBM Plex Sans",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            UiFont::Plex => "IBM Plex",
            other => other.family(),
        }
    }
}

/// The face of what a *character says*: dialogue and speaker names on the canvas, in the
/// inspector's Speaker and Text fields and in play, and the choice labels the player picks
/// from. Identifiers — variable names, aliases, tags, ids — stay in the UI face, so a
/// typeface for dialogue can be as particular as a screenplay's without making a variable
/// name read as something a character said.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextFont {
    #[default]
    SameAsUi,
    Newsreader,
    CourierPrime,
}

impl TextFont {
    pub const ALL: [TextFont; 3] = [
        TextFont::SameAsUi,
        TextFont::Newsreader,
        TextFont::CourierPrime,
    ];

    /// The family, or `None` for "whatever the UI font is".
    pub fn family(self) -> Option<&'static str> {
        match self {
            TextFont::SameAsUi => None,
            TextFont::Newsreader => Some("Newsreader 16pt"),
            TextFont::CourierPrime => Some("Courier Prime"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TextFont::SameAsUi => "Same as UI",
            TextFont::Newsreader => "Newsreader",
            TextFont::CourierPrime => "Courier Prime",
        }
    }
}

/// Everything the Preferences card sets. `#[serde(default)]` on each field, so a file
/// written by an older build (before a setting existed) or a newer one (after one was
/// removed) still loads, keeping what it can read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemePref,
    pub ui_font: UiFont,
    pub text_font: TextFont,
}

impl Settings {
    /// Reads the file at `path`; the defaults for any reason it cannot be read or parsed.
    pub fn load_from(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|json| Settings::from_json(&json))
            .unwrap_or_default()
    }

    /// The settings in the user's config directory, or the defaults.
    pub fn load() -> Settings {
        config_path().map_or_else(Settings::default, |p| Settings::load_from(&p))
    }

    pub fn from_json(json: &str) -> Option<Settings> {
        serde_json::from_str(json).ok()
    }

    pub fn to_json(self) -> String {
        // Cannot fail: plain enums, no maps with non-string keys.
        serde_json::to_string_pretty(&self).unwrap_or_default()
    }

    /// Writes to `path`, through a `.tmp` and a rename so a kill mid-write cannot leave half
    /// a file, creating the directory the first time.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, self.to_json()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn save(&self) -> Result<(), String> {
        match config_path() {
            Some(path) => self.save_to(&path),
            None => Err("no config directory".to_string()),
        }
    }
}

/// `$XDG_CONFIG_HOME/palaver/settings.json`, falling back to `~/.config/...`. Taken as
/// arguments so the rule is testable without touching the process environment.
pub fn config_path_from(xdg: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    let base = match (
        xdg.filter(|s| !s.is_empty()),
        home.filter(|s| !s.is_empty()),
    ) {
        (Some(xdg), _) => PathBuf::from(xdg),
        (None, Some(home)) => PathBuf::from(home).join(".config"),
        (None, None) => return None,
    };
    Some(base.join("palaver").join("settings.json"))
}

pub fn config_path() -> Option<PathBuf> {
    config_path_from(
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_json() {
        let s = Settings {
            theme: ThemePref::Light,
            ui_font: UiFont::Geist,
            text_font: TextFont::CourierPrime,
        };
        assert_eq!(Settings::from_json(&s.to_json()), Some(s));
    }

    /// An older file with only some keys, a newer one with extras, and a value this build
    /// does not know: each keeps whatever it can read and defaults the rest.
    #[test]
    fn a_partial_or_foreign_file_keeps_what_it_can() {
        let partial = Settings::from_json(r#"{ "theme": "dark" }"#).unwrap();
        assert_eq!(partial.theme, ThemePref::Dark);
        assert_eq!(partial.ui_font, UiFont::Inter);

        let extra = Settings::from_json(r#"{ "theme": "light", "something_new": 3 }"#).unwrap();
        assert_eq!(extra.theme, ThemePref::Light);

        // An unknown variant is a parse failure for the whole file, which is the defaults:
        // better than guessing which of the three fonts was meant.
        assert_eq!(Settings::from_json(r#"{ "ui_font": "comic_sans" }"#), None);
    }

    #[test]
    fn a_missing_or_garbled_file_is_the_defaults() {
        let dir = std::env::temp_dir().join(format!("palaver-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            Settings::load_from(&dir.join("absent.json")),
            Settings::default()
        );
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "{ not json").unwrap();
        assert_eq!(Settings::load_from(&bad), Settings::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn saving_creates_the_directory_and_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("palaver-save-{}", std::process::id()));
        let path = dir.join("nested").join("settings.json");
        let s = Settings {
            theme: ThemePref::Dark,
            ..Settings::default()
        };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
        assert!(!path.with_extension("json.tmp").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The launcher isolates a run by pointing `XDG_CONFIG_HOME` somewhere private.
    #[test]
    fn the_config_path_follows_xdg_then_home() {
        assert_eq!(
            config_path_from(Some("/tmp/x"), Some("/home/a")),
            Some(PathBuf::from("/tmp/x/palaver/settings.json"))
        );
        assert_eq!(
            config_path_from(None, Some("/home/a")),
            Some(PathBuf::from("/home/a/.config/palaver/settings.json"))
        );
        assert_eq!(
            config_path_from(Some(""), Some("/home/a")),
            Some(PathBuf::from("/home/a/.config/palaver/settings.json")),
            "an empty variable is unset"
        );
        assert_eq!(config_path_from(None, None), None);
    }

    /// Every choice has a face the bundled files declare, and a label to show.
    #[test]
    fn every_choice_names_a_family_and_a_label() {
        for f in UiFont::ALL {
            assert!(!f.family().is_empty() && !f.label().is_empty());
        }
        assert_eq!(TextFont::SameAsUi.family(), None);
        for f in [TextFont::Newsreader, TextFont::CourierPrime] {
            assert!(f.family().is_some());
        }
    }
}
