//! Pure-function interpreter for `def` bodies called from expressions.
//!
//! Functions invoked via `f(args)` in an expression (or `call f(args)`)
//! run here: statements execute against a local scope seeded with the
//! caller's variables plus bound parameters. Only side-effect-free
//! statements are allowed — anything that would emit a [`Command`]
//! (dialogue, scene changes, audio, menus, jumps...) is rejected with a
//! clear error. Zero-argument functions called via `call name` keep the
//! frame-based path in [`Vm`](crate::vm::Vm) and may contain blocking
//! statements.

use crate::ast::{Expr, FunctionDef, Stmt};
use crate::vm::{eval_binop_pub, eval_expr_ctx};
use std::collections::HashMap;
use vnengine_core::Value;

/// How a function-body statement list terminated.
enum FnFlow {
    /// Ran off the end without `return`.
    Normal,
    /// `break` reached the function boundary (invalid).
    Break,
    /// `continue` reached the function boundary (invalid).
    Continue,
    /// `return [expr]` — carries the function's result value.
    Return(Value),
}

/// Maximum statements a function body may execute per call — guards
/// against `while True:` inside a function hanging the VM.
const MAX_FN_STEPS: usize = 100_000;

/// Evaluate a call to user function `func`: bind `arg_exprs` to
/// `func.params` in a scope layered over `caller_vars`, then interpret
/// the body. Returns the `return` value (`Value::None` when the body
/// falls off the end).
pub fn call_user_function(
    func: &FunctionDef,
    arg_exprs: &[Expr],
    caller_vars: &HashMap<String, Value>,
    fns: &HashMap<String, FunctionDef>,
    rng: &mut u64,
) -> Result<Value, String> {
    if arg_exprs.len() != func.params.len() {
        return Err(format!(
            "function `{}` expects {} argument(s), got {}",
            func.name,
            func.params.len(),
            arg_exprs.len()
        ));
    }
    // The scope starts as a copy of the caller's variables so functions
    // can read globals; writes stay local to the call.
    let mut scope = caller_vars.clone();
    for (param, arg) in func.params.iter().zip(arg_exprs.iter()) {
        let v = eval_expr_ctx(arg, caller_vars, Some(fns), rng)
            .map_err(|e| format!("function `{}` argument error: {e}", func.name))?;
        scope.insert(param.clone(), v);
    }
    let mut budget = MAX_FN_STEPS;
    match exec_block(&func.body, &mut scope, fns, rng, &mut budget)? {
        FnFlow::Return(v) => Ok(v),
        FnFlow::Normal => Ok(Value::None),
        FnFlow::Break => Err(format!("function `{}`: `break` outside a loop", func.name)),
        FnFlow::Continue => Err(format!(
            "function `{}`: `continue` outside a loop",
            func.name
        )),
    }
}

/// Execute a statement list, propagating `break`/`continue`/`return`.
fn exec_block(
    stmts: &[Stmt],
    scope: &mut HashMap<String, Value>,
    fns: &HashMap<String, FunctionDef>,
    rng: &mut u64,
    budget: &mut usize,
) -> Result<FnFlow, String> {
    for stmt in stmts {
        *budget -= 1;
        if *budget == 0 {
            return Err("function step budget exceeded (possible infinite loop)".into());
        }
        match exec_stmt(stmt, scope, fns, rng, budget)? {
            FnFlow::Normal => {}
            other => return Ok(other),
        }
    }
    Ok(FnFlow::Normal)
}

