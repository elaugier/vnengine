use crate::command::{Anchor, Command, SpriteSpec};
use crate::model::{anchor_x, Scene};
use crate::save::{GalleryState, SaveState};
use crate::settings::UserSettings;
use crate::transition::{SlideDirection, Transition};
use crate::Value;
use std::path::Path;

// ---- Value tests ----

#[test]
fn value_truthy() {
    assert!(Value::Bool(true).is_truthy());
    assert!(!Value::Bool(false).is_truthy());
    assert!(Value::Int(1).is_truthy());
    assert!(!Value::Int(0).is_truthy());
    assert!(Value::Float(0.5).is_truthy());
    assert!(!Value::Float(0.0).is_truthy());
    assert!(Value::Str("hi".into()).is_truthy());
    assert!(!Value::Str("".into()).is_truthy());
    assert!(!Value::None.is_truthy());
}

#[test]
fn value_conversions() {
    assert_eq!(Value::Int(42).as_int(), Some(42));
    assert_eq!(Value::Float(3.7).as_int(), Some(3));
    assert_eq!(Value::Bool(true).as_int(), Some(1));
    assert_eq!(Value::None.as_int(), None);

    assert_eq!(Value::Int(5).as_float(), Some(5.0));
    assert_eq!(Value::Float(2.5).as_float(), Some(2.5));
    assert_eq!(Value::Bool(true).as_float(), None);

    assert_eq!(Value::Int(7).as_str(), Some("7".to_string()));
    assert_eq!(Value::Bool(false).as_str(), Some("false".to_string()));
    assert_eq!(Value::Str("hi".into()).as_str(), Some("hi".to_string()));
}

#[test]
fn value_from() {
    let v: Value = true.into();
    assert_eq!(v, Value::Bool(true));
    let v: Value = 42i64.into();
    assert_eq!(v, Value::Int(42));
    let v: Value = 42i32.into();
    assert_eq!(v, Value::Int(42));
    let v: Value = 2.5f64.into();
    assert_eq!(v, Value::Float(2.5));
    let v: Value = "hello".into();
    assert_eq!(v, Value::Str("hello".to_string()));
    let v: Value = String::from("world").into();
    assert_eq!(v, Value::Str("world".to_string()));
}

#[test]
fn value_serde_roundtrip() {
    let v = Value::Str("test".to_string());
    let json = serde_json::to_string(&v).unwrap();
    let v2: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v, v2);
}

// ---- Transition tests ----

#[test]
fn transition_parse_named() {
    assert_eq!(Transition::parse("cut", None), Some(Transition::Cut));
    assert_eq!(
        Transition::parse("fade", None),
        Some(Transition::Fade {
            color: [0.0, 0.0, 0.0],
            duration: 0.5
        })
    );
    assert_eq!(
        Transition::parse("fadewhite", None),
        Some(Transition::Fade {
            color: [1.0, 1.0, 1.0],
            duration: 0.5
        })
    );
    assert_eq!(
        Transition::parse("dissolve", Some(1.5)),
        Some(Transition::Dissolve { duration: 1.5 })
    );
    assert_eq!(
        Transition::parse("slideleft", None),
        Some(Transition::Slide {
            direction: SlideDirection::Left,
            duration: 0.5
        })
    );
    assert_eq!(
        Transition::parse("slideright", None),
        Some(Transition::Slide {
            direction: SlideDirection::Right,
            duration: 0.5
        })
    );
    assert_eq!(
        Transition::parse("slideup", None),
        Some(Transition::Slide {
            direction: SlideDirection::Up,
            duration: 0.5
        })
    );
    assert_eq!(
        Transition::parse("slidedown", None),
        Some(Transition::Slide {
            direction: SlideDirection::Down,
            duration: 0.5
        })
    );
    assert_eq!(
        Transition::parse("zoom", Some(2.0)),
        Some(Transition::Zoom { duration: 2.0 })
    );
}

#[test]
fn transition_parse_case_insensitive() {
    assert_eq!(
        Transition::parse("DISSOLVE", None),
        Some(Transition::Dissolve { duration: 0.5 })
    );
    assert_eq!(
        Transition::parse("  Fade  ", None),
        Some(Transition::Fade {
            color: [0.0, 0.0, 0.0],
            duration: 0.5
        })
    );
}

