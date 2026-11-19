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
        Stmt::Menu {
            prompt, choices, ..
        } => {
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
        StepResult::Finished => panic!("VM finished unexpectedly"),
        _ => panic!("unexpected blocking step"),
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
        Command::Scene {
            image, transition, ..
        } => {
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
            _ => panic!("unexpected blocking step"),
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
        matches!(&body[1], Stmt::AssignOp { target, op, .. } if matches!(target, Expr::Var(n) if n == "x") && *op == BinaryOp::Add)
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
        matches!(&body[1], Stmt::AssignOp { target, op, .. } if matches!(target, Expr::Var(n) if n == "x") && *op == BinaryOp::Sub)
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
        matches!(&body[1], Stmt::AssignOp { target, op, .. } if matches!(target, Expr::Var(n) if n == "x") && *op == BinaryOp::Mul)
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
        matches!(&body[1], Stmt::AssignOp { target, op, .. } if matches!(target, Expr::Var(n) if n == "x") && *op == BinaryOp::Div)
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
            _ => panic!("unexpected blocking step"),
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

// ---- Phase 1: dicts, slicing, operators, builtins, functions, dynamics ----

#[test]
fn expr_dict_literal_and_access() {
    use crate::vm::eval_expr;
    use std::collections::HashMap;
    use vnengine_core::Value;
    let vars = HashMap::new();

    let val = eval_expr(&Parser::parse_expr(r#"{name: "Ana", age: 30}"#), &vars).unwrap();
    match &val {
        Value::Dict(m) => {
            assert_eq!(m["name"], Value::Str("Ana".into()));
            assert_eq!(m["age"], Value::Int(30));
        }
        other => panic!("expected Dict, got {other:?}"),
    }
    // d["key"] form.
    let mut vars1 = HashMap::new();
    vars1.insert("d".to_string(), val.clone());
    assert_eq!(
        eval_expr(&Parser::parse_expr(r#"d["name"]"#), &vars1).unwrap(),
        Value::Str("Ana".into())
    );
    // d.key form.
    let mut vars2 = HashMap::new();
    vars2.insert("d".to_string(), val);
    assert_eq!(
        eval_expr(&Parser::parse_expr("d.age"), &vars2).unwrap(),
        Value::Int(30)
    );
    assert!(eval_expr(&Parser::parse_expr("d.missing"), &vars2).is_err());
}

#[test]
fn expr_string_repeat_rejects_oversized_output() {
    use crate::vm::eval_expr;
    use std::collections::HashMap;
    use vnengine_core::Value;

    let vars = HashMap::new();
    let expr = Parser::parse_expr(r#""abc" * 9223372036854775807"#);
    assert_eq!(
        eval_expr(&expr, &vars).unwrap_err(),
        "string repetition too large"
    );
    let expr = Parser::parse_expr(r#""é" * 3"#);
    assert_eq!(eval_expr(&expr, &vars).unwrap(), Value::Str("ééé".into()));
}

#[test]
fn expr_slicing() {
    use crate::vm::eval_expr;
    use std::collections::HashMap;
    use vnengine_core::Value;
    let vars = HashMap::new();

    assert_eq!(
        eval_expr(&Parser::parse_expr("[1,2,3,4][1:3]"), &vars).unwrap(),
        Value::List(vec![Value::Int(2), Value::Int(3)])
    );
    assert_eq!(
        eval_expr(&Parser::parse_expr("[1,2,3,4][-2:]"), &vars).unwrap(),
        Value::List(vec![Value::Int(3), Value::Int(4)])
    );
    assert_eq!(
        eval_expr(&Parser::parse_expr("\"hello\"[1:4]"), &vars).unwrap(),
        Value::Str("ell".into())
    );
    assert_eq!(
        eval_expr(&Parser::parse_expr("[1,2,3][:]"), &vars).unwrap(),
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
}

#[test]
fn expr_in_is_operators() {
    use crate::vm::eval_expr;
    use std::collections::HashMap;
    use vnengine_core::Value;
    let vars = HashMap::new();

    assert_eq!(
        eval_expr(&Parser::parse_expr("3 in [1,2,3]"), &vars).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval_expr(&Parser::parse_expr("5 not in [1,2,3]"), &vars).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval_expr(&Parser::parse_expr("\"ell\" in \"hello\""), &vars).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval_expr(&Parser::parse_expr("\"k\" in {k: 1}"), &vars).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval_expr(&Parser::parse_expr("None is None"), &vars).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval_expr(&Parser::parse_expr("1 is not None"), &vars).unwrap(),
        Value::Bool(true)
    );
}

#[test]
fn expr_builtins() {
    use crate::vm::eval_expr;
    use std::collections::HashMap;
    use vnengine_core::Value;
    let vars = HashMap::new();
    let ev = |s: &str| eval_expr(&Parser::parse_expr(s), &vars).unwrap();

    assert_eq!(ev("len([1,2,3])"), Value::Int(3));
    assert_eq!(ev("len(\"hello\")"), Value::Int(5));
    assert_eq!(ev("str(42)"), Value::Str("42".into()));
    assert_eq!(ev("int(\"7\")"), Value::Int(7));
    assert_eq!(ev("float(2)"), Value::Float(2.0));
    assert_eq!(ev("abs(-3)"), Value::Int(3));
    assert_eq!(ev("min(3,1,2)"), Value::Int(1));
    assert_eq!(ev("max([4,9,2])"), Value::Int(9));
    assert_eq!(ev("round(2.6)"), Value::Int(3));
    assert_eq!(
        ev("range(3)"),
        Value::List(vec![Value::Int(0), Value::Int(1), Value::Int(2)])
    );
    assert_eq!(
        ev("sorted([3,1,2])"),
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
    assert_eq!(
        ev("keys({b:1,a:2})"),
        Value::List(vec![Value::Str("a".into()), Value::Str("b".into())])
    );
    assert_eq!(ev("has([1,2], 2)"), Value::Bool(true));
    assert_eq!(ev("get({a: 5}, \"a\", 0)"), Value::Int(5));
    // choice draws from the deterministic stream.
    let c = ev("choice([\"x\",\"y\",\"z\"])");
    assert!(matches!(c, Value::Str(_)));
}

#[test]
fn expr_methods_and_string_ops() {
    use crate::vm::eval_expr;
    use std::collections::HashMap;
    use vnengine_core::Value;
    let vars = HashMap::new();
    let ev = |s: &str| eval_expr(&Parser::parse_expr(s), &vars).unwrap();

    assert_eq!(ev("\"AbC\".lower()"), Value::Str("abc".into()));
    assert_eq!(ev("\"a,b,c\".split(\",\")").as_list().unwrap().len(), 3);
    assert_eq!(ev("\"hello\".count(\"l\")"), Value::Int(2));
    assert_eq!(
        ev("[3,1,2].sorted()"),
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
    assert_eq!(ev("[\"a\",\"b\"].join(\"-\")"), Value::Str("a-b".into()));
    assert_eq!(ev("\"ab\" * 3"), Value::Str("ababab".into()));
}

#[test]
fn vm_function_in_expression() {
    let src = r##"
def double(x):
    return x * 2

def greet(name):
    $ msg = "Hi " + name
    return msg

label start:
    $ a = double(21)
    $ b = greet("Ana")
    e "a=[a] b=[b]"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Say { text, .. } => assert_eq!(text, "a=42 b=Hi Ana"),
        other => panic!("expected Say, got {other:?}"),
    }
}

#[test]
fn vm_function_blocks_and_loops() {
    let src = r##"
def sum_to(n):
    $ total = 0
    $ i = 0
    while i < n:
        $ total += i
        $ i += 1
    for x in [1, 2, 3]:
        $ total += x
    return total

label start:
    $ v = sum_to(4)
    e "sum=[v]"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Say { text, .. } => assert_eq!(text, "sum=12"), // 0+1+2+3 + 1+2+3
        other => panic!("expected Say, got {other:?}"),
    }
}

#[test]
fn vm_call_function_with_args_statement() {
    let src = r##"
def announce(msg):
    e "[msg]!"
    return

label start:
    call announce("hey")
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    // `call f(args)` runs the function body as a frame: `e "..."` blocks.
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Say { text, .. } => assert_eq!(text, "hey!"),
        other => panic!("expected Say, got {other:?}"),
    }
}

#[test]
fn vm_function_rejects_blocking_statements() {
    let src = r##"
def bad():
    e "nope"
    return 1

label start:
    $ x = bad()
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    while !matches!(vm.step(), StepResult::Finished) {}
    assert!(vm.state.finished);
}

#[test]
fn vm_dynamic_assets_and_jump() {
    let src = r##"
define e = Character("Eileen")

label start:
    $ mood = "happy"
    $ bg = "school"
    $ sfx = "click"
    scene [bg]
    show e [mood] at center
    play sfx [sfx]
    jump ["end" + "ing"]
label ending:
    e "done"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    assert!(matches!(
        step_until_command(&mut vm),
        Command::Scene { image: Some(ref i), .. } if i == "school"
    ));
    assert!(matches!(
        step_until_command(&mut vm),
        Command::Show(ref s) if s.image == "e happy"
    ));
    assert!(matches!(
        step_until_command(&mut vm),
        Command::PlaySfx { ref track, .. } if track == "click"
    ));
    match step_until_command(&mut vm) {
        Command::Say { text, .. } => assert_eq!(text, "done"),
        other => panic!("expected Say, got {other:?}"),
    }
}

#[test]
fn vm_pause_without_duration_waits_for_input() {
    let src = "label start:\n    pause\n    end\n";
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    assert!(matches!(step_until_command(&mut vm), Command::WaitInput));
}

#[test]
fn vm_with_transitions() {
    // Standalone `with`.
    let src = "label start:\n    with dissolve 0.5\n    end\n";
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    assert!(matches!(
        step_until_command(&mut vm),
        Command::PlayTransition(_)
    ));

    // `say ... with` → transition first, then the say.
    let src = "label start:\n    e \"hi\" with fade 0.3\n    end\n";
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    assert!(matches!(
        step_until_command(&mut vm),
        Command::PlayTransition(_)
    ));
    match step_until_command(&mut vm) {
        Command::Say { text, .. } => assert_eq!(text, "hi"),
        other => panic!("expected Say, got {other:?}"),
    }

    // `menu ... with` → transition first, then the menu.
    let src = "label start:\n    menu \"pick\" with fade 0.3:\n        \"a\":\n            end\n    end\n";
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    assert!(matches!(
        step_until_command(&mut vm),
        Command::PlayTransition(_)
    ));
    match vm.step() {
        StepResult::Menu { prompt, choices } => {
            assert_eq!(prompt.as_deref(), Some("pick"));
            assert_eq!(choices.len(), 1);
        }
        other => panic!("expected Menu, got {other:?}"),
    }
}

#[test]
fn vm_hide_all_and_show_layer() {
    let src = "label start:\n    hide all\n    show e happy layer=midground\n    end\n";
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    assert!(matches!(
        step_until_command(&mut vm),
        Command::HideAll { .. }
    ));
    match step_until_command(&mut vm) {
        Command::Show(ref s) => {
            assert_eq!(s.layer, Some(vnengine_core::model::Layer::Midground));
        }
        other => panic!("expected Show, got {other:?}"),
    }
}

#[test]
fn vm_break_continue() {
    let src = r##"
label start:
    $ total = 0
    for x in [1, 2, 3, 4]:
        if x == 2:
            continue
        if x == 4:
            break
        $ total += x
    e "total=[total]"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Say { text, .. } => assert_eq!(text, "total=4"), // 1 + 3
        other => panic!("expected Say, got {other:?}"),
    }
}

#[test]
fn vm_interpolate_expr() {
    let src = "label start:\n    $ items = [1, 2, 3]\n    $ player = {name: \"Ana\", level: 7}\n    e \"n=[len(items)] name=[player.name] lvl=[player[\\\"level\\\"] + 1] missing=[nope]\"\n    end\n";
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Say { text, .. } => {
            assert_eq!(text, "n=3 name=Ana lvl=8 missing=[nope]");
        }
        other => panic!("expected Say, got {other:?}"),
    }
}

#[test]
fn expr_function_via_ctx() {
    // eval_expr_ctx is crate-visible: user functions resolve in Call.
    use crate::vm::eval_expr_ctx;
    use std::collections::HashMap;
    use vnengine_core::Value;
    let src = "def inc(x):\n    return x + 1\n";
    let script = Parser::parse(src).unwrap();
    let fns: HashMap<String, FunctionDef> = script
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.clone()))
        .collect();
    let vars = HashMap::new();
    let mut rng = 1u64;
    let v = eval_expr_ctx(&Parser::parse_expr("inc(41)"), &vars, Some(&fns), &mut rng).unwrap();
    assert_eq!(v, Value::Int(42));
}

#[test]
fn vm_label_with_params() {
    let src = r##"
label greet(name, punct):
    e "Hello [name][punct]"
    return

label start:
    call greet("Ana", "!")
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Say { text, .. } => assert_eq!(text, "Hello Ana!"),
        other => panic!("expected Say, got {other:?}"),
    }
}

#[test]
fn vm_menu_set_var() {
    let src = r##"
label start:
    menu set mood:
        "happy":
            e "yay"
        "sad":
            e "aww"
    e "mood index=[mood]"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    match vm.step() {
        StepResult::Menu { choices, .. } => assert_eq!(choices.len(), 2),
        other => panic!("expected Menu, got {other:?}"),
    }
    vm.resume(Some(MenuResponse::Selected(1)));
    // Choice body says "aww", then the follow-up line.
    match step_until_command(&mut vm) {
        Command::Say { text, .. } => assert_eq!(text, "aww"),
        other => panic!("expected Say, got {other:?}"),
    }
    vm.resume(None);
    match step_until_command(&mut vm) {
        Command::Say { text, .. } => assert_eq!(text, "mood index=1"),
        other => panic!("expected Say, got {other:?}"),
    }
}

// ---------- Phase 2: ATL ----------

#[test]
fn parse_transform_def_block() {
    let src = r##"
transform bob:
    ease 0.5 xpos 0.5
    pause 0.2
    repeat 2

label start:
    "done"
    end
"##;
    let script = Parser::parse(src).unwrap();
    assert_eq!(script.transforms.len(), 1);
    let t = &script.transforms[0];
    assert_eq!(t.name, "bob");
    assert_eq!(t.body.len(), 3);
    assert!(matches!(t.body[0], AtlStmt::Warp { .. }));
    assert!(matches!(t.body[1], AtlStmt::Pause { .. }));
    assert!(matches!(t.body[2], AtlStmt::Repeat { .. }));
}

#[test]
fn parse_atl_events_and_blocks() {
    let src = r##"
transform fancy:
    on show:
        alpha 0.0
        ease 0.3 alpha 1.0
    on hide:
        ease 0.3 alpha 0.0
    parallel:
        linear 1.0 rotate 0.5
    parallel:
        linear 1.0 yzoom 2.0
    choice:
        xpos 0.2
    choice 3.0:
        xpos 0.8
    time 1.5

label start:
    "done"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let t = &script.transforms[0];
    // on show / on hide / parallel / parallel / choice / choice / time
    assert_eq!(t.body.len(), 7);
    match &t.body[0] {
        AtlStmt::On { event, body } => {
            assert_eq!(event, "show");
            assert_eq!(body.len(), 2);
        }
        other => panic!("expected On, got {other:?}"),
    }
    match &t.body[1] {
        AtlStmt::On { event, .. } => assert_eq!(event, "hide"),
        other => panic!("expected On, got {other:?}"),
    }
    assert!(matches!(t.body[2], AtlStmt::Parallel { .. }));
    assert!(matches!(t.body[4], AtlStmt::Choice { weight: None, .. }));
    assert!(matches!(
        t.body[5],
        AtlStmt::Choice {
            weight: Some(_),
            ..
        }
    ));
}

#[test]
fn parse_show_at_transform() {
    let src = r##"
label start:
    show eileen happy at left, bob
    end
"##;
    let script = Parser::parse(src).unwrap();
    match &script.labels[0].body[0] {
        Stmt::Show { anchor, at, .. } => {
            // `left` resolves to a named anchor; `bob` stays in `at`
            // as a transform name.
            assert!(anchor.is_some());
            assert_eq!(at, &vec!["bob".to_string()]);
        }
        other => panic!("expected Show, got {other:?}"),
    }
}

#[test]
fn parse_scene_onlayer() {
    let src = r##"
label start:
    scene forest onlayer midground
    end
"##;
    let script = Parser::parse(src).unwrap();
    match &script.labels[0].body[0] {
        Stmt::Scene { layer, .. } => {
            assert_eq!(layer.as_deref(), Some("midground"));
        }
        other => panic!("expected Scene, got {other:?}"),
    }
}

#[test]
fn parse_image_composite_def() {
    let src = r##"
image duo:
    "char_a.png" offset=(0.2, 0.0)
    "char_b.png" offset=(0.7, 0.0) alpha=0.9 when flag

label start:
    "done"
    end
"##;
    let script = Parser::parse(src).unwrap();
    assert_eq!(script.images.len(), 1);
    let img = &script.images[0];
    assert_eq!(img.name, "duo");
    assert_eq!(img.layers.len(), 2);
    assert!(img.layers[0].when.is_none());
    assert!(img.layers[1].when.is_some());
    assert!(img.layers[1].alpha.is_some());
}

#[test]
fn vm_show_at_compiles_tracks() {
    let src = r##"
transform bob:
    ease 1.0 ypos 0.6

label start:
    show eileen happy at center, bob
    e "hi"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Show(spec) => {
            assert_eq!(spec.character, "eileen");
            assert_eq!(spec.anims.len(), 1);
            assert_eq!(spec.anims[0].steps.len(), 1);
            assert!((spec.anims[0].steps[0].duration - 1.0).abs() < 1e-4);
        }
        other => panic!("expected Show, got {other:?}"),
    }
}

#[test]
fn vm_show_at_events_compiled() {
    let src = r##"
transform inout:
    on show:
        alpha 0.0
        ease 0.5 alpha 1.0
    on hide:
        ease 0.5 alpha 0.0

label start:
    show eileen happy at center, inout
    e "hi"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Show(spec) => {
            assert!(spec.anim_events.contains_key("show"));
            assert!(spec.anim_events.contains_key("hide"));
            assert_eq!(spec.anim_events["hide"].len(), 1);
        }
        other => panic!("expected Show, got {other:?}"),
    }
}

#[test]
fn vm_atl_choice_uses_seeded_rng() {
    let src = r##"
transform pick:
    choice:
        xpos 0.1
    choice:
        xpos 0.9

label start:
    show e happy at center, pick
    "done"
    end
"##;
    // Two identical VMs must pick the same branch.
    let mut picks = Vec::new();
    for _ in 0..2 {
        let script = Parser::parse(src).unwrap();
        let mut vm = Vm::new(script);
        let cmd = step_until_command(&mut vm);
        match cmd {
            Command::Show(spec) => {
                let step = &spec.anims[0].steps[0];
                let (_, v) = step.targets[0];
                picks.push(v);
            }
            other => panic!("expected Show, got {other:?}"),
        }
    }
    assert_eq!(picks[0], picks[1]);
}

#[test]
fn vm_scene_onlayer_command() {
    let src = r##"
label start:
    scene forest onlayer midground
    "done"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Scene { image, layer, .. } => {
            assert_eq!(image.as_deref(), Some("forest"));
            assert_eq!(layer, Some(vnengine_core::model::Layer::Midground));
        }
        other => panic!("expected Scene, got {other:?}"),
    }
}

