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
    Slide {
        direction: SlideDirection,
        duration: f32,
    },
    /// Zoom/scale the new frame in.
    Zoom { duration: f32 },
    /// Push the old frame out while the new one slides in.
    Push {
        direction: SlideDirection,
        duration: f32,
    },
    /// Reveal the new frame through shrinking strips.
    Blinds {
        /// `true` = vertical strips, `false` = horizontal.
        vertical: bool,
        /// Number of strips.
        strips: u32,
        duration: f32,
    },
    /// Reveal through an expanding (`inward = false`) or closing
    /// (`inward = true`) iris.
    Iris { inward: bool, duration: f32 },
    /// The frame resolves from coarse pixels to full resolution.
    Pixelate { duration: f32 },
    /// The new frame appears through a grid of squares revealed in
    /// random-ish order.
    Squares {
        /// Grid cells per axis (approximately).
        cells: u32,
        duration: f32,
    },
}

impl Transition {
    /// Whether the transition is driven by a fragment mask rather than
    /// by animating scene fields (opacity/offsets). Masked kinds render
    /// through the shader mask keyed by [`Transition::mask_kind`].
    pub fn is_masked(&self) -> bool {
        self.mask_kind() != 0
    }

    /// Shader mask identifier: 0 = none, 1 = vertical blinds,
    /// 2 = horizontal blinds, 3 = iris in, 4 = iris out, 5 = pixelate,
    /// 6 = squares.
    pub fn mask_kind(&self) -> u32 {
        match self {
            Transition::Blinds { vertical, .. } => {
                if *vertical {
                    1
                } else {
                    2
                }
            }
            Transition::Iris { inward, .. } => {
                if *inward {
                    3
                } else {
                    4
                }
            }
            Transition::Pixelate { .. } => 5,
            Transition::Squares { .. } => 6,
            _ => 0,
        }
    }

    /// Extra parameter passed to the mask shader (`p0`): strip count
    /// for blinds, cell count for squares, 0 otherwise.
    pub fn mask_param(&self) -> f32 {
        match self {
            Transition::Blinds { strips, .. } => *strips as f32,
            Transition::Squares { cells, .. } => *cells as f32,
            _ => 0.0,
        }
    }
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
            Transition::Push { duration, .. } => *duration,
            Transition::Blinds { duration, .. } => *duration,
            Transition::Iris { duration, .. } => *duration,
            Transition::Pixelate { duration } => *duration,
            Transition::Squares { duration, .. } => *duration,
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
            "pushleft" => Some(Transition::Push {
                direction: SlideDirection::Left,
                duration,
            }),
            "pushright" => Some(Transition::Push {
                direction: SlideDirection::Right,
                duration,
            }),
            "pushup" => Some(Transition::Push {
                direction: SlideDirection::Up,
                duration,
            }),
            "pushdown" => Some(Transition::Push {
                direction: SlideDirection::Down,
                duration,
            }),
            "blinds" | "blindsv" => Some(Transition::Blinds {
                vertical: true,
                strips: 10,
                duration,
            }),
            "blindsh" => Some(Transition::Blinds {
                vertical: false,
                strips: 10,
                duration,
            }),
            "irisin" => Some(Transition::Iris {
                inward: true,
                duration,
            }),
            "irisout" => Some(Transition::Iris {
                inward: false,
                duration,
            }),
            "pixelate" => Some(Transition::Pixelate { duration }),
            "squares" => Some(Transition::Squares {
                cells: 12,
                duration,
            }),
            _ => None,
        }
    }
}
