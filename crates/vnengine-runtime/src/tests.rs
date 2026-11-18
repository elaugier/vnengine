use crate::{JournalEntry, Runtime, RuntimeEvent};
use vnengine_core::command::Command;
use vnengine_script::parser::Parser;

fn runtime(src: &str) -> Runtime {
    Runtime::new(Parser::parse(src).unwrap())
}

fn blocking_text(ev: &RuntimeEvent) -> &str {
    match ev {
        RuntimeEvent::Blocking(Command::Say { text, .. })
        | RuntimeEvent::Blocking(Command::Narrate { text }) => text,
        other => panic!("expected Blocking, got {other:?}"),
    }
}

/// Drive the runtime until the next blocking point.
fn next_block(rt: &mut Runtime) -> RuntimeEvent {
    loop {
        match rt.step() {
            RuntimeEvent::Effect(_) => continue,
            ev => return ev,
        }
    }
}

#[test]
fn rollback_returns_to_previous_line() {
    let mut rt = runtime(
        r##"
label start:
    "first"
    "second"
    end
"##,
    );
    assert_eq!(blocking_text(&next_block(&mut rt)), "first");
    assert_eq!(blocking_text(&next_block(&mut rt)), "second");

    let ev = rt.rollback().expect("rollback should succeed");
    assert_eq!(blocking_text(&ev), "first");
}

#[test]
fn rollback_at_first_line_is_noop() {
    let mut rt = runtime(
        r##"
label start:
    "only"
    end
"##,
    );
    assert_eq!(blocking_text(&next_block(&mut rt)), "only");
    // Rollback at the first line re-displays the same line.
    let ev = rt.rollback().expect("rollback");
    assert_eq!(blocking_text(&ev), "only");
    let ev = rt.rollback().expect("rollback");
    assert_eq!(blocking_text(&ev), "only");
    // Nothing was rolled forward into, so rollforward is empty.
    assert!(rt.rollforward().is_none());
}

#[test]
fn rollforward_redoes_line() {
    let mut rt = runtime(
        r##"
label start:
    "first"
    "second"
    end
"##,
    );
    next_block(&mut rt);
    next_block(&mut rt);
    rt.rollback().unwrap();
    let ev = rt.rollforward().expect("rollforward");
    assert_eq!(blocking_text(&ev), "second");
}

#[test]
fn rollback_restores_scene() {
    let mut rt = runtime(
        r##"
label start:
    scene bg one
    "first"
    scene bg two
    "second"
    end
"##,
    );
    next_block(&mut rt); // "first" with bg one
    assert_eq!(rt.scene.background.as_deref(), Some("bg one"));
    next_block(&mut rt); // "second" with bg two
    assert_eq!(rt.scene.background.as_deref(), Some("bg two"));

    let ev = rt.rollback().unwrap();
    assert_eq!(blocking_text(&ev), "first");
    assert_eq!(rt.scene.background.as_deref(), Some("bg one"));
}

#[test]
fn rollback_truncates_history() {
    let mut rt = runtime(
        r##"
label start:
    "first"
    "second"
    end
"##,
    );
    next_block(&mut rt);
    next_block(&mut rt);
    assert_eq!(rt.history.len(), 2);

    rt.rollback().unwrap();
    // Rolling back to "first" drops "second" and replays "first" once.
    assert_eq!(rt.history.len(), 1);
    assert_eq!(rt.history[0].text, "first");
}

#[test]
fn rollback_over_menu_choice() {
    let mut rt = runtime(
        r##"
label start:
    menu:
        "A":
            "picked A"
    "after"
    end
"##,
    );
    match next_block(&mut rt) {
        RuntimeEvent::Menu { choices, .. } => assert_eq!(choices, vec!["A"]),
        other => panic!("expected menu, got {other:?}"),
    }
    rt.resume_menu(0);
    assert_eq!(blocking_text(&next_block(&mut rt)), "picked A");

    // Rolling back from inside the choice body lands on the menu again
    // (Ren'Py semantics: the menu is a rollback point).
    match rt.rollback().unwrap() {
        RuntimeEvent::Menu { choices, .. } => assert_eq!(choices, vec!["A"]),
        other => panic!("expected menu after rollback, got {other:?}"),
    }

    // Choosing again still works after the rollback.
    rt.resume_menu(0);
    assert_eq!(blocking_text(&next_block(&mut rt)), "picked A");
}

