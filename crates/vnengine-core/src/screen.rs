//! Declarative screen model (script `screen name:` blocks).
//!
//! The VM compiles a `ScreenDef` into a [`ScreenInst`]: a flat list of
//! resolved [`UiNode`]s with all expressions already evaluated. Shown
//! screens live on the runtime (serialized) so save/load/rollback
//! restore them; `call screen` blocks the VM until a button action
//! resolves it — mirroring how `SpriteTrack`s work for ATL.

use crate::Value;
use serde::{Deserialize, Serialize};

/// What a screen node renders as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiNodeKind {
    /// Rich text at `rect[0..2]` (w/h unused).
    Text,
    /// A flat-colored rectangle.
    Box,
    /// An image quad (`image` = asset path).
    Image,
    /// A `Box` + label + `action` hit-test area.
    Button,
}

/// The action a `button` node performs when clicked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScreenAction {
    /// Abandon the call stack and jump to a label (like `jump`).
    Jump(String),
    /// Set a variable (non-dismissing: the screen stays up).
    SetVariable { name: String, value: Value },
    /// End a `call screen` with a value.
    Return(Value),
    /// Show another screen (`show screen`).
    ShowScreen(String),
    /// Hide a screen (`hide screen`).
    HideScreen(String),
}

/// One resolved UI element of a screen. `rect` is `[x, y, w, h]` in
/// normalized screen coordinates (0..1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiNode {
    pub kind: UiNodeKind,
    pub rect: [f32; 4],
    /// Text content for `Text`/`Button` (already interpolated).
    #[serde(default)]
    pub text: Option<String>,
    /// Asset path for `Image` nodes.
    #[serde(default)]
    pub image: Option<String>,
    /// RGBA color (box fill, text color for `Text`/`Button` label is
    /// separate — text uses `color` too; buttons tint the box).
    #[serde(default = "default_color")]
    pub color: [f32; 4],
    /// Font size for `Text`/`Button` labels (px).
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// Click actions (`Button` only), applied in order. Values are
    /// resolved when the screen is shown — a `Return`/`Set` captures
    /// the variable state at show time.
    #[serde(default)]
    pub action: Vec<ScreenAction>,
    /// Whether `text` contains rich-text tags.
    #[serde(default)]
    pub rich: bool,
}

fn default_color() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}

fn default_font_size() -> f32 {
    24.0
}

/// A resolved screen: flat node list + the definition name it came
/// from (used to replace/hide it by name).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenInst {
    pub name: String,
    pub nodes: Vec<UiNode>,
}

impl ScreenInst {
    /// Indices of the clickable (button-with-action) nodes, in draw
    /// order. Used to map a flat clickable index → node.
    pub fn clickables(&self) -> Vec<usize> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.kind == UiNodeKind::Button && !n.action.is_empty())
            .map(|(i, _)| i)
            .collect()
    }
}