#[test]
fn vm_image_composite_layers_on_show() {
    let src = r##"
image duo:
    "a.png" offset=(0.0, 0.0)
    "b.png" offset=(0.5, 0.0) zoom=0.5

label start:
    show duo at center
    "done"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Show(spec) => {
            assert_eq!(spec.layers.len(), 2);
            assert_eq!(spec.layers[0].image, "a.png");
            assert_eq!(spec.layers[1].image, "b.png");
            assert!((spec.layers[1].zoom - 0.5).abs() < 1e-4);
        }
        other => panic!("expected Show, got {other:?}"),
    }
}

#[test]
fn vm_image_composite_when_filters() {
    let src = r##"
image duo:
    "a.png"
    "b.png" when show_b

label start:
    $ show_b = False
    show duo at center
    "done"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let cmd = step_until_command(&mut vm);
    match cmd {
        Command::Show(spec) => assert_eq!(spec.layers.len(), 1),
        other => panic!("expected Show, got {other:?}"),
    }
}

#[test]
fn vm_new_transition_names() {
    let src = r##"
label start:
    scene bg with blinds
    show e happy at center with pushleft
    "done"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    match step_until_command(&mut vm) {
        Command::Scene { transition, .. } => {
            assert!(matches!(
                transition,
                vnengine_core::transition::Transition::Blinds { .. }
            ));
        }
        other => panic!("expected Scene, got {other:?}"),
    }
    match step_until_command(&mut vm) {
        Command::Show(spec) => {
            assert!(matches!(
                spec.transition,
                Some(vnengine_core::transition::Transition::Push { .. })
            ));
        }
        other => panic!("expected Show, got {other:?}"),
    }
}

