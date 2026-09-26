//! Audio engine wrapping kira for BGM, SFX and voice playback.
//!
//! All sounds play on the main mixer track. Per-category volume is applied
//! to each sound individually via its `volume` setting, and the master
//! volume is applied to the main track.

use kira::sound::static_sound::{StaticSoundData, StaticSoundSettings};
use kira::{AudioManager as KiraManager, AudioManagerSettings, Decibels, DefaultBackend, Tween};
use std::path::Path;
use std::time::Duration;
use vnengine_core::settings::UserSettings;

pub struct AudioEngine {
    manager: KiraManager<DefaultBackend>,
    /// Handle to the currently playing BGM (so we can stop it).
    current_bgm: Option<kira::sound::static_sound::StaticSoundHandle>,
    /// Handle to the currently playing voice line.
    current_voice: Option<kira::sound::static_sound::StaticSoundHandle>,
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
    pub fn play_music(&mut self, audio_dir: &Path, track: &str, fade: f32) {
        let path = audio_dir.join(track);
        let vol = db_from_linear(self.settings.music_volume);
        let settings = StaticSoundSettings::new().loop_region(0.0..).volume(vol);
        match StaticSoundData::from_file(&path).map(|d| d.with_settings(settings)) {
            Ok(data) => {
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
                    Err(e) => log::warn!("failed to play music `{}`: {e}", path.display()),
                }
            }
            Err(e) => log::warn!("failed to load music `{}`: {e}", path.display()),
        }
    }

    /// Stop the currently playing BGM.
    pub fn stop_music(&mut self, fade: f32) {
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

    /// Play a one-shot sound effect.
    pub fn play_sfx(&mut self, audio_dir: &Path, track: &str, volume: f32) {
        let path = audio_dir.join(track);
        let vol = db_from_linear(self.settings.sfx_volume * volume as f64);
        let settings = StaticSoundSettings::new().volume(vol);
        match StaticSoundData::from_file(&path).map(|d| d.with_settings(settings)) {
            Ok(data) => {
                if let Err(e) = self.manager.play(data) {
                    log::warn!("failed to play sfx `{}`: {e}", path.display());
                }
            }
            Err(e) => log::warn!("failed to load sfx `{}`: {e}", path.display()),
        }
    }

    /// Play a voice line. Stops any currently playing voice.
    pub fn play_voice(&mut self, voice_dir: &Path, track: &str) {
        let path = voice_dir.join(track);
        let vol = db_from_linear(self.settings.voice_volume);
        let settings = StaticSoundSettings::new().volume(vol);
        match StaticSoundData::from_file(&path).map(|d| d.with_settings(settings)) {
            Ok(data) => {
                if let Some(mut h) = self.current_voice.take() {
                    h.stop(Tween::default());
                }
                match self.manager.play(data) {
                    Ok(handle) => {
                        self.current_voice = Some(handle);
                    }
                    Err(e) => log::warn!("failed to play voice `{}`: {e}", path.display()),
                }
            }
            Err(e) => log::warn!("failed to load voice `{}`: {e}", path.display()),
        }
    }

    /// Stop any playing voice line.
    pub fn stop_voice(&mut self) {
        if let Some(mut h) = self.current_voice.take() {
            h.stop(Tween::default());
        }
    }
}

/// Convert a linear volume (0..1) to decibels.
pub fn db_from_linear(linear: f64) -> Decibels {
    if linear <= 0.0 {
        return Decibels::SILENCE;
    }
    Decibels(20.0 * linear.log10() as f32)
}
