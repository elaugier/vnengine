//! Audio engine wrapping kira for BGM, SFX and voice playback.
//!
//! All sounds play on the main mixer track. Per-category volume is applied
//! to each sound individually via its `volume` setting, and the master
//! volume is applied to the main track.

use kira::sound::static_sound::{StaticSoundData, StaticSoundSettings};
use kira::{AudioManager as KiraManager, AudioManagerSettings, Decibels, DefaultBackend, Tween};
use std::time::Duration;
use vnengine_core::assets::{AssetSource, AUDIO_EXTS};
use vnengine_core::settings::UserSettings;

/// A looping adaptive-music stem layered over the base BGM.
struct StemLayer {
    /// Track path currently loaded (re-started when it changes).
    track: String,
    handle: kira::sound::static_sound::StaticSoundHandle,
}

pub struct AudioEngine {
    manager: KiraManager<DefaultBackend>,
    /// Handle to the currently playing BGM (so we can stop it).
    current_bgm: Option<kira::sound::static_sound::StaticSoundHandle>,
    /// Handle to the currently playing voice line.
    current_voice: Option<kira::sound::static_sound::StaticSoundHandle>,
    /// Adaptive-music stems keyed by layer name.
    layers: std::collections::HashMap<String, StemLayer>,
    /// Cached settings for volume calculations.
    settings: UserSettings,
}

impl AudioEngine {
    pub fn new(settings: UserSettings) -> anyhow::Result<Self> {
        let manager = KiraManager::<DefaultBackend>::new(AudioManagerSettings::default())?;
        let mut audio = Self {
            manager,
            current_bgm: None,
            current_voice: None,
            layers: std::collections::HashMap::new(),
            settings: UserSettings::default(),
        };
        audio.apply_settings(settings);
        Ok(audio)
    }

    /// Apply user settings (master volume, mute) to the main track.
    pub fn apply_settings(&mut self, settings: UserSettings) {
        self.settings = settings.clone();
        let master = if settings.muted {
            Decibels::SILENCE
        } else {
            db_from_linear(settings.master_volume)
        };
        self.manager
            .main_track()
            .set_volume(master, Tween::default());
    }

    /// Play a looping BGM track. Stops any currently playing BGM.
    /// `track` is relative to `dir` (the audio dir of the project).
    pub fn play_music(&mut self, src: &dyn AssetSource, dir: &str, track: &str, fade: f32) {
        let vol = db_from_linear(self.settings.music_volume);
        let settings = StaticSoundSettings::new().loop_region(0.0..).volume(vol);
        let data = match load_sound(src, dir, track, settings) {
            Ok(data) => data,
            Err(e) => {
                log::warn!("failed to load music `{dir}/{track}`: {e}");
                return;
            }
        };
        self.stop_all_layers(fade);
        if let Some(mut h) = self.current_bgm.take() {
            let tween = if fade > 0.0 {
                Tween {
                    duration: Duration::from_secs_f32(fade),
                    ..Default::default()
                }
            } else {
                Tween::default()
            };
            h.stop(tween);
        }
        match self.manager.play(data) {
            Ok(handle) => {
                self.current_bgm = Some(handle);
            }
            Err(e) => log::warn!("failed to play music `{track}`: {e}"),
        }
    }

    /// Stop the currently playing BGM and all stem layers.
    pub fn stop_music(&mut self, fade: f32) {
        self.stop_all_layers(fade);
        if let Some(mut h) = self.current_bgm.take() {
            let tween = if fade > 0.0 {
                Tween {
                    duration: Duration::from_secs_f32(fade),
                    ..Default::default()
                }
            } else {
                Tween::default()
            };
            h.stop(tween);
        }
    }

    fn stop_all_layers(&mut self, fade: f32) {
        for (_, mut l) in self.layers.drain() {
            l.handle.stop(tween_of(fade));
        }
    }

