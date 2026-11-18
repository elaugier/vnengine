//! Audio engine wrapping kira for BGM, SFX and voice playback.
//!
//! All sounds play on the main mixer track. Per-category volume is applied
//! to each sound individually via its `volume` setting, and the master
//! volume is applied to the main track.

use kira::effect::filter::{FilterBuilder, FilterHandle, FilterMode};
use kira::effect::reverb::{ReverbBuilder, ReverbHandle};
use kira::sound::static_sound::{StaticSoundData, StaticSoundSettings};
use kira::sound::PlaybackState;
use kira::track::{TrackBuilder, TrackHandle};
use kira::{AudioManager as KiraManager, AudioManagerSettings, Decibels, DefaultBackend, Mix, Tween};
use std::time::Duration;
use vnengine_core::assets::{AssetSource, AUDIO_EXTS};
use vnengine_core::command::ChannelFilter;
use vnengine_core::save::ChannelState;
use vnengine_core::settings::UserSettings;

/// A looping adaptive-music stem layered over the base BGM.
struct StemLayer {
    /// Track path currently loaded (re-started when it changes).
    track: String,
    /// Layer gain (0..1) — needed to recompute ducked volume.
    volume: f32,
    handle: kira::sound::static_sound::StaticSoundHandle,
}

/// One named audio channel: a lazily-created kira sub-track carrying a
/// permanent (initially bypassed) filter + reverb so `filter channel`
/// can retune them live through their handles.
struct ChannelAudio {
    track: TrackHandle,
    playing: Option<kira::sound::static_sound::StaticSoundHandle>,
    /// Track path loaded in `playing` — drives the resync diff.
    playing_track: String,
    filter: FilterHandle,
    reverb: ReverbHandle,
    /// Last applied effect config (idempotent resync).
    applied_filter: Option<ChannelFilter>,
}

