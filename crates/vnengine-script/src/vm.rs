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

/// What a frame executes: a named label in the script, or an inline
/// statement block pushed by control flow (menu choice, `if` branch, loop
/// body, function call). Inline bodies are serialized with the frame so a
/// saved game can be restored mid-block without mutating the script.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FrameBody {
    /// Statements of `script.labels` matching this name.
    Label(String),
    /// A statement block owned by the frame itself.
    Inline(Vec<Stmt>),
}

/// Iteration state owned by a `for` loop body frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForLoop {
    /// Loop variable name.
    pub var: String,
    /// Evaluated items to iterate over.
    pub items: Vec<Value>,
    /// Index of the item currently bound to `var`.
    pub index: usize,
}

/// A position within the script: what to execute and where.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    /// The statement source for this frame.
    pub body: FrameBody,
    /// Index of the next statement to execute within `body`.
    pub stmt: usize,
    /// True when this frame was pushed by `call` (label or function).
    /// `return` unwinds through non-call frames until it has popped one
    /// call frame.
    pub is_call: bool,
    /// When set, reaching the end of the body advances the iteration
    /// instead of popping the frame.
    pub for_loop: Option<ForLoop>,
    /// When set, reaching the end of the body re-evaluates this condition
    /// and restarts the body while it stays true.
    pub while_loop: Option<Expr>,
}

impl Frame {
    fn label(name: impl Into<String>) -> Self {
        Self {
            body: FrameBody::Label(name.into()),
            stmt: 0,
            is_call: false,
            for_loop: None,
            while_loop: None,
        }
    }

    fn call(name: impl Into<String>) -> Self {
        Self {
            is_call: true,
            ..Self::label(name)
        }
    }

    fn block(body: Vec<Stmt>) -> Self {
        Self {
            body: FrameBody::Inline(body),
            stmt: 0,
            is_call: false,
            for_loop: None,
            while_loop: None,
        }
    }

    /// Borrow the statement list this frame executes, resolving labels
    /// against the script. `None` if the label does not exist.
    fn body_stmts<'a>(&'a self, script: &'a Script) -> Option<&'a [Stmt]> {
        match &self.body {
            FrameBody::Inline(stmts) => Some(stmts),
            FrameBody::Label(name) => script
                .find_label_index(name)
                .map(|i| script.labels[i].body.as_slice()),
        }
    }
}

/// Serializable VM state for save/load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmState {
    /// Call stack of frames. The top frame is the current execution point.
    pub stack: Vec<Frame>,
    /// Global variables.
    pub variables: HashMap<String, Value>,
    /// Pending menu awaiting a selection. `None` unless we're blocked on
    /// a menu.
    pub pending_menu: Option<PendingMenu>,
    /// PRNG state for `random()`/`randint()`/`srand()`. Serialized so
    /// save/load and replay reproduce the same sequence.
    #[serde(default = "default_rng_seed")]
    pub rng_seed: u64,
    /// Whether the VM has finished (reached `End` or fell off the script).
    pub finished: bool,
}

fn default_rng_seed() -> u64 {
    0x9E37_79B9_7F4A_7C15
}

/// A menu presented to the player, kept in serializable form so the same
/// menu can be rebuilt after a save/restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMenu {
    pub prompt: Option<String>,
    pub choices: Vec<MenuChoiceAst>,
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
            stack: vec![Frame::label("start")],
            variables: HashMap::new(),
            pending_menu: None,
            rng_seed: default_rng_seed(),
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

/// Maximum number of internal steps `Vm::step` may take before giving up.
/// Prevents pathological scripts (e.g. an empty `while True:`) from
/// hanging the game loop.
const MAX_STEPS_PER_CALL: usize = 100_000;

pub struct Vm {
    pub script: Script,
    pub state: VmState,
    /// Character display names, populated from `define Character(...)`.
    pub character_names: HashMap<String, String>,
    /// Character name colors.
    pub character_colors: HashMap<String, [f32; 3]>,
    /// Defined constants.
    pub constants: HashMap<String, Value>,
    /// User-defined functions, keyed by name.
    pub functions: HashMap<String, FunctionDef>,
}

