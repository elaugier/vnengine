//! Headless game runtime.
//!
//! The runtime drives the script [`Vm`] and owns all mutable game state:
//! the visible [`Scene`], the dialogue history, rollback/rollforward
//! stacks and the CG gallery. It has no window, renderer, audio device or
//! filesystem access — hosts (desktop app, WASM shell, test harness)
//! consume [`RuntimeEvent`]s and apply side effects themselves.

use vnengine_core::command::Command;
use vnengine_core::model::{CameraAnim, Scene};
use vnengine_core::save::GalleryState;
pub use vnengine_core::save::{ChannelState, ChannelTrack, JournalEntry, MusicLayerState};
use vnengine_core::screen::ScreenInst;
use vnengine_core::Value;
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
    /// A `call screen` is displayed and waits for a click. Call
    /// [`Runtime::resume_screen`] with the clicked index.
    Screen { inst: ScreenInst },
    /// An `input` prompt waits for text. Call
    /// [`Runtime::resume_input`].
    Input { prompt: Option<String> },
    /// A `confirm` prompt waits for a yes/no answer. Call
    /// [`Runtime::resume_confirm`].
    Confirm { message: Option<String> },
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
    plugin_kv: std::collections::BTreeMap<String, String>,
    music_layers: std::collections::BTreeMap<String, MusicLayerState>,
    screens: Vec<ScreenInst>,
    nvl_start: usize,
    window_mode: String,
    channels: std::collections::BTreeMap<String, ChannelState>,
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
    /// Plugin key/value state (persisted with the game).
    pub plugin_kv: std::collections::BTreeMap<String, String>,
    /// Adaptive-music stems playing at save time.
    pub music_layers: std::collections::BTreeMap<String, MusicLayerState>,
    /// Declarative screens shown at save time (`show screen`).
    pub screens: Vec<ScreenInst>,
    /// Index into `history` where the current NVL page starts.
    pub nvl_start: usize,
    /// `window` mode at save time: `show`, `hide` or `auto`.
    pub window_mode: String,
    /// Named audio channels at save time (`play/queue channel`).
    pub channels: std::collections::BTreeMap<String, ChannelState>,
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
    /// Fade duration of the most recent `music layer` command.
    pub music_layer_fade: f32,
    /// Player inputs recorded for deterministic replay.
    pub journal: Vec<JournalEntry>,
    /// Shared plugin key/value state. The host passes it to plugin
    /// calls (`PluginHost::tick_all` etc.); it is part of saves and
    /// rollback so plugin state restores with the game.
    pub plugin_kv: std::collections::BTreeMap<String, String>,
    /// Adaptive-music stems keyed by layer name. Tracked through
    /// snapshots/rollback like `music`; the host resyncs audio from it.
    pub music_layers: std::collections::BTreeMap<String, MusicLayerState>,
    /// Remaining time of a `pause` command, driven by
    /// [`Runtime::update`] — the runtime's logical clock, not wall time.
    pause_remaining: Option<f32>,
    /// Declarative screens currently shown (`show screen`), in draw
    /// order. The host renders them on top of the dialogue UI.
    pub screens: Vec<ScreenInst>,
    /// Index into `history` where the current NVL page starts —
    /// `nvl clear` sets it to `history.len()`. NVL-mode `say` lines
    /// accumulate on the page; the host renders
    /// `history[nvl_start..]` for `nvl` dialogue.
    pub nvl_start: usize,
    /// Dialogue window mode from `window show|hide|auto`. The host
    /// hides the textbox when this is `hide`.
    pub window_mode: String,
    /// Named audio channels (`play/queue/stop/volume/filter channel`),
    /// tracked through snapshots/rollback like `music`. The host diffs
    /// this against its audio engine and calls `channel_advance` when a
    /// channel's current track finishes.
    pub channels: std::collections::BTreeMap<String, ChannelState>,
    /// Achievements granted via `grant`, kept OUTSIDE snapshots/saves —
    /// persisted in the host's persistent store (Ren'Py `persistent`
    /// semantics). Mirrored into `vm.state.variables["achievements"]`
    /// as a `List` so scripts can test `"x" in achievements`.
    pub achievements: std::collections::BTreeSet<String>,
    /// Last-flushed copy of the `persistent` variable. The live value
    /// scripts mutate is `vm.state.variables["persistent"]`; the host
    /// diffs via `persistent_dirty`/`mark_persistent_clean`.
    persistent: Value,
    rollback_stack: Vec<RollbackSnapshot>,
    rollforward_stack: Vec<RollbackSnapshot>,
    /// True while re-executing a restored snapshot. Suppresses clearing
    /// the rollforward stack (a replayed push is not a new player action).
    replaying: bool,
}