#[test]
fn transition_parse_unknown() {
    assert_eq!(Transition::parse("unknown", None), None);
}

#[test]
fn transition_default_duration() {
    assert_eq!(Transition::Cut.default_duration(), 0.0);
    assert_eq!(
        Transition::Dissolve { duration: 1.5 }.default_duration(),
        1.5
    );
    assert_eq!(
        Transition::Fade {
            color: [0.0; 3],
            duration: 2.0
        }
        .default_duration(),
        2.0
    );
}

#[test]
fn transition_serde_roundtrip() {
    let t = Transition::Slide {
        direction: SlideDirection::Left,
        duration: 1.0,
    };
    let json = serde_json::to_string(&t).unwrap();
    let t2: Transition = serde_json::from_str(&json).unwrap();
    assert_eq!(t, t2);
}

// ---- Command tests ----

#[test]
fn command_is_blocking() {
    assert!(Command::Say {
        speaker: None,
        text: "hi".into(),
        voice: None,
        side_image: None,
        nvl: false,
    }
    .is_blocking());
    assert!(Command::Narrate { text: "n".into() }.is_blocking());
    assert!(Command::Menu {
        prompt: None,
        choices: vec![]
    }
    .is_blocking());
    assert!(Command::Pause { seconds: 1.0 }.is_blocking());
    assert!(Command::End.is_blocking());
}

#[test]
fn command_non_blocking() {
    assert!(!Command::PlayMusic {
        track: "bgm.ogg".into(),
        fade: 0.0
    }
    .is_blocking());
    assert!(!Command::StopMusic { fade: 0.0 }.is_blocking());
    assert!(!Command::PlaySfx {
        track: "sfx.wav".into(),
        volume: 1.0
    }
    .is_blocking());
    assert!(!Command::SetVariable {
        name: "x".into(),
        value: Value::Int(0)
    }
    .is_blocking());
}

// ---- Model / Scene tests ----

#[test]
fn anchor_x_values() {
    assert_eq!(anchor_x(Anchor::Left), 0.2);
    assert_eq!(anchor_x(Anchor::Center), 0.5);
    assert_eq!(anchor_x(Anchor::Right), 0.8);
    assert_eq!(anchor_x(Anchor::OffscreenLeft), -0.2);
    assert_eq!(anchor_x(Anchor::OffscreenRight), 1.2);
    assert_eq!(anchor_x(Anchor::CenterLeft), 0.35);
    assert_eq!(anchor_x(Anchor::CenterRight), 0.65);
    assert_eq!(anchor_x(Anchor::Custom), 0.5);
}

#[test]
fn scene_set_sprite_replaces_same_character() {
    let mut scene = Scene::default();
    let spec1 = SpriteSpec {
        character: "alice".into(),
        image: "alice_happy.png".into(),
        anchor: Anchor::Left,
        position: None,
        scale: None,
        z: None,
        transition: None,
        layer: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    };
    scene.set_sprite(&spec1);
    assert_eq!(scene.sprites.len(), 1);
    assert_eq!(scene.sprites[0].image, "alice_happy.png");

    let spec2 = SpriteSpec {
        character: "alice".into(),
        image: "alice_sad.png".into(),
        anchor: Anchor::Right,
        position: None,
        scale: None,
        z: None,
        transition: None,
        layer: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    };
    scene.set_sprite(&spec2);
    assert_eq!(scene.sprites.len(), 1);
    assert_eq!(scene.sprites[0].image, "alice_sad.png");
    assert_eq!(scene.sprites[0].anchor, Anchor::Right);
}

#[test]
fn scene_set_sprite_multiple_characters() {
    let mut scene = Scene::default();
    scene.set_sprite(&SpriteSpec {
        character: "alice".into(),
        image: "a.png".into(),
        anchor: Anchor::Left,
        position: None,
        scale: None,
        z: None,
        transition: None,
        layer: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    });
    scene.set_sprite(&SpriteSpec {
        character: "bob".into(),
        image: "b.png".into(),
        anchor: Anchor::Right,
        position: None,
        scale: None,
        z: None,
        transition: None,
        layer: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    });
    assert_eq!(scene.sprites.len(), 2);
}