#[test]
fn vm_repeat_tags_track() {
    let src = r##"
transform bob:
    ease 0.5 yoffset -0.05
    ease 0.5 yoffset 0.0
    repeat

label start:
    show e happy at center, bob
    "done"
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    match step_until_command(&mut vm) {
        Command::Show(spec) => {
            assert_eq!(spec.anims.len(), 1);
            assert_eq!(spec.anims[0].steps.len(), 2);
            assert_eq!(spec.anims[0].repeats, None); // infinite
        }
        other => panic!("expected Show, got {other:?}"),
    }
}

// ============ Phase 3: screen language / input / confirm / nvl / window ============

#[test]
fn parse_screen_def_all_nodes() {
    let src = r##"
screen hud:
    text "Score: [score]" x=0.02 y=0.02 size=20 color="#ff0000"
    box x=0.0 y=0.9 w=1.0 h=0.1 color="#000000"
    image "icon" x=0.5 y=0.5 w=0.1 h=0.1 alpha=0.8
    button "Go" x=0.4 y=0.5 w=0.2 h=0.06:
        jump "target"
    if flag:
        text "flag on" x=0.1 y=0.1
    for it in items:
        text it x=0.1 y=0.2

label start:
    "done"
"##;
    let script = Parser::parse(src).unwrap();
    assert_eq!(script.screens.len(), 1);
    let scr = &script.screens[0];
    assert_eq!(scr.name, "hud");
    assert_eq!(scr.body.len(), 6);
    match &scr.body[0] {
        ScreenStmt::Text { size, color, .. } => {
            assert!(size.is_some());
            assert!(color.is_some());
        }
        other => panic!("expected Text, got {other:?}"),
    }
    match &scr.body[1] {
        ScreenStmt::Box { .. } => {}
        other => panic!("expected Box, got {other:?}"),
    }
    match &scr.body[2] {
        ScreenStmt::Image { alpha, .. } => assert!(alpha.is_some()),
        other => panic!("expected Image, got {other:?}"),
    }
    match &scr.body[3] {
        ScreenStmt::Button { actions, .. } => {
            assert_eq!(actions.len(), 1);
            assert!(matches!(actions[0], ButtonAction::Jump(_)));
        }
        other => panic!("expected Button, got {other:?}"),
    }
    match &scr.body[4] {
        ScreenStmt::If { body, .. } => assert_eq!(body.len(), 1),
        other => panic!("expected If, got {other:?}"),
    }
    match &scr.body[5] {
        ScreenStmt::For { var, body, .. } => {
            assert_eq!(var, "it");
            assert_eq!(body.len(), 1);
        }
        other => panic!("expected For, got {other:?}"),
    }
}

