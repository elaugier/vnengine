//! Input handling: keyboard, mouse, and gamepad.
//!
//! The app collects input events from winit and gilrs each frame and exposes
//! a simple query API: `advance_requested()`, `menu_choice()`, etc.

use std::collections::HashSet;

/// Logical actions the player can trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Advance dialogue / confirm menu selection.
    Advance,
    /// Go back / cancel.
    Cancel,
    /// Open the save menu.
    Save,
    /// Open the load menu.
    Load,
    /// Toggle fullscreen.
    Fullscreen,
    /// Toggle auto-advance.
    AutoAdvance,
    /// Toggle skip.
    Skip,
    /// Open settings.
    Settings,
    /// Quit the game.
    Quit,
    /// Navigate up in menus.
    Up,
    /// Navigate down in menus.
    Down,
    /// Navigate left in menus.
    Left,
    /// Navigate right in menus.
    Right,
    /// Open the gallery.
    Gallery,
    /// Go back to title screen.
    Title,
    /// Quick save.
    QuickSave,
    /// Quick load.
    QuickLoad,
    /// Open backlog (history).
    Backlog,
    /// Rollback (go back in history).
    Rollback,
    /// Rollforward.
    Rollforward,
    /// Toggle the debug/studio overlay.
    Debug,
    /// Studio overlay: select the previous label (jump target).
    DebugPrev,
    /// Studio overlay: select the next label (jump target).
    DebugNext,
    /// Studio overlay: jump execution to the selected label.
    DebugJump,
}

/// Mouse click position in logical pixels.
#[derive(Debug, Clone, Copy)]
pub struct Click {
    pub x: f32,
    pub y: f32,
}

/// Input state for the current frame.
pub struct Input {
    /// Actions triggered this frame (edge-triggered).
    pressed: HashSet<Action>,
    /// Actions currently held.
    held: HashSet<Action>,
    /// Mouse clicks this frame.
    clicks: Vec<Click>,
    /// Mouse position in logical pixels.
    mouse_pos: (f32, f32),
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    pub fn new() -> Self {
        Self {
            pressed: HashSet::new(),
            held: HashSet::new(),
            clicks: Vec::new(),
            mouse_pos: (0.0, 0.0),
        }
    }

    /// Clear per-frame state at the start of a frame.
    pub fn begin_frame(&mut self) {
        self.pressed.clear();
        self.clicks.clear();
    }

    /// Clear per-frame state at the end of a frame (after processing).
    pub fn end_frame(&mut self) {
        self.pressed.clear();
        self.clicks.clear();
    }

    /// Record a key press.
    pub fn key_press(&mut self, key: winit::keyboard::KeyCode) {
        if let Some(action) = key_to_action(key) {
            if !self.held.contains(&action) {
                self.pressed.insert(action);
            }
            self.held.insert(action);
        }
    }

    /// Record a key release.
    pub fn key_release(&mut self, key: winit::keyboard::KeyCode) {
        if let Some(action) = key_to_action(key) {
            self.held.remove(&action);
        }
    }

    /// Record a mouse click.
    pub fn mouse_click(&mut self, x: f32, y: f32) {
        self.clicks.push(Click { x, y });
    }

    /// Update mouse position.
    pub fn mouse_move(&mut self, x: f32, y: f32) {
        self.mouse_pos = (x, y);
    }

    /// Was an action pressed this frame (edge-triggered)?
    pub fn pressed(&self, action: Action) -> bool {
        self.pressed.contains(&action)
    }

    /// Is an action currently held?
    pub fn held(&self, action: Action) -> bool {
        self.held.contains(&action)
    }

    /// Was advance pressed or clicked this frame?
    pub fn advance_requested(&self) -> bool {
        self.pressed.contains(&Action::Advance) || !self.clicks.is_empty()
    }

    /// Get mouse clicks this frame.
    pub fn clicks(&self) -> &[Click] {
        &self.clicks
    }

    /// Mouse position in logical pixels.
    pub fn mouse_pos(&self) -> (f32, f32) {
        self.mouse_pos
    }
}

/// Map a physical key to a logical action.
pub fn key_to_action(key: winit::keyboard::KeyCode) -> Option<Action> {
    use winit::keyboard::KeyCode;
    match key {
        KeyCode::Space | KeyCode::Enter | KeyCode::NumpadEnter => Some(Action::Advance),
        KeyCode::Escape | KeyCode::Backspace => Some(Action::Cancel),
        KeyCode::F5 => Some(Action::QuickSave),
        KeyCode::F9 => Some(Action::QuickLoad),
        KeyCode::F6 => Some(Action::Save),
        KeyCode::F7 => Some(Action::Load),
        KeyCode::F11 => Some(Action::Fullscreen),
        KeyCode::KeyA => Some(Action::AutoAdvance),
        KeyCode::KeyS => Some(Action::Skip),
        KeyCode::F1 => Some(Action::Settings),
        KeyCode::F2 => Some(Action::Gallery),
        KeyCode::F3 => Some(Action::Backlog),
        KeyCode::F4 | KeyCode::KeyQ => Some(Action::Quit),
        KeyCode::ArrowUp => Some(Action::Up),
        KeyCode::ArrowDown => Some(Action::Down),
        KeyCode::ArrowLeft => Some(Action::Left),
        KeyCode::ArrowRight => Some(Action::Right),
        KeyCode::KeyH => Some(Action::Backlog),
        KeyCode::PageUp => Some(Action::Rollback),
        KeyCode::PageDown => Some(Action::Rollforward),
        KeyCode::F8 => Some(Action::Debug),
        KeyCode::BracketLeft => Some(Action::DebugPrev),
        KeyCode::BracketRight => Some(Action::DebugNext),
        KeyCode::F10 => Some(Action::DebugJump),
        _ => None,
    }
}