impl Runtime {
    /// Create a runtime at the start of the script.
    pub fn new(script: vnengine_script::ast::Script) -> Self {
        let mut rt = Self {
            vm: Vm::new(script),
            scene: Scene {
                bg_opacity: 1.0,
                ..Default::default()
            },
            history: Vec::new(),
            gallery: GalleryState::default(),
            music: None,
            music_fade: 1.0,
            music_layer_fade: 1.0,
            journal: Vec::new(),
            plugin_kv: std::collections::BTreeMap::new(),
            music_layers: std::collections::BTreeMap::new(),
            pause_remaining: None,
            screens: Vec::new(),
            nvl_start: 0,
            window_mode: "auto".to_string(),
            channels: std::collections::BTreeMap::new(),
            achievements: std::collections::BTreeSet::new(),
            persistent: Value::Dict(Default::default()),
            rollback_stack: Vec::new(),
            rollforward_stack: Vec::new(),
            replaying: false,
        };
        rt.sync_persistent_vars();
        rt
    }

    /// Rebuild a runtime from a saved snapshot.
    pub fn restore(script: vnengine_script::ast::Script, snap: RuntimeSnapshot) -> Self {
        let mut rt = Self::new(script);
        rt.vm.state = snap.vm_state;
        rt.scene = snap.scene;
        rt.music = snap.music;
        rt.journal = snap.journal;
        rt.plugin_kv = snap.plugin_kv;
        rt.music_layers = snap.music_layers;
        rt.screens = snap.screens;
        rt.nvl_start = snap.nvl_start;
        rt.window_mode = snap.window_mode;
        rt.channels = snap.channels;
        rt
    }

    // ---- persistent store + achievements -----------------------------

    /// Install the host's persistent data: the `persistent` variable
    /// (a `Dict` scripts mutate through `$ persistent.x = ...`) and the
    /// granted-achievement set. Both are mirrored into VM variables so
    /// scripts use ordinary dict/list access; rollback never reverts
    /// them (they are re-injected after every snapshot restore).
    pub fn set_persistent(
        &mut self,
        vars: std::collections::BTreeMap<String, Value>,
        achievements: std::collections::BTreeSet<String>,
    ) {
        self.persistent = Value::Dict(vars);
        self.achievements = achievements;
        self.sync_persistent_vars();
    }

    /// Inject `persistent` + `achievements` into the live VM variables.
    /// Called after snapshot restores (which carry the stale saved copy)
    /// and by `set_persistent`.
    fn sync_persistent_vars(&mut self) {
        self.vm
            .state
            .variables
            .insert("persistent".into(), self.persistent.clone());
        let achieved: Vec<Value> = self
            .achievements
            .iter()
            .map(|a| Value::Str(a.clone()))
            .collect();
        self.vm
            .state
            .variables
            .insert("achievements".into(), Value::List(achieved));
    }

    /// The live `persistent` dict as scripts currently see it.
    pub fn persistent(&self) -> &Value {
        self.vm
            .state
            .variables
            .get("persistent")
            .unwrap_or(&self.persistent)
    }

    /// Whether the live `persistent` var differs from the last-flushed
    /// copy (host uses this to decide when to write the store file).
    pub fn persistent_dirty(&self) -> bool {
        self.persistent() != &self.persistent
    }

    /// Mark the live store as flushed to disk.
    pub fn mark_persistent_clean(&mut self) {
        self.persistent = self.persistent().clone();
    }

    /// Record a `grant` achievement; returns `true` on first grant
    /// (host shows a toast). The achievement set is mirrored into
    /// `variables["achievements"]` for `in`/`has` checks.
    pub fn grant_achievement(&mut self, name: &str) -> bool {
        let first = self.achievements.insert(name.to_string());
        if first {
            self.sync_persistent_vars();
        }
        first
    }

    /// Whether an achievement has been granted.
    pub fn achieved(&self, name: &str) -> bool {
        self.achievements.contains(name)
    }

