//! ATL (Animation and Transformation Language) runtime model.
//!
//! The script VM compiles `transform` blocks into [`SpriteTrack`]s —
//! concrete sequences of [`AnimStep`]s with all expressions already
//! evaluated. Tracks live on [`crate::model::ActiveSprite`] and are
//! ticked by the runtime's logical clock, so save/load/rollback resume
//! an animation exactly where it was.

use crate::model::{anchor_x, ActiveSprite};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// An animatable sprite property. Serialized as a `u8` discriminant
/// inside track `from` maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AnimProp {
    XPos,
    YPos,
    XAlign,
    YAlign,
    Zoom,
    XZoom,
    YZoom,
    Alpha,
    Rotate,
    XOff,
    YOff,
    XAnchor,
    YAnchor,
    Z,
}

impl AnimProp {
    /// Parse a DSL property name (`xpos`, `yalign`, `rotate`...).
    pub fn parse(name: &str) -> Option<AnimProp> {
        match name {
            "xpos" => Some(AnimProp::XPos),
            "ypos" => Some(AnimProp::YPos),
            "xalign" => Some(AnimProp::XAlign),
            "yalign" => Some(AnimProp::YAlign),
            "zoom" => Some(AnimProp::Zoom),
            "xzoom" => Some(AnimProp::XZoom),
            "yzoom" => Some(AnimProp::YZoom),
            "alpha" => Some(AnimProp::Alpha),
            "rotate" => Some(AnimProp::Rotate),
            "xoffset" => Some(AnimProp::XOff),
            "yoffset" => Some(AnimProp::YOff),
            "xanchor" => Some(AnimProp::XAnchor),
            "yanchor" => Some(AnimProp::YAnchor),
            "z" => Some(AnimProp::Z),
            _ => None,
        }
    }
}

/// One step inside a [`SpriteTrack`]: a set of property targets reached
/// over `duration` seconds through `warper`.
///
/// - `duration > 0` + targets: tween.
/// - `duration == 0` + targets: instant set.
/// - No targets: wait (`pause <sec>`).
/// - `at`: `time <t>` — wait until the track's elapsed time reaches `t`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimStep {
    pub duration: f32,
    /// Warper name (`linear`, `ease`, `bounce`...). Unknown names
    /// behave as `linear`.
    pub warper: String,
    pub targets: Vec<(AnimProp, f32)>,
    /// Absolute-time sync point (`time <t>`), in seconds.
    #[serde(default)]
    pub at: Option<f32>,
}

/// A compiled, running animation track on a sprite.
///
/// `steps` is the flattened sequence from an ATL block. The track
/// walks them in order; `from` snapshots the property values at the
/// start of the current step so tweens interpolate from wherever the
/// sprite actually is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpriteTrack {
    pub steps: Vec<AnimStep>,
    /// Number of times the whole track runs (`repeat n`); `None`
    /// repeats forever. A completed iteration decrements the counter.
    #[serde(default)]
    pub repeats: Option<u32>,
    /// Index of the step currently running.
    #[serde(default)]
    pub step_idx: usize,
    /// Seconds elapsed inside the current step.
    #[serde(default)]
    pub step_elapsed: f32,
    /// Seconds elapsed since the track started (`time` sync).
    #[serde(default)]
    pub elapsed: f32,
    /// Property start values for the current step.
    #[serde(default)]
    pub from: BTreeMap<u8, f32>,
    /// Whether `from` was captured for the current step.
    #[serde(default)]
    pub primed: bool,
    /// Finished iterations so far.
    #[serde(default)]
    pub iteration: u32,
}

impl SpriteTrack {
    /// A track that plays `steps` exactly once.
    pub fn once(steps: Vec<AnimStep>) -> Self {
        Self {
            steps,
            repeats: Some(1),
            step_idx: 0,
            step_elapsed: 0.0,
            elapsed: 0.0,
            from: BTreeMap::new(),
            primed: false,
            iteration: 0,
        }
    }

    /// True when the track has played all its iterations.
    pub fn finished(&self) -> bool {
        self.steps.is_empty()
            || (self.step_idx >= self.steps.len()
                && self.repeats.map(|r| self.iteration >= r).unwrap_or(false))
    }

    /// Apply the final target values of every step (used by
    /// `reduce_motion` to flatten animations instantly).
    pub fn finish(&self, sprite: &mut ActiveSprite) {
        for step in &self.steps {
            for (prop, v) in &step.targets {
                set_prop(sprite, *prop, *v);
            }
        }
    }

