//! Main application: winit event loop, wgpu init, VM driving, scene updates,
//! transitions, and UI rendering.

use crate::input::{Action, Input};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use vnengine_audio::AudioEngine;
use vnengine_core::command::{Anchor, Command};
use vnengine_core::model::Scene;
use vnengine_core::save::{GalleryState, SaveState};
use vnengine_core::settings::UserSettings;
use vnengine_core::transition::{SlideDirection, Transition};
use vnengine_core::{EngineConfig, Value};
use vnengine_render::renderer::{UiBox, UiDraw, UiText};
use vnengine_render::Renderer;
use vnengine_script::parser::Parser;
use vnengine_script::vm::{MenuResponse, StepResult, Vm};

/// The current UI screen being displayed.
#[derive(Debug, Clone, PartialEq)]
enum Screen {
    /// Normal gameplay (dialogue/narration).
    Dialogue,
    /// A menu of choices is displayed.
    Menu {
        prompt: Option<String>,
        choices: Vec<String>,
        /// Hovered choice index.
        hovered: usize,
    },
    /// The title screen.
    Title {
        /// Selected button index.
        selected: usize,
    },
    /// The save/load menu.
    SaveLoad { mode: SaveLoadMode, selected: usize },
    /// The settings menu.
    Settings { selected: usize },
    /// The CG gallery.
    Gallery { selected: usize },
    /// Game over / end screen.
    End,
    /// Backlog (history) viewer.
    Backlog { scroll: usize },
    /// Confirmation dialog.
    Confirm {
        message: String,
        selected: usize,
        /// What to do if the user confirms.
        action: ConfirmAction,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SaveLoadMode {
    Save,
    Load,
}

/// What action to take when a confirm dialog is accepted.
#[derive(Debug, Clone, PartialEq)]
enum ConfirmAction {
    /// Return to the title screen.
    ToTitle,
    /// Quit the game.
    Quit,
    /// Overwrite a save slot.
    OverwriteSave(u32),
}

/// A blocking command currently being displayed (and waiting for input).
struct BlockingState {
    /// The command that caused the block.
    command: Command,
    /// Typewriter progress: how many characters of the text are visible.
    chars_shown: usize,
    /// Whether the typewriter has finished.
    typewriter_done: bool,
    /// When the line was first shown (for auto-advance timing).
    shown_at: Instant,
    /// The full text being displayed.
    text: String,
    /// The speaker name (if any).
    speaker: Option<String>,
}

/// A transition animation in progress.
struct ActiveTransition {
    transition: Transition,
    elapsed: f32,
    duration: f32,
}

pub struct App {
    pub config: EngineConfig,
    pub project_root: PathBuf,
    pub settings: UserSettings,
    pub vm: Vm,
    pub scene: Scene,
    pub audio: AudioEngine,
    pub renderer: Renderer,
    pub input: Input,
    pub screen: Screen,
    blocking: Option<BlockingState>,
    transition: Option<ActiveTransition>,
    /// Pending menu choices from the VM (stored so we can resume).
    pending_menu: Option<Vec<String>>,
    /// Gallery state (unlocked CGs).
    gallery: GalleryState,
    /// Play time accumulator.
    play_time: f32,
    /// Last frame time.
    last_frame: Instant,
    /// Auto-advance timer.
    auto_timer: f32,
    /// Whether auto-advance is on.
    auto_advance: bool,
    /// Whether skip is on.
    skip: bool,
    /// Dialogue history for the backlog.
    history: Vec<HistoryEntry>,
    /// Rollback stack: snapshots of state before each blocking command.
    rollback_stack: Vec<RollbackSnapshot>,
    /// Rollforward stack: snapshots popped during rollback, for re-advancing.
    rollforward_stack: Vec<RollbackSnapshot>,
    /// Auto-save timer (seconds since last auto-save).
    auto_save_timer: f32,
    /// Screen shake state: (remaining_time, intensity).
    shake: Option<(f32, f32)>,
    /// Logical window size.
    window_size: (u32, u32),
}

/// A snapshot of game state for rollback.
#[derive(Debug, Clone)]
struct RollbackSnapshot {
    vm_state: vnengine_script::vm::VmState,
    scene: Scene,
    blocking_text: Option<String>,
    blocking_speaker: Option<String>,
    screen_is_dialogue: bool,
}

/// A single entry in the dialogue history (backlog).
#[derive(Debug, Clone)]
struct HistoryEntry {
    speaker: Option<String>,
    text: String,
}

/// Quick menu button actions.
#[derive(Debug, Clone, Copy, PartialEq)]
enum QuickMenuAction {
    Auto,
    Skip,
    Save,
    Load,
    Settings,
    Gallery,
    Title,
    Quit,
}

const QUICK_MENU_ACTIONS: [QuickMenuAction; 8] = [
    QuickMenuAction::Auto,
    QuickMenuAction::Skip,
    QuickMenuAction::Save,
    QuickMenuAction::Load,
    QuickMenuAction::Settings,
    QuickMenuAction::Gallery,
    QuickMenuAction::Title,
    QuickMenuAction::Quit,
];

/// Compute quick menu button positions (bottom-right of screen).
fn quick_menu_buttons(w: f32, h: f32) -> [(f32, f32, f32, f32); 8] {
    let btn_w = 60.0;
    let btn_h = 25.0;
    let gap = 5.0;
    let total_w = 8.0 * btn_w + 7.0 * gap;
    let start_x = w - total_w - 10.0;
    let y = h - btn_h - 10.0;
    let mut buttons = [(0.0, 0.0, 0.0, 0.0); 8];
    for i in 0..8 {
        buttons[i] = (start_x + i as f32 * (btn_w + gap), y, btn_w, btn_h);
    }
    buttons
}

impl App {
    /// Create and run the application. This blocks until the window closes.
    pub fn run(config: EngineConfig, project_root: PathBuf) -> anyhow::Result<()> {
        let event_loop = winit::event_loop::EventLoop::new()?;
        let title = config.title.clone();
        let (w, h) = (config.width, config.height);

        let window_attrs = winit::window::Window::default_attributes()
            .with_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64))
            .with_title(title)
            .with_fullscreen(if config.fullscreen {
                Some(winit::window::Fullscreen::Borderless(None))
            } else {
                None
            });

        let window = Arc::new(event_loop.create_window(window_attrs)?);

        // Initialize wgpu.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(event_loop.owned_display_handle()),
        ));
        let adapter = pollster::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions::default(),
        ))?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor::default(),
        ))?;
        let surface = instance.create_surface(window.clone())?;
        // Query the surface's preferred format instead of hardcoding.
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8UnormSrgb))
            .copied()
            .unwrap_or(capabilities.formats.first().copied().unwrap_or(wgpu::TextureFormat::Bgra8UnormSrgb));

        let window_size = window.inner_size();
        let renderer = Renderer::new(
            device,
            queue,
            surface,
            (window_size.width, window_size.height),
            format,
        )?;

        // Load settings.
        let settings_path = project_root.join(&config.save_dir).join("settings.json");
        let settings = UserSettings::load(&settings_path).unwrap_or_default();

        // Load script.
        let script_path = project_root.join(&config.entry_script);
        let script_src = std::fs::read_to_string(&script_path)?;
        let script = Parser::parse(&script_src)?;
        let vm = Vm::new(script);

        // Audio.
        let audio = AudioEngine::new(settings.clone())?;

        // Gallery.
        let gallery_path = project_root.join(&config.save_dir).join("gallery.json");
        let gallery = GalleryState::load(&gallery_path);

        let app = App {
            config,
            project_root,
            settings,
            vm,
            scene: Scene::default(),
            audio,
            renderer,
            input: Input::new(),
            screen: Screen::Title { selected: 0 },
            blocking: None,
            transition: None,
            pending_menu: None,
            gallery,
            play_time: 0.0,
            last_frame: Instant::now(),
            auto_timer: 0.0,
            auto_advance: false,
            skip: false,
            history: Vec::new(),
            rollback_stack: Vec::new(),
            rollforward_stack: Vec::new(),
            auto_save_timer: 0.0,
            shake: None,
            window_size: (window_size.width, window_size.height),
        };

        let app = Arc::new(Mutex::new(app));
        let window_clone = window.clone();
        let app_clone = app.clone();

        event_loop.run_app(&mut AppHandler {
            app: app_clone,
            window: window_clone,
        })?;
        Ok(())
    }

    /// Step the VM forward, processing commands until we hit a blocking one.
    fn step_vm(&mut self) {
        loop {
            match self.vm.step() {
                StepResult::Command(cmd) => {
                    if self.handle_command(cmd.clone()) {
                        // Blocking command: stop stepping.
                        return;
                    }
                    // Non-blocking: continue stepping.
                }
                StepResult::Menu { prompt, choices } => {
                    let texts: Vec<String> = choices.iter().map(|c| c.text.clone()).collect();
                    self.screen = Screen::Menu {
                        prompt,
                        choices: texts.clone(),
                        hovered: 0,
                    };
                    self.pending_menu = Some(texts);
                    return;
                }
                StepResult::Finished => {
                    self.screen = Screen::End;
                    return;
                }
            }
        }
    }

    /// Handle a command. Returns `true` if the command is blocking (requires
    /// user input before the VM can continue).
    fn handle_command(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::Scene { image, transition } => {
                // Start a transition if specified.
                let duration = transition.default_duration();
                if duration > 0.0 && self.scene.background.is_some() {
                    self.scene.prev_background = self.scene.background.take();
                    self.scene.prev_bg_opacity = 1.0;
                    self.scene.bg_opacity = 0.0;
                    self.transition = Some(ActiveTransition {
                        transition,
                        elapsed: 0.0,
                        duration,
                    });
                } else {
                    self.scene.bg_opacity = 1.0;
                }
                self.scene.background = image;
                false
            }
            Command::Show(spec) => {
                self.scene.set_sprite(&spec);
                false
            }
            Command::Hide { character, transition } => {
                let duration = transition.default_duration();
                if duration > 0.0 {
                    // For now, just hide immediately. A full implementation
                    // would fade the sprite out over `duration`.
                    self.scene.hide_sprite(&character);
                } else {
                    self.scene.hide_sprite(&character);
                }
                false
            }
            Command::Say { speaker, text, voice } => {
                if let Some(track) = &voice {
                    let voice_dir = self.project_root.join(&self.config.voice_dir);
                    self.audio.play_voice(&voice_dir, track);
                }
                self.history.push(HistoryEntry {
                    speaker: speaker.clone(),
                    text: text.clone(),
                });
                self.push_rollback_snapshot();
                self.blocking = Some(BlockingState {
                    command: Command::Say {
                        speaker: speaker.clone(),
                        text: text.clone(),
                        voice: voice.clone(),
                    },
                    chars_shown: 0,
                    typewriter_done: false,
                    shown_at: Instant::now(),
                    text,
                    speaker,
                });
                self.screen = Screen::Dialogue;
                true
            }
            Command::Narrate { ref text } => {
                let text = text.clone();
                self.history.push(HistoryEntry {
                    speaker: None,
                    text: text.clone(),
                });
                self.push_rollback_snapshot();
                self.blocking = Some(BlockingState {
                    command: Command::Narrate { text: text.clone() },
                    chars_shown: 0,
                    typewriter_done: false,
                    shown_at: Instant::now(),
                    text,
                    speaker: None,
                });
                self.screen = Screen::Dialogue;
                true
            }
            Command::Menu { .. } => {
                // Handled by StepResult::Menu in step_vm.
                false
            }
            Command::PlayMusic { track, fade } => {
                let audio_dir = self.project_root.join(&self.config.audio_dir);
                self.audio.play_music(&audio_dir, &track, fade);
                false
            }
            Command::StopMusic { fade } => {
                self.audio.stop_music(fade);
                false
            }
            Command::PlaySfx { track, volume } => {
                let audio_dir = self.project_root.join(&self.config.audio_dir);
                self.audio.play_sfx(&audio_dir, &track, volume);
                false
            }
            Command::PlayVoice { track } => {
                let voice_dir = self.project_root.join(&self.config.voice_dir);
                self.audio.play_voice(&voice_dir, &track);
                false
            }
            Command::StopVoice => {
                self.audio.stop_voice();
                false
            }
            Command::Pause { seconds } => {
                self.blocking = Some(BlockingState {
                    command: Command::Pause { seconds },
                    chars_shown: 0,
                    typewriter_done: true,
                    shown_at: Instant::now(),
                    text: String::new(),
                    speaker: None,
                });
                true
            }
            Command::PlayTransition(transition) => {
                let duration = transition.default_duration();
                self.transition = Some(ActiveTransition {
                    transition,
                    elapsed: 0.0,
                    duration,
                });
                false
            }
            Command::Shake { duration, intensity } => {
                self.shake = Some((duration, intensity));
                false
            }
            Command::Transform { character, rotation, scale, offset, opacity, layer } => {
                if let Some(sprite) = self.scene.sprite_mut(&character) {
                    if let Some(r) = rotation {
                        sprite.rotation = r;
                    }
                    if let Some(s) = scale {
                        sprite.scale = s;
                    }
                    if let Some((ox, oy)) = offset {
                        sprite.offset[0] = ox;
                        sprite.offset[1] = oy;
                    }
                    if let Some(o) = opacity {
                        sprite.opacity = o;
                    }
                    if let Some(l) = layer {
                        sprite.layer = l;
                        self.scene.sprites.sort_by_key(|s| (s.layer, s.z));
                    }
                }
                false
            }
            Command::RegisterCg { image } => {
                self.gallery.unlock(&image);
                let gallery_path = self.project_root.join(&self.config.save_dir).join("gallery.json");
                let _ = self.gallery.save(&gallery_path);
                false
            }
            Command::SetVariable { name, value } => {
                self.vm.state.variables.insert(name, value);
                false
            }
            Command::End => {
                self.screen = Screen::End;
                true
            }
        }
    }

    /// Advance past the current blocking command.
    fn advance(&mut self) {
        if self.blocking.is_none() {
            return;
        }
        let b = self.blocking.as_ref().unwrap();
        // If typewriter isn't done, complete it first.
        if !b.typewriter_done {
            if let Some(blocking) = self.blocking.as_mut() {
                blocking.chars_shown =
                    vnengine_render::richtext::visible_char_count(&blocking.text);
                blocking.typewriter_done = true;
            }
            return;
        }
        // Check if it's a Pause command.
        if let Command::Pause { seconds } = &self.blocking.as_ref().unwrap().command {
            let elapsed = b.shown_at.elapsed().as_secs_f32();
            if elapsed < *seconds {
                return; // Not enough time has passed.
            }
        }
        self.blocking = None;
        self.step_vm();
    }

    /// Select a menu choice.
    fn select_choice(&mut self, index: usize) {
        if self.pending_menu.is_some() {
            self.vm.resume(Some(MenuResponse::Selected(index)));
            self.pending_menu = None;
            self.screen = Screen::Dialogue;
            self.step_vm();
        }
    }

    /// Update the typewriter effect and auto-advance.
    fn update(&mut self, dt: f32) {
        self.play_time += dt;

        // Auto-save every 60 seconds.
        self.auto_save_timer += dt;
        if self.auto_save_timer >= 60.0 {
            self.auto_save_timer = 0.0;
            self.auto_save();
        }

        // Decay screen shake.
        if let Some((remaining, intensity)) = &mut self.shake {
            *remaining -= dt;
            if *remaining <= 0.0 {
                self.shake = None;
            }
        }

        // Update transition.
        if let Some(t) = &mut self.transition {
            t.elapsed += dt;
            let progress = (t.elapsed / t.duration).min(1.0);
            match &t.transition {
                Transition::Fade { .. } => {
                    self.scene.bg_opacity = progress;
                    self.scene.prev_bg_opacity = 1.0 - progress;
                }
                Transition::Dissolve { .. } => {
                    self.scene.bg_opacity = progress;
                    self.scene.prev_bg_opacity = 1.0 - progress;
                }
                Transition::Cut => {
                    self.scene.bg_opacity = 1.0;
                    self.scene.prev_bg_opacity = 0.0;
                }
                Transition::Slide { direction, .. } => {
                    self.scene.bg_opacity = 1.0;
                    self.scene.prev_bg_opacity = 1.0 - progress;
                    let offset = match direction {
                        SlideDirection::Left => [progress, 0.0],
                        SlideDirection::Right => [-progress, 0.0],
                        SlideDirection::Up => [0.0, progress],
                        SlideDirection::Down => [0.0, -progress],
                    };
                    // Apply offset to sprites (simplified).
                    for s in &mut self.scene.sprites {
                        s.offset = offset;
                    }
                }
                Transition::Zoom { .. } => {
                    self.scene.bg_opacity = progress;
                    self.scene.prev_bg_opacity = 1.0 - progress;
                }
            }
            if t.elapsed >= t.duration {
                self.scene.prev_background = None;
                self.scene.prev_bg_opacity = 0.0;
                self.scene.bg_opacity = 1.0;
                for s in &mut self.scene.sprites {
                    s.offset = [0.0, 0.0];
                }
                self.transition = None;
            }
        }

        // Update typewriter.
        if let Some(b) = &mut self.blocking {
            if !b.typewriter_done {
                let speed = self.settings.text_speed;
                let total_visible = vnengine_render::richtext::visible_char_count(&b.text);
                if speed <= 0.0 {
                    b.chars_shown = total_visible;
                    b.typewriter_done = true;
                } else {
                    let target = (b.shown_at.elapsed().as_secs_f32() * speed) as usize;
                    b.chars_shown = target.min(total_visible);
                    if b.chars_shown >= total_visible {
                        b.typewriter_done = true;
                    }
                }
            }
        }

        // Auto-advance.
        if self.auto_advance {
            if let Some(b) = &self.blocking {
                if b.typewriter_done {
                    self.auto_timer += dt;
                    if self.auto_timer >= self.settings.auto_advance_delay {
                        self.auto_timer = 0.0;
                        self.advance();
                    }
                }
            }
        } else {
            self.auto_timer = 0.0;
        }

        // Skip mode.
        if self.skip {
            if self.blocking.is_some() {
                self.advance();
            }
        }
    }

    /// Handle input actions.
    fn handle_input(&mut self) {
        // Global actions (work on most screens).
        if self.input.pressed(Action::Fullscreen) {
            self.settings.fullscreen = !self.settings.fullscreen;
        }

        // Handle confirm dialog first — it intercepts most input.
        if let Screen::Confirm { message, selected, action } = &self.screen.clone() {
            let message = message.clone();
            let mut selected = *selected;
            let action = action.clone();
            if self.input.pressed(Action::Up) {
                selected = selected.saturating_sub(1);
            }
            if self.input.pressed(Action::Down) {
                selected = (selected + 1).min(1);
            }
            if self.input.pressed(Action::Advance) {
                if selected == 0 {
                    // Yes
                    self.execute_confirm_action(&action);
                } else {
                    // No — just close the dialog
                    self.screen = Screen::Dialogue;
                }
                return;
            }
            if self.input.pressed(Action::Cancel) {
                self.screen = Screen::Dialogue;
                return;
            }
            // Check mouse clicks on Yes/No buttons.
            for click in self.input.clicks() {
                let (mx, my) = (click.x, click.y);
                let (w, h) = (self.window_size.0 as f32, self.window_size.1 as f32);
                let btn_w = w * 0.15;
                let btn_h = 40.0;
                let yes_x = w * 0.35;
                let no_x = w * 0.55;
                let btn_y = h * 0.55;
                if my >= btn_y && my < btn_y + btn_h {
                    if mx >= yes_x && mx < yes_x + btn_w {
                        self.execute_confirm_action(&action);
                        return;
                    }
                    if mx >= no_x && mx < no_x + btn_w {
                        self.screen = Screen::Dialogue;
                        return;
                    }
                }
            }
            self.screen = Screen::Confirm { message, selected, action };
            return;
        }

        // Screen-specific input.
        match &self.screen.clone() {
            Screen::Title { selected } => {
                let mut sel = *selected;
                let buttons = ["New Game", "Continue", "Load", "Settings", "Gallery", "Quit"];
                let n = buttons.len();
                if self.input.pressed(Action::Down) {
                    sel = (sel + 1) % n;
                }
                if self.input.pressed(Action::Up) {
                    sel = (sel + n - 1) % n;
                }
                // Mouse click on title buttons.
                for click in self.input.clicks() {
                    let (mx, my) = (click.x, click.y);
                    let (w, h) = (self.window_size.0 as f32, self.window_size.1 as f32);
                    let btn_h = 50.0;
                    let start_y = h * 0.3;
                    for i in 0..n {
                        let y = start_y + i as f32 * btn_h;
                        if my >= y && my < y + btn_h && mx > w * 0.3 && mx < w * 0.7 {
                            sel = i;
                            self.activate_title_button(i);
                            return;
                        }
                    }
                }
                if self.input.pressed(Action::Advance) {
                    self.activate_title_button(sel);
                    return;
                }
                self.screen = Screen::Title { selected: sel };
            }
            Screen::Dialogue => {
                // Quick menu hotkeys.
                if self.input.pressed(Action::Save) {
                    self.screen = Screen::SaveLoad { mode: SaveLoadMode::Save, selected: 0 };
                    return;
                }
                if self.input.pressed(Action::Load) {
                    self.screen = Screen::SaveLoad { mode: SaveLoadMode::Load, selected: 0 };
                    return;
                }
                if self.input.pressed(Action::Settings) {
                    self.screen = Screen::Settings { selected: 0 };
                    return;
                }
                if self.input.pressed(Action::Gallery) {
                    self.screen = Screen::Gallery { selected: 0 };
                    return;
                }
                if self.input.pressed(Action::QuickSave) {
                    let _ = self.save(1);
                    return;
                }
                if self.input.pressed(Action::QuickLoad) {
                    let _ = self.load(1);
                    return;
                }
                if self.input.pressed(Action::Backlog) {
                    self.screen = Screen::Backlog { scroll: 0 };
                    return;
                }
                if self.input.pressed(Action::Rollback) {
                    self.rollback();
                    return;
                }
                if self.input.pressed(Action::Rollforward) {
                    self.rollforward();
                    return;
                }
                if self.input.pressed(Action::Title) {
                    self.screen = Screen::Confirm {
                        message: "Return to title screen?".to_string(),
                        selected: 0,
                        action: ConfirmAction::ToTitle,
                    };
                    return;
                }
                if self.input.pressed(Action::Quit) {
                    self.screen = Screen::Confirm {
                        message: "Quit the game?".to_string(),
                        selected: 0,
                        action: ConfirmAction::Quit,
                    };
                    return;
                }
                // Quick menu button clicks.
                if let Some(action) = self.check_quick_menu_click() {
                    match action {
                        QuickMenuAction::Auto => self.auto_advance = !self.auto_advance,
                        QuickMenuAction::Skip => self.skip = !self.skip,
                        QuickMenuAction::Save => {
                            self.screen = Screen::SaveLoad { mode: SaveLoadMode::Save, selected: 0 };
                            return;
                        }
                        QuickMenuAction::Load => {
                            self.screen = Screen::SaveLoad { mode: SaveLoadMode::Load, selected: 0 };
                            return;
                        }
                        QuickMenuAction::Settings => {
                            self.screen = Screen::Settings { selected: 0 };
                            return;
                        }
                        QuickMenuAction::Gallery => {
                            self.screen = Screen::Gallery { selected: 0 };
                            return;
                        }
                        QuickMenuAction::Title => {
                            self.screen = Screen::Confirm {
                                message: "Return to title screen?".to_string(),
                                selected: 0,
                                action: ConfirmAction::ToTitle,
                            };
                            return;
                        }
                        QuickMenuAction::Quit => {
                            self.screen = Screen::Confirm {
                                message: "Quit the game?".to_string(),
                                selected: 0,
                                action: ConfirmAction::Quit,
                            };
                            return;
                        }
                    }
                }
                if self.input.pressed(Action::AutoAdvance) {
                    self.auto_advance = !self.auto_advance;
                }
                if self.input.pressed(Action::Skip) {
                    self.skip = !self.skip;
                }
                if self.input.advance_requested() {
                    self.advance();
                }
            }
            Screen::Menu { ref choices, .. } => {
                let n = choices.len();
                if n == 0 {
                    return;
                }
                let mut hovered = match self.screen {
                    Screen::Menu { hovered, .. } => hovered,
                    _ => 0,
                };
                if self.input.pressed(Action::Down) {
                    hovered = (hovered + 1) % n;
                }
                if self.input.pressed(Action::Up) {
                    hovered = (hovered + n - 1) % n;
                }
                if self.input.pressed(Action::Advance) {
                    self.select_choice(hovered);
                    return;
                }
                for click in self.input.clicks() {
                    let (mx, my) = (click.x, click.y);
                    let (w, h) = (self.window_size.0 as f32, self.window_size.1 as f32);
                    let choice_h = 50.0;
                    let start_y = h * 0.3;
                    for i in 0..n {
                        let y = start_y + i as f32 * choice_h;
                        if my >= y && my < y + choice_h && mx > w * 0.2 && mx < w * 0.8 {
                            self.select_choice(i);
                            return;
                        }
                    }
                }
                self.screen = Screen::Menu {
                    prompt: match &self.screen {
                        Screen::Menu { prompt, .. } => prompt.clone(),
                        _ => None,
                    },
                    choices: choices.clone(),
                    hovered,
                };
            }
            Screen::SaveLoad { mode, selected } => {
                let mode = *mode;
                let mut sel = *selected;
                let n_slots = 9;
                if self.input.pressed(Action::Down) {
                    sel = (sel + 1) % n_slots;
                }
                if self.input.pressed(Action::Up) {
                    sel = (sel + n_slots - 1) % n_slots;
                }
                if self.input.pressed(Action::Cancel) {
                    self.screen = if self.vm.state.finished {
                        Screen::Title { selected: 0 }
                    } else {
                        Screen::Dialogue
                    };
                    return;
                }
                if self.input.pressed(Action::Advance) {
                    let slot = sel as u32;
                    match mode {
                        SaveLoadMode::Save => {
                            if self.vm.state.finished {
                                return; // Can't save after game over.
                            }
                            let _ = self.save(slot);
                            self.screen = Screen::Dialogue;
                            return;
                        }
                        SaveLoadMode::Load => {
                            let _ = self.load(slot);
                            return;
                        }
                    }
                }
                // Mouse click on slots.
                for click in self.input.clicks() {
                    let (mx, my) = (click.x, click.y);
                    let (w, h) = (self.window_size.0 as f32, self.window_size.1 as f32);
                    let cols = 3;
                    let slot_w = w * 0.28;
                    let slot_h = h * 0.2;
                    let gap_x = w * 0.03;
                    let gap_y = h * 0.03;
                    let start_x = w * 0.05;
                    let start_y = h * 0.15;
                    for i in 0..n_slots {
                        let col = i % cols;
                        let row = i / cols;
                        let x = start_x + col as f32 * (slot_w + gap_x);
                        let y = start_y + row as f32 * (slot_h + gap_y);
                        if mx >= x && mx < x + slot_w && my >= y && my < y + slot_h {
                            sel = i;
                            let slot = i as u32;
                            match mode {
                                SaveLoadMode::Save => {
                                    if !self.vm.state.finished {
                                        let _ = self.save(slot);
                                        self.screen = Screen::Dialogue;
                                        return;
                                    }
                                }
                                SaveLoadMode::Load => {
                                    let _ = self.load(slot);
                                    return;
                                }
                            }
                        }
                    }
                }
                self.screen = Screen::SaveLoad { mode, selected: sel };
            }
            Screen::Settings { selected } => {
                let mut sel = *selected;
                let n_settings = 10;
                if self.input.pressed(Action::Down) {
                    sel = (sel + 1) % n_settings;
                }
                if self.input.pressed(Action::Up) {
                    sel = (sel + n_settings - 1) % n_settings;
                }
                if self.input.pressed(Action::Cancel) {
                    self.save_settings();
                    self.screen = if self.vm.state.finished {
                        Screen::Title { selected: 0 }
                    } else {
                        Screen::Dialogue
                    };
                    return;
                }
                // Adjust settings with left/right.
                if self.input.pressed(Action::Left) || self.input.pressed(Action::Right) {
                    let dir: f64 = if self.input.pressed(Action::Right) { 1.0 } else { -1.0 };
                    match sel {
                        0 => self.settings.master_volume = (self.settings.master_volume + dir * 0.1).clamp(0.0, 1.0),
                        1 => self.settings.music_volume = (self.settings.music_volume + dir * 0.1).clamp(0.0, 1.0),
                        2 => self.settings.sfx_volume = (self.settings.sfx_volume + dir * 0.1).clamp(0.0, 1.0),
                        3 => self.settings.voice_volume = (self.settings.voice_volume + dir * 0.1).clamp(0.0, 1.0),
                        4 => self.settings.text_speed = (self.settings.text_speed + dir as f32 * 10.0).clamp(0.0, 200.0),
                        5 => self.settings.auto_advance_delay = (self.settings.auto_advance_delay + dir as f32 * 0.5).clamp(0.0, 10.0),
                        6 => self.settings.fullscreen = !self.settings.fullscreen,
                        7 => self.settings.auto_advance = !self.settings.auto_advance,
                        8 => self.settings.skip_unread = !self.settings.skip_unread,
                        9 => self.settings.muted = !self.settings.muted,
                        _ => {}
                    }
                    self.audio.apply_settings(self.settings.clone());
                }
                self.screen = Screen::Settings { selected: sel };
            }
            Screen::Gallery { selected } => {
                let mut sel = *selected;
                let n = self.gallery.unlocked.len();
                if n == 0 {
                    if self.input.pressed(Action::Cancel) || self.input.pressed(Action::Advance) {
                        self.screen = if self.vm.state.finished {
                            Screen::Title { selected: 0 }
                        } else {
                            Screen::Dialogue
                        };
                    }
                    return;
                }
                if self.input.pressed(Action::Right) {
                    sel = (sel + 1) % n;
                }
                if self.input.pressed(Action::Left) {
                    sel = (sel + n - 1) % n;
                }
                if self.input.pressed(Action::Cancel) {
                    self.screen = if self.vm.state.finished {
                        Screen::Title { selected: 0 }
                    } else {
                        Screen::Dialogue
                    };
                    return;
                }
                self.screen = Screen::Gallery { selected: sel };
            }
            Screen::Backlog { scroll } => {
                let mut sc = *scroll;
                let max_scroll = self.history.len().saturating_sub(1);
                if self.input.pressed(Action::Up) {
                    sc = sc.saturating_sub(1);
                }
                if self.input.pressed(Action::Down) {
                    sc = (sc + 1).min(max_scroll);
                }
                if self.input.pressed(Action::Cancel) || self.input.pressed(Action::Backlog) {
                    self.screen = Screen::Dialogue;
                    return;
                }
                self.screen = Screen::Backlog { scroll: sc };
            }
            Screen::End => {
                if self.input.advance_requested() || self.input.pressed(Action::Cancel) {
                    self.screen = Screen::Title { selected: 0 };
                }
            }
            _ => {}
        }
    }

    /// Activate a title screen button by index.
    fn activate_title_button(&mut self, index: usize) {
        match index {
            0 => {
                // New Game
                self.screen = Screen::Dialogue;
                self.step_vm();
            }
            1 => {
                // Continue — load most recent save
                let save_dir = self.project_root.join(&self.config.save_dir);
                let saves = vnengine_core::save::SaveState::list(&save_dir);
                if let Some(latest) = saves.iter().max_by_key(|s| s.saved_at) {
                    let _ = self.load(latest.slot);
                } else {
                    // No saves, just start new game
                    self.screen = Screen::Dialogue;
                    self.step_vm();
                }
            }
            2 => {
                // Load
                self.screen = Screen::SaveLoad { mode: SaveLoadMode::Load, selected: 0 };
            }
            3 => {
                // Settings
                self.screen = Screen::Settings { selected: 0 };
            }
            4 => {
                // Gallery
                self.screen = Screen::Gallery { selected: 0 };
            }
            5 => {
                // Quit
                self.screen = Screen::Confirm {
                    message: "Quit the game?".to_string(),
                    selected: 0,
                    action: ConfirmAction::Quit,
                };
            }
            _ => {}
        }
    }

    /// Execute the action associated with a confirmed dialog.
    fn execute_confirm_action(&mut self, action: &ConfirmAction) {
        match action {
            ConfirmAction::ToTitle => {
                self.screen = Screen::Title { selected: 0 };
            }
            ConfirmAction::Quit => {
                // We can't directly close the window from here, but
                // setting finished will cause the app to exit on next
                // event. A real implementation would signal the event loop.
                self.vm.state.finished = true;
                self.screen = Screen::End;
            }
            ConfirmAction::OverwriteSave(slot) => {
                let _ = self.save(*slot);
                self.screen = Screen::Dialogue;
            }
        }
    }

    /// Save settings to disk.
    fn save_settings(&self) {
        let settings_path = self.project_root.join(&self.config.save_dir).join("settings.json");
        let _ = self.settings.save(&settings_path);
    }

    /// Push a rollback snapshot of the current state.
    fn push_rollback_snapshot(&mut self) {
        let snapshot = RollbackSnapshot {
            vm_state: self.vm.save_state(),
            scene: self.scene.clone(),
            blocking_text: self.blocking.as_ref().map(|b| b.text.clone()),
            blocking_speaker: self.blocking.as_ref().and_then(|b| b.speaker.clone()),
            screen_is_dialogue: matches!(self.screen, Screen::Dialogue),
        };
        self.rollback_stack.push(snapshot);
        // Limit stack size to prevent unbounded memory growth.
        if self.rollback_stack.len() > 50 {
            self.rollback_stack.remove(0);
        }
        // Clear rollforward stack — a new action invalidates it.
        self.rollforward_stack.clear();
    }

    /// Rollback to the previous state.
    fn rollback(&mut self) {
        if let Some(snapshot) = self.rollback_stack.pop() {
            // Push current state to rollforward stack.
            let current = RollbackSnapshot {
                vm_state: self.vm.save_state(),
                scene: self.scene.clone(),
                blocking_text: self.blocking.as_ref().map(|b| b.text.clone()),
                blocking_speaker: self.blocking.as_ref().and_then(|b| b.speaker.clone()),
                screen_is_dialogue: matches!(self.screen, Screen::Dialogue),
            };
            self.rollforward_stack.push(current);

            // Restore the snapshot.
            self.vm.state = snapshot.vm_state;
            self.scene = snapshot.scene;
            self.blocking = None;
            self.screen = Screen::Dialogue;
            // Re-step to get the blocking command again.
            self.step_vm();
        }
    }

    /// Rollforward (undo a rollback).
    fn rollforward(&mut self) {
        if let Some(snapshot) = self.rollforward_stack.pop() {
            // Push current state back to rollback stack.
            self.rollback_stack.push(RollbackSnapshot {
                vm_state: self.vm.save_state(),
                scene: self.scene.clone(),
                blocking_text: self.blocking.as_ref().map(|b| b.text.clone()),
                blocking_speaker: self.blocking.as_ref().and_then(|b| b.speaker.clone()),
                screen_is_dialogue: matches!(self.screen, Screen::Dialogue),
            });

            // Restore the snapshot.
            self.vm.state = snapshot.vm_state;
            self.scene = snapshot.scene;
            self.blocking = None;
            self.screen = Screen::Dialogue;
            self.step_vm();
        }
    }

    /// Auto-save to slot 0.
    fn auto_save(&mut self) {
        if !self.vm.state.finished {
            let _ = self.save(0);
        }
    }

    /// Check if the player clicked a quick menu button.
    fn check_quick_menu_click(&self) -> Option<QuickMenuAction> {
        let (w, h) = (self.window_size.0 as f32, self.window_size.1 as f32);
        let buttons = quick_menu_buttons(w, h);
        for click in self.input.clicks() {
            for (i, (x, y, bw, bh)) in buttons.iter().enumerate() {
                if click.x >= *x && click.x < x + bw && click.y >= *y && click.y < y + bh {
                    return Some(QUICK_MENU_ACTIONS[i]);
                }
            }
        }
        None
    }

    /// Build the UI draw data for this frame.
    fn build_ui(&mut self) -> UiDraw {
        let mut ui = UiDraw::empty();
        let (w, h) = (self.window_size.0 as f32, self.window_size.1 as f32);

        match &self.screen {
            Screen::Dialogue => {
                if let Some(b) = &self.blocking {
                    // Dialogue box background.
                    let box_h = 0.25;
                    let box_y = 1.0 - box_h - 0.02;
                    ui.boxes.push(UiBox {
                        x: 0.02,
                        y: box_y,
                        w: 0.96,
                        h: box_h,
                        color: [0.0, 0.0, 0.0, 0.7],
                    });

                    // Namebox (styled speaker name box).
                    if let Some(speaker) = &b.speaker {
                        let namebox_w = 0.2;
                        let namebox_h = 0.04;
                        let namebox_y = box_y - namebox_h + 0.005;
                        ui.boxes.push(UiBox {
                            x: 0.04,
                            y: namebox_y,
                            w: namebox_w,
                            h: namebox_h,
                            color: [0.15, 0.15, 0.3, 0.9],
                        });
                        ui.text.push(UiText {
                            text: speaker.clone(),
                            left: w * 0.06,
                            top: namebox_y * h + 5.0,
                            font_size: 22.0,
                            color: [1.0, 1.0, 0.6],
                            bounds: None,
                            rich: true,
                        });
                    }

                    // Dialogue text (typewriter with rich text support).
                    let visible_segs =
                        vnengine_render::richtext::take_visible_chars(&b.text, b.chars_shown);
                    let visible_text: String =
                        visible_segs.iter().map(|s| s.text.as_str()).collect();
                    ui.text.push(UiText {
                        text: visible_text,
                        left: w * 0.06,
                        top: box_y * h + 50.0,
                        font_size: 28.0,
                        color: [1.0, 1.0, 1.0],
                        bounds: None,
                        rich: true,
                    });
                }

                // Quick menu buttons (bottom-right).
                let buttons = quick_menu_buttons(w, h);
                let labels = ["Auto", "Skip", "Save", "Load", "Opt", "CG", "Menu", "Quit"];
                for (i, (x, y, bw, bh)) in buttons.iter().enumerate() {
                    ui.boxes.push(UiBox {
                        x: *x / w,
                        y: *y / h,
                        w: *bw / w,
                        h: *bh / h,
                        color: [0.1, 0.1, 0.2, 0.8],
                    });
                    ui.text.push(UiText {
                        text: labels[i].to_string(),
                        left: x + 5.0,
                        top: y + 3.0,
                        font_size: 14.0,
                        color: [0.8, 0.8, 0.8],
                        bounds: None,
                        rich: false,
                    });
                }
            }
            Screen::Menu { prompt, choices, hovered } => {
                // Dim background.
                ui.boxes.push(UiBox {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                    color: [0.0, 0.0, 0.0, 0.5],
                });

                // Prompt.
                if let Some(p) = prompt {
                    ui.text.push(UiText {
                        text: p.clone(),
                        left: w * 0.1,
                        top: h * 0.2,
                        font_size: 28.0,
                        color: [1.0, 1.0, 1.0],
                        bounds: None,
                        rich: true,
                    });
                }

                // Choices.
                let choice_h = 50.0;
                let start_y = h * 0.3;
                for (i, choice) in choices.iter().enumerate() {
                    let y = start_y + i as f32 * choice_h;
                    let is_hovered = i == *hovered;
                    ui.boxes.push(UiBox {
                        x: 0.2,
                        y: y / h,
                        w: 0.6,
                        h: (choice_h - 5.0) / h,
                        color: if is_hovered {
                            [0.2, 0.4, 0.8, 0.8]
                        } else {
                            [0.1, 0.1, 0.2, 0.7]
                        },
                    });
                    ui.text.push(UiText {
                        text: choice.clone(),
                        left: w * 0.25,
                        top: y + 10.0,
                        font_size: 24.0,
                        color: [1.0, 1.0, 1.0],
                        bounds: None,
                        rich: true,
                    });
                }
            }
            Screen::Title { selected } => {
                // Dark background.
                ui.boxes.push(UiBox {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                    color: [0.05, 0.05, 0.1, 0.95],
                });

                // Title text.
                ui.text.push(UiText {
                    text: self.config.title.clone(),
                    left: w * 0.3,
                    top: h * 0.1,
                    font_size: 48.0,
                    color: [1.0, 1.0, 0.6],
                    bounds: None,
                    rich: false,
                });

                // Buttons.
                let buttons = ["New Game", "Continue", "Load", "Settings", "Gallery", "Quit"];
                let btn_h = 50.0;
                let start_y = h * 0.3;
                for (i, label) in buttons.iter().enumerate() {
                    let y = start_y + i as f32 * btn_h;
                    let is_selected = i == *selected;
                    ui.boxes.push(UiBox {
                        x: 0.3,
                        y: y / h,
                        w: 0.4,
                        h: (btn_h - 5.0) / h,
                        color: if is_selected {
                            [0.2, 0.4, 0.8, 0.8]
                        } else {
                            [0.1, 0.1, 0.2, 0.5]
                        },
                    });
                    ui.text.push(UiText {
                        text: label.to_string(),
                        left: w * 0.35,
                        top: y + 10.0,
                        font_size: 24.0,
                        color: if is_selected {
                            [1.0, 1.0, 1.0]
                        } else {
                            [0.7, 0.7, 0.7]
                        },
                        bounds: None,
                        rich: false,
                    });
                }
            }
            Screen::SaveLoad { mode, selected } => {
                // Dim background.
                ui.boxes.push(UiBox {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                    color: [0.0, 0.0, 0.0, 0.7],
                });

                // Title.
                let title = match mode {
                    SaveLoadMode::Save => "Save Game",
                    SaveLoadMode::Load => "Load Game",
                };
                ui.text.push(UiText {
                    text: title.to_string(),
                    left: w * 0.4,
                    top: h * 0.03,
                    font_size: 32.0,
                    color: [1.0, 1.0, 0.6],
                    bounds: None,
                    rich: false,
                });

                // Save slots grid (3x3).
                let save_dir = self.project_root.join(&self.config.save_dir);
                let saves = vnengine_core::save::SaveState::list(&save_dir);
                let cols = 3;
                let slot_w = w * 0.28;
                let slot_h = h * 0.2;
                let gap_x = w * 0.03;
                let gap_y = h * 0.03;
                let start_x = w * 0.05;
                let start_y = h * 0.15;
                for i in 0..9 {
                    let col = i % cols;
                    let row = i / cols;
                    let x = start_x + col as f32 * (slot_w + gap_x);
                    let y = start_y + row as f32 * (slot_h + gap_y);
                    let is_selected = i == *selected;
                    let save = saves.iter().find(|s| s.slot == i as u32);
                    ui.boxes.push(UiBox {
                        x: x / w,
                        y: y / h,
                        w: slot_w / w,
                        h: slot_h / h,
                        color: if is_selected {
                            [0.2, 0.4, 0.8, 0.8]
                        } else {
                            [0.1, 0.1, 0.2, 0.7]
                        },
                    });
                    let label = if let Some(s) = save {
                        format!("Slot {}\n{}\n{:.0}s", s.slot, s.label, s.play_time)
                    } else {
                        format!("Slot {}\nEmpty", i)
                    };
                    ui.text.push(UiText {
                        text: label,
                        left: x + 10.0,
                        top: y + 10.0,
                        font_size: 16.0,
                        color: [1.0, 1.0, 1.0],
                        bounds: None,
                        rich: false,
                    });
                }

                // Back hint.
                ui.text.push(UiText {
                    text: "ESC to go back".to_string(),
                    left: w * 0.4,
                    top: h * 0.92,
                    font_size: 16.0,
                    color: [0.6, 0.6, 0.6],
                    bounds: None,
                    rich: false,
                });
            }
            Screen::Settings { selected } => {
                // Dim background.
                ui.boxes.push(UiBox {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                    color: [0.0, 0.0, 0.0, 0.7],
                });

                ui.text.push(UiText {
                    text: "Settings".to_string(),
                    left: w * 0.4,
                    top: h * 0.03,
                    font_size: 32.0,
                    color: [1.0, 1.0, 0.6],
                    bounds: None,
                    rich: false,
                });

                let labels = [
                    format!("Master Volume: {:.0}%", self.settings.master_volume * 100.0),
                    format!("Music Volume: {:.0}%", self.settings.music_volume * 100.0),
                    format!("SFX Volume: {:.0}%", self.settings.sfx_volume * 100.0),
                    format!("Voice Volume: {:.0}%", self.settings.voice_volume * 100.0),
                    format!("Text Speed: {:.0} cps", self.settings.text_speed),
                    format!("Auto-Advance Delay: {:.1}s", self.settings.auto_advance_delay),
                    format!("Fullscreen: {}", self.settings.fullscreen),
                    format!("Auto-Advance: {}", self.settings.auto_advance),
                    format!("Skip Unread: {}", self.settings.skip_unread),
                    format!("Muted: {}", self.settings.muted),
                ];
                let start_y = h * 0.15;
                let line_h = 40.0;
                for (i, label) in labels.iter().enumerate() {
                    let y = start_y + i as f32 * line_h;
                    let is_selected = i == *selected;
                    if is_selected {
                        ui.boxes.push(UiBox {
                            x: 0.1,
                            y: y / h,
                            w: 0.8,
                            h: (line_h - 5.0) / h,
                            color: [0.2, 0.4, 0.8, 0.5],
                        });
                    }
                    ui.text.push(UiText {
                        text: label.clone(),
                        left: w * 0.15,
                        top: y + 5.0,
                        font_size: 20.0,
                        color: [1.0, 1.0, 1.0],
                        bounds: None,
                        rich: false,
                    });
                }

                ui.text.push(UiText {
                    text: "ESC to go back | Left/Right to adjust".to_string(),
                    left: w * 0.3,
                    top: h * 0.92,
                    font_size: 16.0,
                    color: [0.6, 0.6, 0.6],
                    bounds: None,
                    rich: false,
                });
            }
            Screen::Gallery { selected } => {
                // Dim background.
                ui.boxes.push(UiBox {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                    color: [0.0, 0.0, 0.0, 0.7],
                });

                ui.text.push(UiText {
                    text: "CG Gallery".to_string(),
                    left: w * 0.4,
                    top: h * 0.03,
                    font_size: 32.0,
                    color: [1.0, 1.0, 0.6],
                    bounds: None,
                    rich: false,
                });

                if self.gallery.unlocked.is_empty() {
                    ui.text.push(UiText {
                        text: "No CGs unlocked yet.".to_string(),
                        left: w * 0.35,
                        top: h * 0.4,
                        font_size: 24.0,
                        color: [0.6, 0.6, 0.6],
                        bounds: None,
                        rich: false,
                    });
                } else {
                    let cols = 4;
                    let cg_w = w * 0.18;
                    let cg_h = h * 0.2;
                    let gap_x = w * 0.02;
                    let gap_y = h * 0.03;
                    let start_x = w * 0.05;
                    let start_y = h * 0.15;
                    for (i, cg) in self.gallery.unlocked.iter().enumerate() {
                        let col = i % cols;
                        let row = i / cols;
                        let x = start_x + col as f32 * (cg_w + gap_x);
                        let y = start_y + row as f32 * (cg_h + gap_y);
                        let is_selected = i == *selected;
                        ui.boxes.push(UiBox {
                            x: x / w,
                            y: y / h,
                            w: cg_w / w,
                            h: cg_h / h,
                            color: if is_selected {
                                [0.3, 0.5, 0.9, 0.8]
                            } else {
                                [0.15, 0.15, 0.25, 0.7]
                            },
                        });
                        ui.text.push(UiText {
                            text: cg.clone(),
                            left: x + 5.0,
                            top: y + 5.0,
                            font_size: 14.0,
                            color: [0.8, 0.8, 0.8],
                            bounds: None,
                            rich: false,
                        });
                    }
                }

                ui.text.push(UiText {
                    text: "ESC to go back".to_string(),
                    left: w * 0.4,
                    top: h * 0.92,
                    font_size: 16.0,
                    color: [0.6, 0.6, 0.6],
                    bounds: None,
                    rich: false,
                });
            }
            Screen::Backlog { scroll } => {
                // Dim background.
                ui.boxes.push(UiBox {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                    color: [0.0, 0.0, 0.0, 0.8],
                });

                ui.text.push(UiText {
                    text: "History (ESC to go back)".to_string(),
                    left: w * 0.05,
                    top: h * 0.02,
                    font_size: 20.0,
                    color: [1.0, 1.0, 0.6],
                    bounds: None,
                    rich: false,
                });

                // Show last N entries.
                let max_display = 15;
                let total = self.history.len();
                let start_idx = total.saturating_sub(max_display + *scroll);
                let end_idx = total.saturating_sub(*scroll);
                let start_y = h * 0.08;
                let line_h = 35.0;
                for (i, entry) in self.history[start_idx..end_idx].iter().enumerate() {
                    let y = start_y + i as f32 * line_h;
                    if let Some(speaker) = &entry.speaker {
                        ui.text.push(UiText {
                            text: format!("{}: {}", speaker, entry.text),
                            left: w * 0.05,
                            top: y,
                            font_size: 18.0,
                            color: [0.9, 0.9, 0.9],
                            bounds: None,
                            rich: true,
                        });
                    } else {
                        ui.text.push(UiText {
                            text: entry.text.clone(),
                            left: w * 0.05,
                            top: y,
                            font_size: 18.0,
                            color: [0.7, 0.7, 0.7],
                            bounds: None,
                            rich: true,
                        });
                    }
                }
            }
            Screen::Confirm { message, selected, .. } => {
                // Dim background.
                ui.boxes.push(UiBox {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                    color: [0.0, 0.0, 0.0, 0.6],
                });

                // Dialog box.
                ui.boxes.push(UiBox {
                    x: 0.25,
                    y: 0.35,
                    w: 0.5,
                    h: 0.25,
                    color: [0.1, 0.1, 0.2, 0.95],
                });

                // Message.
                ui.text.push(UiText {
                    text: message.clone(),
                    left: w * 0.3,
                    top: h * 0.4,
                    font_size: 24.0,
                    color: [1.0, 1.0, 1.0],
                    bounds: None,
                    rich: false,
                });

                // Yes/No buttons.
                let btn_w = w * 0.15;
                let btn_h = 40.0;
                let yes_x = w * 0.35;
                let no_x = w * 0.55;
                let btn_y = h * 0.55;
                ui.boxes.push(UiBox {
                    x: yes_x / w,
                    y: btn_y / h,
                    w: btn_w / w,
                    h: btn_h / h,
                    color: if *selected == 0 {
                        [0.2, 0.6, 0.2, 0.9]
                    } else {
                        [0.15, 0.15, 0.25, 0.7]
                    },
                });
                ui.boxes.push(UiBox {
                    x: no_x / w,
                    y: btn_y / h,
                    w: btn_w / w,
                    h: btn_h / h,
                    color: if *selected == 1 {
                        [0.6, 0.2, 0.2, 0.9]
                    } else {
                        [0.15, 0.15, 0.25, 0.7]
                    },
                });
                ui.text.push(UiText {
                    text: "Yes".to_string(),
                    left: yes_x + 30.0,
                    top: btn_y + 8.0,
                    font_size: 20.0,
                    color: [1.0, 1.0, 1.0],
                    bounds: None,
                    rich: false,
                });
                ui.text.push(UiText {
                    text: "No".to_string(),
                    left: no_x + 35.0,
                    top: btn_y + 8.0,
                    font_size: 20.0,
                    color: [1.0, 1.0, 1.0],
                    bounds: None,
                    rich: false,
                });
            }
            Screen::End => {
                ui.text.push(UiText {
                    text: "The End".to_string(),
                    left: w * 0.4,
                    top: h * 0.45,
                    font_size: 48.0,
                    color: [1.0, 1.0, 1.0],
                    bounds: None,
                    rich: false,
                });
                ui.text.push(UiText {
                    text: "Press Space to return to title".to_string(),
                    left: w * 0.3,
                    top: h * 0.55,
                    font_size: 20.0,
                    color: [0.6, 0.6, 0.6],
                    bounds: None,
                    rich: false,
                });
            }
        }

        ui
    }

    /// Save the game to a slot.
    pub fn save(&self, slot: u32) -> anyhow::Result<()> {
        let save_dir = self.project_root.join(&self.config.save_dir);
        let vm_state = self.vm.save_state();
        let label = self
            .blocking
            .as_ref()
            .map(|b| b.text.chars().take(40).collect())
            .unwrap_or_default();

        // Generate a thumbnail from the current background image.
        let thumbnail = self.generate_thumbnail();

        let state = SaveState {
            slot,
            label,
            play_time: self.play_time,
            vm_state: serde_json::to_vec(&vm_state)?,
            variables: self.vm.state.variables.clone(),
            thumbnail,
            saved_at: chrono::Utc::now().timestamp(),
        };
        state.write(&save_dir)?;
        Ok(())
    }

    /// Generate a small PNG thumbnail from the current background image.
    fn generate_thumbnail(&self) -> Option<Vec<u8>> {
        let bg = self.scene.background.as_ref()?;
        let image_path = self.project_root.join(&self.config.image_dir).join(bg);
        let img = image::open(&image_path).ok()?;
        let thumb = img.resize_to_fill(160, 90, image::imageops::FilterType::Lanczos3);
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        thumb.to_rgba8().write_to(&mut cursor, image::ImageFormat::Png).ok()?;
        Some(buf)
    }

    /// Load the game from a slot.
    pub fn load(&mut self, slot: u32) -> anyhow::Result<()> {
        let save_dir = self.project_root.join(&self.config.save_dir);
        let state = SaveState::read(&save_dir, slot)?;
        let vm_state: vnengine_script::vm::VmState =
            serde_json::from_slice(&state.vm_state)?;
        // Re-parse the script and restore VM state.
        let script_path = self.project_root.join(&self.config.entry_script);
        let script_src = std::fs::read_to_string(&script_path)?;
        let script = Parser::parse(&script_src)?;
        self.vm = Vm::restore(script, vm_state);
        self.vm.state.variables = state.variables;
        self.blocking = None;
        self.screen = Screen::Dialogue;
        self.step_vm();
        Ok(())
    }
}

