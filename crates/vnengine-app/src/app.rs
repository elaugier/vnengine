//! Main application: winit event loop, wgpu init, VM driving, scene updates,
//! transitions, and UI rendering.

use crate::input::{Action, Input};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use vnengine_audio::AudioEngine;
use vnengine_core::assets::{AssetSource, DirAssets, IMAGE_EXTS};
use vnengine_core::command::Command;
use vnengine_core::save::{GalleryState, SaveState};
use vnengine_core::settings::UserSettings;
use vnengine_core::transition::{SlideDirection, Transition};
use vnengine_core::EngineConfig;
use vnengine_render::renderer::{UiBox, UiDraw, UiText};
use vnengine_render::Renderer;
use vnengine_runtime::{Runtime, RuntimeEvent, RuntimeSnapshot};
use vnengine_script::ast::Script;
use vnengine_script::parser::Parser;

/// The current UI screen being displayed.
#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
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
pub enum SaveLoadMode {
    Save,
    Load,
}

/// What action to take when a confirm dialog is accepted.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmAction {
    /// Return to the title screen.
    ToTitle,
    /// Quit the game.
    Quit,
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
    /// Optional side image path.
    side_image: Option<String>,
}

/// A transition animation in progress.
struct ActiveTransition {
    transition: Transition,
    elapsed: f32,
    duration: f32,
}

pub struct App {
    pub config: EngineConfig,
    /// Asset source: project directory or `.vnpkg` package.
    pub assets: Box<dyn AssetSource>,
    /// Directory under which `config.save_dir` resolves (saves, settings,
    /// gallery). For packages this is the directory containing the file.
    pub save_root: PathBuf,
    /// The parsed entry script, kept for save/load restores.
    pub script: Script,
    pub settings: UserSettings,
    /// Headless game state: VM, scene, history, rollback, gallery.
    pub runtime: Runtime,
    pub audio: AudioEngine,
    pub renderer: Renderer,
    pub input: Input,
    pub screen: Screen,
    blocking: Option<BlockingState>,
    transition: Option<ActiveTransition>,
    /// BGM track last applied to the audio engine — compared against
    /// `runtime.music` to keep playback in sync (incl. after load).
    last_bgm: Option<String>,
    /// Stem layers last applied to the audio engine — diffed against
    /// `runtime.music_layers` each frame.
    last_layers: std::collections::BTreeMap<String, vnengine_core::save::MusicLayerState>,
    /// Sandboxed WASM plugins (None if the project ships none).
    pub plugins: Option<vnengine_plugins::PluginHost>,
    /// Script file to watch for hot reload (loose project only).
    script_path: Option<PathBuf>,
    /// Last seen modification time of `script_path`.
    script_mtime: Option<std::time::SystemTime>,
    /// Polling accumulator for the file watcher.
    watch_timer: f32,
    /// Studio debug overlay (F8): variables, scene and audio state.
    debug_overlay: bool,
    /// Label selected in the overlay as jump target (`[`/`]` + F10).
    debug_label: usize,
    /// Locales shipped under `translations/` (`en`, `fr`, ...).
    locales: Vec<String>,
    /// Active string translations for `settings.language`
    /// (original text → localized text).
    translations: std::collections::BTreeMap<String, String>,
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
    /// Auto-save timer (seconds since last auto-save).
    auto_save_timer: f32,
    /// Screen shake state: (remaining_time, intensity).
    shake: Option<(f32, f32)>,
    /// Logical window size.
    window_size: (u32, u32),
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
    for (i, button) in buttons.iter_mut().enumerate() {
        *button = (start_x + i as f32 * (btn_w + gap), y, btn_w, btn_h);
    }
    buttons
}

impl App {
    /// Create and run the application on a loose project directory.
    /// This blocks until the window closes.
    pub fn run(config: EngineConfig, project_root: PathBuf) -> anyhow::Result<()> {
        let script_path = project_root.join(&config.entry_script);
        let script_src = std::fs::read_to_string(&script_path)?;
        let script = Parser::parse(&script_src)?;
        let save_root = project_root.clone();
        Self::run_source(
            config,
            Box::new(DirAssets(project_root)),
            save_root,
            script,
            Some(script_path),
        )
    }

