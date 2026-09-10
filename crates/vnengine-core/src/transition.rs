//! Transition effects used between scenes and when showing/hiding sprites.

use serde::{Deserialize, Serialize};

/// A visual transition between two frames of the scene.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Transition {
    /// Instant cut.
    Cut,
    /// Fade to/from a solid color.
    Fade { color: [f32; 3], duration: f32 },
    /// Cross-fade (dissolve) between old and new frame.
    Dissolve { duration: f32 },
    /// Slide the new frame in from a direction.
    Slide { direction: SlideDirection, duration: f32 },
    /// Zoom/scale the new frame in.
    Zoom { duration: f32 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum SlideDirection {
    Left,
    Right,
    Up,
    Down,
}

impl Default for Transition {
    fn default() -> Self {
        Transition::Dissolve { duration: 0.5 }
    }
}

impl Transition {
    /// Default duration used when none is specified by the script.
    pub fn default_duration(&self) -> f32 {
        match self {
            Transition::Cut => 0.0,
            Transition::Fade { duration, .. } => *duration,
            Transition::Dissolve { duration } => *duration,
            Transition::Slide { duration, .. } => *duration,
            Transition::Zoom { duration } => *duration,
        }
    }

    /// Parse a transition name (optionally with a duration) from script text.
    pub fn parse(name: &str, duration: Option<f32>) -> Option<Transition> {
        let duration = duration.unwrap_or(0.5);
        match name.trim().to_ascii_lowercase().as_str() {
            "cut" => Some(Transition::Cut),
            "fade" | "fadeblack" => Some(Transition::Fade {
                color: [0.0, 0.0, 0.0],
                duration,
            }),
            "fadewhite" => Some(Transition::Fade {
                color: [1.0, 1.0, 1.0],
                duration,
            }),
            "dissolve" => Some(Transition::Dissolve { duration }),
            "slideleft" => Some(Transition::Slide {
                direction: SlideDirection::Left,
                duration,
            }),
            "slideright" => Some(Transition::Slide {
                direction: SlideDirection::Right,
                duration,
            }),
            "slideup" => Some(Transition::Slide {
                direction: SlideDirection::Up,
                duration,
            }),
            "slidedown" => Some(Transition::Slide {
                direction: SlideDirection::Down,
                duration,
            }),
            "zoom" => Some(Transition::Zoom { duration }),
            _ => None,
        }
    }
}