pub struct AudioEngine {
    manager: KiraManager<DefaultBackend>,
    /// Handle to the currently playing BGM (so we can stop it).
    current_bgm: Option<kira::sound::static_sound::StaticSoundHandle>,
    /// Handle to the currently playing voice line.
    current_voice: Option<kira::sound::static_sound::StaticSoundHandle>,
    /// Adaptive-music stems keyed by layer name.
    layers: std::collections::HashMap<String, StemLayer>,
    /// Named channels (`play/queue channel`), keyed by channel name.
    channels: std::collections::HashMap<String, ChannelAudio>,
    /// True while voice playback is ducking the music category.
    ducked: bool,
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
            channels: std::collections::HashMap::new(),
            ducked: false,
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
        let vol = db_from_linear(self.settings.music_volume * self.duck_factor());
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
        let duck = self.duck_factor();
        // Retune an existing stem in place when the track is unchanged.
        if let Some(l) = self.layers.get_mut(name) {
            if track.map(|t| t == l.track).unwrap_or(true) {
                if let Some(v) = volume {
                    l.volume = v;
                    let vol = db_from_linear(self.settings.music_volume * v as f64 * duck);
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
        let vol = db_from_linear(self.settings.music_volume * v as f64 * self.duck_factor());
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
                        volume: v,
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

    // ---- Named channels (Phase 4) -------------------------------------

    /// Duck multiplier applied to the music category while a voice
    /// line is playing (`settings.voice_duck`; 1.0 = disabled).
    fn duck_factor(&self) -> f64 {
        if self.ducked {
            self.settings.voice_duck
        } else {
            1.0
        }
    }

    /// Per-frame host call: (re)applies voice ducking on the music
    /// category. Duck = `voice_duck` while `current_voice` reports
    /// Playing, restored when it stops.
    pub fn update(&mut self) {
        let voice_playing = self
            .current_voice
            .as_ref()
            .map(|h| h.state() == PlaybackState::Playing)
            .unwrap_or(false);
        if voice_playing == self.ducked {
            return;
        }
        self.ducked = voice_playing;
        let duck = self.duck_factor();
        let tween = tween_of(0.4);
        if let Some(h) = &mut self.current_bgm {
            h.set_volume(db_from_linear(self.settings.music_volume * duck), tween);
        }
        for l in self.layers.values_mut() {
            l.handle.set_volume(
                db_from_linear(self.settings.music_volume * l.volume as f64 * duck),
                tween,
            );
        }
    }

    /// Get-or-create the channel's sub-track. Returns `None` when kira
    /// refuses the track (resource limit) — logged.
    fn ensure_channel(&mut self, name: &str) -> Option<&mut ChannelAudio> {
        if !self.channels.contains_key(name) {
            let mut builder = TrackBuilder::new();
            let filter = builder.add_effect(FilterBuilder::new().mix(Mix(0.0)));
            let reverb = builder.add_effect(ReverbBuilder::new().mix(Mix(0.0)));
            match self.manager.add_sub_track(builder) {
                Ok(track) => {
                    self.channels.insert(
                        name.to_string(),
                        ChannelAudio {
                            track,
                            playing: None,
                            playing_track: String::new(),
                            filter,
                            reverb,
                            applied_filter: None,
                        },
                    );
                }
                Err(e) => {
                    log::warn!("cannot create audio channel `{name}`: {e}");
                    return None;
                }
            }
        }
        self.channels.get_mut(name)
    }

    /// Reconcile engine state with the runtime's `channels` map
    /// (same diff pattern as `resync_music`/`resync_layers`). Stops
    /// sounds on channels the runtime dropped, starts new `current`
    /// tracks, applies channel gain and effects. Returns the names of
    /// channels whose non-looping `current` track has finished — the
    /// caller feeds them to `Runtime::channel_advance`.
    pub fn resync_channels(
        &mut self,
        src: &dyn AssetSource,
        dir: &str,
        states: &std::collections::BTreeMap<String, ChannelState>,
    ) -> Vec<String> {
        // Drop channels the runtime no longer tracks.
        let stale: Vec<String> = self
            .channels
            .keys()
            .filter(|k| !states.contains_key(k.as_str()))
            .cloned()
            .collect();
        for name in stale {
            if let Some(mut ch) = self.channels.remove(&name) {
                if let Some(mut h) = ch.playing.take() {
                    h.stop(Tween::default());
                }
            }
        }

        let mut finished = Vec::new();
        for (name, st) in states {
            let sfx_vol = self.settings.sfx_volume;
            let Some(ch) = self.ensure_channel(name) else {
                continue;
            };
            // Channel gain rides on the sub-track; per-sound volume is
            // baked into the sound settings.
            ch.track.set_volume(
                db_from_linear(sfx_vol * st.volume as f64),
                Tween::default(),
            );
            match &st.current {
                Some(ct) if ch.playing_track != ct.track => {
                    if let Some(mut h) = ch.playing.take() {
                        h.stop(tween_of(ct.fade));
                    }
                    ch.playing_track.clear();
                    let mut settings =
                        StaticSoundSettings::new().volume(db_from_linear(ct.volume as f64));
                    if ct.loop_ {
                        settings = settings.loop_region(0.0..);
                    }
                    match load_sound(src, dir, &ct.track, settings) {
                        Ok(data) => match ch.track.play(data) {
                            Ok(h) => {
                                ch.playing = Some(h);
                                ch.playing_track = ct.track.clone();
                            }
                            Err(e) => {
                                log::warn!("failed to play channel `{name}` track `{}`: {e}", ct.track)
                            }
                        },
                        Err(e) => {
                            log::warn!("failed to load channel `{name}` track `{dir}/{}`: {e}", ct.track)
                        }
                    }
                }
                None => {
                    if let Some(mut h) = ch.playing.take() {
                        h.stop(tween_of(0.3));
                        ch.playing_track.clear();
                    }
                }
                _ => {}
            }
            // Report a finished non-looping current so the caller can
            // advance the queue (or drop the stale `current`).
            if let Some(ct) = &st.current {
                if !ct.loop_ {
                    let done = match &ch.playing {
                        Some(h) => h.state() == PlaybackState::Stopped,
                        None => false,
                    };
                    if done {
                        finished.push(name.clone());
                    }
                }
            }
            // Apply effect config only when it changed.
            if ch.applied_filter != st.filter {
                apply_channel_filter(ch, st.filter.as_ref());
                ch.applied_filter = st.filter.clone();
            }
        }
        finished
    }

    /// `stop all`: music + layers + voice + every channel sound.
    pub fn stop_all(&mut self, fade: f32) {
        self.stop_music(fade);
        self.stop_voice();
        for (_, ch) in self.channels.iter_mut() {
            if let Some(mut h) = ch.playing.take() {
                h.stop(tween_of(fade));
                ch.playing_track.clear();
            }
        }
    }
}

/// Push a `ChannelFilter` config onto the channel's permanent effects.
/// `Off`/`None` bypasses both; each sound category zeroes the other
/// effect so at most one processing chain is live.
fn apply_channel_filter(ch: &mut ChannelAudio, filter: Option<&ChannelFilter>) {
    match filter {
        Some(ChannelFilter::Filter {
            mode,
            cutoff,
            resonance,
            mix,
        }) => {
            ch.filter.set_mode(match mode.as_str() {
                "highpass" => FilterMode::HighPass,
                "bandpass" => FilterMode::BandPass,
                "notch" => FilterMode::Notch,
                _ => FilterMode::LowPass,
            });
            ch.filter.set_cutoff(*cutoff, Tween::default());
            ch.filter.set_resonance(*resonance, Tween::default());
            ch.filter
                .set_mix(Mix(*mix as f32), Tween::default());
            ch.reverb.set_mix(Mix(0.0), Tween::default());
        }
        Some(ChannelFilter::Reverb {
            feedback,
            damping,
            stereo_width,
            mix,
        }) => {
            ch.reverb.set_feedback(*feedback, Tween::default());
            ch.reverb.set_damping(*damping, Tween::default());
            ch.reverb.set_stereo_width(*stereo_width, Tween::default());
            ch.reverb.set_mix(Mix(*mix as f32), Tween::default());
            ch.filter.set_mix(Mix(0.0), Tween::default());
        }
        Some(ChannelFilter::Off) | None => {
            ch.filter.set_mix(Mix(0.0), Tween::default());
            ch.reverb.set_mix(Mix(0.0), Tween::default());
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
