//! Engine configuration describing a visual novel project.

use serde::{Deserialize, Serialize};

/// Configuration loaded from the project's `project.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Window title.
    pub title: String,
    /// Internal render resolution (width).
    pub width: u32,
    /// Internal render resolution (height).
    pub height: u32,
    /// Whether the window starts in fullscreen.
    pub fullscreen: bool,
    /// V-sync enabled.
    pub vsync: bool,
    /// Path (relative to project root) to the entry script file.
    pub entry_script: String,
    /// Directory containing image assets.
    pub image_dir: String,
    /// Directory containing audio assets.
    pub audio_dir: String,
    /// Directory containing voice assets.
    pub voice_dir: String,
    /// Directory containing font files (.ttf, .otf).
    /// Loaded into the text engine at startup; families can be selected
    /// with `font default "<family>"` in the script or `font=` on screens.
    #[serde(default = "default_font_dir")]
    pub font_dir: String,
    /// Directory for save files.
    pub save_dir: String,
    /// Default text speed in characters per second (0 = instant).
    pub text_speed: f32,
    /// Names of characters for the script `define` block, keyed by id.
    #[serde(default)]
    pub characters: Vec<CharacterDef>,
}

fn default_font_dir() -> String {
    "fonts".to_string()
}

/// A character definition (id -> display name + color).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterDef {
    pub id: String,
    pub name: String,
    /// Optional name color as [r, g, b] in 0..1.
    pub color: Option<[f32; 3]>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            title: "vnengine".to_string(),
            width: 1280,
            height: 720,
            fullscreen: false,
            vsync: true,
            entry_script: "script/main.vn".to_string(),
            image_dir: "images".to_string(),
            audio_dir: "audio".to_string(),
            voice_dir: "voice".to_string(),
            font_dir: default_font_dir(),
            save_dir: "saves".to_string(),
            text_speed: 40.0,
            characters: Vec::new(),
        }
    }
}