#[test]
fn scene_hide_sprite() {
    let mut scene = Scene::default();
    scene.set_sprite(&SpriteSpec {
        character: "alice".into(),
        image: "a.png".into(),
        anchor: Anchor::Center,
        position: None,
        scale: None,
        z: None,
        transition: None,
        layer: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    });
    assert!(scene.hide_sprite("alice", crate::transition::Transition::Cut));
    assert_eq!(scene.sprites.len(), 0);
    assert!(!scene.hide_sprite("alice", crate::transition::Transition::Cut));
}

#[test]
fn scene_hide_sprite_with_transition_goes_to_exiting() {
    let mut scene = Scene::default();
    scene.set_sprite(&SpriteSpec {
        character: "alice".into(),
        image: "a.png".into(),
        anchor: Anchor::Center,
        position: None,
        scale: None,
        z: None,
        transition: None,
        layer: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    });
    assert!(scene.hide_sprite(
        "alice",
        crate::transition::Transition::Dissolve { duration: 0.5 }
    ));
    assert_eq!(scene.sprites.len(), 0);
    assert_eq!(scene.exiting.len(), 1);
    assert_eq!(scene.exiting[0].sprite.character, "alice");
    assert_eq!(scene.exiting[0].duration, 0.5);
    assert_eq!(scene.exiting[0].progress(), 0.0);
}

#[test]
fn scene_sprite_lookup() {
    let mut scene = Scene::default();
    scene.set_sprite(&SpriteSpec {
        character: "alice".into(),
        image: "a.png".into(),
        anchor: Anchor::Left,
        position: None,
        scale: None,
        z: None,
        transition: None,
        layer: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    });
    assert!(scene.sprite("alice").is_some());
    assert!(scene.sprite("bob").is_none());
    assert!(scene.sprite_mut("alice").is_some());
}

#[test]
fn scene_z_sorting() {
    let mut scene = Scene::default();
    scene.set_sprite(&SpriteSpec {
        character: "back".into(),
        image: "back.png".into(),
        anchor: Anchor::Center,
        position: None,
        scale: None,
        z: Some(0),
        transition: None,
        layer: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    });
    scene.set_sprite(&SpriteSpec {
        character: "front".into(),
        image: "front.png".into(),
        anchor: Anchor::Center,
        position: None,
        scale: None,
        z: Some(10),
        transition: None,
        layer: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    });
    assert_eq!(scene.sprites[0].character, "back");
    assert_eq!(scene.sprites[1].character, "front");
}

#[test]
fn scene_serde_roundtrip() {
    let mut scene = Scene {
        background: Some("bg.png".into()),
        bg_opacity: 0.8,
        ..Default::default()
    };
    scene.set_sprite(&SpriteSpec {
        character: "alice".into(),
        image: "a.png".into(),
        anchor: Anchor::Left,
        position: None,
        scale: None,
        z: None,
        transition: None,
        layer: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    });
    let json = serde_json::to_string(&scene).unwrap();
    let scene2: Scene = serde_json::from_str(&json).unwrap();
    assert_eq!(scene.background, scene2.background);
    assert_eq!(scene.bg_opacity, scene2.bg_opacity);
    assert_eq!(scene.sprites.len(), scene2.sprites.len());
}

// ---- Save/Load tests ----

