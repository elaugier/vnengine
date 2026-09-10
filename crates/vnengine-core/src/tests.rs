#[cfg(test)]
mod tests {
    use crate::command::{Anchor, Command, SpriteSpec};
    use crate::model::{anchor_x, Scene};
    use crate::save::{GalleryState, SaveState};
    use crate::settings::UserSettings;
    use crate::transition::{SlideDirection, Transition};
    use crate::Value;
    use std::collections::HashMap;
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
        let v: Value = 3.14f64.into();
        assert_eq!(v, Value::Float(3.14));
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
        });
        scene.set_sprite(&SpriteSpec {
            character: "bob".into(),
            image: "b.png".into(),
            anchor: Anchor::Right,
            position: None,
            scale: None,
            z: None,
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
        });
        assert!(scene.hide_sprite("alice").is_some());
        assert_eq!(scene.sprites.len(), 0);
        assert!(scene.hide_sprite("alice").is_none());
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
        });
        scene.set_sprite(&SpriteSpec {
            character: "front".into(),
            image: "front.png".into(),
            anchor: Anchor::Center,
            position: None,
            scale: None,
            z: Some(10),
        });
        assert_eq!(scene.sprites[0].character, "back");
        assert_eq!(scene.sprites[1].character, "front");
    }

    #[test]
    fn scene_serde_roundtrip() {
        let mut scene = Scene::default();
        scene.background = Some("bg.png".into());
        scene.bg_opacity = 0.8;
        scene.set_sprite(&SpriteSpec {
            character: "alice".into(),
            image: "a.png".into(),
            anchor: Anchor::Left,
            position: None,
            scale: None,
            z: None,
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
        let mut vars = HashMap::new();
        vars.insert("score".into(), Value::Int(42));
        vars.insert("name".into(), Value::Str("Alice".into()));
        let state = SaveState {
            slot: 0,
            label: "Chapter 1".into(),
            play_time: 120.5,
            vm_state: vec![1, 2, 3],
            variables: vars,
            thumbnail: None,
            saved_at: 1700000000,
        };
        state.write(&tmp).unwrap();
        let loaded = SaveState::read(&tmp, 0).unwrap();
        assert_eq!(loaded.slot, 0);
        assert_eq!(loaded.label, "Chapter 1");
        assert_eq!(loaded.play_time, 120.5);
        assert_eq!(loaded.vm_state, vec![1, 2, 3]);
        assert_eq!(loaded.variables.get("score"), Some(&Value::Int(42)));
        assert_eq!(
            loaded.variables.get("name"),
            Some(&Value::Str("Alice".into()))
        );
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
                variables: HashMap::new(),
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
            variables: HashMap::new(),
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
        let mut s = UserSettings::default();
        s.master_volume = 0.5;
        s.music_volume = 0.3;
        s.fullscreen = true;
        s.muted = true;
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
}