    /// Capture the current runtime state for saving.
    pub fn capture(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            vm_state: self.vm.save_state(),
            scene: self.scene.clone(),
            music: self.music.clone(),
            journal: self.journal.clone(),
            plugin_kv: self.plugin_kv.clone(),
            music_layers: self.music_layers.clone(),
            screens: self.screens.clone(),
            nvl_start: self.nvl_start,
            window_mode: self.window_mode.clone(),
            channels: self.channels.clone(),
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
        if let Some(p) = &self.vm.state.pending_screen {
            return RuntimeEvent::Screen {
                inst: p.inst.clone(),
            };
        }
        if let Some(p) = &self.vm.state.pending_input {
            return RuntimeEvent::Input {
                prompt: p.prompt.clone(),
            };
        }
        if let Some(p) = &self.vm.state.pending_confirm {
            return RuntimeEvent::Confirm {
                message: p.message.clone(),
            };
        }
        self.step()
    }

    /// Whether the VM is currently waiting on a menu selection.
    pub fn is_on_menu(&self) -> bool {
        self.vm.state.pending_menu.is_some()
    }

    /// Hot-reload: swap the script while keeping the current execution
    /// point. Label frames resolve against the new script by name;
    /// inline frames (menu bodies, loop bodies, call frames) keep their
    /// embedded statements, matching Ren'Py-style "reload continues
    /// where you were". Defaults are not re-applied to existing
    /// variables.
    pub fn reload_script(&mut self, script: vnengine_script::ast::Script) {
        let state = self.vm.save_state();
        self.vm = Vm::restore(script, state);
    }

    /// Studio/debug: jump execution to `label`, clearing any pending
    /// pause. Variables, scene and history are preserved — the jump is
    /// a development action, not recorded in the replay journal.
    /// Returns false when the label does not exist.
    pub fn jump_to_label(&mut self, label: &str) -> bool {
        if !self.vm.jump_to_label(label) {
            return false;
        }
        self.pause_remaining = None;
        true
    }

    /// Resume a menu selection. The host should call [`Runtime::step`]
    /// afterwards to continue execution.
    pub fn resume_menu(&mut self, index: usize) {
        self.journal.push(JournalEntry::Menu(index));
        self.vm.resume(Some(MenuResponse::Selected(index)));
    }

    /// Resume a `call screen` with the clicked clickable index. The
    /// click is journaled for deterministic replay. Variable and flow
    /// actions (`set`/`return`/`jump`) are applied by the VM; screen
    /// effects (`show screen`/`hide screen`) update `self.screens`.
    /// The host should call [`Runtime::step`] when
    /// [`Runtime::is_on_screen`] becomes false.
    pub fn resume_screen(&mut self, index: usize) {
        self.journal.push(JournalEntry::Screen(index));
        let pending_name = self
            .vm
            .state
            .pending_screen
            .as_ref()
            .map(|p| p.inst.name.clone());
        for effect in self.vm.resume_screen(index) {
            self.apply_screen_action(&effect);
        }
        // If the click dismissed the `call screen`, drop it from the
        // shown screens too (it was inserted when the event fired).
        if !self.vm.is_on_screen() {
            if let Some(name) = pending_name {
                self.screens.retain(|s| s.name != name);
            }
        }
    }

    /// Apply a `show screen`/`hide screen` action to `self.screens`.
    fn apply_screen_action(&mut self, action: &vnengine_core::screen::ScreenAction) {
        use vnengine_core::screen::ScreenAction;
        match action {
            ScreenAction::ShowScreen(name) => {
                // Re-resolve in the current context so the new screen
                // reflects up-to-date variables; the raw action exprs
                // register for click-time evaluation.
                match self.vm.resolve_screen(name) {
                    Ok((inst, raw)) => {
                        self.vm
                            .state
                            .shown_screen_actions
                            .insert(inst.name.clone(), raw);
                        self.screens.retain(|s| s.name != inst.name);
                        self.screens.push(inst);
                    }
                    Err(e) => log::error!("screen `{name}` could not be resolved: {e}"),
                }
            }
            ScreenAction::HideScreen(name) => {
                self.screens.retain(|s| &s.name != name);
                self.vm.state.shown_screen_actions.remove(name);
            }
            _ => {}
        }
    }

    /// Click on an ambient (non-blocking `show screen`) button. The
    /// VM evaluates the node's raw actions at click time
    /// (`set`/`jump`/`return`/`show screen`/`hide screen`). Returns
    /// true if a `jump` fired and the host should resume stepping.
    pub fn screen_action(&mut self, screen_idx: usize, node_idx: usize) -> bool {
        let Some(name) = self.screens.get(screen_idx).map(|s| s.name.clone()) else {
            return false;
        };
        let (effects, jumped) = self.vm.apply_screen_actions(&name, node_idx);
        for effect in effects {
            self.apply_screen_action(&effect);
        }
        jumped
    }