impl Vm {
    pub fn new(script: Script) -> Self {
        let functions: HashMap<String, FunctionDef> = script
            .functions
            .iter()
            .map(|f| (f.name.clone(), f.clone()))
            .collect();
        let mut vm = Self {
            script,
            state: VmState::new(),
            character_names: HashMap::new(),
            character_colors: HashMap::new(),
            constants: HashMap::new(),
            functions,
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
                if let Ok(v) =
                    eval_expr_rng(&d.value, &self.state.variables, &mut self.state.rng_seed)
                {
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

        let mut budget = MAX_STEPS_PER_CALL;
        loop {
            budget -= 1;
            if budget == 0 {
                log::error!("script error: step budget exceeded (possible infinite loop)");
                self.state.finished = true;
                return StepResult::Finished;
            }

            // Resolve the current frame's statement list.
            let body_len = {
                let Some(frame) = self.state.stack.last() else {
                    self.state.finished = true;
                    return StepResult::Finished;
                };
                match frame.body_stmts(&self.script) {
                    Some(stmts) => stmts.len(),
                    None => {
                        if let FrameBody::Label(name) = &frame.body {
                            log::error!("label `{name}` not found");
                        }
                        self.state.finished = true;
                        return StepResult::Finished;
                    }
                }
            };

            let frame = self.state.stack.last_mut().unwrap();
            if frame.stmt >= body_len {
                // End of this frame's body. Loop frames re-enter instead of
                // popping.
                if let Some(for_loop) = &mut frame.for_loop {
                    for_loop.index += 1;
                    if for_loop.index < for_loop.items.len() {
                        let value = for_loop.items[for_loop.index].clone();
                        self.state.variables.insert(for_loop.var.clone(), value);
                        frame.stmt = 0;
                        continue;
                    }
                } else if let Some(cond) = &frame.while_loop {
                    if eval_expr_rng(cond, &self.state.variables, &mut self.state.rng_seed)
                        .map(|v| v.is_truthy())
                        .unwrap_or(false)
                    {
                        frame.stmt = 0;
                        continue;
                    }
                }
                // Pop the frame and advance the caller past the statement
                // that pushed it.
                self.state.stack.pop();
                match self.state.stack.last_mut() {
                    Some(parent) => {
                        parent.stmt += 1;
                        continue;
                    }
                    None => {
                        self.state.finished = true;
                        return StepResult::Finished;
                    }
                }
            }
            // We need to borrow the statement immutably while potentially
            // mutating the stack. Clone the statement to avoid borrow issues.
            let stmt = {
                let frame = self.state.stack.last().unwrap();
                frame.body_stmts(&self.script).unwrap()[frame.stmt].clone()
            };

            match self.exec_stmt(stmt) {
                Ok(ExecOutcome::Command(cmd)) => {
                    // Advance past this statement, then return the command.
                    if let Some(f) = self.state.stack.last_mut() {
                        f.stmt += 1;
                    }
                    return StepResult::Command(cmd);
                }
                Ok(ExecOutcome::Menu { prompt, choices }) => {
                    // pending_menu was already set by exec_stmt. The frame
                    // still points at the menu statement; it advances when
                    // the choice body completes.
                    return StepResult::Menu { prompt, choices };
                }
                Ok(ExecOutcome::Jump(label)) => {
                    // `jump` abandons the enclosing block context: unwind
                    // inline (non-call) frames, then redirect the nearest
                    // call frame — or the bottom label frame — to the jump
                    // target. Statements after the enclosing block are not
                    // executed, matching Ren'Py semantics.
                    while self.state.stack.len() > 1
                        && !self.state.stack.last().map(|f| f.is_call).unwrap_or(false)
                    {
                        self.state.stack.pop();
                    }
                    if let Some(f) = self.state.stack.last_mut() {
                        f.body = FrameBody::Label(label);
                        f.stmt = 0;
                        f.for_loop = None;
                        f.while_loop = None;
                    }
                    continue;
                }
                Ok(ExecOutcome::Call(label)) => {
                    // Push a new frame for the called label.
                    self.state.stack.push(Frame::call(label));
                    continue;
                }
                Ok(ExecOutcome::Push(frame)) => {
                    self.state.stack.push(frame);
                    continue;
                }
                Ok(ExecOutcome::Return) => {
                    // Unwind through control-flow frames until a call frame
                    // has been popped. `return` outside any `call` ends the
                    // script.
                    while let Some(f) = self.state.stack.pop() {
                        if f.is_call {
                            break;
                        }
                    }
                    match self.state.stack.last_mut() {
                        Some(parent) => {
                            parent.stmt += 1;
                            continue;
                        }
                        None => {
                            self.state.finished = true;
                            return StepResult::Finished;
                        }
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
                if let Some(choice) = menu.choices.get(idx) {
                    // Push the choice body as an inline frame. The parent
                    // frame still points at the menu statement and advances
                    // when the body completes.
                    self.state.stack.push(Frame::block(choice.body.clone()));
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
    /// Push a frame (control-flow block or function body) without advancing
    /// the parent frame's statement index.
    Push(Frame),
    Return,
    Continue,
}

impl Vm {
    fn exec_stmt(&mut self, stmt: Stmt) -> Result<ExecOutcome, String> {
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
                    let xv =
                        eval_expr_rng(&x, &self.state.variables, &mut self.state.rng_seed).ok()?;
                    let yv =
                        eval_expr_rng(&y, &self.state.variables, &mut self.state.rng_seed).ok()?;
                    Some((xv.as_float()?, yv.as_float()?))
                });
                let scale = scale
                    .and_then(|e| {
                        eval_expr_rng(&e, &self.state.variables, &mut self.state.rng_seed).ok()
                    })
                    .and_then(|v| v.as_float())
                    .map(|f| f as f32);
                let z = z
                    .and_then(|e| {
                        eval_expr_rng(&e, &self.state.variables, &mut self.state.rng_seed).ok()
                    })
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
                side_image,
            } => {
                let text = interpolate_text(&text, &self.state.variables);
                if let Some(id) = &speaker {
                    let speaker_name = self.character_names.get(id).cloned();
                    Ok(ExecOutcome::Command(Command::Say {
                        speaker: speaker_name.or(Some(id.clone())),
                        text,
                        voice,
                        side_image,
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
                        Some(expr) => {
                            eval_expr_rng(expr, &self.state.variables, &mut self.state.rng_seed)
                                .map(|v| v.is_truthy())
                                .unwrap_or(true)
                        }
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
                self.state.pending_menu = Some(PendingMenu {
                    prompt: prompt.clone(),
                    choices: ast_choices,
                });
                Ok(ExecOutcome::Menu {
                    prompt,
                    choices: filtered,
                })
            }
            Stmt::Jump { label } => Ok(ExecOutcome::Jump(label)),
            Stmt::Call { label } => {
                // Check if this is a user-defined function.
                if let Some(func) = self.functions.get(&label).cloned() {
                    let mut frame = Frame::block(func.body);
                    frame.is_call = true;
                    return Ok(ExecOutcome::Push(frame));
                }
                Ok(ExecOutcome::Call(label))
            }
            Stmt::Shake {
                intensity,
                duration,
            } => {
                let i = eval_expr_rng(&intensity, &self.state.variables, &mut self.state.rng_seed)
                    .map_err(|e| format!("shake error: {e}"))?;
                let d = eval_expr_rng(&duration, &self.state.variables, &mut self.state.rng_seed)
                    .map_err(|e| format!("shake error: {e}"))?;
                let intensity = i.as_float().unwrap_or(0.5) as f32;
                let duration = d.as_float().unwrap_or(0.5) as f32;
                Ok(ExecOutcome::Command(Command::Shake {
                    intensity,
                    duration,
                }))
            }
            Stmt::Def { .. } => {
                // Function definitions are processed at load time, not executed.
                Ok(ExecOutcome::Continue)
            }
            Stmt::Return => Ok(ExecOutcome::Return),
            Stmt::If {
                branches,
                else_branch,
            } => {
                for (cond, body) in &branches {
                    if eval_expr_rng(cond, &self.state.variables, &mut self.state.rng_seed)
                        .map(|v| v.is_truthy())
                        .unwrap_or(false)
                    {
                        return Ok(ExecOutcome::Push(Frame::block(body.clone())));
                    }
                }
                if let Some(body) = else_branch {
                    return Ok(ExecOutcome::Push(Frame::block(body)));
                }
                Ok(ExecOutcome::Continue)
            }
            Stmt::Pause { seconds } => {
                let s = eval_expr_rng(&seconds, &self.state.variables, &mut self.state.rng_seed)
                    .ok()
                    .and_then(|v| v.as_float())
                    .unwrap_or(1.0) as f32;
                Ok(ExecOutcome::Command(Command::Pause { seconds: s }))
            }
            Stmt::PlayMusic { track, fade } => {
                Ok(ExecOutcome::Command(Command::PlayMusic { track, fade }))
            }
            Stmt::StopMusic { fade } => Ok(ExecOutcome::Command(Command::StopMusic { fade })),
            Stmt::PlaySfx { track, volume } => {
                Ok(ExecOutcome::Command(Command::PlaySfx { track, volume }))
            }
            Stmt::PlayVoice { track } => Ok(ExecOutcome::Command(Command::PlayVoice { track })),
            Stmt::StopVoice => Ok(ExecOutcome::Command(Command::StopVoice)),
            Stmt::Assign { name, value } => {
                let v = eval_expr_rng(&value, &self.state.variables, &mut self.state.rng_seed)
                    .map_err(|e| format!("assignment error: {e}"))?;
                self.state.variables.insert(name, v);
                Ok(ExecOutcome::Continue)
            }
            Stmt::AssignOp { name, op, value } => {
                let current = self.state.variables.get(&name).cloned().ok_or_else(|| {
                    format!("undefined variable `{name}` for compound assignment")
                })?;
                let rhs = eval_expr_rng(&value, &self.state.variables, &mut self.state.rng_seed)
                    .map_err(|e| format!("assignment error: {e}"))?;
                let result = eval_binop(op, current, rhs)?;
                self.state.variables.insert(name, result);
                Ok(ExecOutcome::Continue)
            }
            Stmt::Default { name, value } => {
                // Only assign if the variable doesn't already exist.
                if !self.state.variables.contains_key(&name) {
                    let v = eval_expr_rng(&value, &self.state.variables, &mut self.state.rng_seed)
                        .map_err(|e| format!("default error: {e}"))?;
                    self.state.variables.insert(name, v);
                }
                Ok(ExecOutcome::Continue)
            }
            Stmt::Transform {
                character,
                rotation,
                scale,
                offset,
                opacity,
                layer,
            } => {
                let mut cmd = Command::Transform {
                    character: character.clone(),
                    rotation: None,
                    scale: None,
                    offset: None,
                    opacity: None,
                    layer: None,
                };
                if let Some(r) = rotation {
                    let v = eval_expr_rng(&r, &self.state.variables, &mut self.state.rng_seed)
                        .map_err(|e| format!("transform error: {e}"))?;
                    if let Some(f) = v.as_float() {
                        cmd = match cmd {
                            Command::Transform {
                                character,
                                scale,
                                offset,
                                opacity,
                                layer,
                                ..
                            } => Command::Transform {
                                character,
                                rotation: Some(f as f32),
                                scale,
                                offset,
                                opacity,
                                layer,
                            },
                            _ => unreachable!(),
                        };
                    }
                }
                if let Some(s) = scale {
                    let v = eval_expr_rng(&s, &self.state.variables, &mut self.state.rng_seed)
                        .map_err(|e| format!("transform error: {e}"))?;
                    if let Some(f) = v.as_float() {
                        cmd = match cmd {
                            Command::Transform {
                                character,
                                rotation,
                                offset,
                                opacity,
                                layer,
                                ..
                            } => Command::Transform {
                                character,
                                rotation,
                                scale: Some(f as f32),
                                offset,
                                opacity,
                                layer,
                            },
                            _ => unreachable!(),
                        };
                    }
                }
                if let Some(o) = opacity {
                    let v = eval_expr_rng(&o, &self.state.variables, &mut self.state.rng_seed)
                        .map_err(|e| format!("transform error: {e}"))?;
                    if let Some(f) = v.as_float() {
                        cmd = match cmd {
                            Command::Transform {
                                character,
                                rotation,
                                scale,
                                offset,
                                layer,
                                ..
                            } => Command::Transform {
                                character,
                                rotation,
                                scale,
                                offset,
                                opacity: Some(f as f32),
                                layer,
                            },
                            _ => unreachable!(),
                        };
                    }
                }
                if let Some((x, y)) = offset {
                    let xv = eval_expr_rng(&x, &self.state.variables, &mut self.state.rng_seed)
                        .map_err(|e| format!("transform error: {e}"))?;
                    let yv = eval_expr_rng(&y, &self.state.variables, &mut self.state.rng_seed)
                        .map_err(|e| format!("transform error: {e}"))?;
                    if let (Some(xf), Some(yf)) = (xv.as_float(), yv.as_float()) {
                        cmd = match cmd {
                            Command::Transform {
                                character,
                                rotation,
                                scale,
                                opacity,
                                layer,
                                ..
                            } => Command::Transform {
                                character,
                                rotation,
                                scale,
                                offset: Some((xf as f32, yf as f32)),
                                opacity,
                                layer,
                            },
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
                        Command::Transform {
                            character,
                            rotation,
                            scale,
                            offset,
                            opacity,
                            ..
                        } => Command::Transform {
                            character,
                            rotation,
                            scale,
                            offset,
                            opacity,
                            layer: Some(layer_val),
                        },
                        _ => unreachable!(),
                    };
                }
                Ok(ExecOutcome::Command(cmd))
            }
            Stmt::While { cond, body } => {
                if eval_expr(&cond, &self.state.variables)
                    .map(|v| v.is_truthy())
                    .unwrap_or(false)
                {
                    let mut frame = Frame::block(body);
                    frame.while_loop = Some(cond);
                    return Ok(ExecOutcome::Push(frame));
                }
                Ok(ExecOutcome::Continue)
            }
            Stmt::For {
                var,
                iterable,
                body,
            } => {
                let iter_val =
                    eval_expr_rng(&iterable, &self.state.variables, &mut self.state.rng_seed)
                        .map_err(|e| format!("for loop error: {e}"))?;
                let items: Vec<Value> = match &iter_val {
                    Value::List(items) => items.clone(),
                    _ => return Err("for loop iterable must be a list".into()),
                };
                if items.is_empty() {
                    return Ok(ExecOutcome::Continue);
                }
                // Bind the loop variable to the first item; the frame owns
                // the iteration state and re-enters the body until the
                // items are exhausted.
                self.state.variables.insert(var.clone(), items[0].clone());
                let mut frame = Frame::block(body);
                frame.for_loop = Some(ForLoop {
                    var,
                    items,
                    index: 0,
                });
                Ok(ExecOutcome::Push(frame))
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

/// Evaluate an expression to a [`Value`], without a mutable RNG state.
/// Calls to `random()`/`randint()` consume a throwaway stream seeded from
/// `default_rng_seed()`, keeping the evaluation deterministic.
pub fn eval_expr(expr: &Expr, vars: &HashMap<String, Value>) -> Result<Value, String> {
    let mut rng = default_rng_seed();
    eval_expr_rng(expr, vars, &mut rng)
}

/// Evaluate an expression to a [`Value`], drawing randomness from `rng`.
pub fn eval_expr_rng(
    expr: &Expr,
    vars: &HashMap<String, Value>,
    rng: &mut u64,
) -> Result<Value, String> {
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
                vals.push(eval_expr_rng(item, vars, rng)?);
            }
            Value::List(vals)
        }
        Expr::Index { collection, index } => {
            let coll = eval_expr_rng(collection, vars, rng)?;
            let idx = eval_expr_rng(index, vars, rng)?;
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
        Expr::Call { name, args } => eval_builtin(name, args, vars, rng)?,
        Expr::UnaryOp { op, operand } => {
            let v = eval_expr_rng(operand, vars, rng)?;
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
            let l = eval_expr_rng(lhs, vars, rng)?;
            let r = eval_expr_rng(rhs, vars, rng)?;
            eval_binop(*op, l, r)?
        }
    })
}

/// Builtin expression functions. `random()` returns a float in [0, 1),
/// `randint(lo, hi)` returns an integer in [lo, hi], and `srand(seed)`
/// reseeds the PRNG and returns the seed.
fn eval_builtin(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Value>,
    rng: &mut u64,
) -> Result<Value, String> {
    match name {
        "random" => {
            if !args.is_empty() {
                return Err("random() takes no arguments".into());
            }
            Ok(Value::Float(next_f64(rng)))
        }
        "randint" => {
            if args.len() != 2 {
                return Err("randint() takes two arguments".into());
            }
            let lo = eval_expr_rng(&args[0], vars, rng)?
                .as_int()
                .ok_or("randint() bounds must be integers")?;
            let hi = eval_expr_rng(&args[1], vars, rng)?
                .as_int()
                .ok_or("randint() bounds must be integers")?;
            if lo > hi {
                return Err(format!("randint() invalid range {lo}..{hi}"));
            }
            let span = (hi - lo + 1) as u64;
            Ok(Value::Int(lo + (next_u64(rng) % span) as i64))
        }
        "srand" => {
            if args.len() != 1 {
                return Err("srand() takes one argument".into());
            }
            let seed = eval_expr_rng(&args[0], vars, rng)?
                .as_int()
                .ok_or("srand() seed must be an integer")?;
            *rng = (seed as u64) | 1;
            Ok(Value::Int(seed))
        }
        _ => Err(format!("unknown function `{name}()`")),
    }
}

/// xorshift64* PRNG — fast, deterministic, and stable across platforms.
fn next_u64(rng: &mut u64) -> u64 {
    let mut x = *rng;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *rng = x;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

fn next_f64(rng: &mut u64) -> f64 {
    (next_u64(rng) >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
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
        (Value::Float(a), Value::Float(b)) => {
            Ok(a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Int(a), Value::Float(b)) => Ok((*a as f64)
            .partial_cmp(b)
            .unwrap_or(std::cmp::Ordering::Equal)),
        (Value::Float(a), Value::Int(b)) => Ok(a
            .partial_cmp(&(*b as f64))
            .unwrap_or(std::cmp::Ordering::Equal)),
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
