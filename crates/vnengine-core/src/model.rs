//! Runtime scene model: what is currently visible on screen.
//!
//! The renderer reads this model each frame. The engine mutates it in
//! response to [`Command`]s coming from the script VM.

use crate::command::{Anchor, SpriteSpec};
use serde::{Deserialize, Serialize};

/// Rendering layer for organizing sprites.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Layer {
    /// Background layer (behind everything).
    #[default]
    Background,
    /// Midground layer (behind characters).
    Midground,
    /// Foreground layer (characters/sprites).
    Foreground,
    /// Overlay layer (in front of everything).
    Overlay,
}

/// A sprite currently displayed on screen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveSprite {
    pub character: String,
    pub image: String,
    pub anchor: Anchor,
    pub position: Option<(f32, f32)>,
    pub scale: f32,
    pub z: i32,
    /// Current opacity used for transitions.
    pub opacity: f32,
    /// Current pixel offset used for transitions (slide).
    pub offset: [f32; 2],
    /// Rotation in radians (ATL transform).
    pub rotation: f32,
    /// Which layer this sprite belongs to.
    pub layer: Layer,
}

impl From<&SpriteSpec> for ActiveSprite {
    fn from(s: &SpriteSpec) -> Self {
        Self {
            character: s.character.clone(),
            image: s.image.clone(),
            anchor: s.anchor,
            position: s.position,
            scale: s.scale.unwrap_or(1.0),
            z: s.z.unwrap_or(0),
            opacity: 1.0,
            offset: [0.0, 0.0],
            rotation: 0.0,
            layer: Layer::Foreground,
        }
    }
}

/// The full visual state of the scene at a given moment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Scene {
    /// Background image path.
    pub background: Option<String>,
    /// Background opacity (for transitions).
    pub bg_opacity: f32,
    /// Previous background kept around during a transition.
    pub prev_background: Option<String>,
    pub prev_bg_opacity: f32,
    /// Sprites currently on screen, sorted by layer then z.
    pub sprites: Vec<ActiveSprite>,
}

impl Scene {
    /// Replace or add a sprite for the given character.
    pub fn set_sprite(&mut self, spec: &SpriteSpec) {
        let mut active = ActiveSprite::from(spec);
        // Preserve transition opacity if replacing the same character.
        if let Some(existing) = self.sprites.iter().find(|s| s.character == spec.character) {
            active.opacity = existing.opacity;
            active.layer = existing.layer;
            active.rotation = existing.rotation;
        }
        self.sprites.retain(|s| s.character != spec.character);
        self.sprites.push(active);
        self.sprites.sort_by_key(|s| (s.layer, s.z));
    }

    /// Remove a character's sprite.
    pub fn hide_sprite(&mut self, character: &str) -> Option<ActiveSprite> {
        if let Some(pos) = self.sprites.iter().position(|s| s.character == character) {
            Some(self.sprites.remove(pos))
        } else {
            None
        }
    }

    /// Find a sprite by character id.
    pub fn sprite(&self, character: &str) -> Option<&ActiveSprite> {
        self.sprites.iter().find(|s| s.character == character)
    }

    pub fn sprite_mut(&mut self, character: &str) -> Option<&mut ActiveSprite> {
        self.sprites.iter_mut().find(|s| s.character == character)
    }

    /// Get all sprites on a specific layer.
    pub fn sprites_on_layer(&self, layer: Layer) -> impl Iterator<Item = &ActiveSprite> {
        self.sprites.iter().filter(move |s| s.layer == layer)
    }
}

/// Convert an anchor to a normalized x position (0 = left, 1 = right).
pub fn anchor_x(anchor: Anchor) -> f32 {
    match anchor {
        Anchor::OffscreenLeft => -0.2,
        Anchor::Left => 0.2,
        Anchor::CenterLeft => 0.35,
        Anchor::Center => 0.5,
        Anchor::CenterRight => 0.65,
        Anchor::Right => 0.8,
        Anchor::OffscreenRight => 1.2,
        Anchor::Custom => 0.5,
    }
}