#[test]
fn save_state_write_read_roundtrip() {
    let tmp = std::env::temp_dir().join("vnengine_test_save");
    let _ = std::fs::remove_dir_all(&tmp);
    let state = SaveState {
        slot: 0,
        label: "Chapter 1".into(),
        play_time: 120.5,
        vm_state: vec![1, 2, 3],
        scene: Scene {
            background: Some("bg.png".into()),
            bg_opacity: 1.0,
            ..Default::default()
        },
        blocking: Some(Command::Narrate { text: "hi".into() }),
        music: Some("bgm.ogg".into()),
        journal: vec![crate::save::JournalEntry::Menu(1)],
        plugin_kv: [("plug.k".to_string(), "v".to_string())]
            .into_iter()
            .collect(),
        music_layers: std::collections::BTreeMap::new(),
        screens: Vec::new(),
        nvl_start: 0,
        window_mode: "auto".into(),
        channels: std::collections::BTreeMap::new(),
        thumbnail: None,
        saved_at: 1700000000,
    };
    state.write(&tmp).unwrap();
    let loaded = SaveState::read(&tmp, 0).unwrap();
    assert_eq!(loaded.slot, 0);
    assert_eq!(loaded.label, "Chapter 1");
    assert_eq!(loaded.play_time, 120.5);
    assert_eq!(loaded.vm_state, vec![1, 2, 3]);
    assert_eq!(loaded.scene.background.as_deref(), Some("bg.png"));
    assert!(matches!(
        loaded.blocking,
        Some(Command::Narrate { ref text }) if text == "hi"
    ));
    assert_eq!(loaded.music.as_deref(), Some("bgm.ogg"));
    assert_eq!(loaded.journal.len(), 1);
    assert_eq!(loaded.plugin_kv.get("plug.k"), Some(&"v".to_string()));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn save_state_list_multiple_slots() {
    let tmp = std::env::temp_dir().join("vnengine_test_save_list");
    let _ = std::fs::remove_dir_all(&tmp);
    for slot in [2u32, 0, 1] {
        SaveState {
            slot,
            label: format!("Slot {slot}"),
            play_time: 0.0,
            vm_state: vec![],
            scene: Scene::default(),
            blocking: None,
            music: None,
            journal: vec![],
            plugin_kv: Default::default(),
            music_layers: std::collections::BTreeMap::new(),
            screens: Vec::new(),
            nvl_start: 0,
            window_mode: "auto".into(),
            channels: std::collections::BTreeMap::new(),
            thumbnail: None,
            saved_at: 0,
        }
        .write(&tmp)
        .unwrap();
    }
    let saves = SaveState::list(&tmp);
    assert_eq!(saves.len(), 3);
    assert_eq!(saves[0].slot, 0);
    assert_eq!(saves[1].slot, 1);
    assert_eq!(saves[2].slot, 2);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn save_state_delete() {
    let tmp = std::env::temp_dir().join("vnengine_test_save_del");
    let _ = std::fs::remove_dir_all(&tmp);
    SaveState {
        slot: 5,
        label: "test".into(),
        play_time: 0.0,
        vm_state: vec![],
        scene: Scene::default(),
        blocking: None,
        music: None,
        journal: vec![],
        plugin_kv: Default::default(),
        music_layers: std::collections::BTreeMap::new(),
        screens: Vec::new(),
        nvl_start: 0,
        window_mode: "auto".into(),
        channels: std::collections::BTreeMap::new(),
        thumbnail: None,
        saved_at: 0,
    }
    .write(&tmp)
    .unwrap();
    assert!(tmp.join("save_5.json").exists());
    SaveState::delete(&tmp, 5).unwrap();
    assert!(!tmp.join("save_5.json").exists());
    // Deleting again should not error.
    SaveState::delete(&tmp, 5).unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---- Gallery tests ----

#[test]
fn gallery_unlock_and_check() {
    let mut g = GalleryState::default();
    assert!(!g.is_unlocked("cg01.png"));
    g.unlock("cg01.png");
    assert!(g.is_unlocked("cg01.png"));
    // Duplicate unlock should not add twice.
    g.unlock("cg01.png");
    assert_eq!(g.unlocked.len(), 1);
}

#[test]
fn gallery_save_load_roundtrip() {
    let tmp = std::env::temp_dir().join("vnengine_gallery.json");
    let _ = std::fs::remove_file(&tmp);
    let mut g = GalleryState::default();
    g.unlock("cg01.png");
    g.unlock("cg02.png");
    g.save(&tmp).unwrap();
    let loaded = GalleryState::load(&tmp);
    assert_eq!(loaded.unlocked.len(), 2);
    assert!(loaded.is_unlocked("cg01.png"));
    assert!(loaded.is_unlocked("cg02.png"));
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn gallery_load_missing_returns_default() {
    let g = GalleryState::load(Path::new("/nonexistent/path/gallery.json"));
    assert!(g.unlocked.is_empty());
}

// ---- Settings tests ----

#[test]
fn settings_default_values() {
    let s = UserSettings::default();
    assert_eq!(s.master_volume, 1.0);
    assert_eq!(s.music_volume, 0.8);
    assert_eq!(s.sfx_volume, 1.0);
    assert_eq!(s.voice_volume, 1.0);
    assert_eq!(s.text_speed, 40.0);
    assert!(!s.auto_advance);
    assert!(!s.fullscreen);
    assert!(!s.muted);
}

#[test]
fn settings_save_load_roundtrip() {
    let tmp = std::env::temp_dir().join("vnengine_settings.json");
    let _ = std::fs::remove_file(&tmp);
    let s = UserSettings {
        master_volume: 0.5,
        music_volume: 0.3,
        fullscreen: true,
        muted: true,
        ..Default::default()
    };
    s.save(&tmp).unwrap();
    let loaded = UserSettings::load(&tmp).unwrap();
    assert_eq!(loaded.master_volume, 0.5);
    assert_eq!(loaded.music_volume, 0.3);
    assert!(loaded.fullscreen);
    assert!(loaded.muted);
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn settings_load_missing_returns_default() {
    let s = UserSettings::load(Path::new("/nonexistent/settings.json")).unwrap();
    assert_eq!(s.master_volume, 1.0);
}

#[test]
fn light_at_falloff() {
    use crate::model::{light_at, PointLight};
    let lights = vec![PointLight {
        name: "l".into(),
        x: 0.5,
        y: 0.5,
        radius: 0.5,
        intensity: 1.0,
        color: [1.0, 0.5, 0.0],
    }];
    // At the light center: full intensity added to a 0.2 ambient.
    let c = light_at(0.5, 0.5, &lights, [0.2, 0.2, 0.2]);
    assert!((c[0] - 1.2).abs() < 0.01);
    assert!((c[1] - 0.7).abs() < 0.01);
    assert!((c[2] - 0.2).abs() < 0.01);
    // Outside radius: ambient only.
    let c = light_at(0.0, 0.0, &lights, [0.2, 0.2, 0.2]);
    assert_eq!(c, [0.2, 0.2, 0.2]);
}

#[test]
fn scene_light_upsert_and_remove() {
    use crate::model::PointLight;
    let mut scene = Scene::default();
    let l = PointLight {
        name: "a".into(),
        x: 0.0,
        y: 0.0,
        radius: 1.0,
        intensity: 1.0,
        color: [1.0; 3],
    };
    scene.set_light(l.clone());
    scene.set_light(l);
    assert_eq!(scene.lights.len(), 1);
    assert!(scene.remove_light("a"));
    assert!(!scene.remove_light("a"));
    assert_eq!(scene.ambient, [1.0, 1.0, 1.0]);
}

// ---------- Phase 2: ATL & transitions ----------

#[test]
fn atl_warp_endpoints_and_fallback() {
    use crate::atl::warp;
    for name in ["linear", "ease", "easein", "easeout", "bounce", "elastic"] {
        assert!((warp(name, 0.0)).abs() < 1e-4, "{name} at 0");
        assert!((warp(name, 1.0) - 1.0).abs() < 1e-4, "{name} at 1");
    }
    // Unknown warpers degrade to linear.
    assert_eq!(warp("nonexistent", 0.37), 0.37);
    // Ease is slower than linear at mid-time.
    assert!(warp("easein", 0.5) < 0.5);
    assert!(warp("easeout", 0.5) > 0.5);
}

#[test]
fn atl_track_tween_interpolates() {
    use crate::atl::{AnimProp, AnimStep, SpriteTrack};
    use crate::command::SpriteSpec;
    use crate::model::Scene;
    let mut scene = Scene::default();
    scene.set_sprite(&SpriteSpec {
        character: "e".into(),
        image: "e".into(),
        anchor: crate::command::Anchor::Center,
        position: Some((0.0, 0.5)),
        scale: None,
        z: None,
        transition: None,
        layer: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    });
    let mut s = scene.sprites[0].clone();
    let mut track = SpriteTrack::once(vec![AnimStep {
        duration: 1.0,
        warper: "linear".into(),
        targets: vec![(AnimProp::XPos, 1.0)],
        at: None,
    }]);
    track.tick(0.5, &mut s);
    let x = s.position.unwrap().0;
    assert!((x - 0.5).abs() < 0.01, "midpoint x={x}");
    track.tick(0.5, &mut s);
    assert!((s.position.unwrap().0 - 1.0).abs() < 1e-4);
    assert!(track.finished());
}

#[test]
fn atl_track_instant_set_then_pause() {
    use crate::atl::{AnimProp, AnimStep, SpriteTrack};
    let mut s = crate::model::ActiveSprite {
        character: "e".into(),
        image: "e".into(),
        anchor: crate::command::Anchor::Center,
        position: None,
        scale: [1.0, 1.0],
        z: 0,
        opacity: 1.0,
        offset: [0.0, 0.0],
        rotation: 0.0,
        layer: crate::model::Layer::Foreground,
        anchor_frac: None,
        entering: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    };
    let mut track = SpriteTrack::once(vec![
        AnimStep {
            duration: 0.0,
            warper: "linear".into(),
            targets: vec![(AnimProp::Alpha, 0.5)],
            at: None,
        },
        AnimStep {
            duration: 2.0,
            warper: "linear".into(),
            targets: Vec::new(), // wait
            at: None,
        },
        AnimStep {
            duration: 0.0,
            warper: "linear".into(),
            targets: vec![(AnimProp::Alpha, 1.0)],
            at: None,
        },
    ]);
    // Instant set applies inside the first tick regardless of dt.
    track.tick(0.0, &mut s);
    assert!((s.opacity - 0.5).abs() < 1e-4);
    // The wait step holds the value.
    track.tick(1.0, &mut s);
    assert!((s.opacity - 0.5).abs() < 1e-4);
    assert!(!track.finished());
    track.tick(1.0, &mut s);
    assert!((s.opacity - 1.0).abs() < 1e-4);
    assert!(track.finished());
}

#[test]
fn atl_track_time_sync_waits_for_absolute() {
    use crate::atl::{AnimProp, AnimStep, SpriteTrack};
    let mut s = crate::model::ActiveSprite {
        character: "e".into(),
        image: "e".into(),
        anchor: crate::command::Anchor::Center,
        position: Some((0.0, 0.5)),
        scale: [1.0, 1.0],
        z: 0,
        opacity: 1.0,
        offset: [0.0, 0.0],
        rotation: 0.0,
        layer: crate::model::Layer::Foreground,
        anchor_frac: None,
        entering: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    };
    let mut track = SpriteTrack::once(vec![
        AnimStep {
            duration: 0.0,
            warper: "linear".into(),
            targets: Vec::new(),
            at: Some(1.0), // `time 1.0`
        },
        AnimStep {
            duration: 0.0,
            warper: "linear".into(),
            targets: vec![(AnimProp::XPos, 0.9)],
            at: None,
        },
    ]);
    track.tick(0.5, &mut s); // elapsed 0.5 < 1.0: still waiting
    assert_eq!(s.position.unwrap().0, 0.0);
    track.tick(0.6, &mut s); // elapsed 1.1 >= 1.0: runs through
    assert!((s.position.unwrap().0 - 0.9).abs() < 1e-4);
    assert!(track.finished());
}

#[test]
fn atl_track_repeat_finite_and_infinite() {
    use crate::atl::{AnimProp, AnimStep, SpriteTrack};
    let mk = |repeats| {
        let mut t = SpriteTrack::once(vec![AnimStep {
            duration: 0.0,
            warper: "linear".into(),
            targets: vec![(AnimProp::XOff, 0.1)],
            at: None,
        }]);
        t.repeats = repeats;
        t
    };
    let mut s = crate::model::ActiveSprite {
        character: "e".into(),
        image: "e".into(),
        anchor: crate::command::Anchor::Center,
        position: None,
        scale: [1.0, 1.0],
        z: 0,
        opacity: 1.0,
        offset: [0.0, 0.0],
        rotation: 0.0,
        layer: crate::model::Layer::Foreground,
        anchor_frac: None,
        entering: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    };
    // repeat 3: runs 3 times then finishes.
    let mut t3 = mk(Some(3));
    for _ in 0..3 {
        t3.tick(0.0, &mut s);
    }
    assert!(t3.finished());
    assert_eq!(t3.iteration, 3);
    // Infinite: never finishes.
    let mut ti = mk(None);
    for _ in 0..20 {
        ti.tick(0.0, &mut s);
    }
    assert!(!ti.finished());
    assert!(ti.iteration >= 20);
}

#[test]
fn atl_track_finish_flattens_reduce_motion() {
    use crate::atl::{AnimProp, AnimStep, SpriteTrack};
    let mut s = crate::model::ActiveSprite {
        character: "e".into(),
        image: "e".into(),
        anchor: crate::command::Anchor::Center,
        position: Some((0.0, 0.0)),
        scale: [1.0, 1.0],
        z: 0,
        opacity: 1.0,
        offset: [0.0, 0.0],
        rotation: 0.0,
        layer: crate::model::Layer::Foreground,
        anchor_frac: None,
        entering: None,
        anims: Vec::new(),
        anim_events: Default::default(),
        layers: Vec::new(),
    };
    let track = SpriteTrack::once(vec![
        AnimStep {
            duration: 5.0,
            warper: "ease".into(),
            targets: vec![(AnimProp::XPos, 0.8)],
            at: None,
        },
        AnimStep {
            duration: 1.0,
            warper: "linear".into(),
            targets: vec![(AnimProp::Alpha, 0.2)],
            at: None,
        },
    ]);
    track.finish(&mut s);
    assert!((s.position.unwrap().0 - 0.8).abs() < 1e-4);
    assert!((s.opacity - 0.2).abs() < 1e-4);
}

#[test]
fn transition_parse_new_kinds() {
    use crate::transition::{SlideDirection, Transition};
    assert_eq!(
        Transition::parse("pushleft", None),
        Some(Transition::Push {
            direction: SlideDirection::Left,
            duration: 0.5
        })
    );
    assert_eq!(
        Transition::parse("pushdown", Some(1.5)),
        Some(Transition::Push {
            direction: SlideDirection::Down,
            duration: 1.5
        })
    );
    assert_eq!(
        Transition::parse("blinds", None),
        Some(Transition::Blinds {
            vertical: true,
            strips: 10,
            duration: 0.5
        })
    );
    assert_eq!(
        Transition::parse("blindsh", None),
        Some(Transition::Blinds {
            vertical: false,
            strips: 10,
            duration: 0.5
        })
    );
    assert_eq!(
        Transition::parse("irisin", None),
        Some(Transition::Iris {
            inward: true,
            duration: 0.5
        })
    );
    assert_eq!(
        Transition::parse("irisout", None),
        Some(Transition::Iris {
            inward: false,
            duration: 0.5
        })
    );
    assert_eq!(
        Transition::parse("pixelate", None),
        Some(Transition::Pixelate { duration: 0.5 })
    );
    assert_eq!(
        Transition::parse("squares", Some(2.0)),
        Some(Transition::Squares {
            cells: 12,
            duration: 2.0
        })
    );
}

#[test]
fn transition_mask_classification() {
    use crate::transition::Transition;
    assert!(!Transition::parse("dissolve", None).unwrap().is_masked());
    assert!(!Transition::parse("pushleft", None).unwrap().is_masked());
    assert_eq!(Transition::parse("blinds", None).unwrap().mask_kind(), 1);
    assert_eq!(Transition::parse("blindsh", None).unwrap().mask_kind(), 2);
    assert_eq!(Transition::parse("irisin", None).unwrap().mask_kind(), 3);
    assert_eq!(Transition::parse("irisout", None).unwrap().mask_kind(), 4);
    assert_eq!(Transition::parse("pixelate", None).unwrap().mask_kind(), 5);
    assert_eq!(Transition::parse("squares", None).unwrap().mask_kind(), 6);
    assert_eq!(
        Transition::parse("blinds", None).unwrap().mask_param(),
        10.0
    );
}