fn exec_stmt(
    stmt: &Stmt,
    scope: &mut HashMap<String, Value>,
    fns: &HashMap<String, FunctionDef>,
    rng: &mut u64,
    budget: &mut usize,
) -> Result<FnFlow, String> {
    match stmt {
        Stmt::Assign { name, value } => {
            let v = eval_expr_ctx(value, scope, Some(fns), rng)?;
            scope.insert(name.clone(), v);
            Ok(FnFlow::Normal)
        }
        Stmt::AssignOp { name, op, value } => {
            let current = scope
                .get(name)
                .cloned()
                .ok_or_else(|| format!("undefined variable `{name}` for compound assignment"))?;
            let rhs = eval_expr_ctx(value, scope, Some(fns), rng)?;
            let result = eval_binop_pub(*op, current, rhs)?;
            scope.insert(name.clone(), result);
            Ok(FnFlow::Normal)
        }
        Stmt::Default { name, value } => {
            if !scope.contains_key(name) {
                let v = eval_expr_ctx(value, scope, Some(fns), rng)?;
                scope.insert(name.clone(), v);
            }
            Ok(FnFlow::Normal)
        }
        Stmt::If {
            branches,
            else_branch,
        } => {
            for (cond, body) in branches {
                if eval_expr_ctx(cond, scope, Some(fns), rng)?.is_truthy() {
                    return exec_block(body, scope, fns, rng, budget);
                }
            }
            if let Some(body) = else_branch {
                return exec_block(body, scope, fns, rng, budget);
            }
            Ok(FnFlow::Normal)
        }
        Stmt::While { cond, body } => {
            while eval_expr_ctx(cond, scope, Some(fns), rng)?.is_truthy() {
                match exec_block(body, scope, fns, rng, budget)? {
                    FnFlow::Normal | FnFlow::Continue => {}
                    FnFlow::Break => return Ok(FnFlow::Normal),
                    FnFlow::Return(v) => return Ok(FnFlow::Return(v)),
                }
            }
            Ok(FnFlow::Normal)
        }
        Stmt::For {
            var,
            iterable,
            body,
        } => {
            let iter_val = eval_expr_ctx(iterable, scope, Some(fns), rng)?;
            let items = iterable_items(&iter_val)?;
            for item in items {
                scope.insert(var.clone(), item);
                match exec_block(body, scope, fns, rng, budget)? {
                    FnFlow::Normal | FnFlow::Continue => {}
                    FnFlow::Break => return Ok(FnFlow::Normal),
                    FnFlow::Return(v) => return Ok(FnFlow::Return(v)),
                }
            }
            Ok(FnFlow::Normal)
        }
        Stmt::Break => Ok(FnFlow::Break),
        Stmt::Continue => Ok(FnFlow::Continue),
        Stmt::Return => Ok(FnFlow::Return(Value::None)),
        Stmt::ReturnValue { value } => {
            let v = eval_expr_ctx(value, scope, Some(fns), rng)?;
            Ok(FnFlow::Return(v))
        }
        Stmt::Def { .. } => Ok(FnFlow::Normal),
        other => Err(format!(
            "statement `{}` is not allowed inside a function called from an expression",
            stmt_kind(other)
        )),
    }
}

/// Materialize an iterable into items: lists, dict keys (as strings)
/// and strings (as chars).
fn iterable_items(v: &Value) -> Result<Vec<Value>, String> {
    match v {
        Value::List(items) => Ok(items.clone()),
        Value::Dict(map) => Ok(map.keys().cloned().map(Value::Str).collect()),
        Value::Str(s) => Ok(s.chars().map(|c| Value::Str(c.to_string())).collect()),
        _ => Err("for loop iterable must be a list, dict or string".into()),
    }
}

fn stmt_kind(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::Scene { .. } => "scene",
        Stmt::Show { .. } => "show",
        Stmt::Hide { .. } => "hide",
        Stmt::Say { .. } => "say",
        Stmt::Menu { .. } => "menu",
        Stmt::Jump { .. } => "jump",
        Stmt::Call { .. } => "call",
        Stmt::Return => "return",
        Stmt::ReturnValue { .. } => "return",
        Stmt::With { .. } => "with",
        Stmt::Break => "break",
        Stmt::Continue => "continue",
        Stmt::If { .. } => "if",
        Stmt::Pause { .. } => "pause",
        Stmt::PlayMusic { .. } => "play music",
        Stmt::StopMusic { .. } => "stop music",
        Stmt::PlaySfx { .. } => "play sfx",
        Stmt::PlayVoice { .. } => "voice",
        Stmt::StopVoice => "stop voice",
        Stmt::PlayChannel { .. } => "play channel",
        Stmt::QueueChannel { .. } => "queue channel",
        Stmt::StopChannel { .. } => "stop channel",
        Stmt::StopAll { .. } => "stop all",
        Stmt::VolumeChannel { .. } => "volume channel",
        Stmt::FilterChannel { .. } => "filter channel",
        Stmt::Assign { .. } | Stmt::AssignOp { .. } | Stmt::Default { .. } => "assignment",
        Stmt::While { .. } => "while",
        Stmt::For { .. } => "for",
        Stmt::Transform { .. } => "transform",
        Stmt::Light { .. } => "light",
        Stmt::Unlight { .. } => "unlight",
        Stmt::Ambient { .. } => "ambient",
        Stmt::MusicLayer { .. } => "music layer",
        Stmt::Camera { .. } => "camera",
        Stmt::Def { .. } => "def",
        Stmt::Shake { .. } => "shake",
        Stmt::Plugin { .. } => "plugin",
        Stmt::ShowScreen { .. } => "show screen",
        Stmt::HideScreen { .. } => "hide screen",
        Stmt::CallScreen { .. } => "call screen",
        Stmt::Input { .. } => "input",
        Stmt::Confirm { .. } => "confirm",
        Stmt::NvlClear => "nvl clear",
        Stmt::Window { .. } => "window",
        Stmt::End => "end",
    }
}
