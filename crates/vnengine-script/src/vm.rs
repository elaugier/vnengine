//! Script virtual machine.
//!
//! The VM executes a parsed [`Script`] and produces [`Command`]s for the
//! engine. It is a generator-style VM: the engine calls [`Vm::step`] to get
//! the next command. When a command is *blocking* (e.g. `Say` or `Menu`),
//! the engine must call [`Vm::resume`] with the player's response before
//! calling `step` again.

use crate::ast::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use vnengine_core::command::{Anchor, Command, MenuChoice, SpriteSpec};
use vnengine_core::transition::Transition;
use vnengine_core::Value;

/// A position within the script: which label and which statement index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub label: String,
    pub stmt: usize,
}

/// Serializable VM state for save/load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmState {
    /// Call stack of frames. The top frame is the current execution point.
    pub stack: Vec<Frame>,
    /// Global variables.
    pub variables: HashMap<String, Value>,
    /// Pending menu choices awaiting a selection (index into the menu's
    /// choice list). `None` unless we're blocked on a menu.
    pub pending_menu: Option<Vec<MenuChoiceAst>>,
    /// Whether the VM has finished (reached `End` or fell off the script).
    pub finished: bool,
}

/// A menu choice kept in AST form so it can be serialized and resumed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuChoiceAst {
    pub text: String,
    pub condition: Option<Expr>,
    pub body: Vec<Stmt>,
}

impl VmState {
    pub fn new() -> Self {
        Self {
            stack: vec![Frame {
                label: "start".to_string(),
                stmt: 0,
            }],
            variables: HashMap::new(),
            pending_menu: None,
            finished: false,
        }
    }
}

impl Default for VmState {
    fn default() -> Self {
        Self::new()
    }
}

/// The result of a single VM step.
#[derive(Debug, Clone)]
pub enum StepResult {
    /// The VM produced a command for the engine to execute.
    Command(Command),
    /// The VM is blocked waiting for the player to choose a menu option.
    /// The engine should display the choices and call `resume` with the
    /// selected index.
    Menu {
        prompt: Option<String>,
        choices: Vec<MenuChoice>,
    },
    /// The VM finished executing (game over).
    Finished,
}

pub struct Vm {
    pub script: Script,
    pub state: VmState,
    /// Character display names, populated from `define Character(...)`.
    pub character_names: HashMap<String, String>,
    /// Character name colors.
    pub character_colors: HashMap<String, [f32; 3]>,
    /// Defined constants.
    pub constants: HashMap<String, Value>,
    /// Counter for unique synthetic label names.
    synth_counter: u64,
}

impl Vm {
    pub fn new(script: Script) -> Self {
        let mut vm = Self {
            script,
            state: VmState::new(),
            character_names: HashMap::new(),
            character_colors: HashMap::new(),
            constants: HashMap::new(),
            synth_counter: 0,
        };
        vm.process_definitions();
        vm
    }

    /// Rebuild a VM from a saved state.
    pub fn restore(script: Script, state: VmState) -> Self {
        let mut vm = Self::new(script);
        vm.state = state;
        vm
    }

    pub fn process_definitions(&mut self) {
        for def in &self.script.definitions.clone() {
            match def {
                Definition::Character { id, name, color } => {
                    self.character_names.insert(id.clone(), name.clone());
                    if let Some(c) = color {
                        self.character_colors.insert(id.clone(), *c);
                    }
                }
                Definition::Constant { id, value } => {
                    if let Ok(v) = eval_expr(value, &self.constants) {
                        self.constants.insert(id.clone(), v);
                    }
                }
            }
        }
        // Process top-level `default` statements: only set if the variable
        // doesn't already exist (important for save/load restoration).
        for d in &self.script.defaults.clone() {
            if !self.state.variables.contains_key(&d.name) {
                if let Ok(v) = eval_expr(&d.value, &self.state.variables) {
                    self.state.variables.insert(d.name.clone(), v);
                }
            }
        }
    }

    /// Serialize the current VM state for saving.
    pub fn save_state(&self) -> VmState {
        self.state.clone()
    }