    /// Whether a `call screen` is still awaiting a dismissing action.
    pub fn is_on_screen(&self) -> bool {
        self.vm.is_on_screen()
    }

    /// Resume an `input` prompt with the entered text.
    pub fn resume_input(&mut self, text: String) {
        self.journal.push(JournalEntry::Input(text.clone()));
        self.vm.resume_input(text);
    }

    /// Resume a `confirm` prompt with the answer (`true` = yes).
    pub fn resume_confirm(&mut self, yes: bool) {
        self.journal.push(JournalEntry::Confirm(yes));
        self.vm.resume_confirm(yes);
    }

    /// Advance the runtime's logical clock by `dt` seconds. If a `pause`
    /// command is in progress and expires, execution continues and the
    /// resulting event is returned.
    pub fn update(&mut self, dt: f32) -> Option<RuntimeEvent> {
        // Advance sprite enter/exit transitions on the logical clock;
        // finished exits leave the model (and the save) entirely.
        for e in &mut self.scene.exiting {
            e.elapsed += dt;
            // `on hide` ATL tracks keep the exiting sprite alive until
            // they finish (an infinite `repeat` keeps it forever).
            let mut anims = std::mem::take(&mut e.sprite.anims);
            for t in &mut anims {
                t.tick(dt, &mut e.sprite);
            }
            anims.retain(|t| !t.finished());
            e.sprite.anims = anims;
        }
        self.scene
            .exiting
            .retain(|e| e.progress() < 1.0 || !e.sprite.anims.is_empty());
        // Layer background fade-ins on the same clock.
        for bg in self.scene.layer_bgs.values_mut() {
            if let Some(fade) = &mut bg.fade {
                fade.elapsed += dt;
                bg.opacity = (fade.elapsed / fade.duration).min(1.0);
                if bg.opacity >= 1.0 {
                    bg.fade = None;
                }
            }
        }
        // Camera move on the same logical clock.
        if let Some(anim) = &mut self.scene.camera_anim {
            anim.elapsed += dt;
            self.scene.camera = anim.value();
            if anim.progress() >= 1.0 {
                self.scene.camera_anim = None;
            }
        }
        for s in &mut self.scene.sprites {
            if let Some(anim) = &mut s.entering {
                anim.elapsed += dt;
                if anim.progress() >= 1.0 {
                    s.entering = None;
                }
            }
            // ATL tracks (`show ... at <transform>`): same logical
            // clock; finished tracks drop out.
            let mut anims = std::mem::take(&mut s.anims);
            for t in &mut anims {
                t.tick(dt, s);
            }
            anims.retain(|t| !t.finished());
            s.anims = anims;
        }

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
                RuntimeEvent::Screen { ref inst } => {
                    let Some(&JournalEntry::Screen(click)) = journal.get(ji) else {
                        return Err(format!("journal exhausted at screen #{ji}"));
                    };
                    if click >= inst.clickables().len() {
                        return Err(format!("journal entry #{ji} is out of range"));
                    }
                    ji += 1;
                    rt.resume_screen(click);
                }
                RuntimeEvent::Input { .. } => {
                    let Some(JournalEntry::Input(text)) = journal.get(ji) else {
                        return Err(format!("journal exhausted at input #{ji}"));
                    };
                    ji += 1;
                    rt.resume_input(text.clone());
                }
                RuntimeEvent::Confirm { .. } => {
                    let Some(&JournalEntry::Confirm(yes)) = journal.get(ji) else {
                        return Err(format!("journal exhausted at confirm #{ji}"));
                    };
                    ji += 1;
                    rt.resume_confirm(yes);
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
        // The live persistent vars survive restores — snapshots hold
        // the stale save-time copies.
        let persistent = self.persistent().clone();
        let achievements = Value::List(
            self.achievements
                .iter()
                .map(|a| Value::Str(a.clone()))
                .collect(),
        );
        self.vm.state = snap.vm_state;
        self.vm
            .state
            .variables
            .insert("persistent".into(), persistent);
        self.vm
            .state
            .variables
            .insert("achievements".into(), achievements);
        self.scene = snap.scene;
        self.history.truncate(snap.history_len);
        self.music = snap.music;
        self.plugin_kv = snap.plugin_kv;
        self.music_layers = snap.music_layers;
        self.screens = snap.screens;
        self.nvl_start = snap.nvl_start;
        self.window_mode = snap.window_mode;
        self.channels = snap.channels;
        self.replaying = true;
        let event = self.step();
        self.replaying = false;
        event
    }

    /// Advance a named channel's queue: called by the host when the
    /// channel's current track finishes playing. Returns the track that
    /// should start now, or `None` if the channel went idle (its state
    /// is then dropped, unless a gain/effect was explicitly configured).
    pub fn channel_advance(&mut self, channel: &str) -> Option<ChannelTrack> {
        let st = self.channels.get_mut(channel)?;
        let next = st.queue.first().cloned();
        match next {
            Some(t) => {
                st.queue.remove(0);
                st.current = Some(t.clone());
                Some(t)
            }
            None => {
                st.current = None;
                if st.queue.is_empty() && st.filter.is_none() && st.volume == 1.0 {
                    self.channels.remove(channel);
                }
                None
            }
        }
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
            plugin_kv: self.plugin_kv.clone(),
            music_layers: self.music_layers.clone(),
            screens: self.screens.clone(),
            nvl_start: self.nvl_start,
            window_mode: self.window_mode.clone(),
            channels: self.channels.clone(),
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
                    Command::Scene {
                        image,
                        transition,
                        layer,
                        layers,
                    } => {
                        // Ren'Py `scene` semantics: it resets a layer —
                        // sprites of the target layer exit (with the
                        // scene transition) and the background changes.
                        // Without `onlayer` the default stage is reset:
                        // the background swaps and foreground sprites
                        // (where characters live) exit.
                        let layer = layer.unwrap_or(vnengine_core::model::Layer::Background);
                        let sprite_layer = if layer == vnengine_core::model::Layer::Background {
                            vnengine_core::model::Layer::Foreground
                        } else {
                            layer
                        };
                        self.scene
                            .hide_sprites_on_layer(sprite_layer, transition.clone());
                        if layer == vnengine_core::model::Layer::Background {
                            let duration = transition.default_duration();
                            if duration > 0.0 && self.scene.background.is_some() {
                                self.scene.prev_background = self.scene.background.take();
                                self.scene.prev_bg_opacity = 1.0;
                                self.scene.bg_opacity = 0.0;
                            } else {
                                self.scene.bg_opacity = 1.0;
                            }
                            self.scene.background = image.clone();
                            self.scene.bg_layers = layers;
                        } else {
                            let duration = transition.default_duration();
                            self.scene.layer_bgs.insert(
                                layer as u8,
                                vnengine_core::model::LayerBg {
                                    image: image.clone().unwrap_or_default(),
                                    opacity: if duration > 0.0 { 0.0 } else { 1.0 },
                                    zoom: 1.0,
                                    layers,
                                    fade: if duration > 0.0 {
                                        Some(vnengine_core::model::LayerFade {
                                            elapsed: 0.0,
                                            duration,
                                        })
                                    } else {
                                        None
                                    },
                                },
                            );
                        }
                        return RuntimeEvent::Effect(Command::Scene {
                            image: self.scene.background.clone(),
                            transition,
                            layer: Some(layer),
                            layers: Vec::new(),
                        });
                    }
                    Command::Show(spec) => {
                        self.scene.set_sprite(&spec);
                    }
                    Command::Hide {
                        character,
                        transition,
                    } => {
                        self.scene.hide_sprite(&character, transition);
                    }
                    Command::HideAll { transition } => {
                        self.scene.hide_all_sprites(transition);
                    }
                    Command::Say {
                        speaker,
                        text,
                        voice,
                        side_image,
                        nvl,
                    } => {
                        let cmd = Command::Say {
                            speaker: speaker.clone(),
                            text: text.clone(),
                            voice,
                            side_image,
                            nvl,
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
                        // A new base track resets the stem layers.
                        self.music_layers.clear();
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::StopMusic { fade } => {
                        self.music = None;
                        self.music_fade = fade;
                        self.music_layers.clear();
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::MusicLayer {
                        ref name,
                        ref track,
                        volume,
                        fade,
                    } => {
                        match self.music_layers.get_mut(name) {
                            Some(l) => {
                                if let Some(t) = track {
                                    l.track = t.clone();
                                }
                                if let Some(v) = volume {
                                    l.volume = v;
                                }
                            }
                            None => {
                                if let Some(t) = track {
                                    self.music_layers.insert(
                                        name.clone(),
                                        MusicLayerState {
                                            track: t.clone(),
                                            volume: volume.unwrap_or(1.0),
                                        },
                                    );
                                }
                            }
                        }
                        self.music_layer_fade = fade;
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::MusicLayerOff { ref name, fade } => {
                        self.music_layers.remove(name);
                        self.music_layer_fade = fade;
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::PlayChannel {
                        ref channel,
                        ref track,
                        volume,
                        fade,
                        loop_,
                    } => {
                        self.channels.insert(
                            channel.clone(),
                            ChannelState {
                                current: Some(ChannelTrack {
                                    track: track.clone(),
                                    volume,
                                    fade,
                                    loop_,
                                }),
                                queue: Vec::new(),
                                ..self
                                    .channels
                                    .get(channel.as_str())
                                    .cloned()
                                    .unwrap_or_default()
                            },
                        );
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::QueueChannel {
                        ref channel,
                        ref track,
                        volume,
                        fade,
                        loop_,
                    } => {
                        let ct = ChannelTrack {
                            track: track.clone(),
                            volume,
                            fade,
                            loop_,
                        };
                        let st = self.channels.entry(channel.clone()).or_default();
                        if st.current.is_none() {
                            st.current = Some(ct);
                        } else {
                            st.queue.push(ct);
                        }
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::StopChannel { ref channel, .. } => {
                        self.channels.remove(channel);
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::VolumeChannel {
                        ref channel,
                        volume,
                        ..
                    } => {
                        self.channels.entry(channel.clone()).or_default().volume = volume;
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::FilterChannel {
                        ref channel,
                        ref filter,
                    } => {
                        self.channels.entry(channel.clone()).or_default().filter =
                            Some(filter.clone());
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::StopAll { .. } => {
                        self.channels.clear();
                        self.music = None;
                        self.music_layers.clear();
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::Grant { ref name } => {
                        self.grant_achievement(name);
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::LightSet(light) => {
                        self.scene.set_light(light);
                    }
                    Command::LightRemove { name } => {
                        self.scene.remove_light(&name);
                    }
                    Command::Ambient { color } => {
                        self.scene.ambient = color;
                    }
                    Command::Camera {
                        x,
                        y,
                        zoom,
                        duration,
                    } => {
                        let mut to = self.scene.camera;
                        if let Some(v) = x {
                            to.x = v;
                        }
                        if let Some(v) = y {
                            to.y = v;
                        }
                        if let Some(v) = zoom {
                            to.zoom = v;
                        }
                        if duration > 0.0 {
                            self.scene.camera_anim = Some(CameraAnim {
                                from: self.scene.camera,
                                to,
                                elapsed: 0.0,
                                duration,
                            });
                        } else {
                            self.scene.camera = to;
                            self.scene.camera_anim = None;
                        }
                    }
                    Command::PlaySfx { .. }
                    | Command::PlayVoice { .. }
                    | Command::StopVoice
                    | Command::PlayTransition(_)
                    | Command::Shake { .. }
                    | Command::PluginCall { .. } => {
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::ShowScreen(ref inst) => {
                        // Replaces a screen of the same name.
                        self.screens.retain(|s| s.name != inst.name);
                        self.screens.push(inst.clone());
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::HideScreen { ref name } => {
                        self.screens.retain(|s| &s.name != name);
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::Window { ref mode } => {
                        self.window_mode = mode.clone();
                        return RuntimeEvent::Effect(cmd);
                    }
                    Command::NvlClear => {
                        // New NVL page starts after the current history.
                        self.nvl_start = self.history.len();
                    }
                    Command::Pause { seconds } => {
                        self.pause_remaining = Some(seconds);
                        self.push_rollback(pre_state);
                        return RuntimeEvent::Blocking(cmd);
                    }
                    Command::WaitInput => {
                        // `pause` with no duration: block until the player
                        // advances, like a dialogue line with no text.
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
                                sprite.scale = [s, s];
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
                StepResult::Screen { inst } => {
                    // A `call screen` also shows its inst while pending
                    // so the host renders it like a shown screen.
                    self.screens.retain(|s| s.name != inst.name);
                    self.screens.push(inst.clone());
                    self.push_rollback(pre_state);
                    return RuntimeEvent::Screen { inst };
                }
                StepResult::Input { prompt } => {
                    self.push_rollback(pre_state);
                    return RuntimeEvent::Input { prompt };
                }
                StepResult::Confirm { message } => {
                    self.push_rollback(pre_state);
                    return RuntimeEvent::Confirm { message };
                }
                StepResult::Finished => return RuntimeEvent::Finished,
            }
        }
    }
}

#[cfg(test)]
mod tests;