#[test]
fn parse_screen_lifecycle_stmts() {
    let src = r##"
label start:
    show screen "hud"
    hide screen "hud"
    call screen pick set choice_made
    input player_name "Your name?"
    confirm answer "Sure?"
    nvl clear
    window hide
    window show
"##;
    let script = Parser::parse(src).unwrap();
    let stmts = &script.labels[0].body;
    assert!(matches!(stmts[0], Stmt::ShowScreen { .. }));
    assert!(matches!(stmts[1], Stmt::HideScreen { .. }));
    match &stmts[2] {
        Stmt::CallScreen { set_var, .. } => {
            assert_eq!(set_var.as_deref(), Some("choice_made"))
        }
        other => panic!("expected CallScreen, got {other:?}"),
    }
    match &stmts[3] {
        Stmt::Input { var, prompt } => {
            assert_eq!(var, "player_name");
            assert!(prompt.is_some());
        }
        other => panic!("expected Input, got {other:?}"),
    }
    match &stmts[4] {
        Stmt::Confirm { var, message } => {
            assert_eq!(var, "answer");
            assert!(message.is_some());
        }
        other => panic!("expected Confirm, got {other:?}"),
    }
    assert!(matches!(stmts[5], Stmt::NvlClear));
    assert!(matches!(&stmts[6], Stmt::Window { mode } if mode == "hide"));
    assert!(matches!(&stmts[7], Stmt::Window { mode } if mode == "show"));
}

