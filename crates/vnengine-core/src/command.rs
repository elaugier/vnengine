//! Commands emitted by the script VM and consumed by the engine.
//!
//! The VM produces a stream of [`Command`]s. Some commands are *blocking*:
//! they require the engine to wait for user input or for an animation to
//! finish before the VM can resume execution.

use crate::{transition::Transition, Value};
use serde::{Deserialize, Serialize};

/// Where a sprite should be anchored on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Anchor {
    Left,
    CenterLeft,
    Center,
    CenterRight,
    Right,
    OffscreenLeft,
    OffscreenRight,
    Custom,
}

/// A sprite to display on screen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpriteSpec {
    /// Character identifier (used to replace/hide existing sprites).
    pub character: String,
    /// Image asset path relative to the project's image directory.
    pub image: String,
    /// Anchor position.
    pub anchor: Anchor,
    /// Optional explicit position in normalized screen coords (0..1).
    pub position: Option<(f32, f32)>,
    /// Optional scale factor.
    pub scale: Option<f32>,
    /// Optional z-order (higher = drawn on top).
    pub z: Option<i32>,
    /// Entry transition (`show ... with dissolve`). `None`/`Cut` is instant.
    #[serde(default)]
    pub transition: Option<Transition>,
}

/// A single choice in a menu.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuChoice {
    pub text: String,
    /// Condition expression that must evaluate truthy for the choice to be
    /// selectable. `None` means always available.
    pub condition: Option<String>,
}

/// Commands produced by the script VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    /// Show a background image, replacing the current one.
    Scene {
        image: Option<String>,
        transition: Transition,
    },
    /// Show (or replace) a character sprite.
    Show(SpriteSpec),
    /// Hide a character sprite.
    Hide {
        character: String,
        transition: Transition,
    },
    /// A line of dialogue spoken by a character.
    Say {
        speaker: Option<String>,
        text: String,
        voice: Option<String>,
        /// Optional side image (character portrait next to dialogue box).
        side_image: Option<String>,
    },
    /// Narration text (no speaker).
    Narrate { text: String },
    /// Present a menu of choices. Blocks until the player selects one.
    /// The selected index (0-based) is returned to the VM.
    Menu {
        prompt: Option<String>,
        choices: Vec<MenuChoice>,
    },
    /// Play a music track on loop.
    PlayMusic { track: String, fade: f32 },
    /// Stop the currently playing music.
    StopMusic { fade: f32 },
    /// Upsert or adjust an adaptive-music stem layer. `track` starts or
    /// replaces the stem; `volume` tweens its level. At least one must
    /// be set.
    MusicLayer {
        name: String,
        track: Option<String>,
        volume: Option<f32>,
        fade: f32,
    },
    /// Remove an adaptive-music stem layer.
    MusicLayerOff { name: String, fade: f32 },
    /// Play a one-shot sound effect.
    PlaySfx { track: String, volume: f32 },
    /// Play a voice line.
    PlayVoice { track: String },
    /// Stop any playing voice line.
    StopVoice,
    /// Pause script execution for a number of seconds.
    Pause { seconds: f32 },
    /// Play a transition effect over the current scene.
    PlayTransition(Transition),
    /// Shake the screen for dramatic effect.
    Shake { intensity: f32, duration: f32 },
    /// Apply a transform to a sprite (ATL-style).
    Transform {
        character: String,
        rotation: Option<f32>,
        scale: Option<f32>,
        offset: Option<(f32, f32)>,
        opacity: Option<f32>,
        layer: Option<crate::model::Layer>,
    },
    /// Notify the engine that a CG image was shown (for the gallery).
    RegisterCg { image: String },
    /// Set a global variable.
    SetVariable { name: String, value: Value },
    /// Send a command to a WASM plugin. `payload` is a JSON array of the
    /// evaluated argument values, e.g. `["unlock","chapel"]`.
    PluginCall { plugin: String, payload: String },
    /// Insert or replace a named point light in the scene.
    LightSet(crate::model::PointLight),
    /// Remove a point light by name.
    LightRemove { name: String },
    /// Set ambient light level per channel (1.0 = fully lit).
    Ambient { color: [f32; 3] },
    /// Move the scene camera (parallax + zoom). Unset axes keep their
    /// current values. `duration` > 0 animates smoothly.
    Camera {
        x: Option<f32>,
        y: Option<f32>,
        zoom: Option<f32>,
        duration: f32,
    },
    /// The script reached the end of the game.
    End,
}

impl Command {
    /// Returns `true` if the engine must wait before asking the VM for the
    /// next command.
    pub fn is_blocking(&self) -> bool {
        matches!(
            self,
            Command::Say { .. }
                | Command::Narrate { .. }
                | Command::Menu { .. }
                | Command::Pause { .. }
                | Command::PlayTransition(_)
                | Command::Shake { .. }
                | Command::End
        )
    }
}