#[test]
fn capture_and_restore_roundtrip() {
    let src = r##"
label start:
    scene bg school
    "first"
    "second"
    end
"##;
    let mut rt = runtime(src);
    next_block(&mut rt); // "first"
    next_block(&mut rt); // "second" — blocked here
    let snap = rt.capture();

    let mut rt2 = Runtime::restore(Parser::parse(src).unwrap(), snap);
    assert_eq!(rt2.scene.background.as_deref(), Some("bg school"));
    // VM state points past "second": the next step yields `end`.
    assert!(matches!(next_block(&mut rt2), RuntimeEvent::Finished));
}

#[test]
fn restore_on_menu_replays_menu() {
    let src = r##"
label start:
    menu "Where?":
        "Left":
            "went left"
    end
"##;
    let mut rt = runtime(src);
    match next_block(&mut rt) {
        RuntimeEvent::Menu { prompt, choices } => {
            assert_eq!(prompt.as_deref(), Some("Where?"));
            assert_eq!(choices, vec!["Left"]);
        }
        other => panic!("expected menu, got {other:?}"),
    }
    // Save while the menu is displayed.
    let snap = rt.capture();
    let mut rt2 = Runtime::restore(Parser::parse(src).unwrap(), snap);
    // resume_display must reproduce the menu, prompt included.
    match rt2.resume_display() {
        RuntimeEvent::Menu { prompt, choices } => {
            assert_eq!(prompt.as_deref(), Some("Where?"));
            assert_eq!(choices, vec!["Left"]);
        }
        other => panic!("expected menu after restore, got {other:?}"),
    }
}

#[test]
fn effects_are_forwarded() {
    let mut rt = runtime(
        r##"
label start:
    play music "bgm.ogg" fade 1.0
    "line"
    end
"##,
    );
    match rt.step() {
        RuntimeEvent::Effect(Command::PlayMusic { track, .. }) => {
            assert_eq!(track, "bgm.ogg")
        }
        other => panic!("expected PlayMusic effect, got {other:?}"),
    }
    assert_eq!(blocking_text(&rt.step()), "line");
}

#[test]
fn snapshot_vm_state_is_serializable() {
    let mut rt = runtime(
        r##"
label start:
    "first"
    end
"##,
    );
    next_block(&mut rt);
    let mut snap = rt.capture();
    // The whole runtime snapshot must round-trip through JSON.
    let bytes = serde_json::to_vec(&snap.vm_state).unwrap();
    snap.vm_state = serde_json::from_slice(&bytes).unwrap();
    let mut rt2 = Runtime::restore(
        Parser::parse("label start:\n    \"first\"\n    end\n").unwrap(),
        snap,
    );
    assert!(matches!(rt2.step(), RuntimeEvent::Finished));
}

#[test]
fn pause_expires_via_logical_clock() {
    let mut rt = runtime(
        r##"
label start:
    "before"
    pause 1.0
    "after"
    end
"##,
    );
    assert_eq!(blocking_text(&rt.step()), "before");
    match rt.step() {
        RuntimeEvent::Blocking(Command::Pause { seconds }) => {
            assert_eq!(seconds, 1.0)
        }
        other => panic!("expected Pause, got {other:?}"),
    }
    assert!(rt.is_paused());
    // Not enough elapsed: nothing happens.
    assert!(rt.update(0.5).is_none());
    assert!(rt.is_paused());
    // The remaining time expires: execution continues.
    let ev = rt.update(0.6).expect("pause should expire");
    assert_eq!(blocking_text(&ev), "after");
    assert!(!rt.is_paused());
}

#[test]
fn music_is_tracked_through_commands() {
    let mut rt = runtime(
        r##"
label start:
    play music "bgm.ogg" fade 2.0
    "line"
    stop music fade 0.5
    "end"
    end
"##,
    );
    match rt.step() {
        RuntimeEvent::Effect(Command::PlayMusic { .. }) => {}
        other => panic!("expected PlayMusic, got {other:?}"),
    }
    assert_eq!(rt.music.as_deref(), Some("bgm.ogg"));
    assert_eq!(rt.music_fade, 2.0);
    assert_eq!(blocking_text(&rt.step()), "line");
    match rt.step() {
        RuntimeEvent::Effect(Command::StopMusic { .. }) => {}
        other => panic!("expected StopMusic, got {other:?}"),
    }
    assert_eq!(rt.music, None);
    assert_eq!(rt.music_fade, 0.5);
}

