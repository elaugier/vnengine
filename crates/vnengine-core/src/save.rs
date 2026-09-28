//! Save / load system.
//!
//! A [`SaveState`] captures everything needed to resume the game: the script
//! VM's execution position (including variables), the visible scene and the
//! line or menu currently displayed. It is serialized as JSON.

use crate::command::Command;
use crate::model::Scene;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

fn default_scene() -> Scene {
    Scene {
        bg_opacity: 1.0,
        ..Default::default()
    }
}

/// One recorded player input, used to replay a session deterministically.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JournalEntry {
    /// The player selected menu choice N.
    Menu(usize),
}

/// One adaptive-music stem layer (a synced loop faded in/out
/// independently of the base track).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MusicLayerState {
    pub track: String,
    pub volume: f32,
}

/// A single save slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveState {
    /// Slot index (0-based).
    pub slot: u32,
    /// Human-readable label (e.g. chapter or last spoken line).
    pub label: String,
    /// In-game timestamp (seconds elapsed).
    pub play_time: f32,
    /// Script VM state, serialized as opaque bytes.
    pub vm_state: Vec<u8>,
    /// The visible scene at save time. Older saves lack this field; the
    /// default shows a fully-opaque empty scene.
    #[serde(default = "default_scene")]
    pub scene: Scene,
    /// The blocking command displayed when the save was taken (dialogue
    /// line or pause), so load can re-display it instead of skipping past.
    /// `None` if the save was taken on a menu or between commands.
    #[serde(default)]
    pub blocking: Option<Command>,
    /// BGM track playing at save time, if any. Restored on load.
    #[serde(default)]
    pub music: Option<String>,
    /// Player inputs recorded so far, for deterministic replay.
    #[serde(default)]
    pub journal: Vec<JournalEntry>,
    /// Plugin key/value state (`<plugin>.<key>`), so plugin data
    /// restores with the save and rollback.
    #[serde(default)]
    pub plugin_kv: std::collections::BTreeMap<String, String>,
    /// Adaptive-music stem layers active at save time.
    #[serde(default)]
    pub music_layers: std::collections::BTreeMap<String, MusicLayerState>,
    /// Thumbnail image path (PNG bytes) of the scene.
    pub thumbnail: Option<Vec<u8>>,
    /// Real-world save time (Unix timestamp).
    pub saved_at: i64,
}

impl SaveState {
    /// Save to `<dir>/save_<slot>.json`.
    pub fn write(&self, dir: &Path) -> anyhow::Result<PathBuf> {
        fs::create_dir_all(dir)?;
        let path = dir.join(format!("save_{}.json", self.slot));
        let text = serde_json::to_string(self)?;
        fs::write(&path, text)?;
        Ok(path)
    }

    /// Load a specific slot.
    pub fn read(dir: &Path, slot: u32) -> anyhow::Result<Self> {
        let path = dir.join(format!("save_{}.json", slot));
        let text = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&text)?)
    }

    /// List all save slots present in a directory, sorted by slot index.
    pub fn list(dir: &Path) -> Vec<Self> {
        let mut saves = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(text) = fs::read_to_string(&path) {
                    if let Ok(state) = serde_json::from_str::<Self>(&text) {
                        saves.push(state);
                    }
                }
            }
        }
        saves.sort_by_key(|s| s.slot);
        saves
    }

    /// Delete a save slot.
    pub fn delete(dir: &Path, slot: u32) -> std::io::Result<()> {
        let path = dir.join(format!("save_{}.json", slot));
        if path.exists() {
            fs::remove_file(path)
        } else {
            Ok(())
        }
    }
}

/// Metadata for the CG gallery: which CGs have been unlocked.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GalleryState {
    /// Set of unlocked CG image paths.
    pub unlocked: Vec<String>,
}

impl GalleryState {
    pub fn unlock(&mut self, image: &str) {
        if !self.unlocked.iter().any(|c| c == image) {
            self.unlocked.push(image.to_string());
        }
    }

    pub fn is_unlocked(&self, image: &str) -> bool {
        self.unlocked.iter().any(|c| c == image)
    }

    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}