#[test]
fn parse_button_multiple_actions() {
    let src = r##"
screen pick:
    button "Yes" x=0.3 y=0.5:
        set picked = True
        return 1

label start:
    "x"
"##;
    let script = Parser::parse(src).unwrap();
    match &script.screens[0].body[0] {
        ScreenStmt::Button { actions, .. } => {
            assert_eq!(actions.len(), 2);
            assert!(
                matches!(&actions[0], ButtonAction::Set { target, .. } if matches!(target, Expr::Var(n) if n == "picked"))
            );
            assert!(matches!(actions[1], ButtonAction::Return(_)));
        }
        other => panic!("expected Button, got {other:?}"),
    }
}

#[test]
fn parse_character_voice_image_kind() {
    let src = r##"
define e = Character("Eileen", voice="e_v.ogg", image="e side", kind="nvl")

label start:
    e "hi"
"##;
    let script = Parser::parse(src).unwrap();
    match &script.definitions[0] {
        Definition::Character {
            voice, image, kind, ..
        } => {
            assert!(voice.is_some());
            assert!(image.is_some());
            assert!(kind.is_some());
        }
        other => panic!("expected Character, got {other:?}"),
    }
}

#[test]
fn vm_show_and_hide_screen() {
    let src = r##"
screen hud:
    text "Score: [score]" x=0.02 y=0.02

label start:
    $ score = 42
    show screen hud
    hide screen "hud"
    "done"
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let mut shown = false;
    let mut hidden = false;
    for _ in 0..20 {
        match vm.step() {
            StepResult::Command(Command::ShowScreen(inst)) => {
                assert_eq!(inst.name, "hud");
                assert_eq!(inst.nodes.len(), 1);
                // `text` interpolated with the current variable value.
                match &inst.nodes[0].text {
                    Some(t) => assert_eq!(t, "Score: 42"),
                    None => panic!("text node missing"),
                }
                shown = true;
            }
            StepResult::Command(Command::HideScreen { name }) => {
                assert_eq!(name, "hud");
                hidden = true;
            }
            StepResult::Command(_) => {}
            StepResult::Finished => break,
            _ => panic!("unexpected blocking step"),
        }
    }
    assert!(shown && hidden);
}