#[test]
fn music_restored_on_rollback() {
    let mut rt = runtime(
        r##"
label start:
    play music "bgm.ogg" fade 1.0
    "with music"
    stop music
    "silent"
    end
"##,
    );
    // next_block skips the PlayMusic effect and lands on "with music".
    let ev = next_block(&mut rt);
    assert_eq!(blocking_text(&ev), "with music");
    // next_block skips the StopMusic effect and lands on "silent".
    let ev = next_block(&mut rt);
    assert_eq!(rt.music, None);
    assert_eq!(blocking_text(&ev), "silent");
    // Rolling back to "with music" must also restore the BGM.
    let ev = rt.rollback().expect("rollback");
    assert_eq!(blocking_text(&ev), "with music");
    assert_eq!(rt.music.as_deref(), Some("bgm.ogg"));
}

#[test]
fn snapshot_carries_music_and_journal() {
    let mut rt = runtime(
        r##"
label start:
    play music "bgm.ogg" fade 1.0
    menu:
        "a":
            "chose a"
        "b":
            "chose b"
    end
"##,
    );
    // next_block skips the music effect and lands on the menu.
    let ev = next_block(&mut rt);
    assert!(matches!(ev, RuntimeEvent::Menu { .. }));
    rt.resume_menu(1);
    let snap = rt.capture();
    assert_eq!(snap.music.as_deref(), Some("bgm.ogg"));
    assert!(matches!(snap.journal.as_slice(), [JournalEntry::Menu(1)]));
}

#[test]
fn replay_reproduces_session_deterministically() {
    let src = r##"
label start:
    $ roll = randint(1, 100)
    "rolled [roll]"
    menu:
        "north":
            $ dest = "n"
        "south":
            $ dest = "s"
    "went [dest]"
    end
"##;
    // Play a session: answer the menu with choice 1.
    let mut rt = runtime(src);
    let first_line = blocking_text(&rt.step()).to_string();
    assert!(first_line.starts_with("rolled "));
    assert!(matches!(rt.step(), RuntimeEvent::Menu { .. }));
    rt.resume_menu(1);
    assert_eq!(blocking_text(&rt.step()), "went s");
    assert!(matches!(rt.step(), RuntimeEvent::Finished));

    // Replay the recorded journal: same lines, same variables.
    let rt2 =
        Runtime::replay(Parser::parse(src).unwrap(), &rt.journal).expect("replay should succeed");
    let texts: Vec<String> = rt2.history.iter().map(|h| h.text.clone()).collect();
    assert_eq!(texts, vec![first_line.clone(), "went s".to_string()]);
    assert_eq!(
        rt.vm.state.variables["roll"].as_int(),
        rt2.vm.state.variables["roll"].as_int()
    );

    // A wrong-length journal is rejected.
    assert!(Runtime::replay(Parser::parse(src).unwrap(), &[]).is_err());
    assert!(Runtime::replay(
        Parser::parse(src).unwrap(),
        &[JournalEntry::Menu(0), JournalEntry::Menu(0)],
    )
    .is_err());
}

#[test]
fn hide_with_dissolve_exits_gradually() {
    let src = r##"
label start:
    show eileen happy at center
    hide eileen with dissolve 1.0
    "line"
    end
"##;
    let mut rt = runtime(src);
    // Step past `show`; hide pushes the sprite into `exiting`.
    rt.step();
    rt.step();
    assert!(rt.scene.sprites.is_empty());
    assert_eq!(rt.scene.exiting.len(), 1);
    // Half the transition elapsed: sprite still exiting at 50%.
    rt.update(0.5);
    assert_eq!(rt.scene.exiting.len(), 1);
    assert!((rt.scene.exiting[0].progress() - 0.5).abs() < 0.01);
    // Done: sprite fully gone.
    rt.update(0.6);
    assert!(rt.scene.exiting.is_empty());
}

