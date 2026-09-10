#[cfg(test)]
mod tests {
    use crate::ast::*;
    use crate::parser::Parser;

    #[test]
    fn parse_simple_dialogue() {
        let src = r##"
define e = Character("Eileen", color = "#ff4040")

label start:
    e "Hello, world!"
    "This is narration."
    end
"##;
        let script = Parser::parse(src).unwrap();
        assert_eq!(script.definitions.len(), 1);
        assert_eq!(script.labels.len(), 1);
        assert_eq!(script.labels[0].name, "start");
        assert_eq!(script.labels[0].body.len(), 3);
    }

    #[test]
    fn parse_menu_with_choices() {
        let src = r##"
label start:
    menu:
        "Go left":
            jump left
        "Go right":
            jump right
    end
"##;
        let script = Parser::parse(src).unwrap();
        let label = &script.labels[0];
        match &label.body[0] {
            Stmt::Menu { prompt, choices } => {
                assert!(prompt.is_none());
                assert_eq!(choices.len(), 2);
                assert_eq!(choices[0].text, "Go left");
                assert_eq!(choices[1].text, "Go right");
            }
            other => panic!("expected Menu, got {other:?}"),
        }
    }

    #[test]
    fn parse_if_else() {
        let src = r##"
label start:
    if score > 50:
        e "Great!"
    elif score > 20:
        e "Okay."
    else:
        e "Try again."
    end
"##;
        let script = Parser::parse(src).unwrap();
        let label = &script.labels[0];
        match &label.body[0] {
            Stmt::If { branches, else_branch } => {
                assert_eq!(branches.len(), 2);
                assert!(else_branch.is_some());
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn parse_scene_and_show() {
        let src = r##"
label start:
    scene bg school with dissolve 1.0
    show eileen happy at left
    hide eileen with fade 0.5
    end
"##;
        let script = Parser::parse(src).unwrap();
        let label = &script.labels[0];
        assert!(matches!(&label.body[0], Stmt::Scene { .. }));
        assert!(matches!(&label.body[1], Stmt::Show { .. }));
        assert!(matches!(&label.body[2], Stmt::Hide { .. }));
    }

    #[test]
    fn parse_variables_and_assignments() {
        let src = r##"
label start:
    $ score = 0
    $ score = score + 10
    $ name = "Alice"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let label = &script.labels[0];
        assert!(matches!(&label.body[0], Stmt::Assign { .. }));
        assert!(matches!(&label.body[1], Stmt::Assign { .. }));
        assert!(matches!(&label.body[2], Stmt::Assign { .. }));
    }

    #[test]
    fn parse_audio_commands() {
        let src = r##"
label start:
    play music "bgm.ogg" fade 2.0
    stop music fade 1.0
    play sfx "click.wav" volume 0.8
    voice "line01.ogg"
    stop voice
    end
"##;
        let script = Parser::parse(src).unwrap();
        let label = &script.labels[0];
        assert!(matches!(&label.body[0], Stmt::PlayMusic { .. }));
        assert!(matches!(&label.body[1], Stmt::StopMusic { .. }));
        assert!(matches!(&label.body[2], Stmt::PlaySfx { .. }));
        assert!(matches!(&label.body[3], Stmt::PlayVoice { .. }));
        assert!(matches!(&label.body[4], Stmt::StopVoice));
    }

    #[test]
    fn parse_jump_call_return() {
        let src = r##"
label start:
    jump next
label next:
    call sub
    return
label sub:
    return
"##;
        let script = Parser::parse(src).unwrap();
        assert_eq!(script.labels.len(), 3);
        assert!(matches!(&script.labels[0].body[0], Stmt::Jump { .. }));
        assert!(matches!(&script.labels[1].body[0], Stmt::Call { .. }));
        assert!(matches!(&script.labels[1].body[1], Stmt::Return));
    }

    #[test]
    fn parse_demo_script() {
        let src = include_str!("../../../demo/script/main.vn");
        let script = Parser::parse(src).unwrap();
        assert!(script.labels.len() >= 5);
        assert!(script.find_label("start").is_some());
        assert!(script.find_label("talk").is_some());
        assert!(script.find_label("library").is_some());
        assert!(script.find_label("sunset").is_some());
        assert!(script.find_label("ending").is_some());
    }

    #[test]
    fn eval_expressions() {
        use crate::vm::eval_expr;
        use vnengine_core::Value;
        use std::collections::HashMap;

        let mut vars = HashMap::new();
        vars.insert("score".to_string(), Value::Int(42));

        let expr = Parser::parse_expr("score + 10");
        let result = eval_expr(&expr, &vars).unwrap();
        assert_eq!(result, Value::Int(52));

        let expr = Parser::parse_expr("score > 50");
        let result = eval_expr(&expr, &vars).unwrap();
        assert_eq!(result, Value::Bool(false));

        let expr = Parser::parse_expr("score > 40 and score < 50");
        let result = eval_expr(&expr, &vars).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    // ---- VM integration tests ----

    use crate::vm::{MenuResponse, StepResult, Vm};
    use vnengine_core::command::Command;

    fn step_until_command(vm: &mut Vm) -> Command {
        loop {
            match vm.step() {
                StepResult::Command(c) => return c,
                StepResult::Menu { .. } => panic!("unexpected menu"),
                StepResult::Finished => panic!("VM finished unexpectedly"),
            }
        }
    }

    #[test]
    fn vm_executes_say_command() {
        let src = r##"
define e = Character("Eileen", color = "#ff4040")

label start:
    e "Hello, world!"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Say { speaker, text, voice } => {
                assert_eq!(speaker.as_deref(), Some("Eileen"));
                assert_eq!(text, "Hello, world!");
                assert!(voice.is_none());
            }
            other => panic!("expected Say, got {other:?}"),
        }
    }

    #[test]
    fn vm_executes_scene_command() {
        let src = r##"
label start:
    scene bg school with dissolve 1.0
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Scene { image, transition } => {
                assert_eq!(image.as_deref(), Some("bg school"));
                assert_eq!(transition.default_duration(), 1.0);
            }
            other => panic!("expected Scene, got {other:?}"),
        }
    }

    #[test]
    fn vm_executes_show_command() {
        let src = r##"
label start:
    show eileen happy at left
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Show(spec) => {
                assert_eq!(spec.character, "eileen");
                assert_eq!(spec.image, "eileen happy");
                assert_eq!(spec.anchor, vnengine_core::command::Anchor::Left);
            }
            other => panic!("expected Show, got {other:?}"),
        }
    }

    #[test]
    fn vm_executes_jump() {
        let src = r##"
label start:
    jump next
label next:
    "Arrived!"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        // First step: jump (non-blocking, continues)
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Narrate { text } => assert_eq!(text, "Arrived!"),
            other => panic!("expected Narrate after jump, got {other:?}"),
        }
    }