    /// Advance the track by `dt` seconds, writing interpolated values
    /// into `sprite`. Steps of duration 0 apply instantly and the walk
    /// continues within the same call, so a sequence of instant sets
    /// never takes extra frames.
    pub fn tick(&mut self, dt: f32, sprite: &mut ActiveSprite) {
        self.elapsed += dt;
        let mut guard = 0usize;
        loop {
            guard += 1;
            if guard > 10_000 || self.finished() {
                break;
            }
            let step = self.steps[self.step_idx].clone();
            // `time <t>`: hold until the track reaches t.
            if let Some(at) = step.at {
                if self.elapsed < at {
                    break;
                }
            }
            if !self.primed {
                for (prop, _) in &step.targets {
                    self.from.insert(*prop as u8, get_prop(sprite, *prop));
                }
                self.primed = true;
            }
            if step.duration <= 0.0 {
                for (prop, v) in &step.targets {
                    set_prop(sprite, *prop, *v);
                }
                self.advance();
                continue;
            }
            self.step_elapsed += dt;
            let t = (self.step_elapsed / step.duration).min(1.0);
            let w = warp(&step.warper, t);
            for (prop, v) in &step.targets {
                let from = *self.from.get(&(*prop as u8)).unwrap_or(v);
                set_prop(sprite, *prop, from + (v - from) * w);
            }
            if self.step_elapsed >= step.duration {
                for (prop, v) in &step.targets {
                    set_prop(sprite, *prop, *v);
                }
                self.advance();
                continue;
            }
            break;
        }
    }

    /// Move to the next step, wrapping on `repeat`.
    fn advance(&mut self) {
        self.step_idx += 1;
        self.step_elapsed = 0.0;
        self.from.clear();
        self.primed = false;
        if self.step_idx >= self.steps.len() {
            self.iteration += 1;
            match self.repeats {
                None => self.step_idx = 0,
                Some(r) if self.iteration < r => self.step_idx = 0,
                Some(_) => {} // finished: step_idx stays >= len
            }
        }
    }
}

/// Ease a normalized time `t` through a named warper.
///
/// Supported: `linear`, `ease`/`easeio` (smoothstep), `easein`,
/// `easeout`, `bounce` (out), `elastic` (out). Unknown names fall
/// back to linear so scripts stay playable.
pub fn warp(name: &str, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match name {
        "ease" | "easeio" | "easeinout" => t * t * (3.0 - 2.0 * t),
        "easein" => t * t,
        "easeout" => 1.0 - (1.0 - t) * (1.0 - t),
        "bounce" => bounce_out(t),
        "elastic" => elastic_out(t),
        _ => t, // linear and unknowns
    }
}

fn bounce_out(t: f32) -> f32 {
    const N: f32 = 7.5625;
    const D: f32 = 2.75;
    if t < 1.0 / D {
        N * t * t
    } else if t < 2.0 / D {
        let t = t - 1.5 / D;
        N * t * t + 0.75
    } else if t < 2.5 / D {
        let t = t - 2.25 / D;
        N * t * t + 0.9375
    } else {
        let t = t - 2.625 / D;
        N * t * t + 0.984375
    }
}

fn elastic_out(t: f32) -> f32 {
    if t <= 0.0 || t >= 1.0 {
        t
    } else {
        let c = std::f32::consts::TAU / 3.0;
        2.0f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * c).sin() + 1.0
    }
}

/// Read an animatable property from a sprite.
pub fn get_prop(s: &ActiveSprite, prop: AnimProp) -> f32 {
    match prop {
        AnimProp::XPos | AnimProp::XAlign => s.position.map(|p| p.0).unwrap_or(anchor_x(s.anchor)),
        AnimProp::YPos | AnimProp::YAlign => s.position.map(|p| p.1).unwrap_or(0.5),
        AnimProp::Zoom => (s.scale[0] + s.scale[1]) * 0.5,
        AnimProp::XZoom => s.scale[0],
        AnimProp::YZoom => s.scale[1],
        AnimProp::Alpha => s.opacity,
        AnimProp::Rotate => s.rotation,
        AnimProp::XOff => s.offset[0],
        AnimProp::YOff => s.offset[1],
        AnimProp::XAnchor => s.anchor_frac.map(|a| a.0).unwrap_or(0.5),
        AnimProp::YAnchor => s.anchor_frac.map(|a| a.1).unwrap_or(0.5),
        AnimProp::Z => s.z as f32,
    }
}

/// Write an animatable property on a sprite.
pub fn set_prop(s: &mut ActiveSprite, prop: AnimProp, v: f32) {
    match prop {
        AnimProp::XPos | AnimProp::XAlign => {
            let y = s.position.map(|p| p.1).unwrap_or(0.5);
            s.position = Some((v, y));
        }
        AnimProp::YPos | AnimProp::YAlign => {
            let x = s.position.map(|p| p.0).unwrap_or(anchor_x(s.anchor));
            s.position = Some((x, v));
        }
        AnimProp::Zoom => s.scale = [v, v],
        AnimProp::XZoom => s.scale[0] = v,
        AnimProp::YZoom => s.scale[1] = v,
        AnimProp::Alpha => s.opacity = v,
        // DSL convention: radians, like `transform <c> rotation=`.
        AnimProp::Rotate => s.rotation = v,
        AnimProp::XOff => s.offset[0] = v,
        AnimProp::YOff => s.offset[1] = v,
        AnimProp::XAnchor => {
            let y = s.anchor_frac.map(|a| a.1).unwrap_or(0.5);
            s.anchor_frac = Some((v, y));
        }
        AnimProp::YAnchor => {
            let x = s.anchor_frac.map(|a| a.0).unwrap_or(0.5);
            s.anchor_frac = Some((x, v));
        }
        AnimProp::Z => s.z = v as i32,
    }
}
