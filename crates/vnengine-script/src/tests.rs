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
        Stmt::If {
            branches,
            else_branch,
        } => {
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
    use std::collections::HashMap;
    use vnengine_core::Value;

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
    match vm.step() {
        StepResult::Command(c) => c,
        StepResult::Menu { .. } => panic!("unexpected menu"),
        StepResult::Finished => panic!("VM finished unexpectedly"),
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
        Command::Say {
            speaker,
            text,
            voice,
            ..
        } => {
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
    assert_eq!(
        vm.state.variables.get("score"),
        Some(&vnengine_core::Value::Int(42))
    );
    assert_eq!(
        vm.state.variables.get("name"),
        Some(&vnengine_core::Value::Str("Alice".into()))
    );
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
        if let StepResult::Command(Command::Narrate { text }) = vm.step() {
            assert_eq!(text, "High score!");
            return;
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
        if let StepResult::Command(Command::Narrate { text }) = vm.step() {
            assert_eq!(text, "Low score.");
            return;
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
    let restored_state: crate::vm::VmState = serde_json::from_slice(&serialized).unwrap();
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
use std::collections::HashMap;
use vnengine_core::Value;

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
    vars.insert("pi".to_string(), Value::Float(3.5));
    let result = interpolate_text("Pi is [pi]", &vars);
    assert_eq!(result, "Pi is 3.5");
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
    assert!(
        matches!(&body[1], Stmt::AssignOp { name, op, .. } if name == "x" && *op == BinaryOp::Add)
    );
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
    assert!(
        matches!(&body[1], Stmt::AssignOp { name, op, .. } if name == "x" && *op == BinaryOp::Sub)
    );
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
    assert!(
        matches!(&body[1], Stmt::AssignOp { name, op, .. } if name == "x" && *op == BinaryOp::Mul)
    );
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
    assert!(
        matches!(&body[1], Stmt::AssignOp { name, op, .. } if name == "x" && *op == BinaryOp::Div)
    );
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
    vm.state
        .variables
        .insert("affection".to_string(), Value::Int(42));
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

#[test]
fn parse_user_function() {
    let src = r##"
def greet():
    e "Hello!"
    return

label start:
    call greet
    e "Back in start"
    end
"##;
    let script = Parser::parse(src).unwrap();
    assert_eq!(script.functions.len(), 1);
    assert_eq!(script.functions[0].name, "greet");
    assert_eq!(script.functions[0].params.len(), 0);
}

#[test]
fn vm_call_user_function() {
    let src = r##"
def greet():
    e "Hello from function!"
    return

label start:
    call greet
    e "Back in start"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    // First say: from the function
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Say { text, .. } => assert_eq!(text, "Hello from function!"),
        other => panic!("expected Say, got {other:?}"),
    }
    vm.resume(None);
    // Second say: back in start
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Say { text, .. } => assert_eq!(text, "Back in start"),
        other => panic!("expected Say, got {other:?}"),
    }
}

#[test]
fn vm_function_with_params() {
    let src = r##"
def set_name(name):
    $ current_name = name
    return

label start:
    $ current_name = "None"
    call set_name
    e "Name is [current_name]"
    end
"##;
    let script = Parser::parse(src).unwrap();
    // Just verify it parses and the function has 1 param.
    assert_eq!(script.functions[0].params.len(), 1);
    assert_eq!(script.functions[0].params[0], "name");
}

#[test]
fn vm_transform_statement() {
    let src = r##"
label start:
    show eileen happy at center
    transform eileen rotation=1.57 scale=2.0
    e "Transformed!"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    // First command: Show
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Show(_) => {}
        other => panic!("expected Show, got {other:?}"),
    }
    vm.resume(None);
    // Second command: Transform
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Transform {
            character,
            rotation,
            scale,
            ..
        } => {
            assert_eq!(character, "eileen");
            assert!(rotation.is_some());
            assert!(scale.is_some());
        }
        other => panic!("expected Transform, got {other:?}"),
    }
}

// ---- Regression tests: control flow across blocking points ----

/// Drive the VM, collecting narrations and resolving every menu by
/// picking `picker(i)` for the i-th menu encountered.
fn run_collecting(src: &str, picker: impl Fn(usize) -> usize) -> Vec<String> {
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let mut out = Vec::new();
    let mut menus = 0;
    // Bounded so a broken VM (e.g. infinite loop) fails instead of hanging.
    for _ in 0..1000 {
        match vm.step() {
            StepResult::Command(Command::Narrate { text }) => out.push(text),
            StepResult::Command(Command::Say { text, .. }) => out.push(text),
            StepResult::Command(_) => {}
            StepResult::Menu { choices, .. } => {
                let idx = picker(menus).min(choices.len() - 1);
                menus += 1;
                vm.resume(Some(MenuResponse::Selected(idx)));
            }
            StepResult::Finished => return out,
        }
    }
    panic!("VM did not finish within step budget")
}

#[test]
fn vm_menu_does_not_skip_next_statement() {
    let src = r##"
label start:
    menu:
        "Pick":
            "picked"
    "after menu"
    end
"##;
    assert_eq!(run_collecting(src, |_| 0), vec!["picked", "after menu"]);
}

#[test]
fn vm_two_consecutive_menus() {
    let src = r##"
label start:
    menu:
        "A1":
            "first choice"
    menu:
        "B1":
            "second choice"
    "after menus"
    end
"##;
    assert_eq!(
        run_collecting(src, |_| 0),
        vec!["first choice", "second choice", "after menus"]
    );
}

#[test]
fn vm_two_sequential_ifs_same_depth() {
    let src = r##"
label start:
    if True:
        "first if"
    if True:
        "second if"
    end
"##;
    assert_eq!(run_collecting(src, |_| 0), vec!["first if", "second if"]);
}

#[test]
fn vm_two_sequential_else_ifs_same_depth() {
    let src = r##"
label start:
    if False:
        "skipped A"
    else:
        "first else"
    if False:
        "skipped B"
    else:
        "second else"
    end
"##;
    assert_eq!(
        run_collecting(src, |_| 0),
        vec!["first else", "second else"]
    );
}

#[test]
fn vm_nested_for_loops() {
    let src = r##"
label start:
    $ count = 0
    for i in [1, 2]:
        for j in [10, 20]:
            $ count += 1
    "done [count]"
    end
"##;
    assert_eq!(run_collecting(src, |_| 0), vec!["done 4"]);
}

#[test]
fn vm_for_then_if_then_for() {
    // Sequential control-flow blocks at the same indentation must not
    // share synthetic state.
    let src = r##"
label start:
    $ total = 0
    for i in [1, 2]:
        $ total += i
    if total > 0:
        "positive"
    for j in [5]:
        $ total += j
    "total [total]"
    end
"##;
    assert_eq!(run_collecting(src, |_| 0), vec!["positive", "total 8"]);
}

#[test]
fn vm_jump_out_of_if_abandons_block() {
    // A `jump` inside a block abandons the enclosing context:
    // statements after the block must not run once the jump target
    // finishes (Ren'Py semantics).
    let src = r##"
label start:
    if True:
        jump other
    "after if"
label other:
    "in other"
    end
"##;
    assert_eq!(run_collecting(src, |_| 0), vec!["in other"]);
}

#[test]
fn vm_jump_out_of_loop_does_not_resume() {
    let src = r##"
label start:
    $ i = 0
    while i < 10:
        $ i += 1
        jump done
    "never reached"
label done:
    "done i=[i]"
    end
"##;
    assert_eq!(run_collecting(src, |_| 0), vec!["done i=1"]);
}

#[test]
fn vm_jump_inside_call_returns_to_caller() {
    // `jump` inside a called label keeps the return point: falling off
    // the jump target resumes after the `call`.
    let src = r##"
label start:
    call sub
    "after call"
    end
label sub:
    jump dest
label dest:
    "in dest"
"##;
    assert_eq!(run_collecting(src, |_| 0), vec!["in dest", "after call"]);
}

// ---- Regression tests: restore from a VmState mid-block ----

fn restore_vm(src: &str, state: &crate::vm::VmState) -> Vm {
    let bytes = serde_json::to_vec(state).unwrap();
    let restored: crate::vm::VmState = serde_json::from_slice(&bytes).unwrap();
    Vm::restore(Parser::parse(src).unwrap(), restored)
}

#[test]
fn vm_restore_inside_menu_body() {
    let src = r##"
label start:
    menu:
        "Go":
            "one"
            "two"
    "after"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    loop {
        match vm.step() {
            StepResult::Menu { choices, .. } => {
                assert_eq!(choices.len(), 1);
                vm.resume(Some(MenuResponse::Selected(0)));
            }
            StepResult::Command(Command::Narrate { text }) => {
                assert_eq!(text, "one");
                break;
            }
            _ => {}
        }
    }
    // The VM is blocked inside the menu choice body. A fresh VM restored
    // from this state must continue the same body.
    let mut vm2 = restore_vm(src, &vm.save_state());
    match step_until_command(&mut vm2) {
        Command::Narrate { text } => assert_eq!(text, "two"),
        other => panic!("expected Narrate 'two', got {other:?}"),
    }
}

#[test]
fn vm_restore_inside_if_body() {
    let src = r##"
label start:
    if True:
        "in if"
        "still in if"
    "after if"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = step_until_command(&mut vm);
    assert!(matches!(cmd, Command::Narrate { .. }));

    let mut vm2 = restore_vm(src, &vm.save_state());
    match step_until_command(&mut vm2) {
        Command::Narrate { text } => assert_eq!(text, "still in if"),
        other => panic!("expected Narrate 'still in if', got {other:?}"),
    }
}

#[test]
fn vm_restore_inside_for_body() {
    let src = r##"
label start:
    $ count = 0
    for i in [1, 2, 3]:
        "iter"
        $ count += 1
    "after [count]"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = step_until_command(&mut vm);
    assert!(matches!(cmd, Command::Narrate { .. }));

    let mut vm2 = restore_vm(src, &vm.save_state());
    // Should emit "iter" for iterations 2 and 3, then "after 3".
    match step_until_command(&mut vm2) {
        Command::Narrate { text } => assert_eq!(text, "iter"),
        other => panic!("expected Narrate 'iter', got {other:?}"),
    }
    match step_until_command(&mut vm2) {
        Command::Narrate { text } => assert_eq!(text, "iter"),
        other => panic!("expected Narrate 'iter', got {other:?}"),
    }
    match step_until_command(&mut vm2) {
        Command::Narrate { text } => assert_eq!(text, "after 3"),
        other => panic!("expected Narrate 'after 3', got {other:?}"),
    }
}

#[test]
fn vm_restore_inside_function_call() {
    let src = r##"
def sub():
    "in func"
    "func second"
    return

label start:
    call sub
    "after call"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = step_until_command(&mut vm);
    assert!(matches!(cmd, Command::Narrate { .. }));

    let mut vm2 = restore_vm(src, &vm.save_state());
    match step_until_command(&mut vm2) {
        Command::Narrate { text } => assert_eq!(text, "func second"),
        other => panic!("expected Narrate 'func second', got {other:?}"),
    }
    match step_until_command(&mut vm2) {
        Command::Narrate { text } => assert_eq!(text, "after call"),
        other => panic!("expected Narrate 'after call', got {other:?}"),
    }
}

#[test]
fn expr_call_random_is_deterministic() {
    // Two runs with the same initial seed produce the same sequence.
    let src = r##"
label start:
    $ a = random()
    $ b = random()
    "done"
    end
"##;
    let run = || {
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        while !matches!(vm.step(), StepResult::Finished) {}
        (
            vm.state.variables.get("a").cloned(),
            vm.state.variables.get("b").cloned(),
        )
    };
    let (a1, b1) = run();
    let (a2, b2) = run();
    assert_eq!(a1, a2);
    assert_eq!(b1, b2);
    // Successive draws differ.
    assert_ne!(a1, b1);
}

#[test]
fn expr_call_randint_bounds() {
    let src = r##"
label start:
    $ a = randint(3, 3)
    $ b = randint(1, 6)
    "done"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    while !matches!(vm.step(), StepResult::Finished) {}
    assert_eq!(vm.state.variables["a"].as_int(), Some(3));
    let b = vm.state.variables["b"].as_int().unwrap();
    assert!((1..=6).contains(&b));
}

#[test]
fn expr_call_srand_reseeds() {
    let src = r##"
label start:
    $ s = srand(42)
    $ a = random()
    "done"
    end
"##;
    let run = || {
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        while !matches!(vm.step(), StepResult::Finished) {}
        vm.state.variables.get("a").cloned()
    };
    assert_eq!(run(), run());
}

#[test]
fn expr_call_unknown_function_errors() {
    let src = r##"
label start:
    $ a = nosuchfn(1)
    "unreached"
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    // An eval error terminates the VM instead of continuing.
    vm.step();
    assert!(vm.state.finished);
}

#[test]
fn expr_call_argument_count_errors() {
    for src in [
        "label start:\n    $ a = randint(1)",
        "label start:\n    $ a = srand()",
    ] {
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        vm.step();
        assert!(vm.state.finished, "expected error for {src:?}");
    }
}

#[test]
fn rng_state_survives_save_restore() {
    let src = r##"
label start:
    $ a = random()
    "line"
    $ b = random()
    "done"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    // Run to the blocking line, then save.
    let vmsave = loop {
        match vm.step() {
            StepResult::Command(Command::Narrate { .. }) => break vm.save_state(),
            StepResult::Finished => panic!("unexpected finish"),
            _ => {}
        }
    };
    let mut vm2 = restore_vm(src, &vmsave);
    // The next random() after restore must equal the original's next draw.
    while !matches!(vm.step(), StepResult::Finished) {}
    while !matches!(vm2.step(), StepResult::Finished) {}
    assert_eq!(
        vm.state.variables["b"].as_float(),
        vm2.state.variables["b"].as_float()
    );
}

#[test]
fn parse_plugin_command() {
    let src = r##"
label start:
    plugin codex "unlock", "chapel", 42
    end
"##;
    let script = Parser::parse(src).unwrap();
    match &script.labels[0].body[0] {
        Stmt::Plugin { name, args } => {
            assert_eq!(name, "codex");
            assert_eq!(args.len(), 3);
        }
        other => panic!("expected Plugin, got {other:?}"),
    }
}

#[test]
fn vm_emits_plugin_call_with_json_payload() {
    let src = r##"
label start:
    $ who = "chapel"
    plugin codex "unlock", who, 42
    "after"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = loop {
        match vm.step() {
            StepResult::Command(c) => break c,
            StepResult::Finished => panic!("unexpected finish"),
            _ => {}
        }
    };
    match cmd {
        Command::PluginCall { plugin, payload } => {
            assert_eq!(plugin, "codex");
            let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
            assert_eq!(v, serde_json::json!(["unlock", "chapel", 42]));
        }
        other => panic!("expected PluginCall, got {other:?}"),
    }
}

#[test]
fn call_stack_overflow_terminates() {
    // `a` calls itself forever: the depth limit must end the script
    // instead of growing the stack without bound.
    let src = "label a:\n    call a\n";
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    vm.state.stack = vec![crate::vm::Frame {
        body: crate::vm::FrameBody::Label("a".into()),
        stmt: 0,
        is_call: false,
        for_loop: None,
        while_loop: None,
    }];
    while !matches!(vm.step(), StepResult::Finished) {}
    assert!(vm.state.finished);
    assert!(vm.state.stack.len() <= 1024);
}

#[test]
fn light_statement_parses() {
    let src = "label start:\n    light \"torch\" x=0.7 y=0.4 radius=0.3 intensity=1.2 color=\"#ffaa44\"\n    unlight \"torch\"\n    ambient 0.3 0.3 0.4\n    camera x=0.1 zoom=1.2\n    end\n";
    let script = Parser::parse(src).unwrap();
    let body = &script.labels[0].body;
    match &body[0] {
        Stmt::Light { name, color, .. } => {
            assert_eq!(name, "torch");
            let c = color.unwrap();
            assert!((c[0] - 1.0).abs() < 0.01);
            assert!((c[1] - 0.667).abs() < 0.01);
        }
        other => panic!("expected Light, got {other:?}"),
    }
    assert!(matches!(&body[1], Stmt::Unlight { name } if name == "torch"));
    assert!(matches!(&body[2], Stmt::Ambient { .. }));
    match &body[3] {
        Stmt::Camera { x, y, zoom, .. } => {
            assert!(x.is_some() && y.is_none() && zoom.is_some());
        }
        other => panic!("expected Camera, got {other:?}"),
    }
}

#[test]
fn light_requires_named_params() {
    let src = "label start:\n    light \"torch\" x=0.5\n    end\n";
    assert!(Parser::parse(src).is_err());
}