    /// Create and run the application with an arbitrary asset source
    /// (directory or package) and an already-loaded script.
    /// `watch` enables hot reload: when the given script file's mtime
    /// changes, it is re-parsed and swapped into the runtime in place.
    pub fn run_source(
        config: EngineConfig,
        assets: Box<dyn AssetSource>,
        save_root: PathBuf,
        script: Script,
        watch: Option<PathBuf>,
    ) -> anyhow::Result<()> {
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

        // TODO: migrate to `ActiveEventLoop::create_window` inside
        // `resumed()` when the event loop is refactored to
        // `ApplicationHandler`.
        #[allow(deprecated)]
        let window = Arc::new(event_loop.create_window(window_attrs)?);

        // Initialize wgpu.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(event_loop.owned_display_handle()),
        ));
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let surface = instance.create_surface(window.clone())?;
        // Query the surface's preferred format instead of hardcoding.
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8UnormSrgb))
            .copied()
            .unwrap_or(
                capabilities
                    .formats
                    .first()
                    .copied()
                    .unwrap_or(wgpu::TextureFormat::Bgra8UnormSrgb),
            );

        let window_size = window.inner_size();
        let renderer = Renderer::new(
            device,
            queue,
            surface,
            (window_size.width, window_size.height),
            format,
        )?;

        // Load settings.
        let settings_path = save_root.join(&config.save_dir).join("settings.json");
        let settings = UserSettings::load(&settings_path).unwrap_or_default();

        let mut runtime = Runtime::new(script.clone());

        // Audio.
        let audio = AudioEngine::new(settings.clone())?;

        // Gallery.
        let gallery_path = save_root.join(&config.save_dir).join("gallery.json");
        runtime.gallery = GalleryState::load(&gallery_path);

        // Sandboxed WASM plugins declared under `plugins/`.
        let mut plugins = vnengine_plugins::PluginHost::new()?;
        vnengine_plugins::load_source(&mut plugins, &*assets, "plugins");
        plugins.init_all(&mut runtime.plugin_kv);
        let plugins = (!plugins.is_empty()).then_some(plugins);

        let app = App {
            config,
            assets,
            save_root,
            script,
            settings,
            runtime,
            audio,
            renderer,
            input: Input::new(),
            screen: Screen::Title { selected: 0 },
            blocking: None,
            transition: None,
            last_bgm: None,
            last_layers: std::collections::BTreeMap::new(),
            script_mtime: watch
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok()),
            watch_timer: 0.0,
            script_path: watch,
            debug_overlay: false,
            debug_label: 0,
            locales: Vec::new(),
            translations: std::collections::BTreeMap::new(),
            plugins,
            play_time: 0.0,
            last_frame: Instant::now(),
            auto_timer: 0.0,
            auto_advance: false,
            skip: false,
            auto_save_timer: 0.0,
            shake: None,
            window_size: (window_size.width, window_size.height),
        };

        let mut app = app;
        app.reload_translations();
        let app = Arc::new(Mutex::new(app));
        let window_clone = window.clone();
        let app_clone = app.clone();

        event_loop.run_app(&mut AppHandler {
            app: app_clone,
            window: window_clone,
        })?;
        Ok(())
    }

    /// Step the runtime forward until it blocks on user-visible content.
    fn step_vm(&mut self) {
        let event = self.runtime.step();
        self.pump(event);
    }

    /// Dispatch runtime events until one blocks on the player.
    fn pump(&mut self, mut event: RuntimeEvent) {
        loop {
            if self.dispatch(event) {
                return;
            }
            event = self.runtime.step();
        }
    }

    /// Handle a runtime event. Returns `true` once the runtime is blocked
    /// on user-visible content (dialogue, menu, end screen).
    fn dispatch(&mut self, event: RuntimeEvent) -> bool {
        match event {
            RuntimeEvent::Effect(cmd) => {
                self.apply_effect(cmd);
                false
            }
            RuntimeEvent::Blocking(cmd) => {
                self.set_blocking(cmd, false);
                true
            }
            RuntimeEvent::Menu { prompt, choices } => {
                self.screen = Screen::Menu {
                    prompt: prompt.map(|p| self.tr(&p)),
                    choices: choices.iter().map(|c| self.tr(c)).collect(),
                    hovered: 0,
                };
                true
            }
            RuntimeEvent::Finished => {
                self.screen = Screen::End;
                true
            }
        }
    }

    /// Apply a host-side side effect: audio, transition timers, screen
    /// shake, gallery persistence. Scene state itself is already updated
    /// by the runtime.
    fn apply_effect(&mut self, cmd: Command) {
        // Accessibility: reduce motion flattens every animated effect —
        // transitions play instantly, camera moves snap, shakes/overlay
        // transitions are dropped. Scene state still converges because
        // durations of zero finish on the first logical tick.
        if self.settings.reduce_motion {
            let scene = &mut self.runtime.scene;
            match cmd {
                Command::Scene { .. } => {
                    scene.prev_background = None;
                    scene.prev_bg_opacity = 0.0;
                    scene.bg_opacity = 1.0;
                    scene.bg_zoom = 1.0;
                    scene.prev_bg_zoom = 1.0;
                    scene.fade_overlay = [0.0; 4];
                    return;
                }
                Command::Show(_) | Command::Hide { .. } => {
                    for s in &mut scene.sprites {
                        s.entering = None;
                    }
                    scene.exiting.clear();
                    return;
                }
                Command::Camera { .. } => {
                    if let Some(anim) = scene.camera_anim.take() {
                        scene.camera = anim.to;
                    }
                    return;
                }
                Command::Shake { .. } | Command::PlayTransition(_) => return,
                _ => {}
            }
        }
        match cmd {
            Command::Scene { transition, .. } => {
                let duration = transition.default_duration();
                if duration > 0.0 && self.runtime.scene.prev_background.is_some() {
                    self.transition = Some(ActiveTransition {
                        transition,
                        elapsed: 0.0,
                        duration,
                    });
                }
            }
            // PlayMusic/StopMusic are handled by `resync_music`, which
            // follows `runtime.music` and also covers load/rollback.
            Command::PlaySfx { track, volume } => {
                self.audio
                    .play_sfx(&*self.assets, &self.config.audio_dir, &track, volume);
            }
            Command::PlayVoice { track } => {
                self.audio
                    .play_voice(&*self.assets, &self.config.voice_dir, &track);
            }
            Command::StopVoice => self.audio.stop_voice(),
            Command::PlayTransition(transition) => {
                let duration = transition.default_duration();
                self.transition = Some(ActiveTransition {
                    transition,
                    elapsed: 0.0,
                    duration,
                });
            }
            Command::Shake {
                duration,
                intensity,
            } => {
                self.shake = Some((duration, intensity));
            }
            Command::PluginCall { plugin, payload } => {
                if let Some(host) = &mut self.plugins {
                    host.send_command(&mut self.runtime.plugin_kv, &plugin, &payload);
                }
            }
            Command::RegisterCg { .. } => {
                let gallery_path = self
                    .save_root
                    .join(&self.config.save_dir)
                    .join("gallery.json");
                let _ = self.runtime.gallery.save(&gallery_path);
            }
            _ => {}
        }
    }

    /// Display a blocking command (dialogue line or pause). When
    /// `complete` is true the text is shown fully revealed, skipping the
    /// typewriter (used when restoring a save).
    fn set_blocking(&mut self, cmd: Command, complete: bool) {
        let (text, speaker, side_image) = match &cmd {
            Command::Say {
                speaker,
                text,
                side_image,
                ..
            } => (text.clone(), speaker.clone(), side_image.clone()),
            Command::Narrate { text } => (text.clone(), None, None),
            _ => (String::new(), None, None),
        };
        let text = self.tr(&text);
        let speaker = speaker.map(|s| self.tr(&s));
        // Voice playback for Say.
        if let Command::Say {
            voice: Some(track), ..
        } = &cmd
        {
            self.audio
                .play_voice(&*self.assets, &self.config.voice_dir, track);
        }
        let typewriter_done =
            complete || !matches!(cmd, Command::Say { .. } | Command::Narrate { .. });
        let chars_shown = if typewriter_done {
            vnengine_render::richtext::visible_char_count(&text)
        } else {
            0
        };
        self.blocking = Some(BlockingState {
            command: cmd,
            chars_shown,
            typewriter_done,
            shown_at: Instant::now(),
            text,
            speaker,
            side_image,
        });
        self.screen = Screen::Dialogue;
    }

    /// Localize a dialogue/UI string via the active translation table
    /// (identity when no translation is loaded).
    fn tr(&self, text: &str) -> String {
        self.translations
            .get(text)
            .cloned()
            .unwrap_or_else(|| text.to_string())
    }

    /// Discover locales under `translations/<lang>.json` and load the
    /// table for `settings.language` (empty table if missing).
    fn reload_translations(&mut self) {
        self.locales = self
            .assets
            .list("translations")
            .into_iter()
            .filter_map(|f| {
                f.strip_prefix("translations/")
                    .and_then(|n| n.strip_suffix(".json"))
                    .map(str::to_string)
            })
            .collect();
        self.locales.sort();
        let path = format!("translations/{}.json", self.settings.language);
        self.translations = self
            .assets
            .read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
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
        // A pause command is not skippable: the runtime's logical clock
        // expires it via `Runtime::update`.
        if matches!(b.command, Command::Pause { .. }) {
            return;
        }
        self.blocking = None;
        self.step_vm();
    }

    /// Apply `runtime.music` to the audio engine whenever it drifts —
    /// covers live playback, save/load and rollback/rollforward.
    fn resync_music(&mut self) {
        if self.runtime.music == self.last_bgm {
            return;
        }
        match &self.runtime.music {
            Some(track) => self.audio.play_music(
                &*self.assets,
                &self.config.audio_dir,
                track,
                self.runtime.music_fade,
            ),
            None => self.audio.stop_music(self.runtime.music_fade),
        }
        self.last_bgm = self.runtime.music.clone();
    }

    /// Apply `runtime.music_layers` to the audio engine whenever it
    /// drifts — covers live `music layer` commands, save/load and
    /// rollback/rollforward, like `resync_music` for the base track.
    fn resync_layers(&mut self) {
        if self.runtime.music_layers == self.last_layers {
            return;
        }
        let fade = self.runtime.music_layer_fade;
        for (name, l) in &self.runtime.music_layers {
            if self.last_layers.get(name) != Some(l) {
                self.audio.set_music_layer(
                    &*self.assets,
                    &self.config.audio_dir,
                    name,
                    Some(&l.track),
                    Some(l.volume),
                    fade,
                );
            }
        }
        for name in self.last_layers.keys() {
            if !self.runtime.music_layers.contains_key(name) {
                self.audio.remove_music_layer(name, fade);
            }
        }
        self.last_layers = self.runtime.music_layers.clone();
    }

    /// Poll the entry script's mtime (~2 Hz) and hot-reload it in
    /// place when it changes. Parse errors keep the old script —
    /// development never crashes on a half-saved file.
    fn poll_hot_reload(&mut self, dt: f32) {
        let Some(path) = &self.script_path else {
            return;
        };
        self.watch_timer += dt;
        if self.watch_timer < 0.5 {
            return;
        }
        self.watch_timer = 0.0;
        let Ok(mtime) = std::fs::metadata(path).and_then(|m| m.modified()) else {
            return;
        };
        if self.script_mtime == Some(mtime) {
            return;
        }
        self.script_mtime = Some(mtime);
        match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|src| Parser::parse(&src).map_err(|e| format!("{e:?}")))
        {
            Ok(script) => {
                self.runtime.reload_script(script.clone());
                self.script = script;
                // Edited image files reload too.
                self.renderer.textures.clear();
                log::info!("hot reload: {}", path.display());
            }
            Err(e) => log::warn!("hot reload skipped — script error: {e}"),
        }
    }

    /// Select a menu choice.
    fn select_choice(&mut self, index: usize) {
        if self.runtime.is_on_menu() {
            self.runtime.resume_menu(index);
            if let Some(host) = &mut self.plugins {
                host.notify_choice(&mut self.runtime.plugin_kv, index);
            }
            self.blocking = None;
            self.screen = Screen::Dialogue;
            self.step_vm();
        }
    }

    /// Update the typewriter effect and auto-advance.
    fn update(&mut self, dt: f32) {
        self.play_time += dt;

        // Advance the runtime's logical clock (expires `pause` commands).
        if let Some(event) = self.runtime.update(dt) {
            self.dispatch(event);
        }

        // Plugin tick: each WASM module runs within its fuel/memory
        // budget; KV state is swapped through `runtime.plugin_kv` so it
        // persists with saves and rollback.
        if let Some(host) = &mut self.plugins {
            host.tick_all(&mut self.runtime.plugin_kv, dt);
        }

        // Keep BGM playback in sync with the runtime's tracked music
        // state — this is also what restores music after load/rollback.
        self.resync_music();
        self.resync_layers();
        self.poll_hot_reload(dt);

        // Auto-save every 60 seconds.
        self.auto_save_timer += dt;
        if self.auto_save_timer >= 60.0 {
            self.auto_save_timer = 0.0;
            self.auto_save();
        }

        // Decay screen shake.
        if let Some((remaining, _)) = &mut self.shake {
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
                Transition::Fade { color, .. } => {
                    // Two-phase fade through the overlay color: the old
                    // scene vanishes into it, then the new one emerges.
                    if progress < 0.5 {
                        self.runtime.scene.bg_opacity = 0.0;
                        self.runtime.scene.prev_bg_opacity = 1.0 - progress * 2.0;
                    } else {
                        self.runtime.scene.prev_bg_opacity = 0.0;
                        self.runtime.scene.bg_opacity = progress * 2.0 - 1.0;
                    }
                    let overlay = (1.0 - (progress * 2.0 - 1.0).abs()).max(0.0);
                    self.runtime.scene.fade_overlay = [color[0], color[1], color[2], overlay];
                }
                Transition::Dissolve { .. } => {
                    self.runtime.scene.bg_opacity = progress;
                    self.runtime.scene.prev_bg_opacity = 1.0 - progress;
                }
                Transition::Cut => {
                    self.runtime.scene.bg_opacity = 1.0;
                    self.runtime.scene.prev_bg_opacity = 0.0;
                }
                Transition::Slide { direction, .. } => {
                    self.runtime.scene.bg_opacity = 1.0;
                    self.runtime.scene.prev_bg_opacity = 1.0 - progress;
                    let offset = match direction {
                        SlideDirection::Left => [progress, 0.0],
                        SlideDirection::Right => [-progress, 0.0],
                        SlideDirection::Up => [0.0, progress],
                        SlideDirection::Down => [0.0, -progress],
                    };
                    // Apply offset to sprites (simplified).
                    for s in &mut self.runtime.scene.sprites {
                        s.offset = offset;
                    }
                }
                Transition::Zoom { .. } => {
                    // New background zooms in from 80% while fading in;
                    // the old one zooms out as it fades.
                    self.runtime.scene.bg_opacity = progress;
                    self.runtime.scene.prev_bg_opacity = 1.0 - progress;
                    self.runtime.scene.bg_zoom = 0.8 + 0.2 * progress;
                    self.runtime.scene.prev_bg_zoom = 1.0 + 0.3 * progress;
                }
            }
            if t.elapsed >= t.duration {
                self.runtime.scene.prev_background = None;
                self.runtime.scene.prev_bg_opacity = 0.0;
                self.runtime.scene.bg_opacity = 1.0;
                self.runtime.scene.bg_zoom = 1.0;
                self.runtime.scene.prev_bg_zoom = 1.0;
                self.runtime.scene.fade_overlay = [0.0; 4];
                for s in &mut self.runtime.scene.sprites {
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
        if self.skip && self.blocking.is_some() {
            self.advance();
        }
    }

    /// Handle input actions.
    fn handle_input(&mut self) {
        // Global actions (work on most screens).
        if self.input.pressed(Action::Fullscreen) {
            self.settings.fullscreen = !self.settings.fullscreen;
        }
        if self.input.pressed(Action::Debug) {
            self.debug_overlay = !self.debug_overlay;
        }
        // Studio label jump: `[`/`]` pick a target, F10 executes it.
        // Only active while the overlay is open; normal play unaffected.
        if self.debug_overlay {
            let n = self.runtime.vm.script.labels.len();
            if n > 0 {
                if self.input.pressed(Action::DebugNext) {
                    self.debug_label = (self.debug_label + 1) % n;
                }
                if self.input.pressed(Action::DebugPrev) {
                    self.debug_label = (self.debug_label + n - 1) % n;
                }
                self.debug_label = self.debug_label.min(n - 1);
                if self.input.pressed(Action::DebugJump) {
                    let name = self.runtime.vm.script.labels[self.debug_label].name.clone();
                    if self.runtime.jump_to_label(&name) {
                        self.blocking = None;
                        self.screen = Screen::Dialogue;
                        self.step_vm();
                    }
                }
            }
        }

        // Handle confirm dialog first — it intercepts most input.
        if let Screen::Confirm {
            message,
            selected,
            action,
        } = &self.screen.clone()
        {
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
            self.screen = Screen::Confirm {
                message,
                selected,
                action,
            };
            return;
        }

        // Screen-specific input.
        match &self.screen.clone() {
            Screen::Title { selected } => {
                let mut sel = *selected;
                let buttons = [
                    "New Game", "Continue", "Load", "Settings", "Gallery", "Quit",
                ];
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
                    self.screen = Screen::SaveLoad {
                        mode: SaveLoadMode::Save,
                        selected: 0,
                    };
                    return;
                }
                if self.input.pressed(Action::Load) {
                    self.screen = Screen::SaveLoad {
                        mode: SaveLoadMode::Load,
                        selected: 0,
                    };
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
                            self.screen = Screen::SaveLoad {
                                mode: SaveLoadMode::Save,
                                selected: 0,
                            };
                            return;
                        }
                        QuickMenuAction::Load => {
                            self.screen = Screen::SaveLoad {
                                mode: SaveLoadMode::Load,
                                selected: 0,
                            };
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
                    self.screen = if self.runtime.vm.state.finished {
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
                            if self.runtime.vm.state.finished {
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
                                    if !self.runtime.vm.state.finished {
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
                self.screen = Screen::SaveLoad {
                    mode,
                    selected: sel,
                };
            }
            Screen::Settings { selected } => {
                let mut sel = *selected;
                let n_settings = 13;
                if self.input.pressed(Action::Down) {
                    sel = (sel + 1) % n_settings;
                }
                if self.input.pressed(Action::Up) {
                    sel = (sel + n_settings - 1) % n_settings;
                }
                if self.input.pressed(Action::Cancel) {
                    self.save_settings();
                    self.screen = if self.runtime.vm.state.finished {
                        Screen::Title { selected: 0 }
                    } else {
                        Screen::Dialogue
                    };
                    return;
                }
                // Adjust settings with left/right.
                if self.input.pressed(Action::Left) || self.input.pressed(Action::Right) {
                    let dir: f64 = if self.input.pressed(Action::Right) {
                        1.0
                    } else {
                        -1.0
                    };
                    match sel {
                        0 => {
                            self.settings.master_volume =
                                (self.settings.master_volume + dir * 0.1).clamp(0.0, 1.0)
                        }
                        1 => {
                            self.settings.music_volume =
                                (self.settings.music_volume + dir * 0.1).clamp(0.0, 1.0)
                        }
                        2 => {
                            self.settings.sfx_volume =
                                (self.settings.sfx_volume + dir * 0.1).clamp(0.0, 1.0)
                        }
                        3 => {
                            self.settings.voice_volume =
                                (self.settings.voice_volume + dir * 0.1).clamp(0.0, 1.0)
                        }
                        4 => {
                            self.settings.text_speed =
                                (self.settings.text_speed + dir as f32 * 10.0).clamp(0.0, 200.0)
                        }
                        5 => {
                            self.settings.auto_advance_delay = (self.settings.auto_advance_delay
                                + dir as f32 * 0.5)
                                .clamp(0.0, 10.0)
                        }
                        6 => self.settings.fullscreen = !self.settings.fullscreen,
                        7 => self.settings.auto_advance = !self.settings.auto_advance,
                        8 => self.settings.skip_unread = !self.settings.skip_unread,
                        9 => self.settings.muted = !self.settings.muted,
                        10 => {
                            // Cycle shipped locales ("" = no translation).
                            let mut langs = vec![String::new()];
                            langs.extend(self.locales.iter().cloned());
                            let cur = langs
                                .iter()
                                .position(|l| *l == self.settings.language)
                                .unwrap_or(0);
                            let next = (cur as isize + dir as isize)
                                .rem_euclid(langs.len() as isize)
                                as usize;
                            self.settings.language = langs[next].clone();
                            self.reload_translations();
                        }
                        11 => {
                            self.settings.text_scale =
                                (self.settings.text_scale + dir as f32 * 0.1).clamp(0.5, 2.0)
                        }
                        12 => self.settings.reduce_motion = !self.settings.reduce_motion,
                        _ => {}
                    }
                    self.audio.apply_settings(self.settings.clone());
                }
                self.screen = Screen::Settings { selected: sel };
            }
            Screen::Gallery { selected } => {
                let mut sel = *selected;
                let n = self.runtime.gallery.unlocked.len();
                if n == 0 {
                    if self.input.pressed(Action::Cancel) || self.input.pressed(Action::Advance) {
                        self.screen = if self.runtime.vm.state.finished {
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
                    self.screen = if self.runtime.vm.state.finished {
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
                let max_scroll = self.runtime.history.len().saturating_sub(1);
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
            Screen::End if self.input.advance_requested() || self.input.pressed(Action::Cancel) => {
                self.screen = Screen::Title { selected: 0 };
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
                let save_dir = self.save_root.join(&self.config.save_dir);
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
                self.screen = Screen::SaveLoad {
                    mode: SaveLoadMode::Load,
                    selected: 0,
                };
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
                self.runtime.vm.state.finished = true;
                self.screen = Screen::End;
            }
        }
    }

    /// Save settings to disk.
    fn save_settings(&self) {
        let settings_path = self
            .save_root
            .join(&self.config.save_dir)
            .join("settings.json");
        let _ = self.settings.save(&settings_path);
    }

    /// Rollback to the previous blocking point (Ren'Py-style).
    fn rollback(&mut self) {
        self.blocking = None;
        if let Some(event) = self.runtime.rollback() {
            self.pump(event);
        }
    }

    /// Rollforward (undo a rollback).
    fn rollforward(&mut self) {
        self.blocking = None;
        if let Some(event) = self.runtime.rollforward() {
            self.pump(event);
        }
    }

    /// Auto-save to slot 0.
    fn auto_save(&mut self) {
        if !self.runtime.vm.state.finished {
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

                    // Side image (character portrait next to dialogue box).
                    if let Some(side) = &b.side_image {
                        ui.images.push(vnengine_render::renderer::UiImage {
                            path: side.clone(),
                            left: w * 0.75,
                            top: box_y * h - h * 0.15,
                            size: Some((w * 0.2, h * 0.2)),
                            opacity: 1.0,
                        });
                    }

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
            Screen::Menu {
                prompt,
                choices,
                hovered,
            } => {
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
                let buttons = [
                    "New Game", "Continue", "Load", "Settings", "Gallery", "Quit",
                ];
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
                let save_dir = self.save_root.join(&self.config.save_dir);
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
                    format!(
                        "Auto-Advance Delay: {:.1}s",
                        self.settings.auto_advance_delay
                    ),
                    format!("Fullscreen: {}", self.settings.fullscreen),
                    format!("Auto-Advance: {}", self.settings.auto_advance),
                    format!("Skip Unread: {}", self.settings.skip_unread),
                    format!("Muted: {}", self.settings.muted),
                    format!(
                        "Language: {}",
                        if self.settings.language.is_empty() {
                            "default"
                        } else {
                            &self.settings.language
                        }
                    ),
                    format!("Text Scale: {:.0}%", self.settings.text_scale * 100.0),
                    format!("Reduce Motion: {}", self.settings.reduce_motion),
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

                if self.runtime.gallery.unlocked.is_empty() {
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
                    for (i, cg) in self.runtime.gallery.unlocked.iter().enumerate() {
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
                let total = self.runtime.history.len();
                let start_idx = total.saturating_sub(max_display + *scroll);
                let end_idx = total.saturating_sub(*scroll);
                let start_y = h * 0.08;
                let line_h = 35.0;
                for (i, entry) in self.runtime.history[start_idx..end_idx].iter().enumerate() {
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
            Screen::Confirm {
                message, selected, ..
            } => {
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

        if self.debug_overlay {
            self.debug_panel(&mut ui);
        }
        // Accessibility: global text scale (settings.text_scale).
        if (self.settings.text_scale - 1.0).abs() > f32::EPSILON {
            for t in &mut ui.text {
                t.font_size *= self.settings.text_scale;
            }
        }
        ui
    }

    /// Studio overlay (F8): current position, variables, scene state,
    /// music/layers and plugin KV — live inspection for creators.
    fn debug_panel(&self, ui: &mut UiDraw) {
        let (w, h) = (self.window_size.0 as f32, self.window_size.1 as f32);
        let px = w * 0.62;
        let py = h * 0.02;
        let pw = w * 0.36;
        let ph = h * 0.5;
        ui.boxes.push(UiBox {
            x: px / w,
            y: py / h,
            w: pw / w,
            h: ph / h,
            color: [0.0, 0.0, 0.0, 0.75],
        });
        // Normalize to pixels for text placement.
        let line_h = 16.0;
        let mut lines: Vec<String> = Vec::new();
        if let Some(f) = self.runtime.vm.state.stack.last() {
            let label = match &f.body {
                vnengine_script::vm::FrameBody::Label(l) => l.clone(),
                vnengine_script::vm::FrameBody::Inline(_) => "<inline>".into(),
            };
            lines.push(format!(
                "frame: {label} +{} (depth {})",
                f.stmt,
                self.runtime.vm.state.stack.len()
            ));
        }
        let labels = &self.runtime.vm.script.labels;
        if !labels.is_empty() {
            let sel = self.debug_label.min(labels.len() - 1);
            lines.push(format!("jump: [{}] {}  (F10)", sel, labels[sel].name));
        }
        lines.push(format!(
            "music: {}",
            self.runtime.music.as_deref().unwrap_or("-")
        ));
        for (n, l) in &self.runtime.music_layers {
            lines.push(format!("  layer {n}: {} vol {:.2}", l.track, l.volume));
        }
        let cam = self.runtime.scene.camera;
        lines.push(format!(
            "camera: ({:.2}, {:.2}) zoom {:.2} | lights: {} ambient {:.2}",
            cam.x,
            cam.y,
            cam.zoom,
            self.runtime.scene.lights.len(),
            self.runtime.scene.ambient[0]
        ));
        lines.push(format!(
            "sprites: {} exiting: {} | plugins kv: {}",
            self.runtime.scene.sprites.len(),
            self.runtime.scene.exiting.len(),
            self.runtime.plugin_kv.len()
        ));
        let mut vars: Vec<_> = self.runtime.vm.state.variables.iter().collect();
        vars.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in vars.iter().take(14) {
            lines.push(format!("{k} = {v:?}"));
        }
        if vars.len() > 14 {
            lines.push(format!("… {} more vars", vars.len() - 14));
        }
        for (i, line) in lines.iter().enumerate() {
            ui.text.push(UiText {
                text: line.clone(),
                left: px + 10.0,
                top: py + 8.0 + i as f32 * line_h,
                font_size: 13.0,
                color: [0.7, 0.95, 0.7],
                bounds: None,
                rich: false,
            });
        }
    }

    /// Save the game to a slot.
    pub fn save(&self, slot: u32) -> anyhow::Result<()> {
        let save_dir = self.save_root.join(&self.config.save_dir);
        let snap = self.runtime.capture();
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
            vm_state: serde_json::to_vec(&snap.vm_state)?,
            scene: snap.scene,
            blocking: self.blocking.as_ref().map(|b| b.command.clone()),
            music: snap.music,
            journal: snap.journal,
            plugin_kv: snap.plugin_kv,
            music_layers: snap.music_layers,
            thumbnail,
            saved_at: chrono::Utc::now().timestamp(),
        };
        state.write(&save_dir)?;
        Ok(())
    }

    /// Generate a small PNG thumbnail from the current background image.
    fn generate_thumbnail(&self) -> Option<Vec<u8>> {
        let bg = self.runtime.scene.background.as_ref()?;
        let (_, bytes) = self
            .assets
            .read_any(&format!("{}/{bg}", self.config.image_dir), IMAGE_EXTS)
            .ok()?;
        let img = image::load_from_memory(&bytes).ok()?;
        let thumb = img.resize_to_fill(160, 90, image::imageops::FilterType::Lanczos3);
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        thumb
            .to_rgba8()
            .write_to(&mut cursor, image::ImageFormat::Png)
            .ok()?;
        Some(buf)
    }

    /// Load the game from a slot.
    pub fn load(&mut self, slot: u32) -> anyhow::Result<()> {
        let save_dir = self.save_root.join(&self.config.save_dir);
        let state = SaveState::read(&save_dir, slot)?;
        let vm_state: vnengine_script::vm::VmState = serde_json::from_slice(&state.vm_state)?;
        self.runtime = Runtime::restore(
            self.script.clone(),
            RuntimeSnapshot {
                vm_state,
                scene: state.scene,
                music: state.music,
                journal: state.journal,
                plugin_kv: state.plugin_kv,
                music_layers: state.music_layers,
            },
        );
        self.blocking = None;
        self.transition = None;
        if self.runtime.is_on_menu() {
            // The save was taken on a menu: re-display it.
            let event = self.runtime.resume_display();
            self.dispatch(event);
        } else if let Some(cmd) = state.blocking {
            // Re-display the line that was shown when saving (fully
            // revealed, no typewriter replay).
            self.set_blocking(cmd, true);
        } else {
            self.screen = Screen::Dialogue;
            self.step_vm();
        }
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
            && !app.runtime.vm.state.finished
            && app.blocking.is_none()
            && !app.runtime.is_on_menu()
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
                app.input.mouse_move(position.x as f32, position.y as f32);
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
                let dt = now.duration_since(app.last_frame).as_secs_f32().min(0.1);
                app.last_frame = now;
                app.handle_input();
                app.update(dt);
                let ui = app.build_ui();
                let mut scene = app.runtime.scene.clone();
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
                let App {
                    renderer,
                    assets,
                    config,
                    ..
                } = &mut *app;
                if let Err(e) = renderer.render(&scene, &**assets, &config.image_dir, &ui) {
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