    #[test]
    fn vm_executes_menu_and_resume() {
        let src = r##"
label start:
    menu:
        "Go left":
            "You went left!"
        "Go right":
            "You went right!"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        // Step until we hit the menu.
        loop {
            match vm.step() {
                StepResult::Menu { prompt, choices } => {
                    assert!(prompt.is_none());
                    assert_eq!(choices.len(), 2);
                    assert_eq!(choices[0].text, "Go left");
                    assert_eq!(choices[1].text, "Go right");
                    // Select the second choice.
                    vm.resume(Some(MenuResponse::Selected(1)));
                    break;
                }
                StepResult::Command(_) => {}
                StepResult::Finished => panic!("unexpected finish"),
            }
        }
        // Now we should get the narration from the second choice.
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Narrate { text } => assert_eq!(text, "You went right!"),
            other => panic!("expected Narrate, got {other:?}"),
        }
    }

    #[test]
    fn vm_executes_variable_assignment() {
        let src = r##"
label start:
    $ score = 42
    $ name = "Alice"
    "Done."
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        // Assignments are Continue (non-blocking), so they execute before
        // the first blocking command. Step to the narration.
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Narrate { text } => assert_eq!(text, "Done."),
            other => panic!("expected Narrate, got {other:?}"),
        }
        // Variables should have been set.
        assert_eq!(vm.state.variables.get("score"), Some(&vnengine_core::Value::Int(42)));
        assert_eq!(vm.state.variables.get("name"), Some(&vnengine_core::Value::Str("Alice".into())));
    }

    #[test]
    fn vm_executes_if_true_branch() {
        let src = r##"
label start:
    $ score = 100
    if score > 50:
        "High score!"
    else:
        "Low score."
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        // Skip the SetVariable, get the narration.
        loop {
            match vm.step() {
                StepResult::Command(Command::Narrate { text }) => {
                    assert_eq!(text, "High score!");
                    return;
                }
                _ => {}
            }
        }
    }

    #[test]
    fn vm_executes_if_false_branch() {
        let src = r##"
label start:
    $ score = 10
    if score > 50:
        "High score!"
    else:
        "Low score."
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        loop {
            match vm.step() {
                StepResult::Command(Command::Narrate { text }) => {
                    assert_eq!(text, "Low score.");
                    return;
                }
                _ => {}
            }
        }
    }

    #[test]
    fn vm_executes_play_music() {
        let src = r##"
label start:
    play music "bgm.ogg" fade 2.0
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::PlayMusic { track, fade } => {
                assert_eq!(track, "bgm.ogg");
                assert_eq!(fade, 2.0);
            }
            other => panic!("expected PlayMusic, got {other:?}"),
        }
    }

    #[test]
    fn vm_finishes_on_end() {
        let src = r##"
label start:
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        loop {
            match vm.step() {
                StepResult::Command(Command::End) => {}
                StepResult::Finished => return,
                _ => {}
            }
        }
    }

    #[test]
    fn vm_save_restore_roundtrip() {
        let src = r##"
label start:
    $ score = 42
    "Line one."
    "Line two."
    "Line three."
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        // $ score = 42 is Continue (non-blocking), so first command is "Line one."
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Narrate { text } => assert_eq!(text, "Line one."),
            other => panic!("expected Narrate, got {other:?}"),
        }

        // Save state. VM is now positioned at "Line two."
        let state = vm.save_state();
        let serialized = serde_json::to_vec(&state).unwrap();

        // Create a new VM and restore.
        let script2 = Parser::parse(src).unwrap();
        let restored_state: crate::vm::VmState =
            serde_json::from_slice(&serialized).unwrap();
        let mut vm2 = Vm::restore(script2, restored_state);

        // Next command should be "Line two."
        let cmd = step_until_command(&mut vm2);
        match cmd {
            Command::Narrate { text } => assert_eq!(text, "Line two."),
            other => panic!("expected Narrate, got {other:?}"),
        }
    }
}