#[test]
fn vm_call_screen_return_binds_var() {
    let src = r##"
screen pick:
    button "One" x=0.2 y=0.4:
        return 1
    button "Two" x=0.6 y=0.4:
        return 2

label start:
    call screen pick set chosen
    "picked [chosen]"
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    match vm.step() {
        StepResult::Screen { inst } => {
            assert_eq!(inst.name, "pick");
            assert_eq!(inst.clickables().len(), 2);
            // Click the second button.
            vm.resume_screen(1);
        }
        other => panic!("expected Screen, got {other:?}"),
    }
    assert!(!vm.is_on_screen());
    assert_eq!(
        vm.state.variables.get("chosen"),
        Some(&vnengine_core::Value::Int(2))
    );
    // Next step produces the narration line.
    match vm.step() {
        StepResult::Command(Command::Narrate { text }) => {
            assert_eq!(text, "picked 2");
        }
        other => panic!("expected Narrate, got {other:?}"),
    }
}

#[test]
fn vm_screen_set_action_stays_up() {
    let src = r##"
screen counter:
    button "Add" x=0.2 y=0.4:
        set count = count + 1

label start:
    $ count = 0
    call screen counter
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    match vm.step() {
        StepResult::Screen { inst } => {
            assert_eq!(inst.clickables().len(), 1);
            vm.resume_screen(0);
        }
        other => panic!("expected Screen, got {other:?}"),
    }
    // `set` is non-dismissing: the screen is still pending.
    assert!(vm.is_on_screen());
    assert_eq!(
        vm.state.variables.get("count"),
        Some(&vnengine_core::Value::Int(1))
    );
}

#[test]
fn vm_input_and_confirm_bind_variables() {
    let src = r##"
label start:
    input player_name "Your name?"
    confirm ok "Really?"
    "hello [player_name], ok=[ok]"
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    match vm.step() {
        StepResult::Input { prompt } => {
            assert_eq!(prompt.as_deref(), Some("Your name?"));
            vm.resume_input("Alice".to_string());
        }
        other => panic!("expected Input, got {other:?}"),
    }
    match vm.step() {
        StepResult::Confirm { message } => {
            assert_eq!(message.as_deref(), Some("Really?"));
            vm.resume_confirm(true);
        }
        other => panic!("expected Confirm, got {other:?}"),
    }
    match vm.step() {
        StepResult::Command(Command::Narrate { text }) => {
            assert_eq!(text, "hello Alice, ok=true");
        }
        other => panic!("expected Narrate, got {other:?}"),
    }
}

