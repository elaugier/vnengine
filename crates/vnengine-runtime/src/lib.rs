//! Headless game runtime.
//!
//! The runtime drives the script [`Vm`] and owns all mutable game state:
//! the visible [`Scene`], the dialogue history, rollback/rollforward
//! stacks and the CG gallery. It has no window, renderer, audio device or
//! filesystem access — hosts (desktop app, WASM shell, test harness)
//! consume [`RuntimeEvent`]s and apply side effects themselves.

use vnengine_core::command::Command;
use vnengine_core::model::Scene;
use vnengine_core::save::GalleryState;
pub use vnengine_core::save::JournalEntry;
use vnengine_script::vm::{MenuResponse, StepResult, Vm, VmState};

/// Maximum number of rollback snapshots kept.
const MAX_ROLLBACK: usize = 50;

/// A single line in the dialogue backlog.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub speaker: Option<String>,
    pub text: String,
}

/// What the host should do after a [`Runtime::step`].
#[derive(Debug)]
pub enum RuntimeEvent {
    /// A non-blocking side effect the host must apply: audio playback,
    /// transitions, screen shake, CG persistence. Scene-related commands
    /// have already been applied to [`Runtime::scene`].
    Effect(Command),
    /// The runtime is blocked on user-visible content (dialogue line,
    /// pause, end). Call [`Runtime::advance`] when the player continues.
    Blocking(Command),
    /// A choice menu is displayed. Call [`Runtime::select`].
    Menu {
        prompt: Option<String>,
        choices: Vec<String>,
    },
    /// The script finished.
    Finished,
}

/// A saved position for rollback: the VM state *before* the blocking
/// statement executed, so restoring and re-stepping reproduces the same
/// blocking point.
#[derive(Debug, Clone)]
struct RollbackSnapshot {
    vm_state: VmState,
    scene: Scene,
    history_len: usize,
    music: Option<String>,
}

/// Everything needed to resume a game, as stored in a save file.
#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    pub vm_state: VmState,
    pub scene: Scene,
    /// BGM track playing at save time, if any.
    pub music: Option<String>,
    /// Player inputs recorded so far (deterministic replay).
    pub journal: Vec<JournalEntry>,
}

/// The headless runtime.
pub struct Runtime {
    pub vm: Vm,
    pub scene: Scene,
    /// Dialogue backlog (backlog screen).
    pub history: Vec<HistoryEntry>,
    /// CG gallery unlock state.
    pub gallery: GalleryState,
    /// BGM track currently playing, tracked through commands, snapshots
    /// and restores. The host re-applies it via [`Runtime::music`].
    pub music: Option<String>,
    /// Fade duration of the most recent music command, in seconds.
    pub music_fade: f32,
    /// Player inputs recorded for deterministic replay.
    pub journal: Vec<JournalEntry>,
    /// Remaining time of a `pause` command, driven by
    /// [`Runtime::update`] — the runtime's logical clock, not wall time.
    pause_remaining: Option<f32>,
    rollback_stack: Vec<RollbackSnapshot>,
    rollforward_stack: Vec<RollbackSnapshot>,
    /// True while re-executing a restored snapshot. Suppresses clearing
    /// the rollforward stack (a replayed push is not a new player action).
    replaying: bool,
}

impl Runtime {
    /// Create a runtime at the start of the script.
    pub fn new(script: vnengine_script::ast::Script) -> Self {
        Self {
            vm: Vm::new(script),
            scene: Scene {
                bg_opacity: 1.0,
                ..Default::default()
            },
            history: Vec::new(),
            gallery: GalleryState::default(),
            music: None,
            music_fade: 1.0,
            journal: Vec::new(),
            pause_remaining: None,
            rollback_stack: Vec::new(),
            rollforward_stack: Vec::new(),
            replaying: false,
        }
    }

    /// Rebuild a runtime from a saved snapshot.
    pub fn restore(script: vnengine_script::ast::Script, snap: RuntimeSnapshot) -> Self {
        let mut rt = Self::new(script);
        rt.vm.state = snap.vm_state;
        rt.scene = snap.scene;
        rt.music = snap.music;
        rt.journal = snap.journal;
        rt
    }

