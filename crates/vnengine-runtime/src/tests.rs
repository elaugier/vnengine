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