#[test]
fn vm_nvl_character_marks_say() {
    let src = r##"
define n = Character("Narrator", kind="nvl")

label start:
    n "full screen text"
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    match vm.step() {
        StepResult::Command(Command::Say { nvl, .. }) => assert!(nvl),
        other => panic!("expected nvl Say, got {other:?}"),
    }
}

#[test]
fn vm_window_and_nvlclear_commands() {
    let src = r##"
label start:
    window hide
    nvl clear
    window show
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    match vm.step() {
        StepResult::Command(Command::Window { mode }) => assert_eq!(mode, "hide"),
        other => panic!("expected Window, got {other:?}"),
    }
    match vm.step() {
        StepResult::Command(Command::NvlClear) => {}
        other => panic!("expected NvlClear, got {other:?}"),
    }
    match vm.step() {
        StepResult::Command(Command::Window { mode }) => assert_eq!(mode, "show"),
        other => panic!("expected Window, got {other:?}"),
    }
}

#[test]
fn vm_screen_if_for_expands() {
    let src = r##"
screen listing:
    if show_extra:
        text "extra" x=0.1 y=0.1
    for it in items:
        text it x=0.1 y=0.2

label start:
    $ show_extra = True
    $ items = ["a", "b"]
    show screen listing
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    let mut inst = None;
    for _ in 0..10 {
        match vm.step() {
            StepResult::Command(Command::ShowScreen(i)) => inst = Some(i),
            StepResult::Command(_) => {}
            StepResult::Finished => break,
            _ => panic!("unexpected blocking step"),
        }
    }
    let inst = inst.expect("screen was shown");
    // 1 conditional text + 2 loop iterations.
    assert_eq!(inst.nodes.len(), 3);
    assert_eq!(inst.nodes[1].text.as_deref(), Some("a"));
    assert_eq!(inst.nodes[2].text.as_deref(), Some("b"));
}

// ---------- Phase 4: named audio channels ----------

#[test]
fn parse_channel_commands() {
    let src = r##"
label start:
    play channel "amb" "wind.ogg" volume=0.5 fade=1.0 loop
    queue channel amb "rain.ogg"
    stop channel amb fade=0.5
    stop all fade=0.2
    volume channel amb 0.7 fade=0.3
    filter channel amb lowpass cutoff=800 mix=0.9
    filter channel amb reverb feedback=0.6
    filter channel amb off
    end
"##;
    let script = Parser::parse(src).unwrap();
    let b = &script.labels[0].body;
    match &b[0] {
        Stmt::PlayChannel {
            channel,
            volume,
            fade,
            loop_,
            ..
        } => {
            assert!(matches!(channel, Expr::Str(n) if n == "amb"));
            assert!(volume.is_some() && fade.is_some() && *loop_);
        }
        other => panic!("expected PlayChannel, got {other:?}"),
    }
    match &b[1] {
        Stmt::QueueChannel {
            channel,
            track,
            loop_,
            ..
        } => {
            assert!(matches!(channel, Expr::Str(n) if n == "amb"));
            assert!(matches!(track, Expr::Str(t) if t == "rain.ogg"));
            assert!(!*loop_);
        }
        other => panic!("expected QueueChannel, got {other:?}"),
    }
    assert!(matches!(&b[2], Stmt::StopChannel { fade: Some(_), .. }));
    assert!(matches!(&b[3], Stmt::StopAll { fade: Some(_) }));
    assert!(matches!(&b[4], Stmt::VolumeChannel { .. }));
    match &b[5] {
        Stmt::FilterChannel { kind, params, .. } => {
            assert_eq!(kind, "lowpass");
            assert_eq!(params.len(), 2);
        }
        other => panic!("expected FilterChannel, got {other:?}"),
    }
    assert!(matches!(&b[6], Stmt::FilterChannel { kind, .. } if kind == "reverb"));
    assert!(matches!(&b[7], Stmt::FilterChannel { kind, .. } if kind == "off"));
}

#[test]
fn channel_keywords_still_allow_character_names() {
    // `queue`/`volume`/`filter` without a `channel` second token fall
    // through to say parsing — a character may be named e.g. `volume`.
    let src = r##"
define volume = Character("Vol", color = "#ffffff")

label start:
    volume "I speak"
    end
"##;
    let script = Parser::parse(src).unwrap();
    assert!(matches!(&script.labels[0].body[0], Stmt::Say { .. }));
}

