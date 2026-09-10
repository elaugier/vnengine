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
    Title,
    /// The save/load menu.
    SaveLoad { mode: SaveLoadMode },
    /// The settings menu.
    Settings,
    /// The CG gallery.
    Gallery,
    /// Game over / end screen.
    End,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SaveLoadMode {
    Save,
    Load,
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
    /// Logical window size.
    window_size: (u32, u32),
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
            screen: Screen::Dialogue,
            blocking: None,
            transition: None,
            pending_menu: None,
            gallery,
            play_time: 0.0,
            last_frame: Instant::now(),
            auto_timer: 0.0,
            auto_advance: false,
            skip: false,
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
            Command::Shake { .. } => {
                // TODO: implement screen shake.
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
                blocking.chars_shown = blocking.text.len();
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
                if speed <= 0.0 {
                    b.chars_shown = b.text.len();
                    b.typewriter_done = true;
                } else {
                    let target = (b.shown_at.elapsed().as_secs_f32() * speed) as usize;
                    b.chars_shown = target.min(b.text.len());
                    if b.chars_shown >= b.text.len() {
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
        // Global actions.
        if self.input.pressed(Action::Quit) {
            // Request window close via the event loop.
        }
        if self.input.pressed(Action::Fullscreen) {
            self.settings.fullscreen = !self.settings.fullscreen;
        }
        if self.input.pressed(Action::AutoAdvance) {
            self.auto_advance = !self.auto_advance;
        }
        if self.input.pressed(Action::Skip) {
            self.skip = !self.skip;
        }

        match self.screen {
            Screen::Dialogue => {
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
                // Navigate with arrow keys (held for repeat).
                if self.input.pressed(Action::Advance) {
                    // Confirm selection.
                    self.select_choice(hovered);
                    return;
                }
                // Use mouse clicks for menu selection.
                for click in self.input.clicks() {
                    let (mx, my) = (click.x, click.y);
                    let (w, h) = (self.window_size.0 as f32, self.window_size.1 as f32);
                    // Menu choices are drawn in the center, each 50px tall.
                    let choice_h = 50.0;
                    let total_h = n as f32 * choice_h;
                    let start_y = h * 0.3;
                    for i in 0..n {
                        let y = start_y + i as f32 * choice_h;
                        if my >= y && my < y + choice_h && mx > w * 0.2 && mx < w * 0.8 {
                            self.select_choice(i);
                            return;
                        }
                    }
                }
                let _ = hovered;
            }
            Screen::Title => {
                if self.input.advance_requested() {
                    self.screen = Screen::Dialogue;
                    self.step_vm();
                }
            }
            Screen::End => {
                if self.input.advance_requested() {
                    // Could go back to title.
                }
            }
            _ => {}
        }
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

                    // Speaker name.
                    if let Some(speaker) = &b.speaker {
                        ui.text.push(UiText {
                            text: speaker.clone(),
                            left: w * 0.06,
                            top: box_y * h + 10.0,
                            font_size: 24.0,
                            color: [1.0, 1.0, 0.6],
                            bounds: None,
                        });
                    }

                    // Dialogue text (typewriter).
                    let visible: String = b.text.chars().take(b.chars_shown).collect();
                    ui.text.push(UiText {
                        text: visible,
                        left: w * 0.06,
                        top: box_y * h + 50.0,
                        font_size: 28.0,
                        color: [1.0, 1.0, 1.0],
                        bounds: None,
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
                    });
                }
            }
            Screen::End => {
                ui.text.push(UiText {
                    text: "The End".to_string(),
                    left: w * 0.4,
                    top: h * 0.45,
                    font_size: 48.0,
                    color: [1.0, 1.0, 1.0],
                    bounds: None,
                });
            }
            _ => {}
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
        let state = SaveState {
            slot,
            label,
            play_time: self.play_time,
            vm_state: serde_json::to_vec(&vm_state)?,
            variables: self.vm.state.variables.clone(),
            thumbnail: None,
            saved_at: chrono::Utc::now().timestamp(),
        };
        state.write(&save_dir)?;
        Ok(())
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
        // Kick off the VM on first resume so the first scene/command loads.
        let mut app = self.app.lock();
        if !app.vm.state.finished && app.blocking.is_none() && app.pending_menu.is_none() {
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
                let scene = app.scene.clone();
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
