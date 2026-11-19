//! Core data model and shared types for the vnengine visual novel engine.
//!
//! This crate contains the types shared between the script VM, the renderer,
//! the audio backend and the application loop. It deliberately has no heavy
//! dependencies so it can be reused everywhere.

pub mod assets;
pub mod atl;
pub mod command;
pub mod config;
pub mod model;
pub mod save;
pub mod screen;
pub mod settings;
pub mod transition;
pub mod value;

pub use atl::*;
pub use command::Command;
pub use config::EngineConfig;
pub use model::*;
pub use save::SaveState;
pub use screen::{ScreenAction, ScreenInst, UiNode, UiNodeKind};
pub use settings::UserSettings;
pub use transition::Transition;
pub use value::Value;

#[cfg(test)]
mod tests;