#[test]
fn vm_channel_commands() {
    let src = r##"
label start:
    play channel amb "wind.ogg" volume=0.5 fade=1.0 loop
    queue channel amb "rain.ogg"
    volume channel amb 0.7
    filter channel amb lowpass cutoff=800 mix=0.9
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    match step_until_command(&mut vm) {
        Command::PlayChannel {
            channel,
            track,
            volume,
            fade,
            loop_,
        } => {
            assert_eq!(channel, "amb");
            assert_eq!(track, "wind.ogg");
            assert!((volume - 0.5).abs() < 1e-6);
            assert!((fade - 1.0).abs() < 1e-6);
            assert!(loop_);
        }
        other => panic!("expected PlayChannel, got {other:?}"),
    }
    match step_until_command(&mut vm) {
        Command::QueueChannel { channel, track, .. } => {
            assert_eq!(channel, "amb");
            assert_eq!(track, "rain.ogg");
        }
        other => panic!("expected QueueChannel, got {other:?}"),
    }
    match step_until_command(&mut vm) {
        Command::VolumeChannel {
            channel, volume, ..
        } => {
            assert_eq!(channel, "amb");
            assert!((volume - 0.7).abs() < 1e-6);
        }
        other => panic!("expected VolumeChannel, got {other:?}"),
    }
    match step_until_command(&mut vm) {
        Command::FilterChannel { channel, filter } => {
            assert_eq!(channel, "amb");
            match filter {
                vnengine_core::command::ChannelFilter::Filter {
                    mode, cutoff, mix, ..
                } => {
                    assert_eq!(mode, "lowpass");
                    assert!((cutoff - 800.0).abs() < 1e-6);
                    assert!((mix - 0.9).abs() < 1e-6);
                }
                other => panic!("expected Filter, got {other:?}"),
            }
        }
        other => panic!("expected FilterChannel, got {other:?}"),
    }
}

// ---------- Phase 5: lvalues, persistent, achievements ----------

#[test]
fn parse_lvalue_assignments() {
    let src = r##"
label start:
    $ d.k = 1
    $ d["key"] = 2
    $ list[0] += 3
    set persistent.unlocked = True
    end
"##;
    let script = Parser::parse(src).unwrap();
    let b = &script.labels[0].body;
    match &b[0] {
        Stmt::Assign { target, .. } => {
            assert!(matches!(target, Expr::Attr { name, .. } if name == "k"));
        }
        other => panic!("expected Assign, got {other:?}"),
    }
    assert!(matches!(
        &b[1],
        Stmt::Assign {
            target: Expr::Index { .. },
            ..
        }
    ));
    assert!(matches!(
        &b[2],
        Stmt::AssignOp {
            target: Expr::Index { .. },
            ..
        }
    ));
    match &b[3] {
        Stmt::Assign { target, .. } => match target {
            Expr::Attr { object, name } => {
                assert!(matches!(&**object, Expr::Var(n) if n == "persistent"));
                assert_eq!(name, "unlocked");
            }
            other => panic!("expected Attr target, got {other:?}"),
        },
        other => panic!("expected Assign, got {other:?}"),
    }
}

#[test]
fn vm_lvalue_assignments() {
    let src = r##"
label start:
    $ d = { "k": 1 }
    $ d.k += 4
    $ d.new = "hi"
    $ l = [1, 2, 3]
    $ l[1] = 20
    $ l[-1] += 5
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    loop {
        if matches!(vm.step(), StepResult::Finished) {
            break;
        }
    }
    let d = vm.state.variables.get("d").unwrap();
    match d {
        Value::Dict(m) => {
            assert_eq!(m.get("k"), Some(&Value::Int(5)));
            assert_eq!(m.get("new"), Some(&Value::Str("hi".into())));
        }
        other => panic!("expected dict, got {other:?}"),
    }
    match vm.state.variables.get("l").unwrap() {
        Value::List(items) => {
            assert_eq!(items[1], Value::Int(20));
            assert_eq!(items[2], Value::Int(8));
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn parse_achievement_and_grant() {
    let src = r##"
achievement endings "See every ending"
achievement speed

label start:
    grant endings
    grant ["ach_" + "custom"]
    end
"##;
    let script = Parser::parse(src).unwrap();
    assert_eq!(script.achievements.len(), 2);
    assert_eq!(script.achievements[0].name, "endings");
    assert_eq!(
        script.achievements[0].desc.as_deref(),
        Some("See every ending")
    );
    assert!(script.achievements[1].desc.is_none());
    assert!(matches!(&script.labels[0].body[0], Stmt::Grant { .. }));
}

#[test]
fn vm_grant_known_achievement() {
    let src = r##"
achievement win "You won"

label start:
    grant win
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    match step_until_command(&mut vm) {
        Command::Grant { name } => assert_eq!(name, "win"),
        other => panic!("expected Grant, got {other:?}"),
    }
}

#[test]
fn vm_grant_unknown_achievement_errors() {
    let src = r##"
label start:
    grant nope
    end
"##;
    let script = Parser::parse(src).unwrap();
    let mut vm = Vm::new(script);
    // The VM error propagates as a Panic/StepResult error or finished —
    // assert it does not produce a Grant command.
    match vm.step() {
        StepResult::Command(Command::Grant { .. }) => panic!("unknown grant should error"),
        _ => {}
    }
}
