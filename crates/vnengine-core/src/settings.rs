//! User-adjustable settings persisted between sessions.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Persisted user settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    /// Master volume in 0..1.
    pub master_volume: f64,
    /// BGM volume in 0..1.
    pub music_volume: f64,
    /// SFX volume in 0..1.
    pub sfx_volume: f64,
    /// Voice volume in 0..1.
    pub voice_volume: f64,
    /// Text speed in characters per second (0 = instant).
    pub text_speed: f32,
    /// Whether text auto-advances.
    pub auto_advance: bool,
    /// Auto-advance delay in seconds after a line completes.
    pub auto_advance_delay: f32,
    /// Whether the game runs fullscreen.
    pub fullscreen: bool,
    /// Whether to skip unread text.
    pub skip_unread: bool,
    /// Mute audio entirely.
    pub muted: bool,
    /// UI/dialogue language (`translations/<lang>.json` in assets).
    /// Older settings files deserialize to English.
    #[serde(default = "default_language")]
    pub language: String,
    /// Accessibility: global text scale (1.0 = 100%).
    #[serde(default = "default_text_scale")]
    pub text_scale: f32,
    /// Accessibility: flatten transitions, camera moves and shakes.
    #[serde(default)]
    pub reduce_motion: bool,
    /// Music duck level while a voice line plays (0..1 multiplier on
    /// the music category; 1.0 disables ducking).
    #[serde(default = "default_voice_duck")]
    pub voice_duck: f64,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_voice_duck() -> f64 {
    0.5
}

fn default_text_scale() -> f32 {
    1.0
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            master_volume: 1.0,
            music_volume: 0.8,
            sfx_volume: 1.0,
            voice_volume: 1.0,
            text_speed: 40.0,
            auto_advance: false,
            auto_advance_delay: 1.5,
            fullscreen: false,
            skip_unread: false,
            muted: false,
            language: default_language(),
            text_scale: 1.0,
            reduce_motion: false,
            voice_duck: 0.5,
        }
    }
}

impl UserSettings {
    /// Load settings from a JSON file, returning defaults if missing.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(serde_json::from_str::<Self>(&text).unwrap_or_default()),
            Err(_) => Ok(Self::default()),
        }
    }

    /// Save settings to a JSON file.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        fs::write(path, text)?;
        Ok(())
    }
}
