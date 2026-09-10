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

    // ---- Interpolation tests ----

    use crate::vm::interpolate_text;
    use vnengine_core::Value;
    use std::collections::HashMap;

    #[test]
    fn interpolate_simple_var() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), Value::Str("Eileen".to_string()));
        let result = interpolate_text("Hello, [name]!", &vars);
        assert_eq!(result, "Hello, Eileen!");
    }

    #[test]
    fn interpolate_multiple_vars() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), Value::Str("Alice".to_string()));
        vars.insert("count".to_string(), Value::Int(42));
        let result = interpolate_text("[name] has [count] items", &vars);
        assert_eq!(result, "Alice has 42 items");
    }

    #[test]
    fn interpolate_unknown_var_left_as_is() {
        let vars = HashMap::new();
        let result = interpolate_text("Hello, [unknown]!", &vars);
        assert_eq!(result, "Hello, [unknown]!");
    }

    #[test]
    fn interpolate_escaped_bracket() {
        let vars = HashMap::new();
        let result = interpolate_text("Array [[0]] is first", &vars);
        assert_eq!(result, "Array [0] is first");
    }

    #[test]
    fn interpolate_no_brackets() {
        let vars = HashMap::new();
        let result = interpolate_text("Just plain text", &vars);
        assert_eq!(result, "Just plain text");
    }

    #[test]
    fn interpolate_unclosed_bracket() {
        let vars = HashMap::new();
        let result = interpolate_text("Hello [name", &vars);
        assert_eq!(result, "Hello [name");
    }

    #[test]
    fn interpolate_float_var() {
        let mut vars = HashMap::new();
        vars.insert("pi".to_string(), Value::Float(3.14));
        let result = interpolate_text("Pi is [pi]", &vars);
        assert_eq!(result, "Pi is 3.14");
    }

    #[test]
    fn interpolate_bool_var() {
        let mut vars = HashMap::new();
        vars.insert("ready".to_string(), Value::Bool(true));
        let result = interpolate_text("Ready: [ready]", &vars);
        assert_eq!(result, "Ready: true");
    }

    #[test]
    fn vm_interpolates_in_say() {
        let src = r##"
label start:
    $ name = "World"
    e "Hello, [name]!"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        // Skip the assignment (non-blocking), get the Say command.
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Say { text, .. } => assert_eq!(text, "Hello, World!"),
            other => panic!("expected Say, got {other:?}"),
        }
    }

    #[test]
    fn vm_interpolates_in_menu() {
        let src = r##"
label start:
    $ item = "sword"
    menu "Choose your [item]":
        "Take [item]":
            end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        // Skip assignment, expect a Menu.
        let result = vm.step();
        match result {
            StepResult::Menu { prompt, choices } => {
                assert_eq!(prompt.as_deref(), Some("Choose your sword"));
                assert_eq!(choices[0].text, "Take sword");
            }
            other => panic!("expected Menu, got {other:?}"),
        }
    }

    // ---- Compound assignment tests ----

    #[test]
    fn parse_compound_assign_plus() {
        let src = r##"
label start:
    $ x = 5
    $ x += 3
    end
"##;
        let script = Parser::parse(src).unwrap();
        let body = &script.labels[0].body;
        assert!(matches!(&body[1], Stmt::AssignOp { name, op, .. } if name == "x" && *op == BinaryOp::Add));
    }

    #[test]
    fn parse_compound_assign_minus() {
        let src = r##"
label start:
    $ x = 5
    $ x -= 3
    end
"##;
        let script = Parser::parse(src).unwrap();
        let body = &script.labels[0].body;
        assert!(matches!(&body[1], Stmt::AssignOp { name, op, .. } if name == "x" && *op == BinaryOp::Sub));
    }

    #[test]
    fn parse_compound_assign_star() {
        let src = r##"
label start:
    $ x = 5
    $ x *= 3
    end
"##;
        let script = Parser::parse(src).unwrap();
        let body = &script.labels[0].body;
        assert!(matches!(&body[1], Stmt::AssignOp { name, op, .. } if name == "x" && *op == BinaryOp::Mul));
    }

    #[test]
    fn parse_compound_assign_slash() {
        let src = r##"
label start:
    $ x = 6
    $ x /= 2
    end
"##;
        let script = Parser::parse(src).unwrap();
        let body = &script.labels[0].body;
        assert!(matches!(&body[1], Stmt::AssignOp { name, op, .. } if name == "x" && *op == BinaryOp::Div));
    }

    #[test]
    fn vm_executes_compound_plus() {
        let src = r##"
label start:
    $ x = 10
    $ x += 5
    e "Value is [x]"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        // Skip both assignments (non-blocking), get the Say command.
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Say { text, .. } => assert_eq!(text, "Value is 15"),
            other => panic!("expected Say, got {other:?}"),
        }
    }

    #[test]
    fn vm_executes_compound_minus() {
        let src = r##"
label start:
    $ x = 10
    $ x -= 3
    e "Value is [x]"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Say { text, .. } => assert_eq!(text, "Value is 7"),
            other => panic!("expected Say, got {other:?}"),
        }
    }

    #[test]
    fn vm_executes_compound_string_concat() {
        let src = r##"
label start:
    $ msg = "Hello"
    $ msg += " World"
    e "[msg]"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Say { text, .. } => assert_eq!(text, "Hello World"),
            other => panic!("expected Say, got {other:?}"),
        }
    }

    // ---- default variable tests ----

    #[test]
    fn parse_toplevel_default() {
        let src = r##"
default affection = 0
default met_eileen = False

label start:
    end
"##;
        let script = Parser::parse(src).unwrap();
        assert_eq!(script.defaults.len(), 2);
        assert_eq!(script.defaults[0].name, "affection");
        assert_eq!(script.defaults[1].name, "met_eileen");
    }

    #[test]
    fn parse_inline_default_in_label() {
        let src = r##"
label start:
    default score = 100
    end
"##;
        let script = Parser::parse(src).unwrap();
        let body = &script.labels[0].body;
        assert!(matches!(&body[0], Stmt::Default { name, .. } if name == "score"));
    }

    #[test]
    fn vm_toplevel_default_sets_var() {
        let src = r##"
default affection = 5

label start:
    e "Affection is [affection]"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Say { text, .. } => assert_eq!(text, "Affection is 5"),
            other => panic!("expected Say, got {other:?}"),
        }
    }

    #[test]
    fn vm_default_does_not_overwrite_existing() {
        // Simulate a restored variable by pre-setting it before the VM runs.
        let src = r##"
default affection = 0

label start:
    e "Affection is [affection]"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        // Simulate a save restore: set the variable before processing defaults.
        vm.state.variables.insert("affection".to_string(), Value::Int(42));
        // Re-run process_definitions to test the "don't overwrite" logic.
        vm.process_definitions();
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Say { text, .. } => assert_eq!(text, "Affection is 42"),
            other => panic!("expected Say, got {other:?}"),
        }
    }

    #[test]
    fn vm_inline_default_in_label() {
        let src = r##"
label start:
    default score = 100
    e "Score is [score]"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Say { text, .. } => assert_eq!(text, "Score is 100"),
            other => panic!("expected Say, got {other:?}"),
        }
    }

    #[test]
    fn parse_while_loop() {
        let src = r##"
label start:
    $ i = 0
    while i < 3:
        $ i += 1
    e "Done, i = [i]"
    end
"##;
        let script = Parser::parse(src).unwrap();
        assert_eq!(script.labels[0].body.len(), 4); // assign, while, say, end
    }

    #[test]
    fn vm_while_loop() {
        let src = r##"
label start:
    $ i = 0
    while i < 3:
        $ i += 1
    e "Done, i = [i]"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Say { text, .. } => assert_eq!(text, "Done, i = 3"),
            other => panic!("expected Say, got {other:?}"),
        }
    }

    #[test]
    fn parse_for_loop() {
        let src = r##"
label start:
    $ total = 0
    for x in [1, 2, 3]:
        $ total += x
    e "Total = [total]"
    end
"##;
        let script = Parser::parse(src).unwrap();
        // Just verify it parses without error.
        assert!(script.labels[0].body.len() >= 3);
    }

    #[test]
    fn vm_for_loop() {
        let src = r##"
label start:
    $ total = 0
    for x in [1, 2, 3]:
        $ total += x
    e "Total = [total]"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Say { text, .. } => assert_eq!(text, "Total = 6"),
            other => panic!("expected Say, got {other:?}"),
        }
    }

    #[test]
    fn vm_list_literal() {
        use crate::vm::eval_expr;
        use std::collections::HashMap;
        use vnengine_core::Value;
        let expr = Parser::parse_expr("[1, 2, 3]");
        let vars = HashMap::new();
        let val = eval_expr(&expr, &vars).unwrap();
        match val {
            Value::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], Value::Int(1));
                assert_eq!(items[2], Value::Int(3));
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn vm_list_index() {
        use crate::vm::eval_expr;
        use std::collections::HashMap;
        use vnengine_core::Value;
        let expr = Parser::parse_expr("[10, 20, 30][1]");
        let vars = HashMap::new();
        let val = eval_expr(&expr, &vars).unwrap();
        assert_eq!(val, Value::Int(20));
    }

    #[test]
    fn vm_list_concat() {
        use crate::vm::eval_expr;
        use std::collections::HashMap;
        use vnengine_core::Value;
        let expr = Parser::parse_expr("[1, 2] + [3, 4]");
        let vars = HashMap::new();
        let val = eval_expr(&expr, &vars).unwrap();
        match val {
            Value::List(items) => assert_eq!(items.len(), 4),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn vm_while_with_say_body() {
        let src = r##"
label start:
    $ i = 0
    while i < 2:
        $ i += 1
        e "Iteration [i]"
    e "Finished"
    end
"##;
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        // First say: "Iteration 1"
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Say { text, .. } => assert_eq!(text, "Iteration 1"),
            other => panic!("expected Say, got {other:?}"),
        }
        vm.resume(None);
        // Second say: "Iteration 2"
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Say { text, .. } => assert_eq!(text, "Iteration 2"),
            other => panic!("expected Say, got {other:?}"),
        }
        vm.resume(None);
        // Third say: "Finished"
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Say { text, .. } => assert_eq!(text, "Finished"),
            other => panic!("expected Say, got {other:?}"),
        }
    }
}