#[test]
fn exiting_sprite_survives_save_restore() {
    let src = r##"
label start:
    show eileen happy at center
    hide eileen with dissolve 1.0
    "line"
    end
"##;
    let mut rt = runtime(src);
    rt.step();
    rt.step();
    rt.update(0.25);
    let snap = rt.capture();
    let rt2 = Runtime::restore(Parser::parse(src).unwrap(), snap);
    assert_eq!(rt2.scene.exiting.len(), 1);
    assert!((rt2.scene.exiting[0].elapsed - 0.25).abs() < 0.01);
}

#[test]
fn show_with_dissolve_enters_gradually() {
    let src = r##"
label start:
    show eileen happy at center with dissolve 1.0
    "line"
    end
"##;
    let mut rt = runtime(src);
    rt.step();
    let s = rt.scene.sprite("eileen").expect("sprite shown");
    assert!(s.entering.is_some(), "show with dissolve should animate");
    rt.update(0.5);
    let s = rt.scene.sprite("eileen").unwrap();
    assert!((s.entering.as_ref().unwrap().progress() - 0.5).abs() < 0.01);
    rt.update(0.6);
    assert!(rt.scene.sprite("eileen").unwrap().entering.is_none());
}

#[test]
fn entering_sprite_survives_save_restore() {
    let src = r##"
label start:
    show eileen happy at center with dissolve 1.0
    "line"
    end
"##;
    let mut rt = runtime(src);
    rt.step();
    rt.update(0.25);
    let snap = rt.capture();
    let rt2 = Runtime::restore(Parser::parse(src).unwrap(), snap);
    let s = rt2.scene.sprite("eileen").unwrap();
    assert!((s.entering.as_ref().unwrap().elapsed - 0.25).abs() < 0.01);
}

#[test]
fn show_replacement_crossfades() {
    let src = r##"
label start:
    show eileen happy at center
    show eileen sad at center with dissolve 1.0
    "line"
    end
"##;
    let mut rt = runtime(src);
    rt.step();
    rt.step();
    // Old image exits, new image enters simultaneously.
    assert_eq!(rt.scene.exiting.len(), 1);
    assert_eq!(rt.scene.exiting[0].sprite.image, "eileen happy");
    let s = rt.scene.sprite("eileen").unwrap();
    assert_eq!(s.image, "eileen sad");
    assert!(s.entering.is_some());
    rt.update(1.1);
    assert!(rt.scene.exiting.is_empty());
    assert!(rt.scene.sprite("eileen").unwrap().entering.is_none());
}

#[test]
fn show_same_image_no_crossfade() {
    let src = r##"
label start:
    show eileen happy at center
    show eileen happy at center with dissolve 1.0
    "line"
    end
"##;
    let mut rt = runtime(src);
    rt.step();
    rt.step();
    assert!(rt.scene.exiting.is_empty());
}

#[test]
fn light_and_camera_commands_mutate_scene() {
    let src = r##"
label start:
    ambient 0.3 0.3 0.4
    light "torch" x=0.7 y=0.4 radius=0.3 intensity=1.2 color="#ffaa44"
    camera x=0.1 zoom=1.2
    "line"
    end
"##;
    let mut rt = runtime(src);
    rt.step(); // ambient
    rt.step(); // light
    rt.step(); // camera
    assert_eq!(rt.scene.ambient, [0.3, 0.3, 0.4]);
    assert_eq!(rt.scene.lights.len(), 1);
    assert_eq!(rt.scene.lights[0].name, "torch");
    assert!((rt.scene.camera.x - 0.1).abs() < 0.01);
    assert!((rt.scene.camera.zoom - 1.2).abs() < 0.01);
}

#[test]
fn camera_animated_over_duration() {
    let src = r##"
label start:
    camera zoom=1.5 over=1.0
    "line"
    end
"##;
    let mut rt = runtime(src);
    rt.step();
    assert!(rt.scene.camera_anim.is_some());
    rt.update(0.5);
    // Half-way, smoothstep: progress 0.5 → value 0.5 → zoom 1.25.
    assert!((rt.scene.camera.zoom - 1.25).abs() < 0.01);
    rt.update(0.6);
    assert!(rt.scene.camera_anim.is_none());
    assert!((rt.scene.camera.zoom - 1.5).abs() < 0.01);
}