    /// Capture the current runtime state for saving.
    pub fn capture(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            vm_state: self.vm.save_state(),
            scene: self.scene.clone(),
            music: self.music.clone(),
            journal: self.journal.clone(),
        }
    }

    /// After [`Runtime::restore`], produce the event matching the saved
    /// position: the pending menu if a menu was displayed, or the next
    /// step otherwise. The host is responsible for re-displaying the
    /// saved blocking line (stored in the save file itself).
    pub fn resume_display(&mut self) -> RuntimeEvent {
        if let Some(menu) = &self.vm.state.pending_menu {
            return RuntimeEvent::Menu {
                prompt: menu.prompt.clone(),
                choices: menu.choices.iter().map(|c| c.text.clone()).collect(),
            };
        }
        self.step()
    }

    /// Whether the VM is currently waiting on a menu selection.
    pub fn is_on_menu(&self) -> bool {
        self.vm.state.pending_menu.is_some()
    }

    /// Resume a menu selection. The host should call [`Runtime::step`]
    /// afterwards to continue execution.
    pub fn resume_menu(&mut self, index: usize) {
        self.journal.push(JournalEntry::Menu(index));
        self.vm.resume(Some(MenuResponse::Selected(index)));
    }

    /// Advance the runtime's logical clock by `dt` seconds. If a `pause`
    /// command is in progress and expires, execution continues and the
    /// resulting event is returned.
    pub fn update(&mut self, dt: f32) -> Option<RuntimeEvent> {
        if let Some(remaining) = &mut self.pause_remaining {
            *remaining -= dt;
            if *remaining <= 0.0 {
                self.pause_remaining = None;
                return Some(self.step());
            }
        }
        None
    }

    /// Whether a `pause` command is counting down.
    pub fn is_paused(&self) -> bool {
        self.pause_remaining.is_some()
    }

    /// Re-run `script` from the start, answering every menu with the
    /// choices recorded in `journal`. Errors if the journal does not
    /// cover a menu or contains extra entries. Used to verify that a
    /// session replays deterministically.
    pub fn replay(
        script: vnengine_script::ast::Script,
        journal: &[JournalEntry],
    ) -> Result<Self, String> {
        let mut rt = Self::new(script);
        let mut ji = 0;
        for _ in 0..100_000 {
            match rt.step() {
                RuntimeEvent::Menu { choices, .. } => {
                    let Some(&JournalEntry::Menu(choice)) = journal.get(ji) else {
                        return Err(format!("journal exhausted at menu #{ji}"));
                    };
                    if choice >= choices.len() {
                        return Err(format!("journal entry #{ji} is out of range"));
                    }
                    ji += 1;
                    rt.resume_menu(choice);
                }
                RuntimeEvent::Finished => break,
                RuntimeEvent::Effect(_) | RuntimeEvent::Blocking(_) => {}
            }
        }
        if ji != journal.len() {
            return Err(format!(
                "journal has {} unplayed entries",
                journal.len() - ji
            ));
        }
        Ok(rt)
    }

    /// Roll back to the previous blocking point (dialogue line or menu),
    /// Ren'Py-style. Returns the event to display, or `None` if there is
    /// nothing to roll back to.
    pub fn rollback(&mut self) -> Option<RuntimeEvent> {
        // The snapshot of the currently displayed blocking point sits on
        // top of the stack: move it to rollforward, then restore the one
        // before it (or itself again if it is the first).
        if let Some(current) = self.rollback_stack.pop() {
            self.rollforward_stack.push(current);
        }
        let snap = self
            .rollback_stack
            .pop()
            .or_else(|| self.rollforward_stack.pop())?;
        Some(self.restore_snapshot(snap))
    }

    /// Undo a rollback, re-advancing to the next blocking point.
    pub fn rollforward(&mut self) -> Option<RuntimeEvent> {
        let snap = self.rollforward_stack.pop()?;
        Some(self.restore_snapshot(snap))
    }

    /// Restore a snapshot and re-execute its blocking statement so the
    /// same line/menu is displayed again.
    fn restore_snapshot(&mut self, snap: RollbackSnapshot) -> RuntimeEvent {
        self.vm.state = snap.vm_state;
        self.scene = snap.scene;
        self.history.truncate(snap.history_len);
        self.music = snap.music;
        self.replaying = true;
        let event = self.step();
        self.replaying = false;
        event
    }

    /// Record a rollback snapshot for a blocking point. `vm_state` must
    /// point *at* the statement that produced it, so restoring replays
    /// it.
    fn push_rollback(&mut self, vm_state: VmState) {
        self.rollback_stack.push(RollbackSnapshot {
            vm_state,
            scene: self.scene.clone(),
            history_len: self.history.len(),
            music: self.music.clone(),
        });
        if self.rollback_stack.len() > MAX_ROLLBACK {
            self.rollback_stack.remove(0);
        }
        // A genuinely new player action invalidates the rollforward stack;
        // replayed pushes (from rollback/rollforward restores) must not.
        if !self.replaying {
            self.rollforward_stack.clear();
        }
    }

    /// Execute until the next blocking point or the end, applying scene
    /// state internally and yielding effects for the host.
    pub fn step(&mut self) -> RuntimeEvent {
        // Any new execution point supersedes a running pause.
        self.pause_remaining = None;
        loop {
            // Capture the state *before* the next statement executes: if
            // the statement blocks, this is exactly the position a
            // rollback must restore to re-display the same point.
            let pre_state = self.vm.save_state();
            match self.vm.step() {
                StepResult::Command(cmd) => match cmd {
                    Command::Scene { image, transition } => {
                        let duration = transition.default_duration();
                        if duration > 0.0 && self.scene.background.is_some() {
                            self.scene.prev_background = self.scene.background.take();
                            self.scene.prev_bg_opacity = 1.0;
                            self.scene.bg_opacity = 0.0;
                        } else {
                            self.scene.bg_opacity = 1.0;
                        }
                        self.scene.background = image;
                        return RuntimeEvent::Effect(Command::Scene {
                            image: self.scene.background.clone(),
                            transition,
                        });
                    }
                    Command::Show(spec) => {
                        self.scene.set_sprite(&spec);
                    }
                    Command::Hide { character, .. } => {
                        self.scene.hide_sprite(&character);
                    }
                    Command::Say {
                        speaker,
                        text,
                        voice,
                        side_image,
                    } => {
                        let cmd = Command::Say {
                            speaker: speaker.clone(),
                            text: text.clone(),
                            voice,
                            side_image,
                        };
                        self.push_rollback(pre_state);
                        self.history.push(HistoryEntry { speaker, text });
                        return RuntimeEvent::Blocking(cmd);
                    }
                    Command::Narrate { text } => {
                        let cmd = Command::Narrate { text: text.clone() };
                        self.push_rollback(pre_state);
                        self.history.push(HistoryEntry {
                            speaker: None,
                            text,
                        });
                        return RuntimeEvent::Blocking(cmd);
                    }
                    Command::Menu { .. } => {
                        // Menus come through StepResult::Menu; a bare
                        // Command::Menu is ignored.
                    }
                    Command::PlayMusic { ref track, fade } => {
                        self.music = Some(track.clone());
                        self.music_fade = fade;
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::StopMusic { fade } => {
                        self.music = None;
                        self.music_fade = fade;
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::PlaySfx { .. }
                    | Command::PlayVoice { .. }
                    | Command::StopVoice
                    | Command::PlayTransition(_)
                    | Command::Shake { .. } => {
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::Pause { seconds } => {
                        self.pause_remaining = Some(seconds);
                        self.push_rollback(pre_state);
                        return RuntimeEvent::Blocking(cmd);
                    }
                    Command::Transform {
                        character,
                        rotation,
                        scale,
                        offset,
                        opacity,
                        layer,
                    } => {
                        if let Some(sprite) = self.scene.sprite_mut(&character) {
                            if let Some(r) = rotation {
                                sprite.rotation = r;
                            }
                            if let Some(s) = scale {
                                sprite.scale = s;
                            }
                            if let Some((ox, oy)) = offset {
                                sprite.offset = [ox, oy];
                            }
                            if let Some(o) = opacity {
                                sprite.opacity = o;
                            }
                            if let Some(l) = layer {
                                sprite.layer = l;
                                self.scene.sprites.sort_by_key(|s| (s.layer, s.z));
                            }
                        }
                    }
                    Command::RegisterCg { image } => {
                        self.gallery.unlock(&image);
                        return RuntimeEvent::Effect(Command::RegisterCg { image });
                    }
                    Command::SetVariable { name, value } => {
                        self.vm.state.variables.insert(name, value);
                    }
                    Command::End => {
                        self.push_rollback(pre_state);
                        return RuntimeEvent::Finished;
                    }
                },
                StepResult::Menu { prompt, choices } => {
                    self.push_rollback(pre_state);
                    return RuntimeEvent::Menu {
                        prompt,
                        choices: choices.iter().map(|c| c.text.clone()).collect(),
                    };
                }
                StepResult::Finished => return RuntimeEvent::Finished,
            }
        }
    }
}

#[cfg(test)]
mod tests;