    /// Take the next step. Returns a command or a menu prompt. The engine
    /// should call this repeatedly; when it returns a blocking command or a
    /// menu, the engine performs the action and then calls `resume`.
    pub fn step(&mut self) -> StepResult {
        if self.state.finished {
            return StepResult::Finished;
        }

        // If we have a pending menu, the engine must resume first.
        if self.state.pending_menu.is_some() {
            // This shouldn't happen if the engine drives correctly; return a
            // no-op pause to avoid spinning.
            return StepResult::Command(Command::Pause { seconds: 0.0 });
        }

        loop {
            let frame = match self.state.stack.last_mut() {
                Some(f) => f,
                None => {
                    self.state.finished = true;
                    return StepResult::Finished;
                }
            };

            // Find the current label's body.
            let label_idx = match self.script.find_label_index(&frame.label) {
                Some(i) => i,
                None => {
                    log::error!("label `{}` not found", frame.label);
                    self.state.finished = true;
                    return StepResult::Finished;
                }
            };

            if frame.stmt >= self.script.labels[label_idx].body.len() {
                // End of label: pop the frame (return) or finish.
                self.state.stack.pop();
                if let Some(parent) = self.state.stack.last_mut() {
                    parent.stmt += 1;
                    continue;
                } else {
                    self.state.finished = true;
                    return StepResult::Finished;
                }
            }

            // We need to borrow the statement immutably while potentially
            // mutating the stack. Clone the statement to avoid borrow issues.
            let stmt = self.script.labels[label_idx].body[frame.stmt].clone();
            let stmt_idx = frame.stmt;

            match self.exec_stmt(stmt, stmt_idx) {
                Ok(ExecOutcome::Command(cmd)) => {
                    // Advance past this statement, then return the command.
                    if let Some(f) = self.state.stack.last_mut() {
                        f.stmt += 1;
                    }
                    return StepResult::Command(cmd);
                }
                Ok(ExecOutcome::Menu { prompt, choices }) => {
                    // pending_menu was already set by exec_stmt.
                    return StepResult::Menu { prompt, choices };
                }
                Ok(ExecOutcome::Jump(label)) => {
                    // Replace current frame with the jump target.
                    if let Some(f) = self.state.stack.last_mut() {
                        f.label = label;
                        f.stmt = 0;
                    }
                    continue;
                }
                Ok(ExecOutcome::Call(label)) => {
                    // Push a new frame for the called label.
                    self.state.stack.push(Frame {
                        label,
                        stmt: 0,
                    });
                    continue;
                }
                Ok(ExecOutcome::Return) => {
                    self.state.stack.pop();
                    if let Some(parent) = self.state.stack.last_mut() {
                        parent.stmt += 1;
                        continue;
                    } else {
                        self.state.finished = true;
                        return StepResult::Finished;
                    }
                }
                Ok(ExecOutcome::Continue) => {
                    // Advance to the next statement.
                    if let Some(f) = self.state.stack.last_mut() {
                        f.stmt += 1;
                    }
                    continue;
                }
                Err(e) => {
                    log::error!("script error: {e}");
                    self.state.finished = true;
                    return StepResult::Finished;
                }
            }
        }
    }