    /// Upsert or retune an adaptive-music stem. A new or changed `track`
    /// starts its loop seeked to the base BGM's position so stems stay
    /// in sync; `volume` tweens with `fade`.
    pub fn set_music_layer(
        &mut self,
        src: &dyn AssetSource,
        dir: &str,
        name: &str,
        track: Option<&str>,
        volume: Option<f32>,
        fade: f32,
    ) {
        let tween = tween_of(fade);
        // Retune an existing stem in place when the track is unchanged.
        if let Some(l) = self.layers.get_mut(name) {
            if track.map(|t| t == l.track).unwrap_or(true) {
                if let Some(v) = volume {
                    let vol = db_from_linear(self.settings.music_volume * v as f64);
                    l.handle.set_volume(vol, tween);
                }
                return;
            }
            // Track changed: stop the old stem, fall through to reload.
            if let Some(mut l) = self.layers.remove(name) {
                l.handle.stop(tween);
            }
        }
        let Some(track) = track else { return };
        let v = volume.unwrap_or(1.0);
        let vol = db_from_linear(self.settings.music_volume * v as f64);
        let settings = StaticSoundSettings::new().loop_region(0.0..).volume(vol);
        let data = match load_sound(src, dir, track, settings) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("failed to load music layer `{dir}/{track}`: {e}");
                return;
            }
        };
        match self.manager.play(data) {
            Ok(mut handle) => {
                // Sync the stem to the base track's playback position.
                if let Some(bgm) = &self.current_bgm {
                    handle.seek_to(bgm.position());
                }
                self.layers.insert(
                    name.to_string(),
                    StemLayer {
                        track: track.to_string(),
                        handle,
                    },
                );
            }
            Err(e) => log::warn!("failed to play music layer `{track}`: {e}"),
        }
    }

    /// Remove a stem layer with a fade-out.
    pub fn remove_music_layer(&mut self, name: &str, fade: f32) {
        if let Some(mut l) = self.layers.remove(name) {
            l.handle.stop(tween_of(fade));
        }
    }

    /// Play a one-shot sound effect.
    pub fn play_sfx(&mut self, src: &dyn AssetSource, dir: &str, track: &str, volume: f32) {
        let vol = db_from_linear(self.settings.sfx_volume * volume as f64);
        let settings = StaticSoundSettings::new().volume(vol);
        match load_sound(src, dir, track, settings) {
            Ok(data) => {
                if let Err(e) = self.manager.play(data) {
                    log::warn!("failed to play sfx `{track}`: {e}");
                }
            }
            Err(e) => log::warn!("failed to load sfx `{dir}/{track}`: {e}"),
        }
    }

    /// Play a voice line. Stops any currently playing voice.
    pub fn play_voice(&mut self, src: &dyn AssetSource, dir: &str, track: &str) {
        let vol = db_from_linear(self.settings.voice_volume);
        let settings = StaticSoundSettings::new().volume(vol);
        match load_sound(src, dir, track, settings) {
            Ok(data) => {
                if let Some(mut h) = self.current_voice.take() {
                    h.stop(Tween::default());
                }
                match self.manager.play(data) {
                    Ok(handle) => {
                        self.current_voice = Some(handle);
                    }
                    Err(e) => log::warn!("failed to play voice `{track}`: {e}"),
                }
            }
            Err(e) => log::warn!("failed to load voice `{dir}/{track}`: {e}"),
        }
    }

    /// Stop any playing voice line.
    pub fn stop_voice(&mut self) {
        if let Some(mut h) = self.current_voice.take() {
            h.stop(Tween::default());
        }
    }
}

/// Read `dir/track` (probing common audio extensions) and decode it into
/// a `StaticSoundData`.
fn load_sound(
    src: &dyn AssetSource,
    dir: &str,
    track: &str,
    settings: StaticSoundSettings,
) -> anyhow::Result<StaticSoundData> {
    let (_, bytes) = src.read_any(&format!("{dir}/{track}"), AUDIO_EXTS)?;
    Ok(StaticSoundData::from_cursor(std::io::Cursor::new(bytes))?.with_settings(settings))
}

fn tween_of(secs: f32) -> Tween {
    if secs > 0.0 {
        Tween {
            duration: Duration::from_secs_f32(secs),
            ..Default::default()
        }
    } else {
        Tween::default()
    }
}

/// Convert a linear volume (0..1) to decibels.
pub fn db_from_linear(linear: f64) -> Decibels {
    if linear <= 0.0 {
        return Decibels::SILENCE;
    }
    Decibels(20.0 * linear.log10() as f32)
}