struct AppHandler {
    app: Arc<Mutex<App>>,
    window: Arc<winit::window::Window>,
}

impl winit::application::ApplicationHandler for AppHandler {
    fn resumed(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        // Kick off the VM on first resume — but only if we're past the
        // title screen. The title screen starts the VM when "New Game"
        // or "Continue" is selected.
        let mut app = self.app.lock();
        if !matches!(app.screen, Screen::Title { .. })
            && !app.vm.state.finished
            && app.blocking.is_none()
            && app.pending_menu.is_none()
        {
            app.step_vm();
        }
        self.window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        let mut app = self.app.lock();
        match event {
            winit::event::WindowEvent::Resized(size) => {
                app.window_size = (size.width, size.height);
                app.renderer.resize((size.width, size.height));
                self.window.request_redraw();
            }
            winit::event::WindowEvent::KeyboardInput { event, .. } => {
                use winit::event::ElementState;
                if let winit::event::KeyEvent {
                    physical_key: winit::keyboard::PhysicalKey::Code(key),
                    state,
                    ..
                } = event
                {
                    match state {
                        ElementState::Pressed => app.input.key_press(key),
                        ElementState::Released => app.input.key_release(key),
                    }
                }
            }
            winit::event::WindowEvent::CursorMoved { position, .. } => {
                app.input
                    .mouse_move(position.x as f32, position.y as f32);
            }
            winit::event::WindowEvent::MouseInput { state, button, .. } => {
                use winit::event::{ElementState, MouseButton};
                if state == ElementState::Pressed && button == MouseButton::Left {
                    let pos = app.input.mouse_pos();
                    app.input.mouse_click(pos.0, pos.1);
                }
            }
            winit::event::WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            winit::event::WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = now
                    .duration_since(app.last_frame)
                    .as_secs_f32()
                    .min(0.1);
                app.last_frame = now;
                app.handle_input();
                app.update(dt);
                let ui = app.build_ui();
                let image_root = app.project_root.join(&app.config.image_dir);
                let mut scene = app.scene.clone();
                // Apply screen shake offset.
                if let Some((remaining, intensity)) = app.shake {
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut hasher = DefaultHasher::new();
                    remaining.to_bits().hash(&mut hasher);
                    let seed = hasher.finish();
                    let ox = ((seed % 1000) as f32 / 1000.0 - 0.5) * intensity * 20.0;
                    let oy = (((seed / 1000) % 1000) as f32 / 1000.0 - 0.5) * intensity * 20.0;
                    for sprite in &mut scene.sprites {
                        sprite.offset[0] += ox;
                        sprite.offset[1] += oy;
                    }
                }
                if let Err(e) = app.renderer.render(&scene, &image_root, &ui) {
                    log::error!("render error: {e}");
                }
                // Clear per-frame input state after processing.
                app.input.end_frame();
                self.window.request_redraw();
            }
            _ => {}
        }
    }
}