    /// Resume execution after a blocking command. For a menu, pass the
    /// selected index in `response`. For other blocking commands, pass
    /// `None`.
    pub fn resume(&mut self, response: Option<MenuResponse>) {
        if let Some(menu) = self.state.pending_menu.take() {
            if let Some(MenuResponse::Selected(idx)) = response {
                if let Some(choice) = menu.get(idx) {
                    // Execute the choice body by pushing it as a temporary
                    // frame. We store the body in a synthetic label.
                    let label_name = format!("__menu_{}", self.state.stack.len());
                    let body = choice.body.clone();
                    self.script.labels.push(Label {
                        name: label_name.clone(),
                        body,
                    });
                    // Advance past the menu statement in the current frame.
                    if let Some(f) = self.state.stack.last_mut() {
                        f.stmt += 1;
                    }
                    self.state.stack.push(Frame {
                        label: label_name,
                        stmt: 0,
                    });
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum MenuResponse {
    Selected(usize),
}

enum ExecOutcome {
    Command(Command),
    Menu {
        prompt: Option<String>,
        choices: Vec<MenuChoice>,
    },
    Jump(String),
    Call(String),
    Return,
    Continue,
}

impl Vm {
    fn exec_stmt(&mut self, stmt: Stmt, _stmt_idx: usize) -> Result<ExecOutcome, String> {
        match stmt {
            Stmt::Scene { image, transition } => {
                let trans = transition
                    .and_then(|t| Transition::parse(&t.name, t.duration))
                    .unwrap_or(Transition::Cut);
                Ok(ExecOutcome::Command(Command::Scene {
                    image,
                    transition: trans,
                }))
            }
            Stmt::Show {
                character,
                image,
                anchor,
                position,
                scale,
                z,
                transition: _,
            } => {
                let anchor = anchor_to_anchor(&anchor);
                let pos = position.and_then(|(x, y)| {
                    let xv = eval_expr(&x, &self.state.variables).ok()?;
                    let yv = eval_expr(&y, &self.state.variables).ok()?;
                    Some((xv.as_float()?, yv.as_float()?))
                });
                let scale = scale
                    .and_then(|e| eval_expr(&e, &self.state.variables).ok())
                    .and_then(|v| v.as_float())
                    .map(|f| f as f32);
                let z = z.and_then(|e| eval_expr(&e, &self.state.variables).ok())
                    .and_then(|v| v.as_int())
                    .map(|i| i as i32);
                Ok(ExecOutcome::Command(Command::Show(SpriteSpec {
                    character,
                    image,
                    anchor,
                    position: pos.map(|(x, y)| (x as f32, y as f32)),
                    scale,
                    z,
                })))
            }
            Stmt::Hide {
                character,
                transition,
            } => {
                let trans = transition
                    .and_then(|t| Transition::parse(&t.name, t.duration))
                    .unwrap_or(Transition::Cut);
                Ok(ExecOutcome::Command(Command::Hide {
                    character,
                    transition: trans,
                }))
            }
            Stmt::Say {
                speaker,
                text,
                voice,
            } => {
                let text = interpolate_text(&text, &self.state.variables);
                if let Some(id) = &speaker {
                    let speaker_name = self.character_names.get(id).cloned();
                    Ok(ExecOutcome::Command(Command::Say {
                        speaker: speaker_name.or(Some(id.clone())),
                        text,
                        voice,
                    }))
                } else {
                    Ok(ExecOutcome::Command(Command::Narrate { text }))
                }
            }
            Stmt::Menu { prompt, choices } => {
                let prompt = prompt.map(|p| interpolate_text(&p, &self.state.variables));
                let mut filtered = Vec::new();
                let mut ast_choices = Vec::new();
                for c in choices {
                    let selectable = match &c.condition {
                        Some(expr) => eval_expr(expr, &self.state.variables)
                            .map(|v| v.is_truthy())
                            .unwrap_or(true),
                        None => true,
                    };
                    if selectable {
                        let interp_text = interpolate_text(&c.text, &self.state.variables);
                        ast_choices.push(MenuChoiceAst {
                            text: interp_text.clone(),
                            condition: None,
                            body: c.body.clone(),
                        });
                        filtered.push(MenuChoice {
                            text: interp_text,
                            condition: None,
                        });
                    }
                }
                if filtered.is_empty() {
                    return Ok(ExecOutcome::Continue);
                }
                // Stash the AST choices for resume.
                self.state.pending_menu = Some(ast_choices);
                Ok(ExecOutcome::Menu {
                    prompt,
                    choices: filtered,
                })
            }
            Stmt::Jump { label } => Ok(ExecOutcome::Jump(label)),
            Stmt::Call { label } => Ok(ExecOutcome::Call(label)),
            Stmt::Return => Ok(ExecOutcome::Return),
            Stmt::If {
                branches,
                else_branch,
            } => {
                for (cond, body) in &branches {
                    if eval_expr(cond, &self.state.variables)
                        .map(|v| v.is_truthy())
                        .unwrap_or(false)
                    {
                        // Execute the branch body by pushing it as a
                        // synthetic label (like a call).
                        let label_name = format!("__if_{}", self.state.stack.len());
                        self.script.labels.push(Label {
                            name: label_name.clone(),
                            body: body.clone(),
                        });
                        return Ok(ExecOutcome::Call(label_name));
                    }
                }
                if let Some(body) = else_branch {
                    let label_name = format!("__else_{}", self.state.stack.len());
                    self.script.labels.push(Label {
                        name: label_name.clone(),
                        body,
                    });
                    return Ok(ExecOutcome::Call(label_name));
                }
                Ok(ExecOutcome::Continue)
            }
            Stmt::Pause { seconds } => {
                let s = eval_expr(&seconds, &self.state.variables)
                    .ok()
                    .and_then(|v| v.as_float())
                    .unwrap_or(1.0) as f32;
                Ok(ExecOutcome::Command(Command::Pause { seconds: s }))
            }
            Stmt::PlayMusic { track, fade } => Ok(ExecOutcome::Command(Command::PlayMusic {
                track,
                fade,
            })),
            Stmt::StopMusic { fade } => Ok(ExecOutcome::Command(Command::StopMusic { fade })),
            Stmt::PlaySfx { track, volume } => {
                Ok(ExecOutcome::Command(Command::PlaySfx { track, volume }))
            }
            Stmt::PlayVoice { track } => Ok(ExecOutcome::Command(Command::PlayVoice { track })),
            Stmt::StopVoice => Ok(ExecOutcome::Command(Command::StopVoice)),
            Stmt::Assign { name, value } => {
                let v = eval_expr(&value, &self.state.variables)
                    .map_err(|e| format!("assignment error: {e}"))?;
                self.state.variables.insert(name, v);
                Ok(ExecOutcome::Continue)
            }
            Stmt::AssignOp { name, op, value } => {
                let current = self
                    .state
                    .variables
                    .get(&name)
                    .cloned()
                    .ok_or_else(|| format!("undefined variable `{name}` for compound assignment"))?;
                let rhs = eval_expr(&value, &self.state.variables)
                    .map_err(|e| format!("assignment error: {e}"))?;
                let result = eval_binop(op, current, rhs)?;
                self.state.variables.insert(name, result);
                Ok(ExecOutcome::Continue)
            }
            Stmt::Default { name, value } => {
                // Only assign if the variable doesn't already exist.
                if !self.state.variables.contains_key(&name) {
                    let v = eval_expr(&value, &self.state.variables)
                        .map_err(|e| format!("default error: {e}"))?;
                    self.state.variables.insert(name, v);
                }
                Ok(ExecOutcome::Continue)
            }
            Stmt::Transform { character, rotation, scale, offset, opacity, layer } => {
                let mut cmd = Command::Transform {
                    character: character.clone(),
                    rotation: None,
                    scale: None,
                    offset: None,
                    opacity: None,
                    layer: None,
                };
                if let Some(r) = rotation {
                    let v = eval_expr(&r, &self.state.variables)
                        .map_err(|e| format!("transform error: {e}"))?;
                    if let Some(f) = v.as_float() {
                        cmd = match cmd {
                            Command::Transform { character, scale, offset, opacity, layer, .. } => {
                                Command::Transform {
                                    character,
                                    rotation: Some(f as f32),
                                    scale,
                                    offset,
                                    opacity,
                                    layer,
                                }
                            }
                            _ => unreachable!(),
                        };
                    }
                }
                if let Some(s) = scale {
                    let v = eval_expr(&s, &self.state.variables)
                        .map_err(|e| format!("transform error: {e}"))?;
                    if let Some(f) = v.as_float() {
                        cmd = match cmd {
                            Command::Transform { character, rotation, offset, opacity, layer, .. } => {
                                Command::Transform {
                                    character,
                                    rotation,
                                    scale: Some(f as f32),
                                    offset,
                                    opacity,
                                    layer,
                                }
                            }
                            _ => unreachable!(),
                        };
                    }
                }
                if let Some(o) = opacity {
                    let v = eval_expr(&o, &self.state.variables)
                        .map_err(|e| format!("transform error: {e}"))?;
                    if let Some(f) = v.as_float() {
                        cmd = match cmd {
                            Command::Transform { character, rotation, scale, offset, layer, .. } => {
                                Command::Transform {
                                    character,
                                    rotation,
                                    scale,
                                    offset,
                                    opacity: Some(f as f32),
                                    layer,
                                }
                            }
                            _ => unreachable!(),
                        };
                    }
                }
                if let Some((x, y)) = offset {
                    let xv = eval_expr(&x, &self.state.variables)
                        .map_err(|e| format!("transform error: {e}"))?;
                    let yv = eval_expr(&y, &self.state.variables)
                        .map_err(|e| format!("transform error: {e}"))?;
                    if let (Some(xf), Some(yf)) = (xv.as_float(), yv.as_float()) {
                        cmd = match cmd {
                            Command::Transform { character, rotation, scale, opacity, layer, .. } => {
                                Command::Transform {
                                    character,
                                    rotation,
                                    scale,
                                    offset: Some((xf as f32, yf as f32)),
                                    opacity,
                                    layer,
                                }
                            }
                            _ => unreachable!(),
                        };
                    }
                }
                if let Some(l) = layer {
                    let layer_val = match l.as_str() {
                        "background" => vnengine_core::Layer::Background,
                        "midground" => vnengine_core::Layer::Midground,
                        "foreground" => vnengine_core::Layer::Foreground,
                        "overlay" => vnengine_core::Layer::Overlay,
                        _ => vnengine_core::Layer::Foreground,
                    };
                    cmd = match cmd {
                        Command::Transform { character, rotation, scale, offset, opacity, .. } => {
                            Command::Transform {
                                character,
                                rotation,
                                scale,
                                offset,
                                opacity,
                                layer: Some(layer_val),
                            }
                        }
                        _ => unreachable!(),
                    };
                }
                Ok(ExecOutcome::Command(cmd))
            }
            Stmt::While { cond, body } => {
                // Check condition. If true, push body as a synthetic label
                // and execute it. When the body finishes, it jumps back to
                // a check label that re-evaluates the While.
                if eval_expr(&cond, &self.state.variables)
                    .map(|v| v.is_truthy())
                    .unwrap_or(false)
                {
                    let id = self.synth_counter;
                    self.synth_counter += 1;
                    let label_name = format!("__while_{}", id);
                    let check_label = format!("__while_check_{}", id);
                    // Check label re-evaluates the while.
                    self.script.labels.push(Label {
                        name: check_label.clone(),
                        body: vec![Stmt::While {
                            cond: cond.clone(),
                            body: body.clone(),
                        }],
                    });
                    // Body label: body + jump back to check.
                    let mut full_body = body.clone();
                    full_body.push(Stmt::Jump {
                        label: check_label,
                    });
                    self.script.labels.push(Label {
                        name: label_name.clone(),
                        body: full_body,
                    });
                    // Use Call outcome so the body frame is pushed without
                    // the step loop incrementing the body's stmt counter.
                    return Ok(ExecOutcome::Call(label_name));
                }
                Ok(ExecOutcome::Continue)
            }
            Stmt::For { var, iterable, body } => {
                // Check if this is a continuation (re-entry from check label).
                // We scan for a continuation marker that was set during first entry.
                let mut cont_id: Option<u64> = None;
                for (key, val) in &self.state.variables {
                    if let Some(rest) = key.strip_prefix("__for_continue_") {
                        if val.is_truthy() {
                            if let Ok(id) = rest.parse::<u64>() {
                                cont_id = Some(id);
                                break;
                            }
                        }
                    }
                }

                if let Some(id) = cont_id {
                    // Continuation: advance the index.
                    let items_key = format!("__for_{}_items", id);
                    let idx_key = format!("__for_{}_idx", id);
                    let cont_key = format!("__for_continue_{}", id);
                    let items = match self.state.variables.get(&items_key) {
                        Some(Value::List(items)) => items.clone(),
                        _ => return Ok(ExecOutcome::Continue),
                    };
                    let idx = match self.state.variables.get(&idx_key) {
                        Some(Value::Int(i)) => *i,
                        _ => 0,
                    };
                    let next_idx = idx + 1;
                    if next_idx >= items.len() as i64 {
                        // Loop done — clean up and continue.
                        self.state.variables.remove(&items_key);
                        self.state.variables.remove(&idx_key);
                        self.state.variables.remove(&cont_key);
                        return Ok(ExecOutcome::Continue);
                    }
                    // Update index and loop variable.
                    self.state
                        .variables
                        .insert(idx_key, Value::Int(next_idx));
                    self.state
                        .variables
                        .insert(var.clone(), items[next_idx as usize].clone());
                    // Push body frame again using Call.
                    let body_id = self.synth_counter;
                    self.synth_counter += 1;
                    let label_name = format!("__for_body_{}", body_id);
                    let current_label = self
                        .state
                        .stack
                        .last()
                        .map(|f| f.label.clone())
                        .unwrap_or_default();
                    let mut full_body = body.clone();
                    full_body.push(Stmt::Jump {
                        label: current_label,
                    });
                    self.script.labels.push(Label {
                        name: label_name.clone(),
                        body: full_body,
                    });
                    return Ok(ExecOutcome::Call(label_name));
                }

                // First entry: evaluate the iterable.
                let iter_val = eval_expr(&iterable, &self.state.variables)
                    .map_err(|e| format!("for loop error: {e}"))?;
                let items: Vec<Value> = match &iter_val {
                    Value::List(items) => items.clone(),
                    _ => return Err("for loop iterable must be a list".into()),
                };
                if items.is_empty() {
                    return Ok(ExecOutcome::Continue);
                }

                let id = self.synth_counter;
                self.synth_counter += 1;
                // Set the loop variable to the first item.
                self.state.variables.insert(var.clone(), items[0].clone());
                // Store items and index.
                self.state
                    .variables
                    .insert(format!("__for_{}_items", id), Value::List(items));
                self.state
                    .variables
                    .insert(format!("__for_{}_idx", id), Value::Int(0));

                // Create check label.
                let check_label = format!("__for_check_{}", id);
                self.script.labels.push(Label {
                    name: check_label.clone(),
                    body: vec![Stmt::For {
                        var: var.clone(),
                        iterable: iterable.clone(),
                        body: body.clone(),
                    }],
                });
                // Mark continuation for the check label.
                self.state
                    .variables
                    .insert(format!("__for_continue_{}", id), Value::Bool(true));

                // Body label: body + jump back to check.
                let body_id = self.synth_counter;
                self.synth_counter += 1;
                let label_name = format!("__for_body_{}", body_id);
                let mut full_body = body.clone();
                full_body.push(Stmt::Jump {
                    label: check_label,
                });
                self.script.labels.push(Label {
                    name: label_name.clone(),
                    body: full_body,
                });
                return Ok(ExecOutcome::Call(label_name));
            }
            Stmt::End => {
                self.state.finished = true;
                Ok(ExecOutcome::Command(Command::End))
            }
        }
    }
}

fn anchor_to_anchor(anchor: &Option<AnchorExpr>) -> Anchor {
    match anchor {
        Some(AnchorExpr::Left) => Anchor::Left,
        Some(AnchorExpr::CenterLeft) => Anchor::CenterLeft,
        Some(AnchorExpr::Center) => Anchor::Center,
        Some(AnchorExpr::CenterRight) => Anchor::CenterRight,
        Some(AnchorExpr::Right) => Anchor::Right,
        Some(AnchorExpr::OffscreenLeft) => Anchor::OffscreenLeft,
        Some(AnchorExpr::OffscreenRight) => Anchor::OffscreenRight,
        Some(AnchorExpr::Expr(_)) => Anchor::Custom,
        None => Anchor::Center,
    }
}

/// Evaluate an expression to a [`Value`].
pub fn eval_expr(expr: &Expr, vars: &HashMap<String, Value>) -> Result<Value, String> {
    Ok(match expr {
        Expr::Var(name) => vars
            .get(name)
            .cloned()
            .ok_or_else(|| format!("undefined variable `{name}`"))?,
        Expr::Str(s) => Value::Str(s.clone()),
        Expr::Int(i) => Value::Int(*i),
        Expr::Float(f) => Value::Float(*f),
        Expr::Bool(b) => Value::Bool(*b),
        Expr::None => Value::None,
        Expr::List(items) => {
            let mut vals = Vec::new();
            for item in items {
                vals.push(eval_expr(item, vars)?);
            }
            Value::List(vals)
        }
        Expr::Index { collection, index } => {
            let coll = eval_expr(collection, vars)?;
            let idx = eval_expr(index, vars)?;
            match (&coll, &idx) {
                (Value::List(items), Value::Int(i)) => {
                    if *i < 0 || *i as usize >= items.len() {
                        return Err(format!("list index out of range: {i}"));
                    }
                    items[*i as usize].clone()
                }
                (Value::Str(s), Value::Int(i)) => {
                    if *i < 0 || *i as usize >= s.len() {
                        return Err(format!("string index out of range: {i}"));
                    }
                    Value::Str(s.chars().nth(*i as usize).unwrap_or_default().to_string())
                }
                _ => return Err("cannot index non-collection".into()),
            }
        }
        Expr::UnaryOp { op, operand } => {
            let v = eval_expr(operand, vars)?;
            match op {
                UnaryOp::Neg => match v {
                    Value::Int(i) => Value::Int(-i),
                    Value::Float(f) => Value::Float(-f),
                    _ => return Err("cannot negate non-number".into()),
                },
                UnaryOp::Not => Value::Bool(!v.is_truthy()),
            }
        }
        Expr::BinaryOp { op, lhs, rhs } => {
            let l = eval_expr(lhs, vars)?;
            let r = eval_expr(rhs, vars)?;
            eval_binop(*op, l, r)?
        }
    })
}

fn eval_binop(op: BinaryOp, l: Value, r: Value) -> Result<Value, String> {
    use BinaryOp::*;
    Ok(match op {
        Add => match (l, r) {
            (Value::Int(a), Value::Int(b)) => Value::Int(a + b),
            (Value::Float(a), Value::Float(b)) => Value::Float(a + b),
            (Value::Float(a), Value::Int(b)) => Value::Float(a + b as f64),
            (Value::Int(a), Value::Float(b)) => Value::Float(a as f64 + b),
            (Value::Str(a), Value::Str(b)) => Value::Str(a + &b),
            (Value::Str(a), other) => Value::Str(a + &other.as_str().unwrap_or_default()),
            (other, Value::Str(b)) => Value::Str(other.as_str().unwrap_or_default() + &b),
            (Value::List(a), Value::List(b)) => {
                let mut combined = a;
                combined.extend(b);
                Value::List(combined)
            }
            _ => return Err("invalid operands for `+`".into()),
        },
        Sub => num_op(l, r, |a, b| a - b, |a, b| a - b)?,
        Mul => num_op(l, r, |a, b| a * b, |a, b| a * b)?,
        Div => num_op(l, r, |a, b| a / b, |a, b| a / b)?,
        Mod => num_op(l, r, |a, b| a % b, |a, b| a % b)?,
        Eq => Value::Bool(values_eq(&l, &r)),
        Neq => Value::Bool(!values_eq(&l, &r)),
        Lt => Value::Bool(cmp(&l, &r)? == std::cmp::Ordering::Less),
        Gt => Value::Bool(cmp(&l, &r)? == std::cmp::Ordering::Greater),
        Le => Value::Bool(cmp(&l, &r)? != std::cmp::Ordering::Greater),
        Ge => Value::Bool(cmp(&l, &r)? != std::cmp::Ordering::Less),
        And => Value::Bool(l.is_truthy() && r.is_truthy()),
        Or => Value::Bool(l.is_truthy() || r.is_truthy()),
    })
}

fn num_op(
    l: Value,
    r: Value,
    ifn: fn(i64, i64) -> i64,
    iff: fn(f64, f64) -> f64,
) -> Result<Value, String> {
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(ifn(a, b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(iff(a, b))),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(iff(a, b as f64))),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(iff(a as f64, b))),
        _ => Err("invalid numeric operands".into()),
    }
}

fn values_eq(l: &Value, r: &Value) -> bool {
    match (l, r) {
        (Value::None, Value::None) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
        (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
        _ => false,
    }
}

fn cmp(l: &Value, r: &Value) -> Result<std::cmp::Ordering, String> {
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(a.cmp(b)),
        (Value::Float(a), Value::Float(b)) => Ok(a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)),
        (Value::Int(a), Value::Float(b)) => Ok((*a as f64).partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)),
        (Value::Float(a), Value::Int(b)) => Ok(a.partial_cmp(&(*b as f64)).unwrap_or(std::cmp::Ordering::Equal)),
        (Value::Str(a), Value::Str(b)) => Ok(a.cmp(b)),
        _ => Err("cannot compare these values".into()),
    }
}

/// Replace `[var_name]` references in a text string with variable values.
/// `[[` produces a literal `[` and `]]` produces a literal `]`.
pub fn interpolate_text(text: &str, vars: &HashMap<String, Value>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' {
            // Escaped: [[ → [
            if i + 1 < chars.len() && chars[i + 1] == '[' {
                out.push('[');
                i += 2;
                continue;
            }
            // Find closing ]
            if let Some(close) = chars[i + 1..].iter().position(|&c| c == ']') {
                let name: String = chars[i + 1..i + 1 + close].iter().collect();
                let name = name.trim();
                if let Some(val) = vars.get(name) {
                    out.push_str(&val.to_string());
                } else {
                    // Unknown variable: leave as-is
                    out.push('[');
                    out.push_str(name);
                    out.push(']');
                }
                i += 1 + close + 1;
            } else {
                // No closing bracket: literal
                out.push('[');
                i += 1;
            }
        } else if chars[i] == ']' {
            // Escaped: ]] → ]
            if i + 1 < chars.len() && chars[i + 1] == ']' {
                out.push(']');
                i += 2;
                continue;
            }
            out.push(']');
            i += 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}
