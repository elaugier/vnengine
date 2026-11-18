//! Runtime scene model: what is currently visible on screen.
//!
//! The renderer reads this model each frame. The engine mutates it in
//! response to [`Command`]s coming from the script VM.

use crate::command::{Anchor, SpriteSpec};
use crate::transition::Transition;
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
    /// Entrance animation in progress; `opacity`/`offset`/`scale` hold
    /// the *target* values while this runs.
    #[serde(default)]
    pub entering: Option<EnterAnim>,
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
            layer: s.layer.unwrap_or(Layer::Foreground),
            entering: s.transition.as_ref().and_then(|t| {
                let duration = t.default_duration();
                (duration > 0.0).then(|| EnterAnim {
                    transition: t.clone(),
                    elapsed: 0.0,
                    duration,
                })
            }),
        }
    }
}

/// A sprite entrance animation (`show ... with dissolve`). Serialized
/// with the sprite so save/restore resumes mid-fade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnterAnim {
    pub transition: Transition,
    pub elapsed: f32,
    pub duration: f32,
}

impl EnterAnim {
    pub fn progress(&self) -> f32 {
        if self.duration <= 0.0 {
            1.0
        } else {
            (self.elapsed / self.duration).min(1.0)
        }
    }
}

/// A sprite being removed with a transition (`hide ... with dissolve`).
/// `elapsed` is advanced by the runtime's logical clock; the sprite is
/// dropped when `elapsed >= duration`. Serialized with the scene so
/// saves and rollback restore an in-progress fade correctly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitingSprite {
    pub sprite: ActiveSprite,
    pub transition: Transition,
    pub elapsed: f32,
    pub duration: f32,
}

impl ExitingSprite {
    /// Progress in 0..1 of the exit transition.
    pub fn progress(&self) -> f32 {
        if self.duration <= 0.0 {
            1.0
        } else {
            (self.elapsed / self.duration).min(1.0)
        }
    }
}

/// The full visual state of the scene at a given moment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    /// Background image path.
    pub background: Option<String>,
    /// Background opacity (for transitions).
    pub bg_opacity: f32,
    /// Previous background kept around during a transition.
    pub prev_background: Option<String>,
    pub prev_bg_opacity: f32,
    /// Background zoom factor (1.0 = fitted). Animated by `zoom`.
    #[serde(default = "default_zoom")]
    pub bg_zoom: f32,
    /// Previous-background zoom during a `zoom` transition.
    #[serde(default = "default_zoom")]
    pub prev_bg_zoom: f32,
    /// Opaque overlay during a `fade` transition (color + alpha).
    /// Peaks mid-transition while the scene swaps underneath.
    #[serde(default)]
    pub fade_overlay: [f32; 4],
    /// Sprites currently on screen, sorted by layer then z.
    pub sprites: Vec<ActiveSprite>,
    /// Sprites mid-exit transition (drawn with their remaining opacity).
    #[serde(default)]
    pub exiting: Vec<ExitingSprite>,
    /// Point lights in the scene (2.5D lighting).
    #[serde(default)]
    pub lights: Vec<PointLight>,
    /// Ambient light level per channel. Defaults to fully lit so old
    /// saves/scenes look unchanged.
    #[serde(default = "default_ambient")]
    pub ambient: [f32; 3],
    /// Camera viewpoint (parallax + zoom).
    #[serde(default)]
    pub camera: Camera,
    /// Camera move in progress (`camera ... over=<sec>`).
    #[serde(default)]
    pub camera_anim: Option<CameraAnim>,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            background: None,
            bg_opacity: 0.0,
            prev_background: None,
            prev_bg_opacity: 0.0,
            bg_zoom: 1.0,
            prev_bg_zoom: 1.0,
            fade_overlay: [0.0; 4],
            sprites: Vec::new(),
            exiting: Vec::new(),
            lights: Vec::new(),
            ambient: [1.0, 1.0, 1.0],
            camera: Camera::default(),
            camera_anim: None,
        }
    }
}

impl Scene {
    /// Replace or add a sprite for the given character. When `show` has
    /// a timed transition, the replaced sprite's previous image exits
    /// through `exiting` while the new one enters — a true cross-fade
    /// instead of an image swap.
    pub fn set_sprite(&mut self, spec: &SpriteSpec) {
        let mut active = ActiveSprite::from(spec);
        if let Some(existing) = self
            .sprites
            .iter()
            .find(|s| s.character == spec.character)
            .cloned()
        {
            // Preserve transition state if replacing the same character.
            active.opacity = existing.opacity;
            active.layer = existing.layer;
            active.rotation = existing.rotation;
            // Same image re-shown keeps the old sprite's enter progress
            // (re-show is a no-op visually); a different image cross-fades.
            if existing.image == spec.image {
                active.entering = existing.entering.clone();
            } else if let Some(t) = &spec.transition {
                let duration = t.default_duration();
                if duration > 0.0 {
                    self.exiting.push(ExitingSprite {
                        sprite: existing,
                        transition: t.clone(),
                        elapsed: 0.0,
                        duration,
                    });
                }
            }
        }
        self.sprites.retain(|s| s.character != spec.character);
        self.sprites.push(active);
        self.sprites.sort_by_key(|s| (s.layer, s.z));
    }