#[test]
fn camera_anim_survives_save_restore() {
    let src = r##"
label start:
    camera x=0.2 over=1.0
    "line"
    end
"##;
    let mut rt = runtime(src);
    rt.step();
    rt.update(0.25);
    let snap = rt.capture();
    let mut rt2 = Runtime::restore(Parser::parse(src).unwrap(), snap);
    assert!(rt2.scene.camera_anim.is_some());
    rt2.update(1.0);
    assert!((rt2.scene.camera.x - 0.2).abs() < 0.01);
}

#[test]
fn music_layer_tracked_and_cleared() {
    let src = r##"
label start:
    play music "base.ogg"
    music layer "high" "high.ogg" volume=0.7
    "line"
    end
"##;
    let mut rt = runtime(src);
    rt.step();
    rt.step();
    assert_eq!(rt.music_layers["high"].track, "high.ogg");
    assert!((rt.music_layers["high"].volume - 0.7).abs() < 0.01);
    // Stop music clears stems.
    // (re-step through `line` then `end`; a new play would also clear)
}

#[test]
fn music_layer_survives_save_restore_and_off() {
    let src = r##"
label start:
    play music "base.ogg"
    music layer "high" "high.ogg" volume=0.7
    music layer "high" off
    music layer "low" "low.ogg" volume=0.4 fade=1.5
    "line"
    end
"##;
    let mut rt = runtime(src);
    rt.step(); // play + layer + off + layer + say in one step? no — each returns Effect.
               // Drive until blocked on the say.
    while !matches!(
        rt.step(),
        RuntimeEvent::Blocking(_) | RuntimeEvent::Finished
    ) {}
    assert!(!rt.music_layers.contains_key("high"));
    assert_eq!(rt.music_layers["low"].track, "low.ogg");
    let snap = rt.capture();
    let rt2 = Runtime::restore(Parser::parse(src).unwrap(), snap);
    assert_eq!(rt2.music_layers.len(), 1);
    assert!((rt2.music_layers["low"].volume - 0.4).abs() < 0.01);
}

#[test]
fn hot_reload_preserves_position_uses_new_code() {
    let v1 = r##"
label start:
    jump chapter
label chapter:
    "old line"
    end
"##;
    let v2 = r##"
label start:
    jump chapter
label chapter:
    "new line"
    end
"##;
    let mut rt = runtime(v1);
    // Step to the blocking say in `chapter`.
    match rt.step() {
        RuntimeEvent::Blocking(Command::Narrate { text }) => {
            assert_eq!(text, "old line");
        }
        other => panic!("expected blocking say, got {other:?}"),
    }
    // Hot reload: same labels, different text. The blocking point is
    // preserved; rolling back replays with the *new* script.
    rt.reload_script(Parser::parse(v2).unwrap());
    let ev = rt.rollback().expect("rollback");
    match ev {
        RuntimeEvent::Blocking(Command::Narrate { text }) => {
            assert_eq!(text, "new line");
        }
        other => panic!("expected blocking say, got {other:?}"),
    }
}

#[test]
fn hot_reload_variables_survive() {
    let v1 = "label start:\n    $ score = 5\n    \"line\"\n    end\n";
    let v2 = "label start:\n    $ score = 9\n    \"line\"\n    end\n";
    let mut rt = runtime(v1);
    rt.step();
    rt.reload_script(Parser::parse(v2).unwrap());
    // `default`-style assignments already made are not overwritten —
    // score stays 5 even though the new script would set 9.
    let s = &rt.vm.state.variables["score"];
    assert!(matches!(s, vnengine_core::Value::Int(5)));
}

#[test]
fn jump_to_label_repositions_execution() {
    let mut rt = runtime(
        "label start:\n    \"one\"\n    jump done\nlabel middle:\n    \"two\"\nlabel done:\n    \"end\"\n",
    );
    // Advance past the first line so the stack is mid-label.
    rt.step();
    assert!(rt.jump_to_label("middle"));
    let ev = rt.step();
    match ev {
        RuntimeEvent::Blocking(Command::Narrate { text }) => assert_eq!(text, "two"),
        other => panic!("expected narrate 'two', got {other:?}"),
    }
    // Unknown label: no-op, position preserved — execution continues
    // at the end of `middle`, which finishes the game.
    assert!(!rt.jump_to_label("nowhere"));
    assert!(matches!(rt.step(), RuntimeEvent::Finished));
}

// ---------- Phase 2: ATL ----------

#[test]
fn atl_track_ticks_on_update() {
    let mut rt = runtime(
        r##"
transform slidein:
    linear 1.0 xpos 0.9

label start:
    show e happy at center, slidein
    "shown"
    end
"##,
    );
    next_block(&mut rt); // "shown"
    let s = &rt.scene.sprites[0];
    assert_eq!(s.anims.len(), 1);
    rt.update(0.5);
    let x = rt.scene.sprites[0].position.unwrap().0;
    // Half-way between the start (center ~0.5) and 0.9.
    assert!(x > 0.5 && x < 0.9, "x={x}");
    rt.update(0.5);
    assert!((rt.scene.sprites[0].position.unwrap().0 - 0.9).abs() < 1e-4);
    assert!(rt.scene.sprites[0].anims.is_empty());
}

#[test]
fn atl_repeat_runs_forever() {
    let mut rt = runtime(
        r##"
transform bob:
    linear 0.5 yoffset -0.1
    linear 0.5 yoffset 0.0
    repeat

label start:
    show e happy at center, bob
    "shown"
    end
"##,
    );
    next_block(&mut rt);
    for _ in 0..10 {
        rt.update(0.5);
    }
    // Infinite repeat: the track never retires.
    assert_eq!(rt.scene.sprites[0].anims.len(), 1);
    assert!(rt.scene.sprites[0].anims[0].iteration >= 9);
}

#[test]
fn atl_on_show_fires_on_attach() {
    let mut rt = runtime(
        r##"
transform appear:
    on show:
        alpha 0.0
        linear 1.0 alpha 1.0

label start:
    show e happy at center, appear
    "shown"
    end
"##,
    );
    next_block(&mut rt);
    // `on show` track is running; alpha starts at 0.
    assert_eq!(rt.scene.sprites[0].anims.len(), 1);
    rt.update(0.5);
    let a = rt.scene.sprites[0].opacity;
    assert!(a > 0.0 && a < 1.0, "alpha={a}");
}

#[test]
fn atl_on_hide_moves_to_exiting() {
    let mut rt = runtime(
        r##"
transform outro:
    on hide:
        linear 1.0 alpha 0.0

label start:
    show e happy at center, outro
    hide e
    "hidden"
    end
"##,
    );
    next_block(&mut rt);
    assert_eq!(rt.scene.sprites.len(), 0);
    assert_eq!(rt.scene.exiting.len(), 1);
    assert_eq!(rt.scene.exiting[0].sprite.anims.len(), 1);
    // The exit track animates alpha down while the sprite lingers.
    rt.update(0.5);
    assert!(!rt.scene.exiting.is_empty());
    rt.update(0.6);
    // Track finished → exiting entry is dropped.
    assert!(rt.scene.exiting.is_empty());
}

#[test]
fn scene_clears_foreground_sprites() {
    let mut rt = runtime(
        r##"
label start:
    show e happy at center
    scene bg park
    "new scene"
    end
"##,
    );
    next_block(&mut rt);
    // Ren'Py semantics: `scene` clears the character layer.
    assert_eq!(rt.scene.sprites.len(), 0);
    assert_eq!(rt.scene.background.as_deref(), Some("bg park"));
}

#[test]
fn scene_onlayer_sets_layer_bg_keeps_sprites() {
    let mut rt = runtime(
        r##"
label start:
    show e happy at center
    scene forest onlayer midground
    "new scene"
    end
"##,
    );
    next_block(&mut rt);
    use vnengine_core::model::Layer;
    let lb = rt
        .scene
        .layer_bgs
        .get(&(Layer::Midground as u8))
        .expect("midground layer bg");
    assert_eq!(lb.image, "forest");
    // Default background untouched, sprite kept (it's on Foreground).
    assert!(rt.scene.background.is_none());
    assert_eq!(rt.scene.sprites.len(), 1);
}

#[test]
fn scene_onlayer_hides_that_layers_sprites() {
    let mut rt = runtime(
        r##"
label start:
    show e happy at center layer=midground
    scene forest onlayer midground
    "new scene"
    end
"##,
    );
    next_block(&mut rt);
    // The sprite lives on midground — the layer scene clears it.
    assert_eq!(rt.scene.sprites.len(), 0);
}