    /// Remove a character's sprite. With a timed transition the sprite
    /// moves to `exiting` and keeps rendering until its duration elapses.
    /// Returns whether the sprite was present.
    pub fn hide_sprite(&mut self, character: &str, transition: Transition) -> bool {
        let duration = transition.default_duration();
        if let Some(pos) = self.sprites.iter().position(|s| s.character == character) {
            let sprite = self.sprites.remove(pos);
            if duration > 0.0 {
                self.exiting.push(ExitingSprite {
                    sprite,
                    transition,
                    elapsed: 0.0,
                    duration,
                });
            }
            true
        } else {
            false
        }
    }

    /// Hide every sprite (`hide` with no character). Timed transitions
    /// move each sprite to `exiting` like [`Scene::hide_sprite`].
    pub fn hide_all_sprites(&mut self, transition: Transition) {
        let duration = transition.default_duration();
        let sprites = std::mem::take(&mut self.sprites);
        if duration > 0.0 {
            for sprite in sprites {
                self.exiting.push(ExitingSprite {
                    sprite,
                    transition: transition.clone(),
                    elapsed: 0.0,
                    duration,
                });
            }
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

    /// Insert or replace a point light (matched by name).
    pub fn set_light(&mut self, light: PointLight) {
        self.lights.retain(|l| l.name != light.name);
        self.lights.push(light);
    }

    /// Remove a point light by name.
    pub fn remove_light(&mut self, name: &str) -> bool {
        let n = self.lights.len();
        self.lights.retain(|l| l.name != name);
        self.lights.len() != n
    }
}

/// Parallax factor per layer: how much the camera moves it.
/// Foreground tracks the camera fully; backgrounds lag for depth.
pub fn parallax(layer: Layer) -> f32 {
    match layer {
        Layer::Background => 0.3,
        Layer::Midground => 0.6,
        Layer::Foreground => 1.0,
        Layer::Overlay => 0.0,
    }
}

/// Light level (rgb multiplier) at a normalized scene position.
pub fn light_at(x: f32, y: f32, lights: &[PointLight], ambient: [f32; 3]) -> [f32; 3] {
    let mut c = ambient;
    for l in lights {
        let dx = x - l.x;
        let dy = y - l.y;
        let d = (dx * dx + dy * dy).sqrt();
        if l.radius > 0.0 {
            let f = (1.0 - d / l.radius).max(0.0) * l.intensity;
            c[0] += l.color[0] * f;
            c[1] += l.color[1] * f;
            c[2] += l.color[2] * f;
        }
    }
    c
}

/// A named point light in scene space (normalized 0..1 coords, y down).
/// Vertex lighting: each quad corner is lit by `ambient` plus every
/// light's falloff `intensity * (1 - dist/radius)` modulated by color.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointLight {
    pub name: String,
    pub x: f32,
    pub y: f32,
    /// Falloff radius in normalized units.
    pub radius: f32,
    /// Brightness multiplier.
    pub intensity: f32,
    pub color: [f32; 3],
}

/// Scene camera for parallax/zoom. `x`/`y` are viewpoint offsets in
/// normalized coords; nearer layers move more than far ones.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Camera {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        }
    }
}

/// Camera move in progress (`camera x=.. over=2.0`). Driven by the
/// runtime's logical clock and serialized with the scene.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CameraAnim {
    pub from: Camera,
    pub to: Camera,
    pub elapsed: f32,
    pub duration: f32,
}

impl CameraAnim {
    pub fn progress(&self) -> f32 {
        if self.duration <= 0.0 {
            1.0
        } else {
            (self.elapsed / self.duration).min(1.0)
        }
    }

    /// Interpolated camera at the current progress (smoothstep easing).
    pub fn value(&self) -> Camera {
        let t = self.progress();
        let t = t * t * (3.0 - 2.0 * t);
        Camera {
            x: self.from.x + (self.to.x - self.from.x) * t,
            y: self.from.y + (self.to.y - self.from.y) * t,
            zoom: self.from.zoom + (self.to.zoom - self.from.zoom) * t,
        }
    }
}

fn default_zoom() -> f32 {
    1.0
}

fn default_ambient() -> [f32; 3] {
    [1.0, 1.0, 1.0]
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