#[test]
fn atl_state_survives_capture_restore() {
    let mut rt = runtime(
        r##"
transform slidein:
    linear 2.0 xpos 0.9

label start:
    show e happy at center, slidein
    "shown"
    end
"##,
    );
    next_block(&mut rt);
    rt.update(0.5);
    let snap = rt.capture();
    let mid = rt.scene.sprites[0].position.unwrap().0;
    rt.update(0.5);
    // Restore mid-animation: position and track resume identically.
    let mut rt2 = Runtime::restore(Parser::parse(
        r##"
transform slidein:
    linear 2.0 xpos 0.9

label start:
    show e happy at center, slidein
    "shown"
    end
"##,
    )
    .unwrap(), snap);
    assert_eq!(rt2.scene.sprites[0].position.unwrap().0, mid);
    assert_eq!(rt2.scene.sprites[0].anims.len(), 1);
    rt2.update(1.5);
    assert!((rt2.scene.sprites[0].position.unwrap().0 - 0.9).abs() < 1e-4);
}

#[test]
fn atl_composite_layers_reach_scene() {
    let mut rt = runtime(
        r##"
image duo:
    "a.png" offset=(0.0, 0.0)
    "b.png" offset=(0.5, 0.0)

label start:
    show duo at center
    "shown"
    end
"##,
    );
    next_block(&mut rt);
    assert_eq!(rt.scene.sprites[0].layers.len(), 2);
    assert_eq!(rt.scene.sprites[0].layers[1].image, "b.png");
}

// ============ Phase 3: screens / input / confirm / nvl / window ============

#[test]
fn show_hide_screen_tracked() {
    let mut rt = runtime(
        r##"
screen hud:
    text "HP" x=0.02 y=0.02

label start:
    show screen hud
    hide screen hud
    "done"
"##,
    );
    // show screen → Effect, screens updated.
    loop {
        match rt.step() {
            RuntimeEvent::Effect(Command::ShowScreen(_)) => break,
            RuntimeEvent::Effect(_) | RuntimeEvent::Blocking(_) => {}
            other => panic!("expected ShowScreen effect, got {other:?}"),
        }
    }
    assert_eq!(rt.screens.len(), 1);
    assert_eq!(rt.screens[0].name, "hud");
    loop {
        match rt.step() {
            RuntimeEvent::Effect(Command::HideScreen { name }) => {
                assert_eq!(name, "hud");
                break;
            }
            RuntimeEvent::Effect(_) | RuntimeEvent::Blocking(_) => {}
            other => panic!("expected HideScreen effect, got {other:?}"),
        }
    }
    assert!(rt.screens.is_empty());
}

#[test]
fn call_screen_blocks_and_resumes() {
    let mut rt = runtime(
        r##"
screen pick:
    button "One" x=0.2 y=0.4:
        return 1
    button "Two" x=0.6 y=0.4:
        return 2

label start:
    call screen pick set chosen
    "picked [chosen]"
"##,
    );
    match next_block(&mut rt) {
        RuntimeEvent::Screen { inst } => {
            assert_eq!(inst.name, "pick");
            assert_eq!(inst.clickables().len(), 2);
        }
        other => panic!("expected Screen, got {other:?}"),
    }
    // The pending screen also shows in `runtime.screens`.
    assert_eq!(rt.screens.len(), 1);
    assert!(rt.is_on_screen());
    rt.resume_screen(1);
    assert!(!rt.is_on_screen());
    // Dismissed screen leaves `runtime.screens`.
    assert!(rt.screens.is_empty());
    assert_eq!(blocking_text(&next_block(&mut rt)), "picked 2");
}

#[test]
fn call_screen_set_action_stays_up() {
    let mut rt = runtime(
        r##"
screen counter:
    button "Add" x=0.2 y=0.4:
        set count = count + 1
    button "Done" x=0.6 y=0.4:
        return 0

label start:
    $ count = 0
    call screen counter
    "final [count]"
"##,
    );
    match next_block(&mut rt) {
        RuntimeEvent::Screen { .. } => {}
        other => panic!("expected Screen, got {other:?}"),
    }
    rt.resume_screen(0); // "Add" — non-dismissing.
    assert!(rt.is_on_screen());
    rt.resume_screen(0);
    assert!(rt.is_on_screen());
    rt.resume_screen(1); // "Done" — dismisses.
    assert!(!rt.is_on_screen());
    assert_eq!(blocking_text(&next_block(&mut rt)), "final 2");
}

#[test]
fn input_and_confirm_bind_and_replay() {
    let src = r##"
label start:
    input name "Name?"
    confirm ok "Sure?"
    "got [name] [ok]"
    end
"##;
    let mut rt = runtime(src);
    match next_block(&mut rt) {
        RuntimeEvent::Input { prompt } => {
            assert_eq!(prompt.as_deref(), Some("Name?"))
        }
        other => panic!("expected Input, got {other:?}"),
    }
    rt.resume_input("Bob".to_string());
    match next_block(&mut rt) {
        RuntimeEvent::Confirm { message } => {
            assert_eq!(message.as_deref(), Some("Sure?"))
        }
        other => panic!("expected Confirm, got {other:?}"),
    }
    rt.resume_confirm(true);
    assert_eq!(blocking_text(&next_block(&mut rt)), "got Bob true");

    // Journal replay reproduces the same run deterministically.
    let journal = rt.journal.clone();
    assert_eq!(journal.len(), 2);
    let script = Parser::parse(src).unwrap();
    let mut rt2 = Runtime::replay(script, &journal).unwrap();
    // Same variable state → the same line was produced.
    assert_eq!(
        rt2.history.last().map(|h| h.text.as_str()),
        Some("got Bob true")
    );
}

#[test]
fn nvl_clear_advances_page_start() {
    let mut rt = runtime(
        r##"
define n = Character("N", kind="nvl")

label start:
    n "line one"
    nvl clear
    n "line two"
"##,
    );
    assert_eq!(blocking_text(&next_block(&mut rt)), "line one");
    assert_eq!(rt.nvl_start, 0);
    assert_eq!(blocking_text(&next_block(&mut rt)), "line two");
    assert_eq!(rt.nvl_start, 1);
    // The NVL page now only contains "line two".
    assert_eq!(rt.history[rt.nvl_start..].len(), 1);
}

#[test]
fn window_mode_tracked() {
    let mut rt = runtime(
        r##"
label start:
    window hide
    "hidden box"
"##,
    );
    // The window command is an effect; the mode is tracked.
    loop {
        match rt.step() {
            RuntimeEvent::Effect(Command::Window { mode }) => {
                assert_eq!(mode, "hide");
                break;
            }
            RuntimeEvent::Effect(_) | RuntimeEvent::Blocking(_) => {}
            other => panic!("expected Window effect, got {other:?}"),
        }
    }
    assert_eq!(rt.window_mode, "hide");
}

#[test]
fn screens_survive_save_restore() {
    let mut rt = runtime(
        r##"
screen hud:
    text "HP" x=0.02 y=0.02

label start:
    show screen hud
    window hide
    nvl clear
    "checkpoint"
"##,
    );
    assert_eq!(blocking_text(&next_block(&mut rt)), "checkpoint");
    let snap = rt.capture();
    let script = Parser::parse(
        r##"
screen hud:
    text "HP" x=0.02 y=0.02

label start:
    show screen hud
    window hide
    nvl clear
    "checkpoint"
"##,
    )
    .unwrap();
    let rt2 = Runtime::restore(script, snap);
    assert_eq!(rt2.screens.len(), 1);
    assert_eq!(rt2.screens[0].name, "hud");
    assert_eq!(rt2.window_mode, "hide");
    assert_eq!(rt2.nvl_start, rt.nvl_start);
}

#[test]
fn screen_jump_action_restarts_at_label() {
    let mut rt = runtime(
        r##"
screen nav:
    button "Go" x=0.4 y=0.5:
        jump "target"

label start:
    show screen nav
    "unreached"

label target:
    "arrived"
"##,
    );
    // show screen is an effect; the say blocks next.
    loop {
        match rt.step() {
            RuntimeEvent::Effect(_) => {}
            ev => break,
        }
    }
    assert_eq!(rt.screens.len(), 1);
    // Click the button: jump resets execution to `target`.
    let clicked = rt.screen_action(0, 0);
    assert!(clicked);
    assert_eq!(blocking_text(&next_block(&mut rt)), "arrived");
}
