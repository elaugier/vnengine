//! Script virtual machine.
//!
//! The VM executes a parsed [`Script`] and produces [`Command`]s for the
//! engine. It is a generator-style VM: the engine calls [`Vm::step`] to get
//! the next command. When a command is *blocking* (e.g. `Say` or `Menu`),
//! the engine must call [`Vm::resume`] with the player's response before
//! calling `step` again.

use crate::ast::*;
use crate::funcs::call_user_function;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use vnengine_core::atl::{AnimProp, AnimStep, SpriteTrack};
use vnengine_core::command::{Anchor, Command, MenuChoice, SpriteSpec};
use vnengine_core::model::{Layer, SpriteLayer};
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
    /// Pending `call screen` awaiting a button action.
    #[serde(default)]
    pub pending_screen: Option<PendingScreen>,
    /// Raw button-action expressions of ambient shown screens, keyed
    /// by screen name — evaluated at click time (Ren'Py semantics).
    #[serde(default)]
    pub shown_screen_actions:
        std::collections::BTreeMap<String, Vec<Vec<crate::ast::ButtonAction>>>,
    /// Pending `input` awaiting a text entry.
    #[serde(default)]
    pub pending_input: Option<PendingInput>,
    /// Pending `confirm` awaiting a yes/no answer.
    #[serde(default)]
    pub pending_confirm: Option<PendingConfirm>,
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
    /// `menu ... set <var>:` — receives the chosen index on resume.
    #[serde(default)]
    pub set_var: Option<String>,
    pub choices: Vec<MenuChoiceAst>,
}

/// A menu choice kept in AST form so it can be serialized and resumed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuChoiceAst {
    pub text: String,
    pub condition: Option<Expr>,
    pub body: Vec<Stmt>,
}

/// A `call screen` block in flight, kept serializable so the same
/// screen can be rebuilt after a save/restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingScreen {
    /// The resolved screen being displayed.
    pub inst: vnengine_core::screen::ScreenInst,
    /// `call screen ... set <var>` — receives the `return` value.
    #[serde(default)]
    pub set_var: Option<String>,
    /// Raw button-action expressions parallel to `inst.nodes` —
    /// evaluated at click time.
    #[serde(default)]
    pub node_actions: Vec<Vec<crate::ast::ButtonAction>>,
}

/// An `input` block in flight — binds `var` to the entered text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingInput {
    pub var: String,
    /// The prompt shown to the player (kept for save/restore).
    #[serde(default)]
    pub prompt: Option<String>,
}

/// A `confirm` block in flight — binds `var` to the yes/no answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingConfirm {
    pub var: String,
    /// The message shown to the player (kept for save/restore).
    #[serde(default)]
    pub message: Option<String>,
}

impl VmState {
    pub fn new() -> Self {
        Self {
            stack: vec![Frame::label("start")],
            variables: HashMap::new(),
            pending_menu: None,
            pending_screen: None,
            shown_screen_actions: std::collections::BTreeMap::new(),
            pending_input: None,
            pending_confirm: None,
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
    /// The VM is blocked on a `call screen`. The engine displays the
    /// resolved screen and calls `resume_screen` with the clicked
    /// clickable index.
    Screen {
        inst: vnengine_core::screen::ScreenInst,
    },
    /// The VM is blocked on an `input` statement.
    Input { prompt: Option<String> },
    /// The VM is blocked on a `confirm` statement.
    Confirm { message: Option<String> },
    /// The VM finished executing (game over).
    Finished,
}

/// Maximum number of internal steps `Vm::step` may take before giving up.
/// Prevents pathological scripts (e.g. an empty `while True:`) from
/// hanging the game loop.
const MAX_STEPS_PER_CALL: usize = 100_000;

/// Maximum call/block stack depth. A runaway `call` chain (infinite
/// recursion) aborts with an error instead of exhausting memory.
const MAX_STACK_DEPTH: usize = 1024;

pub struct Vm {
    pub script: Script,
    pub state: VmState,
    /// Character display names, populated from `define Character(...)`.
    pub character_names: HashMap<String, String>,
    /// Character name colors.
    pub character_colors: HashMap<String, [f32; 3]>,
    /// Character default voice tracks (`Character(..., voice=...)`).
    pub character_voice: HashMap<String, Expr>,
    /// Character default side images (`Character(..., image=...)`).
    pub character_image: HashMap<String, Expr>,
    /// Character kinds (`kind="nvl"` → NVL display).
    pub character_kind: HashMap<String, Expr>,
    /// Defined constants.
    pub constants: HashMap<String, Value>,
    /// User-defined functions, keyed by name.
    pub functions: HashMap<String, FunctionDef>,
    /// Output queued to be emitted before the next statement executes
    /// (e.g. `say ... with <trans>` plays the transition first, `menu
    /// ... with <trans>` delays the menu prompt). Not part of `VmState`:
    /// entries are drained within a step or two, before any blocking
    /// point a save could capture.
    deferred: VecDeque<Deferred>,
}

/// A deferred VM output: either a plain command (the say that follows
/// its `with` transition) or a whole menu presentation.
enum Deferred {
    Command(Command),
    Menu {
        prompt: Option<String>,
        set_var: Option<String>,
        choices: Vec<MenuChoice>,
        ast_choices: Vec<MenuChoiceAst>,
    },
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
            character_voice: HashMap::new(),
            character_image: HashMap::new(),
            character_kind: HashMap::new(),
            constants: HashMap::new(),
            functions,
            deferred: VecDeque::new(),
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
                Definition::Character {
                    id,
                    name,
                    color,
                    voice,
                    image,
                    kind,
                } => {
                    self.character_names.insert(id.clone(), name.clone());
                    if let Some(c) = color {
                        self.character_colors.insert(id.clone(), *c);
                    }
                    if let Some(v) = voice {
                        self.character_voice.insert(id.clone(), v.clone());
                    }
                    if let Some(i) = image {
                        self.character_image.insert(id.clone(), i.clone());
                    }
                    if let Some(k) = kind {
                        self.character_kind.insert(id.clone(), k.clone());
                    }
                }
                Definition::Constant { id, value } => {
                    let mut rng = default_rng_seed();
                    if let Ok(v) =
                        eval_expr_ctx(value, &self.constants, Some(&self.functions), &mut rng)
                    {
                        self.constants.insert(id.clone(), v);
                    }
                }
            }
        }
        // Process top-level `default` statements: only set if the variable
        // doesn't already exist (important for save/load restoration).
        for d in &self.script.defaults.clone() {
            if !self.state.variables.contains_key(&d.name) {
                if let Ok(v) = eval_expr_ctx(
                    &d.value,
                    &self.state.variables,
                    Some(&self.functions),
                    &mut self.state.rng_seed,
                ) {
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

        // If a blocking prompt is pending, the engine must resume first.
        if self.state.pending_menu.is_some()
            || self.state.pending_screen.is_some()
            || self.state.pending_input.is_some()
            || self.state.pending_confirm.is_some()
        {
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

            // Deferred output (e.g. a `with` transition preceding a
            // say/menu) is emitted before executing the next statement.
            if let Some(d) = self.deferred.pop_front() {
                match d {
                    Deferred::Command(cmd) => return StepResult::Command(cmd),
                    Deferred::Menu {
                        prompt,
                        set_var,
                        choices,
                        ast_choices,
                    } => {
                        // The menu statement was already consumed when the
                        // transition command advanced the frame; rewind so
                        // the choice body completes onto the statement
                        // after the menu, like a normal menu.
                        if let Some(f) = self.state.stack.last_mut() {
                            f.stmt = f.stmt.saturating_sub(1);
                        }
                        self.state.pending_menu = Some(PendingMenu {
                            prompt: prompt.clone(),
                            set_var,
                            choices: ast_choices,
                        });
                        return StepResult::Menu { prompt, choices };
                    }
                }
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
                    if eval_expr_ctx(
                        cond,
                        &self.state.variables,
                        Some(&self.functions),
                        &mut self.state.rng_seed,
                    )
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
                Ok(ExecOutcome::Screen { inst }) => {
                    return StepResult::Screen { inst };
                }
                Ok(ExecOutcome::Input { prompt }) => {
                    return StepResult::Input { prompt };
                }
                Ok(ExecOutcome::Confirm { message }) => {
                    return StepResult::Confirm { message };
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
                    if self.state.stack.len() >= MAX_STACK_DEPTH {
                        log::error!("script error: call stack overflow");
                        self.state.finished = true;
                        return StepResult::Finished;
                    }
                    self.state.stack.push(Frame::call(label));
                    continue;
                }
                Ok(ExecOutcome::Push(frame)) => {
                    if self.state.stack.len() >= MAX_STACK_DEPTH {
                        log::error!("script error: block stack overflow");
                        self.state.finished = true;
                        return StepResult::Finished;
                    }
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
                Ok(ExecOutcome::Break) => {
                    // Unwind frames through the innermost loop frame
                    // (inclusive), then continue after the loop.
                    let mut broke = false;
                    while let Some(f) = self.state.stack.pop() {
                        if f.for_loop.is_some() || f.while_loop.is_some() {
                            broke = true;
                            break;
                        }
                    }
                    if !broke {
                        log::error!("script error: `break` outside a loop");
                        self.state.finished = true;
                        return StepResult::Finished;
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
                Ok(ExecOutcome::ContinueLoop) => {
                    // Unwind non-loop frames, then force the innermost
                    // loop frame to its end-of-body path: `for` advances
                    // the iteration, `while` re-evaluates its condition.
                    while self
                        .state
                        .stack
                        .last()
                        .map(|f| f.for_loop.is_none() && f.while_loop.is_none())
                        .unwrap_or(false)
                    {
                        self.state.stack.pop();
                    }
                    let Some(frame) = self.state.stack.last_mut() else {
                        self.state.finished = true;
                        return StepResult::Finished;
                    };
                    let body_len = frame
                        .body_stmts(&self.script)
                        .map(|s| s.len())
                        .unwrap_or(0);
                    frame.stmt = body_len;
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
    /// Studio/debug: reset execution to the start of `name`'s body.
    /// Returns false when the label does not exist (state untouched).
    pub fn jump_to_label(&mut self, name: &str) -> bool {
        if self.script.find_label_index(name).is_none() {
            return false;
        }
        self.state.stack = vec![Frame::label(name)];
        self.state.pending_menu = None;
        true
    }

    pub fn resume(&mut self, response: Option<MenuResponse>) {
        if let Some(menu) = self.state.pending_menu.take() {
            if let Some(MenuResponse::Selected(idx)) = response {
                // `menu ... set <var>:` stores the chosen index.
                if let Some(var) = menu.set_var {
                    self.state
                        .variables
                        .insert(var, Value::Int(idx as i64));
                }
                if let Some(choice) = menu.choices.get(idx) {
                    // Push the choice body as an inline frame. The parent
                    // frame still points at the menu statement and advances
                    // when the body completes.
                    self.state.stack.push(Frame::block(choice.body.clone()));
                }
            }
        }
    }

    /// Resolve a `screen <name>:` definition into a `ScreenInst` —
    /// every expression is evaluated in the current variable context,
    /// `if`/`for` expanded. Returns the inst plus the *raw* button
    /// action expressions (parallel to `nodes`), which the VM keeps so
    /// `set`/`return` evaluate at click time — not at show time.
    pub fn resolve_screen(
        &mut self,
        name: &str,
    ) -> Result<
        (
            vnengine_core::screen::ScreenInst,
            Vec<Vec<crate::ast::ButtonAction>>,
        ),
        String,
    > {
        let def = self
            .script
            .find_screen(name)
            .cloned()
            .ok_or_else(|| format!("unknown screen `{name}`"))?;
        let mut nodes = Vec::new();
        let mut raw_actions = Vec::new();
        self.resolve_screen_body(&def.body, &mut nodes, &mut raw_actions)?;
        Ok((
            vnengine_core::screen::ScreenInst {
                name: name.to_string(),
                nodes,
            },
            raw_actions,
        ))
    }

    fn resolve_screen_body(
        &mut self,
        body: &[crate::ast::ScreenStmt],
        out: &mut Vec<vnengine_core::screen::UiNode>,
        out_actions: &mut Vec<Vec<crate::ast::ButtonAction>>,
    ) -> Result<(), String> {
        use crate::ast::ScreenStmt;
        use vnengine_core::screen::{UiNode, UiNodeKind};
        for stmt in body {
            match stmt {
                ScreenStmt::Box { x, y, w, h, color } => {
                    out.push(UiNode {
                        kind: UiNodeKind::Box,
                        rect: [
                            self.eval_f32(x)?,
                            self.eval_f32(y)?,
                            self.eval_f32(w)?,
                            self.eval_f32(h)?,
                        ],
                        text: None,
                        image: None,
                        color: match color {
                            Some(c) => self.eval_color(c)?,
                            None => [0.0, 0.0, 0.0, 0.5],
                        },
                        font_size: 24.0,
                        action: Vec::new(),
                        rich: false,
                    });
                    out_actions.push(Vec::new());
                }
                ScreenStmt::Text {
                    text,
                    x,
                    y,
                    size,
                    color,
                } => {
                    let raw = self.eval_str(text)?;
                    out.push(UiNode {
                        kind: UiNodeKind::Text,
                        rect: [self.eval_f32(x)?, self.eval_f32(y)?, 0.0, 0.0],
                        text: Some(self.interpolate(&raw)),
                        image: None,
                        color: match color {
                            Some(c) => self.eval_color(c)?,
                            None => [1.0, 1.0, 1.0, 1.0],
                        },
                        font_size: match size {
                            Some(s) => self.eval_f32(s)?,
                            None => 24.0,
                        },
                        action: Vec::new(),
                        rich: true,
                    });
                    out_actions.push(Vec::new());
                }
                ScreenStmt::Image {
                    image,
                    x,
                    y,
                    w,
                    h,
                    alpha,
                } => {
                    out.push(UiNode {
                        kind: UiNodeKind::Image,
                        rect: [
                            self.eval_f32(x)?,
                            self.eval_f32(y)?,
                            match w {
                                Some(e) => self.eval_f32(e)?,
                                None => 0.0,
                            },
                            match h {
                                Some(e) => self.eval_f32(e)?,
                                None => 0.0,
                            },
                        ],
                        text: None,
                        image: Some(self.eval_str(image)?),
                        color: [
                            1.0,
                            1.0,
                            1.0,
                            match alpha {
                                Some(e) => self.eval_f32(e)?,
                                None => 1.0,
                            },
                        ],
                        font_size: 24.0,
                        action: Vec::new(),
                        rich: false,
                    });
                    out_actions.push(Vec::new());
                }
                ScreenStmt::Button {
                    label,
                    x,
                    y,
                    w,
                    h,
                    color,
                    actions,
                } => {
                    let raw = self.eval_str(label)?;
                    out.push(UiNode {
                        kind: UiNodeKind::Button,
                        rect: [
                            self.eval_f32(x)?,
                            self.eval_f32(y)?,
                            match w {
                                Some(e) => self.eval_f32(e)?,
                                None => 0.15,
                            },
                            match h {
                                Some(e) => self.eval_f32(e)?,
                                None => 0.06,
                            },
                        ],
                        text: Some(self.interpolate(&raw)),
                        image: None,
                        color: match color {
                            Some(c) => self.eval_color(c)?,
                            None => [0.2, 0.2, 0.4, 0.9],
                        },
                        font_size: 22.0,
                        // Display-only resolved copy; the raw exprs in
                        // `out_actions` are what actually run on click.
                        action: self.resolve_button_actions(actions)?,
                        rich: true,
                    });
                    out_actions.push(actions.clone());
                }
                ScreenStmt::If { cond, body } => {
                    if self.eval(cond)?.is_truthy() {
                        self.resolve_screen_body(body, out, out_actions)?;
                    }
                }
                ScreenStmt::For {
                    var,
                    iterable,
                    body,
                } => {
                    let iter_val = self.eval(iterable)?;
                    let items: Vec<Value> = match &iter_val {
                        Value::List(items) => items.clone(),
                        Value::Dict(map) => map.keys().cloned().map(Value::Str).collect(),
                        Value::Str(s) => {
                            s.chars().map(|c| Value::Str(c.to_string())).collect()
                        }
                        _ => {
                            return Err(format!(
                                "screen `for` iterable must be list/dict/str, got {iter_val}"
                            ))
                        }
                    };
                    for item in items {
                        self.state.variables.insert(var.clone(), item);
                        self.resolve_screen_body(body, out, out_actions)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Screen name evaluation: a bare identifier is a literal screen
    /// name (like `jump` targets); other expressions are evaluated.
    fn eval_screen_name(&mut self, e: &Expr) -> Result<String, String> {
        match e {
            // A bare identifier is a literal screen name unless it is a
            // defined variable — mirrors `jump` label semantics.
            Expr::Var(name) if !self.state.variables.contains_key(name) => {
                Ok(name.clone())
            }
            _ => self.eval_str(e),
        }
    }

    /// Evaluate `play|queue channel` operands: channel name, track path,
    /// optional volume (default 1.0) and fade (default 0).
    fn eval_channel_track(
        &mut self,
        channel: &Expr,
        track: &Expr,
        volume: &Option<Expr>,
        fade: &Option<Expr>,
    ) -> Result<(String, String, f32, f32), String> {
        let channel = self.eval_str(channel)?;
        let track = self.eval_str(track)?;
        let volume = match volume {
            Some(e) => self
                .eval(e)
                .and_then(|v| v.as_float().ok_or("expected number".into()))?
                as f32,
            None => 1.0,
        };
        let fade = match fade {
            Some(e) => self
                .eval(e)
                .and_then(|v| v.as_float().ok_or("expected number".into()))?
                as f32,
            None => 0.0,
        };
        Ok((channel, track, volume, fade))
    }

    /// Resolve a button's action block into serializable actions.
    fn resolve_button_actions(
        &mut self,
        actions: &[crate::ast::ButtonAction],
    ) -> Result<Vec<vnengine_core::screen::ScreenAction>, String> {
        use vnengine_core::screen::ScreenAction;
        let mut out = Vec::new();
        for a in actions {
            out.push(match a {
                crate::ast::ButtonAction::Jump(e) => {
                    ScreenAction::Jump(self.eval_str(e)?)
                }
                crate::ast::ButtonAction::Set { name, value } => ScreenAction::SetVariable {
                    name: name.clone(),
                    value: self.eval(value)?,
                },
                crate::ast::ButtonAction::Return(e) => ScreenAction::Return(match e {
                    Some(e) => self.eval(e)?,
                    None => Value::None,
                }),
                crate::ast::ButtonAction::ShowScreen(e) => {
                    ScreenAction::ShowScreen(self.eval_screen_name(e)?)
                }
                crate::ast::ButtonAction::HideScreen(e) => {
                    ScreenAction::HideScreen(self.eval_screen_name(e)?)
                }
            });
        }
        Ok(out)
    }

    /// Evaluate a `color=` screen attribute: hex string `#rrggbb` or a
    /// 3/4-element list of 0..1 floats.
    fn eval_color(&mut self, e: &Expr) -> Result<[f32; 4], String> {
        match self.eval(e)? {
            Value::Str(s) => {
                let h = s.trim_start_matches('#');
                if h.len() == 6 {
                    let r = u8::from_str_radix(&h[0..2], 16).map_err(|_| "bad hex")?;
                    let g = u8::from_str_radix(&h[2..4], 16).map_err(|_| "bad hex")?;
                    let b = u8::from_str_radix(&h[4..6], 16).map_err(|_| "bad hex")?;
                    Ok([
                        r as f32 / 255.0,
                        g as f32 / 255.0,
                        b as f32 / 255.0,
                        1.0,
                    ])
                } else {
                    Err(format!("bad color `{s}`"))
                }
            }
            Value::List(items) => {
                let mut c = [1.0, 1.0, 1.0, 1.0];
                for (i, v) in items.iter().take(4).enumerate() {
                    c[i] = v
                        .as_float()
                        .ok_or_else(|| "color components must be numbers".to_string())?
                        as f32;
                }
                Ok(c)
            }
            v => Err(format!("bad color value {v}")),
        }
    }

    /// Evaluate a node's raw button actions at click time. Returns
    /// `(screen_effects, jumped)` — `set`/`return`/`jump` act on VM
    /// state, `show screen`/`hide screen` come back as effects for the
    /// runtime's `screens` list.
    fn run_button_actions(
        &mut self,
        actions: &[crate::ast::ButtonAction],
    ) -> (Vec<vnengine_core::screen::ScreenAction>, bool) {
        use vnengine_core::screen::ScreenAction;
        let mut effects = Vec::new();
        let mut jumped = false;
        for action in actions {
            match action {
                crate::ast::ButtonAction::Jump(e) => {
                    let label = match e {
                        Expr::Var(n) => n.clone(),
                        other => self.eval_str(other).unwrap_or_default(),
                    };
                    self.state.stack = vec![Frame::label(&label)];
                    self.state.pending_screen = None;
                    jumped = true;
                }
                crate::ast::ButtonAction::Set { name, value } => {
                    if let Ok(v) = self.eval(value) {
                        self.state.variables.insert(name.clone(), v);
                    }
                }
                crate::ast::ButtonAction::Return(e) => {
                    let v = match e {
                        Some(e) => self.eval(e).unwrap_or(Value::None),
                        None => Value::None,
                    };
                    let p = self.state.pending_screen.take();
                    if let Some(var) = p.and_then(|p| p.set_var) {
                        self.state.variables.insert(var, v);
                    }
                }
                crate::ast::ButtonAction::ShowScreen(e) => {
                    if let Ok(name) = self.eval_screen_name(e) {
                        effects.push(ScreenAction::ShowScreen(name));
                    }
                }
                crate::ast::ButtonAction::HideScreen(e) => {
                    if let Ok(name) = self.eval_screen_name(e) {
                        effects.push(ScreenAction::HideScreen(name));
                    }
                }
            }
            // `jump` abandons execution; `return` dismisses the call —
            // later actions are moot in both cases.
            if jumped || matches!(action, crate::ast::ButtonAction::Return(_)) {
                break;
            }
        }
        (effects, jumped)
    }

    /// Resume a `call screen` with the clicked clickable index.
    /// Button actions are evaluated at click time (Ren'Py semantics);
    /// `ShowScreen`/`HideScreen` effects are returned for the runtime
    /// to apply to its `screens` list.
    pub fn resume_screen(
        &mut self,
        idx: usize,
    ) -> Vec<vnengine_core::screen::ScreenAction> {
        let Some(pending) = &self.state.pending_screen else {
            return Vec::new();
        };
        let Some(&node_idx) = pending.inst.clickables().get(idx) else {
            return Vec::new();
        };
        let actions = pending
            .node_actions
            .get(node_idx)
            .cloned()
            .unwrap_or_default();
        let (effects, jumped) = self.run_button_actions(&actions);
        // The `call screen` statement is complete: advance the frame so
        // `step` doesn't re-execute it (a `jump` reset the stack, so
        // only non-jump dismissals advance here).
        if self.state.pending_screen.is_none() && !jumped {
            if let Some(f) = self.state.stack.last_mut() {
                f.stmt += 1;
            }
        }
        effects
    }

    /// Whether a `call screen` is still awaiting a dismissing action.
    pub fn is_on_screen(&self) -> bool {
        self.state.pending_screen.is_some()
    }

    /// Bind the pending `input` variable to `text` and continue.
    pub fn resume_input(&mut self, text: String) {
        if let Some(p) = self.state.pending_input.take() {
            self.state.variables.insert(p.var, Value::Str(text));
            if let Some(f) = self.state.stack.last_mut() {
                f.stmt += 1;
            }
        }
    }

    /// Bind the pending `confirm` variable to `yes` and continue.
    pub fn resume_confirm(&mut self, yes: bool) {
        if let Some(p) = self.state.pending_confirm.take() {
            self.state.variables.insert(p.var, Value::Bool(yes));
            if let Some(f) = self.state.stack.last_mut() {
                f.stmt += 1;
            }
        }
    }

    /// Apply a click on an ambient `show screen` button. Evaluates the
    /// node's raw actions at click time; returns `(effects, jumped)` —
    /// the runtime applies the effects and resumes stepping when a
    /// `jump` fired.
    pub fn apply_screen_actions(
        &mut self,
        screen: &str,
        node_idx: usize,
    ) -> (Vec<vnengine_core::screen::ScreenAction>, bool) {
        let actions = self
            .state
            .shown_screen_actions
            .get(screen)
            .and_then(|v| v.get(node_idx))
            .cloned()
            .unwrap_or_default();
        self.run_button_actions(&actions)
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
    Screen {
        inst: vnengine_core::screen::ScreenInst,
    },
    Input {
        prompt: Option<String>,
    },
    Confirm {
        message: Option<String>,
    },
    Jump(String),
    Call(String),
    /// Push a frame (control-flow block or function body) without advancing
    /// the parent frame's statement index.
    Push(Frame),
    Return,
    Continue,
    /// `break` — unwind through the innermost loop frame.
    Break,
    /// `continue` — restart the innermost loop's next iteration.
    ContinueLoop,
}

impl Vm {
    /// Evaluate an expression with access to user functions.
    fn eval(&mut self, expr: &Expr) -> Result<Value, String> {
        eval_expr_ctx(
            expr,
            &self.state.variables,
            Some(&self.functions),
            &mut self.state.rng_seed,
        )
    }

    /// Evaluate an expression and coerce the result to a string (used
    /// for asset names, labels, layer names...).
    fn eval_str(&mut self, expr: &Expr) -> Result<String, String> {
        let v = self.eval(expr)?;
        v.as_str()
            .ok_or_else(|| format!("expected a string, got {v}"))
    }

    /// Evaluate an expression and coerce to `f32`.
    fn eval_f32(&mut self, expr: &Expr) -> Result<f32, String> {
        let v = self.eval(expr)?;
        v.as_float()
            .map(|f| f as f32)
            .ok_or_else(|| format!("expected a number, got {v}"))
    }

    /// Compile a named `transform` block into running tracks plus
    /// event templates (`on show`/`replace`/`hide`). All expressions
    /// are evaluated in the current variable context and `choice`
    /// branches draw from the serialized PRNG — the result is plain
    /// serializable data for the scene model.
    fn compile_atl(
        &mut self,
        name: &str,
    ) -> Result<(Vec<SpriteTrack>, BTreeMap<String, Vec<SpriteTrack>>), String> {
        let def = self
            .script
            .find_transform(name)
            .ok_or_else(|| format!("unknown transform `{name}`"))?
            .clone();
        let mut out = Vec::new();
        let mut events = BTreeMap::new();
        self.compile_atl_body(&def.body, &mut out, &mut events)?;
        Ok((out, events))
    }

    /// Flatten an ATL block into tracks. Sequential statements share
    /// one track; each `parallel` block is its own track; `choice`
    /// groups resolve immediately through the seeded PRNG; `on`
    /// blocks compile into `events`; `repeat` tags the last track.
    fn compile_atl_body(
        &mut self,
        body: &[AtlStmt],
        out: &mut Vec<SpriteTrack>,
        events: &mut BTreeMap<String, Vec<SpriteTrack>>,
    ) -> Result<(), String> {
        let mut cur: Vec<AnimStep> = Vec::new();
        let mut i = 0;
        while i < body.len() {
            match &body[i] {
                AtlStmt::Warp {
                    warper,
                    duration,
                    props,
                } => {
                    let d = self.eval_f32(duration)?;
                    let targets = self.eval_atl_props(props)?;
                    cur.push(AnimStep {
                        duration: d.max(0.0),
                        warper: warper.clone(),
                        targets,
                        at: None,
                    });
                }
                AtlStmt::Set { props } => {
                    let targets = self.eval_atl_props(props)?;
                    cur.push(AnimStep {
                        duration: 0.0,
                        warper: "linear".into(),
                        targets,
                        at: None,
                    });
                }
                AtlStmt::Pause { seconds } => {
                    let s = self.eval_f32(seconds)?.max(0.0);
                    cur.push(AnimStep {
                        duration: s,
                        warper: "linear".into(),
                        targets: Vec::new(),
                        at: None,
                    });
                }
                AtlStmt::Time { at } => {
                    let t = self.eval_f32(at)?.max(0.0);
                    cur.push(AnimStep {
                        duration: 0.0,
                        warper: "linear".into(),
                        targets: Vec::new(),
                        at: Some(t),
                    });
                }
                AtlStmt::Parallel { body } => {
                    if !cur.is_empty() {
                        out.push(SpriteTrack::once(std::mem::take(&mut cur)));
                    }
                    self.compile_atl_body(body, out, events)?;
                }
                AtlStmt::On { event, body } => {
                    let mut ev_tracks = Vec::new();
                    self.compile_atl_body(body, &mut ev_tracks, events)?;
                    events
                        .entry(event.clone())
                        .or_insert_with(Vec::new)
                        .extend(ev_tracks);
                }
                AtlStmt::Choice { .. } => {
                    // Consecutive `choice` blocks form one group.
                    let mut j = i;
                    let mut weight_exprs: Vec<Option<Expr>> = Vec::new();
                    while j < body.len() {
                        if let AtlStmt::Choice { weight, .. } = &body[j] {
                            weight_exprs.push(weight.clone());
                            j += 1;
                        } else {
                            break;
                        }
                    }
                    let mut weights = Vec::with_capacity(j - i);
                    for w in &weight_exprs {
                        let v = match w {
                            Some(e) => self.eval(e)?.as_float().unwrap_or(1.0).max(0.0),
                            None => 1.0,
                        };
                        weights.push(v);
                    }
                    let total: f64 = weights.iter().sum();
                    let mut r = next_f64(&mut self.state.rng_seed) * total;
                    let mut pick = 0usize;
                    for (k, w) in weights.iter().enumerate() {
                        r -= w;
                        if r <= 0.0 {
                            pick = k;
                            break;
                        }
                    }
                    if !cur.is_empty() {
                        out.push(SpriteTrack::once(std::mem::take(&mut cur)));
                    }
                    if let AtlStmt::Choice { body: cb, .. } = &body[i + pick] {
                        self.compile_atl_body(cb, out, events)?;
                    }
                    i = j;
                    continue;
                }
                AtlStmt::Repeat { count } => {
                    let n = match count {
                        Some(e) => {
                            let v = self.eval(e)?;
                            let v = v
                                .as_int()
                                .ok_or_else(|| format!("repeat expects an integer, got {v}"))?;
                            Some(v.max(0) as u32)
                        }
                        None => None, // infinite
                    };
                    if !cur.is_empty() {
                        out.push(SpriteTrack::once(std::mem::take(&mut cur)));
                    }
                    if let Some(last) = out.last_mut() {
                        last.repeats = n;
                    }
                }
            }
            i += 1;
        }
        if !cur.is_empty() {
            out.push(SpriteTrack::once(cur));
        }
        Ok(())
    }

    /// Evaluate `(prop-name, expr)` pairs into `(AnimProp, f32)` targets.
    fn eval_atl_props(
        &mut self,
        props: &[(String, Expr)],
    ) -> Result<Vec<(AnimProp, f32)>, String> {
        let mut out = Vec::with_capacity(props.len());
        for (name, e) in props {
            let prop = AnimProp::parse(name)
                .ok_or_else(|| format!("unknown ATL property `{name}`"))?;
            out.push((prop, self.eval_f32(e)?));
        }
        Ok(out)
    }

    /// Resolve an `image name:` composite into concrete layers by
    /// evaluating `when`/`alpha`/`offset`/`zoom` in the current
    /// context. Returns `None` when `name` isn't a composite.
    fn resolve_image_layers(&mut self, name: &str) -> Result<Option<Vec<SpriteLayer>>, String> {
        let Some(def) = self.script.find_image(name).cloned() else {
            return Ok(None);
        };
        let mut layers = Vec::new();
        for l in &def.layers {
            let show = match &l.when {
                Some(e) => self.eval(e)?.is_truthy(),
                None => true,
            };
            if !show {
                continue;
            }
            let image = self.eval_str(&l.image)?;
            let offset = match &l.offset {
                Some((x, y)) => [self.eval_f32(x)?, self.eval_f32(y)?],
                None => [0.0, 0.0],
            };
            let zoom = match &l.zoom {
                Some(e) => self.eval_f32(e)?,
                None => 1.0,
            };
            let alpha = match &l.alpha {
                Some(e) => self.eval_f32(e)?,
                None => 1.0,
            };
            layers.push(SpriteLayer {
                image,
                offset,
                zoom,
                alpha,
            });
        }
        if layers.is_empty() {
            return Err(format!("image composite `{name}` resolved to zero layers"));
        }
        Ok(Some(layers))
    }

    /// Interpolate `[expr]` references in dialogue text. Falls back to
    /// the literal `[name]` when the content is not a valid expression
    /// or references an undefined variable (backward compatible with
    /// the old `[var]`-only syntax).
    fn interpolate(&mut self, text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '[' {
                if i + 1 < chars.len() && chars[i + 1] == '[' {
                    out.push('[');
                    i += 2;
                    continue;
                }
                // Find the matching `]`, honoring nested brackets so
                // expressions like `[player["level"]]` resolve fully.
                let mut depth = 0i32;
                let mut close = None;
                for (j, &c) in chars[i + 1..].iter().enumerate() {
                    match c {
                        '[' => depth += 1,
                        ']' => {
                            if depth == 0 {
                                close = Some(j);
                                break;
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                }
                if let Some(close) = close {
                    let inner: String = chars[i + 1..i + 1 + close].iter().collect();
                    let inner = inner.trim();
                    let mut resolved = None;
                    if let Ok(expr) = crate::parser::Parser::try_parse_expr(inner) {
                        if let Ok(v) = self.eval(&expr) {
                            resolved = Some(v.to_string());
                        }
                    }
                    match resolved {
                        Some(s) => out.push_str(&s),
                        None => {
                            out.push('[');
                            out.push_str(inner);
                            out.push(']');
                        }
                    }
                    i += 1 + close + 1;
                    continue;
                }
                out.push('[');
                i += 1;
            } else if chars[i] == ']' {
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

    fn exec_stmt(&mut self, stmt: Stmt) -> Result<ExecOutcome, String> {
        match stmt {
            Stmt::Scene {
                image,
                transition,
                layer,
            } => {
                let image = match &image {
                    Some(e) => Some(self.eval_str(e)?),
                    None => None,
                };
                let trans = transition
                    .and_then(|t| Transition::parse(&t.name, t.duration))
                    .unwrap_or(Transition::Cut);
                let layer = layer.map(|l| parse_layer(&l));
                // `scene <image-name>` where the name is an `image` def
                // resolves to a composite (extra layers on the bg).
                let layers = match &image {
                    Some(name) => self.resolve_image_layers(name)?.unwrap_or_default(),
                    None => Vec::new(),
                };
                Ok(ExecOutcome::Command(Command::Scene {
                    image,
                    transition: trans,
                    layer,
                    layers,
                }))
            }
            Stmt::Show {
                character,
                image,
                anchor,
                position,
                scale,
                z,
                layer,
                transition,
                at,
            } => {
                let transition = transition.and_then(|t| Transition::parse(&t.name, t.duration));
                let anchor = anchor_to_anchor(&anchor);
                let pos = position
                    .map(|(x, y)| {
                        let xv = self.eval(&x)?.as_float();
                        let yv = self.eval(&y)?.as_float();
                        Ok::<_, String>((xv, yv))
                    })
                    .transpose()?
                    .and_then(|(xv, yv)| Some((xv? as f32, yv? as f32)));
                let scale = scale
                    .map(|e| self.eval(&e).and_then(|v| v.as_float().ok_or("expected number".into())))
                    .transpose()?
                    .map(|f| f as f32);
                let z = z
                    .map(|e| self.eval(&e).and_then(|v| v.as_int().ok_or("expected number".into())))
                    .transpose()?
                    .map(|i| i as i32);
                let layer = layer.map(|l| parse_layer(&l));
                let image = self.eval_str(&image)?;
                let layers = self.resolve_image_layers(&image)?.unwrap_or_default();
                // `at <transform>` names compile to ATL tracks + events.
                let mut anims = Vec::new();
                let mut anim_events: BTreeMap<String, Vec<SpriteTrack>> = BTreeMap::new();
                for name in &at {
                    let (tracks, events) = self.compile_atl(name)?;
                    anims.extend(tracks);
                    for (k, v) in events {
                        anim_events.entry(k).or_insert_with(Vec::new).extend(v);
                    }
                }
                Ok(ExecOutcome::Command(Command::Show(SpriteSpec {
                    character,
                    image,
                    anchor,
                    position: pos,
                    scale,
                    z,
                    layer,
                    transition,
                    anims,
                    anim_events,
                    layers,
                })))
            }
            Stmt::Hide {
                character,
                transition,
            } => {
                let trans = transition
                    .and_then(|t| Transition::parse(&t.name, t.duration))
                    .unwrap_or(Transition::Cut);
                match character {
                    Some(character) => Ok(ExecOutcome::Command(Command::Hide {
                        character,
                        transition: trans,
                    })),
                    None => Ok(ExecOutcome::Command(Command::HideAll { transition: trans })),
                }
            }
            Stmt::Say {
                speaker,
                text,
                voice,
                side_image,
                transition,
            } => {
                let text = self.interpolate(&text);
                // Character defaults fill unset voice/side_image and
                // decide the NVL display kind.
                let (char_voice, char_image, char_kind) = match &speaker {
                    Some(id) => (
                        self.character_voice.get(id).cloned(),
                        self.character_image.get(id).cloned(),
                        self.character_kind.get(id).cloned(),
                    ),
                    None => (None, None, None),
                };
                let voice = match voice.or(char_voice).as_ref() {
                    Some(e) => Some(self.eval_str(e)?),
                    None => None,
                };
                let side_image = match side_image.or(char_image).as_ref() {
                    Some(e) => Some(self.eval_str(e)?),
                    None => None,
                };
                let nvl = match &char_kind {
                    Some(e) => self.eval_str(e).map(|k| k == "nvl").unwrap_or(false),
                    None => false,
                };
                let cmd = if let Some(id) = &speaker {
                    let speaker_name = self.character_names.get(id).cloned();
                    Command::Say {
                        speaker: speaker_name.or(Some(id.clone())),
                        text,
                        voice,
                        side_image,
                        nvl,
                    }
                } else {
                    Command::Narrate { text }
                };
                if let Some(t) = transition {
                    if let Some(tr) = Transition::parse(&t.name, t.duration) {
                        if tr.default_duration() > 0.0 {
                            // Emit the transition now; the say itself is
                            // drained from `deferred` on the next step.
                            self.deferred.push_back(Deferred::Command(cmd));
                            return Ok(ExecOutcome::Command(Command::PlayTransition(tr)));
                        }
                    }
                }
                Ok(ExecOutcome::Command(cmd))
            }
            Stmt::Menu {
                prompt,
                transition,
                set_var,
                choices,
            } => {
                let prompt = match &prompt {
                    Some(e) => {
                        let s = self.eval_str(e)?;
                        Some(self.interpolate(&s))
                    }
                    None => None,
                };
                let mut filtered = Vec::new();
                let mut ast_choices = Vec::new();
                for c in choices {
                    let selectable = match &c.condition {
                        Some(expr) => self
                            .eval(expr)
                            .map(|v| v.is_truthy())
                            .unwrap_or(true),
                        None => true,
                    };
                    if selectable {
                    let interp_text = self.interpolate(&c.text);
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
            // `menu ... with <trans>`: play the transition first and
            // present the menu on the next step.
            if let Some(t) = transition {
                if let Some(tr) = Transition::parse(&t.name, t.duration) {
                    if tr.default_duration() > 0.0 {
                        self.deferred.push_back(Deferred::Menu {
                            prompt: prompt.clone(),
                            set_var: set_var.clone(),
                            choices: filtered,
                            ast_choices,
                        });
                        return Ok(ExecOutcome::Command(Command::PlayTransition(tr)));
                    }
                }
            }
            // Stash the AST choices for resume.
            self.state.pending_menu = Some(PendingMenu {
                prompt: prompt.clone(),
                set_var,
                choices: ast_choices,
            });
            Ok(ExecOutcome::Menu {
                prompt,
                choices: filtered,
            })
        }
        Stmt::Jump { label } => {
            // A bare identifier is a literal label name (backward
            // compatible); any other expression is evaluated to one.
            let name = match &label {
                Expr::Var(n) => n.clone(),
                other => self.eval_str(other)?,
            };
            Ok(ExecOutcome::Jump(name))
        }
        Stmt::Call { label } => {
            match &label {
                // `call f(args)` — bind parameters as variables, then run
                // the body as a call frame so blocking statements (`say`,
                // `menu`...) work. Parameters persist as regular
                // variables after the call, like Ren'Py's store.
                Expr::Call { name, args } => {
                    if let Some(func) = self.functions.get(name).cloned() {
                        if args.len() != func.params.len() {
                            return Err(format!(
                                "function `{name}` expects {} argument(s), got {}",
                                func.params.len(),
                                args.len()
                            ));
                        }
                        for (param, arg) in func.params.iter().zip(args.iter()) {
                            let v = self.eval(arg)?;
                            self.state.variables.insert(param.clone(), v);
                        }
                        let mut frame = Frame::block(func.body);
                        frame.is_call = true;
                        return Ok(ExecOutcome::Push(frame));
                    }
                    // `call label(arg, ...)` — bind the label's declared
                    // parameters, then jump to it as a call frame.
                    let label_params = self
                        .script
                        .find_label(name)
                        .map(|l| l.params.clone());
                    if let Some(params) = label_params {
                        if args.len() != params.len() {
                            return Err(format!(
                                "label `{name}` expects {} argument(s), got {}",
                                params.len(),
                                args.len()
                            ));
                        }
                        for (param, arg) in params.iter().zip(args.iter()) {
                            let v = self.eval(arg)?;
                            self.state.variables.insert(param.clone(), v);
                        }
                        return Ok(ExecOutcome::Call(name.clone()));
                    }
                    return Err(format!("unknown function or label `{name}`"));
                }
                // `call name` — zero-arg user function (frame-based, so
                // the body may contain blocking statements like `say`)
                // or a plain label.
                Expr::Var(name) => {
                    if let Some(func) = self.functions.get(name).cloned() {
                        let mut frame = Frame::block(func.body);
                        frame.is_call = true;
                        return Ok(ExecOutcome::Push(frame));
                    }
                    Ok(ExecOutcome::Call(name.clone()))
                }
                // Dynamic target: evaluate to a label name.
                other => {
                    let name = self.eval_str(other)?;
                    Ok(ExecOutcome::Call(name))
                }
            }
        }
        Stmt::Return => Ok(ExecOutcome::Return),
        Stmt::ReturnValue { value } => {
            // Inside a label `call` the value is discarded; inside a
            // function body this statement never reaches exec_stmt
            // (funcs::exec_stmt handles it). Evaluate for RNG parity.
            let _ = self.eval(&value)?;
            Ok(ExecOutcome::Return)
        }
        Stmt::With { transition } => {
            let trans = Transition::parse(&transition.name, transition.duration)
                .unwrap_or(Transition::Cut);
            Ok(ExecOutcome::Command(Command::PlayTransition(trans)))
        }
        Stmt::Break => Ok(ExecOutcome::Break),
        Stmt::Continue => Ok(ExecOutcome::ContinueLoop),
            Stmt::Shake {
                intensity,
                duration,
            } => {
                let i = self
                    .eval(&intensity)
                    .map_err(|e| format!("shake error: {e}"))?;
                let d = self
                    .eval(&duration)
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
            Stmt::If {
                branches,
                else_branch,
            } => {
                for (cond, body) in &branches {
                    if self
                        .eval(cond)
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
            Stmt::Pause { seconds } => match seconds {
                Some(e) => {
                    let s = self
                        .eval(&e)
                        .ok()
                        .and_then(|v| v.as_float())
                        .unwrap_or(1.0) as f32;
                    Ok(ExecOutcome::Command(Command::Pause { seconds: s }))
                }
                None => Ok(ExecOutcome::Command(Command::WaitInput)),
            },
            Stmt::PlayMusic { track, fade } => {
                let track = self.eval_str(&track)?;
                Ok(ExecOutcome::Command(Command::PlayMusic { track, fade }))
            }
            Stmt::StopMusic { fade } => Ok(ExecOutcome::Command(Command::StopMusic { fade })),
            Stmt::PlaySfx { track, volume } => {
                let track = self.eval_str(&track)?;
                Ok(ExecOutcome::Command(Command::PlaySfx { track, volume }))
            }
            Stmt::PlayVoice { track } => {
                let track = self.eval_str(&track)?;
                Ok(ExecOutcome::Command(Command::PlayVoice { track }))
            }
            Stmt::StopVoice => Ok(ExecOutcome::Command(Command::StopVoice)),
            Stmt::Assign { name, value } => {
                let v = self
                    .eval(&value)
                    .map_err(|e| format!("assignment error: {e}"))?;
                self.state.variables.insert(name, v);
                Ok(ExecOutcome::Continue)
            }
            Stmt::AssignOp { name, op, value } => {
                let current = self.state.variables.get(&name).cloned().ok_or_else(|| {
                    format!("undefined variable `{name}` for compound assignment")
                })?;
                let rhs = self
                    .eval(&value)
                    .map_err(|e| format!("assignment error: {e}"))?;
                let result = eval_binop_pub(op, current, rhs)?;
                self.state.variables.insert(name, result);
                Ok(ExecOutcome::Continue)
            }
            Stmt::Default { name, value } => {
                // Only assign if the variable doesn't already exist.
                if !self.state.variables.contains_key(&name) {
                    let v = self
                        .eval(&value)
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
                let mut num = |e: &Option<Expr>| -> Result<Option<f32>, String> {
                    match e {
                        Some(e) => self
                            .eval(e)
                            .and_then(|v| v.as_float().ok_or("expected number".into()))
                            .map(|f| Some(f as f32)),
                        None => Ok(None),
                    }
                };
                let rotation = num(&rotation)?;
                let scale = num(&scale)?;
                let opacity = num(&opacity)?;
                let offset = match &offset {
                    Some((x, y)) => {
                        let xv = self.eval(x)?.as_float().ok_or("expected number")?;
                        let yv = self.eval(y)?.as_float().ok_or("expected number")?;
                        Some((xv as f32, yv as f32))
                    }
                    None => None,
                };
                let layer = layer.map(|l| parse_layer(&l));
                Ok(ExecOutcome::Command(Command::Transform {
                    character,
                    rotation,
                    scale,
                    offset,
                    opacity,
                    layer,
                }))
            }
            Stmt::While { cond, body } => {
                if self
                    .eval(&cond)
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
                let iter_val = self
                    .eval(&iterable)
                    .map_err(|e| format!("for loop error: {e}"))?;
                let items: Vec<Value> = match &iter_val {
                    Value::List(items) => items.clone(),
                    Value::Dict(map) => map.keys().cloned().map(Value::Str).collect(),
                    Value::Str(s) => s
                        .chars()
                        .map(|c| Value::Str(c.to_string()))
                        .collect(),
                    _ => return Err("for loop iterable must be a list, dict or string".into()),
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
            Stmt::Plugin { name, args } => {
                let mut values = Vec::with_capacity(args.len());
                for arg in &args {
                    values.push(
                        self.eval(arg)
                            .map_err(|e| format!("plugin `{name}` argument error: {e}"))?,
                    );
                }
                // Public payload: plain JSON (strings/numbers/bools/
                // null/arrays), not the internal tagged-Value encoding.
                let json: Vec<serde_json::Value> = values.iter().map(value_to_json).collect();
                let payload = serde_json::to_string(&json)
                    .map_err(|e| format!("plugin `{name}` payload error: {e}"))?;
                Ok(ExecOutcome::Command(Command::PluginCall {
                    plugin: name,
                    payload,
                }))
            }
            Stmt::Light {
                name,
                x,
                y,
                radius,
                intensity,
                color,
            } => {
                let mut f = |e: &Expr| -> Result<f32, String> {
                    self.eval(e)
                        .and_then(|v| v.as_float().ok_or("expected number".into()))
                        .map(|f| f as f32)
                };
                Ok(ExecOutcome::Command(Command::LightSet(
                    vnengine_core::model::PointLight {
                        name,
                        x: f(&x)?,
                        y: f(&y)?,
                        radius: f(&radius)?,
                        intensity: f(&intensity)?,
                        color: color.unwrap_or([1.0, 1.0, 1.0]),
                    },
                )))
            }
            Stmt::Unlight { name } => Ok(ExecOutcome::Command(Command::LightRemove { name })),
            Stmt::Ambient { r, g, b } => {
                let mut f = |e: &Expr| -> Result<f32, String> {
                    self.eval(e)
                        .and_then(|v| v.as_float().ok_or("expected number".into()))
                        .map(|f| f as f32)
                };
                Ok(ExecOutcome::Command(Command::Ambient {
                    color: [f(&r)?, f(&g)?, f(&b)?],
                }))
            }
            Stmt::Camera { x, y, zoom, over } => {
                let mut f = |e: &Option<Expr>| -> Result<Option<f32>, String> {
                    match e {
                        Some(e) => self
                            .eval(e)
                            .and_then(|v| v.as_float().ok_or("expected number".into()))
                            .map(|f| Some(f as f32)),
                        None => Ok(None),
                    }
                };
                Ok(ExecOutcome::Command(Command::Camera {
                    x: f(&x)?,
                    y: f(&y)?,
                    zoom: f(&zoom)?,
                    duration: f(&over)?.unwrap_or(0.0),
                }))
            }
            Stmt::MusicLayer {
                name,
                track,
                volume,
                fade,
                off,
            } => {
                let track = match &track {
                    Some(e) => Some(self.eval_str(e)?),
                    None => None,
                };
                let mut f = |e: &Option<Expr>| -> Result<Option<f32>, String> {
                    match e {
                        Some(e) => self
                            .eval(e)
                            .and_then(|v| v.as_float().ok_or("expected number".into()))
                            .map(|f| Some(f as f32)),
                        None => Ok(None),
                    }
                };
                let fade = f(&fade)?.unwrap_or(0.0);
                if off {
                    return Ok(ExecOutcome::Command(Command::MusicLayerOff { name, fade }));
                }
                if track.is_none() && volume.is_none() {
                    return Err("music layer requires a track or volume".into());
                }
                Ok(ExecOutcome::Command(Command::MusicLayer {
                    name,
                    track,
                    volume: f(&volume)?,
                    fade,
                }))
            }
            Stmt::PlayChannel {
                channel,
                track,
                volume,
                fade,
                loop_,
            } => {
                let (channel, track, volume, fade) =
                    self.eval_channel_track(&channel, &track, &volume, &fade)?;
                Ok(ExecOutcome::Command(Command::PlayChannel {
                    channel,
                    track,
                    volume,
                    fade,
                    loop_,
                }))
            }
            Stmt::QueueChannel {
                channel,
                track,
                volume,
                fade,
                loop_,
            } => {
                let (channel, track, volume, fade) =
                    self.eval_channel_track(&channel, &track, &volume, &fade)?;
                Ok(ExecOutcome::Command(Command::QueueChannel {
                    channel,
                    track,
                    volume,
                    fade,
                    loop_,
                }))
            }
            Stmt::StopChannel { channel, fade } => {
                let channel = self.eval_str(&channel)?;
                let fade = match &fade {
                    Some(e) => self
                        .eval(e)
                        .and_then(|v| v.as_float().ok_or("expected number".into()))?
                        as f32,
                    None => 0.0,
                };
                Ok(ExecOutcome::Command(Command::StopChannel { channel, fade }))
            }
            Stmt::StopAll { fade } => {
                let fade = match &fade {
                    Some(e) => self
                        .eval(e)
                        .and_then(|v| v.as_float().ok_or("expected number".into()))?
                        as f32,
                    None => 0.0,
                };
                Ok(ExecOutcome::Command(Command::StopAll { fade }))
            }
            Stmt::VolumeChannel {
                channel,
                volume,
                fade,
            } => {
                let channel = self.eval_str(&channel)?;
                let volume = self
                    .eval(&volume)
                    .and_then(|v| v.as_float().ok_or("expected number".into()))?
                    as f32;
                let fade = match &fade {
                    Some(e) => self
                        .eval(e)
                        .and_then(|v| v.as_float().ok_or("expected number".into()))?
                        as f32,
                    None => 0.0,
                };
                Ok(ExecOutcome::Command(Command::VolumeChannel {
                    channel,
                    volume,
                    fade,
                }))
            }
            Stmt::FilterChannel {
                channel,
                kind,
                params,
            } => {
                let channel = self.eval_str(&channel)?;
                let get = |name: &str, default: f64, vm: &mut Self| -> Result<f64, String> {
                    match params.iter().find(|(k, _)| k == name) {
                        Some((_, e)) => vm
                            .eval(e)
                            .and_then(|v| v.as_float().ok_or("expected number".into())),
                        None => Ok(default),
                    }
                };
                let filter = match kind.as_str() {
                    "off" | "none" => vnengine_core::command::ChannelFilter::Off,
                    "reverb" => vnengine_core::command::ChannelFilter::Reverb {
                        feedback: get("feedback", 0.5, self)?,
                        damping: get("damping", 0.5, self)?,
                        stereo_width: get("width", 1.0, self)?,
                        mix: get("mix", 0.5, self)?,
                    },
                    m @ ("lowpass" | "highpass" | "bandpass" | "notch") => {
                        vnengine_core::command::ChannelFilter::Filter {
                            mode: m.to_string(),
                            cutoff: get("cutoff", 800.0, self)?,
                            resonance: get("resonance", 0.0, self)?,
                            mix: get("mix", 1.0, self)?,
                        }
                    }
                    other => {
                        return Err(format!(
                            "unknown filter kind `{other}` (lowpass|highpass|bandpass|notch|reverb|off)"
                        ))
                    }
                };
                Ok(ExecOutcome::Command(Command::FilterChannel { channel, filter }))
            }
            Stmt::ShowScreen { screen } => {
                let name = self.eval_screen_name(&screen)?;
                let (inst, raw) = self.resolve_screen(&name)?;
                self.state
                    .shown_screen_actions
                    .insert(name, raw);
                Ok(ExecOutcome::Command(Command::ShowScreen(inst)))
            }
            Stmt::HideScreen { screen } => {
                let name = self.eval_screen_name(&screen)?;
                self.state.shown_screen_actions.remove(&name);
                Ok(ExecOutcome::Command(Command::HideScreen { name }))
            }
            Stmt::CallScreen { screen, set_var } => {
                let name = self.eval_screen_name(&screen)?;
                let (inst, raw) = self.resolve_screen(&name)?;
                self.state.pending_screen = Some(PendingScreen {
                    inst: inst.clone(),
                    set_var,
                    node_actions: raw,
                });
                Ok(ExecOutcome::Screen { inst })
            }
            Stmt::Input { var, prompt } => {
                let prompt = match &prompt {
                    Some(e) => {
                        let s = self.eval_str(e)?;
                        Some(self.interpolate(&s))
                    }
                    None => None,
                };
                self.state.pending_input = Some(PendingInput {
                    var,
                    prompt: prompt.clone(),
                });
                Ok(ExecOutcome::Input { prompt })
            }
            Stmt::Confirm { var, message } => {
                let message = match &message {
                    Some(e) => {
                        let s = self.eval_str(e)?;
                        Some(self.interpolate(&s))
                    }
                    None => None,
                };
                self.state.pending_confirm = Some(PendingConfirm {
                    var,
                    message: message.clone(),
                });
                Ok(ExecOutcome::Confirm { message })
            }
            Stmt::NvlClear => Ok(ExecOutcome::Command(Command::NvlClear)),
            Stmt::Window { mode } => Ok(ExecOutcome::Command(Command::Window { mode })),
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
/// User-defined functions are unavailable in this context — use
/// [`eval_expr_ctx`] with `fns` for full evaluation.
pub fn eval_expr_rng(
    expr: &Expr,
    vars: &HashMap<String, Value>,
    rng: &mut u64,
) -> Result<Value, String> {
    eval_expr_ctx(expr, vars, None, rng)
}

/// Full expression evaluation. `fns` optionally provides user-defined
/// functions for `Expr::Call` resolution (builtins take precedence).
pub(crate) fn eval_expr_ctx(
    expr: &Expr,
    vars: &HashMap<String, Value>,
    fns: Option<&HashMap<String, FunctionDef>>,
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
            let mut vals = Vec::with_capacity(items.len());
            for item in items {
                vals.push(eval_expr_ctx(item, vars, fns, rng)?);
            }
            Value::List(vals)
        }
        Expr::Dict(entries) => {
            let mut map = std::collections::BTreeMap::new();
            for (k, v) in entries {
                map.insert(k.clone(), eval_expr_ctx(v, vars, fns, rng)?);
            }
            Value::Dict(map)
        }
        Expr::Index { collection, index } => {
            let coll = eval_expr_ctx(collection, vars, fns, rng)?;
            let idx = eval_expr_ctx(index, vars, fns, rng)?;
            index_value(&coll, &idx)?
        }
        Expr::Slice {
            collection,
            start,
            end,
        } => {
            let coll = eval_expr_ctx(collection, vars, fns, rng)?;
            let start = match start {
                Some(e) => Some(
                    eval_expr_ctx(e, vars, fns, rng)?
                        .as_int()
                        .ok_or("slice bound must be an integer")?,
                ),
                None => None,
            };
            let end = match end {
                Some(e) => Some(
                    eval_expr_ctx(e, vars, fns, rng)?
                        .as_int()
                        .ok_or("slice bound must be an integer")?,
                ),
                None => None,
            };
            slice_value(&coll, start, end)?
        }
        Expr::Attr { object, name } => {
            let obj = eval_expr_ctx(object, vars, fns, rng)?;
            match &obj {
                Value::Dict(map) => map
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("dict has no key `{name}`")),
                _ => Err("`.` attribute access only works on dicts".into()),
            }?
        }
        Expr::MethodCall {
            object,
            name,
            args,
        } => {
            let obj = eval_expr_ctx(object, vars, fns, rng)?;
            let mut vals = Vec::with_capacity(args.len());
            for a in args {
                vals.push(eval_expr_ctx(a, vars, fns, rng)?);
            }
            eval_method(&obj, name, &vals)?
        }
        Expr::Call { name, args } => match eval_builtin(name, args, vars, fns, rng) {
            Ok(v) => v,
            Err(e) => {
                if e.starts_with("unknown function") {
                    if let Some(fns) = fns {
                        if let Some(func) = fns.get(name) {
                            return call_user_function(func, args, vars, fns, rng);
                        }
                    }
                }
                return Err(e);
            }
        },
        Expr::UnaryOp { op, operand } => {
            let v = eval_expr_ctx(operand, vars, fns, rng)?;
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
            let l = eval_expr_ctx(lhs, vars, fns, rng)?;
            let r = eval_expr_ctx(rhs, vars, fns, rng)?;
            eval_binop_pub(*op, l, r)?
        }
    })
}

/// `collection[index]` — lists/strings by integer (negative counts
/// from the end), dicts by string key.
fn index_value(coll: &Value, idx: &Value) -> Result<Value, String> {
    match (coll, idx) {
        (Value::List(items), Value::Int(i)) => {
            let len = items.len() as i64;
            let i = if *i < 0 { len + i } else { *i };
            if i < 0 || i >= len {
                return Err(format!("list index out of range: {i}"));
            }
            Ok(items[i as usize].clone())
        }
        (Value::Str(s), Value::Int(i)) => {
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let i = if *i < 0 { len + i } else { *i };
            if i < 0 || i >= len {
                return Err(format!("string index out of range: {i}"));
            }
            Ok(Value::Str(chars[i as usize].to_string()))
        }
        (Value::Dict(map), Value::Str(k)) => map
            .get(k)
            .cloned()
            .ok_or_else(|| format!("dict has no key `{k}`")),
        (Value::Dict(_), _) => Err("dict keys are strings".into()),
        _ => Err("cannot index non-collection".into()),
    }
}

/// `collection[start:end]` — Python-style bounds (negative counts
/// from the end, clamped to the collection).
fn slice_value(coll: &Value, start: Option<i64>, end: Option<i64>) -> Result<Value, String> {
    fn bounds(len: i64, start: Option<i64>, end: Option<i64>) -> (usize, usize) {
        let norm = |v: Option<i64>, default: i64| -> i64 {
            let v = v.unwrap_or(default);
            if v < 0 {
                (len + v).max(0)
            } else {
                v.min(len)
            }
        };
        let s = norm(start, 0);
        let e = norm(end, len);
        (s.max(0) as usize, e.max(s).max(0) as usize)
    }
    match coll {
        Value::List(items) => {
            let (s, e) = bounds(items.len() as i64, start, end);
            Ok(Value::List(items[s.min(e)..e.min(items.len())].to_vec()))
        }
        Value::Str(s) => {
            let chars: Vec<char> = s.chars().collect();
            let (s, e) = bounds(chars.len() as i64, start, end);
            Ok(Value::Str(chars[s.min(e)..e.min(chars.len())].iter().collect()))
        }
        _ => Err("cannot slice non-collection".into()),
    }
}

/// `expr.name(args...)` — read-only methods on strings, lists and
/// dicts. Mutating methods are intentionally unsupported: values are
/// immutable, use `x + [item]` or reassignment instead.
fn eval_method(obj: &Value, name: &str, args: &[Value]) -> Result<Value, String> {
    // String methods.
    if let Value::Str(s) = obj {
        return match name {
            "upper" => no_args(name, args).map(|_| Value::Str(s.to_uppercase())),
            "lower" => no_args(name, args).map(|_| Value::Str(s.to_lowercase())),
            "strip" => no_args(name, args).map(|_| Value::Str(s.trim().to_string())),
            "lstrip" => no_args(name, args).map(|_| Value::Str(s.trim_start().to_string())),
            "rstrip" => no_args(name, args).map(|_| Value::Str(s.trim_end().to_string())),
            "split" => {
                if args.is_empty() {
                    Ok(Value::List(
                        s.split_whitespace()
                            .map(|p| Value::Str(p.to_string()))
                            .collect(),
                    ))
                } else if args.len() == 1 {
                    let sep = args[0].as_str().unwrap_or_default();
                    Ok(Value::List(
                        s.split(sep.as_str())
                            .map(|p| Value::Str(p.to_string()))
                            .collect(),
                    ))
                } else {
                    Err(format!("str.{name}() takes at most 1 argument"))
                }
            }
            "replace" => {
                if args.len() != 2 {
                    return Err(format!("str.{name}() takes 2 arguments"));
                }
                let (old, new) = (args[0].as_str().unwrap_or_default(), args[1].as_str().unwrap_or_default());
                Ok(Value::Str(s.replace(old.as_str(), new.as_str())))
            }
            "startswith" | "endswith" => {
                if args.len() != 1 {
                    return Err(format!("str.{name}() takes 1 argument"));
                }
                let p = args[0].as_str().unwrap_or_default();
                Ok(Value::Bool(if name == "startswith" {
                    s.starts_with(p.as_str())
                } else {
                    s.ends_with(p.as_str())
                }))
            }
            "count" | "find" | "index" => {
                if args.len() != 1 {
                    return Err(format!("str.{name}() takes 1 argument"));
                }
                let sub = args[0].as_str().unwrap_or_default();
                match name {
                    "count" => Ok(Value::Int(s.matches(sub.as_str()).count() as i64)),
                    "find" => Ok(Value::Int(
                        s.find(sub.as_str()).map(|i| i as i64).unwrap_or(-1),
                    )),
                    _ => Ok(Value::Int(
                        s.find(sub.as_str()).map(|i| i as i64).unwrap_or(-1),
                    )),
                }
            }
            _ => Err(format!("unknown string method `{name}`")),
        };
    }
    // List methods.
    if let Value::List(items) = obj {
        return match name {
            "count" => {
                if args.len() != 1 {
                    return Err("list.count() takes 1 argument".into());
                }
                Ok(Value::Int(
                    items.iter().filter(|v| values_eq_pub(v, &args[0])).count() as i64,
                ))
            }
            "index" => {
                if args.len() != 1 {
                    return Err("list.index() takes 1 argument".into());
                }
                Ok(Value::Int(
                    items
                        .iter()
                        .position(|v| values_eq_pub(v, &args[0]))
                        .map(|i| i as i64)
                        .unwrap_or(-1),
                ))
            }
            "join" => {
                if args.len() != 1 {
                    return Err("list.join() takes 1 argument (separator)".into());
                }
                let sep = args[0].as_str().unwrap_or_default();
                let parts: Vec<String> = items.iter().map(|v| v.to_string()).collect();
                Ok(Value::Str(parts.join(sep.as_str())))
            }
            "sorted" => no_args(name, args).and_then(|_| {
                let mut sorted = items.clone();
                sorted.sort_by(|a, b| value_cmp(a, b).unwrap_or(std::cmp::Ordering::Equal));
                Ok(Value::List(sorted))
            }),
            "reversed" => no_args(name, args).map(|_| {
                let mut r = items.clone();
                r.reverse();
                Value::List(r)
            }),
            _ => Err(format!("unknown list method `{name}`")),
        };
    }
    // Dict methods.
    if let Value::Dict(map) = obj {
        return match name {
            "keys" => no_args(name, args).map(|_| {
                Value::List(map.keys().cloned().map(Value::Str).collect())
            }),
            "values" => no_args(name, args).map(|_| Value::List(map.values().cloned().collect())),
            "items" => no_args(name, args).map(|_| {
                Value::List(
                    map.iter()
                        .map(|(k, v)| Value::List(vec![Value::Str(k.clone()), v.clone()]))
                        .collect(),
                )
            }),
            "get" => {
                if args.is_empty() || args.len() > 2 {
                    return Err("dict.get() takes 1 or 2 arguments".into());
                }
                let key = args[0].as_str().unwrap_or_default();
                Ok(map
                    .get(key.as_str())
                    .cloned()
                    .unwrap_or_else(|| args.get(1).cloned().unwrap_or(Value::None)))
            }
            _ => Err(format!("unknown dict method `{name}`")),
        };
    }
    Err(format!("method `{name}` not supported on {obj}"))
}

fn no_args(name: &str, args: &[Value]) -> Result<(), String> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("method `{name}` takes no arguments"))
    }
}

/// Builtin expression functions. In addition to the PRNG helpers
/// (`random`, `randint`, `srand`), this covers conversions, collection
/// utilities and string helpers. User functions are resolved by the
/// caller when the builtin lookup fails.
fn eval_builtin(
    name: &str,
    args: &[Expr],
    vars: &HashMap<String, Value>,
    fns: Option<&HashMap<String, FunctionDef>>,
    rng: &mut u64,
) -> Result<Value, String> {
    // Evaluate all argument expressions up front for the builtins that
    // need values rather than syntax.
    fn eval_args(
        args: &[Expr],
        vars: &HashMap<String, Value>,
        fns: Option<&HashMap<String, FunctionDef>>,
        rng: &mut u64,
    ) -> Result<Vec<Value>, String> {
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            vals.push(eval_expr_ctx(a, vars, fns, rng)?);
        }
        Ok(vals)
    }
    let arity = |n: usize| -> Result<(), String> {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!("{name}() takes {n} argument(s), got {}", args.len()))
        }
    };
    match name {
        "random" => {
            arity(0)?;
            Ok(Value::Float(next_f64(rng)))
        }
        "randint" => {
            arity(2)?;
            let vals = eval_args(args, vars, fns, rng)?;
            let lo = vals[0].as_int().ok_or("randint() bounds must be integers")?;
            let hi = vals[1].as_int().ok_or("randint() bounds must be integers")?;
            if lo > hi {
                return Err(format!("randint() invalid range {lo}..{hi}"));
            }
            let span = (hi - lo + 1) as u64;
            Ok(Value::Int(lo + (next_u64(rng) % span) as i64))
        }
        "srand" => {
            arity(1)?;
            let vals = eval_args(args, vars, fns, rng)?;
            let seed = vals[0]
                .as_int()
                .ok_or("srand() seed must be an integer")?;
            *rng = (seed as u64) | 1;
            Ok(Value::Int(seed))
        }
        "len" => {
            arity(1)?;
            let vals = eval_args(args, vars, fns, rng)?;
            match &vals[0] {
                Value::List(l) => Ok(Value::Int(l.len() as i64)),
                Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
                Value::Dict(d) => Ok(Value::Int(d.len() as i64)),
                _ => Err("len() argument must be a list, string or dict".into()),
            }
        }
        "str" => {
            arity(1)?;
            let vals = eval_args(args, vars, fns, rng)?;
            Ok(Value::Str(vals[0].to_string()))
        }
        "int" => {
            arity(1)?;
            let vals = eval_args(args, vars, fns, rng)?;
            match &vals[0] {
                Value::Int(i) => Ok(Value::Int(*i)),
                Value::Float(f) => Ok(Value::Int(*f as i64)),
                Value::Bool(b) => Ok(Value::Int(*b as i64)),
                Value::Str(s) => s
                    .trim()
                    .parse::<i64>()
                    .map(Value::Int)
                    .map_err(|_| format!("cannot convert `{s}` to int")),
                _ => Err("cannot convert to int".into()),
            }
        }
        "float" => {
            arity(1)?;
            let vals = eval_args(args, vars, fns, rng)?;
            match &vals[0] {
                Value::Int(i) => Ok(Value::Float(*i as f64)),
                Value::Float(f) => Ok(Value::Float(*f)),
                Value::Bool(b) => Ok(Value::Float(*b as i64 as f64)),
                Value::Str(s) => s
                    .trim()
                    .parse::<f64>()
                    .map(Value::Float)
                    .map_err(|_| format!("cannot convert `{s}` to float")),
                _ => Err("cannot convert to float".into()),
            }
        }
        "bool" => {
            arity(1)?;
            let vals = eval_args(args, vars, fns, rng)?;
            Ok(Value::Bool(vals[0].is_truthy()))
        }
        "abs" => {
            arity(1)?;
            let vals = eval_args(args, vars, fns, rng)?;
            match &vals[0] {
                Value::Int(i) => Ok(Value::Int(i.abs())),
                Value::Float(f) => Ok(Value::Float(f.abs())),
                _ => Err("abs() argument must be a number".into()),
            }
        }
        "min" | "max" => {
            if args.is_empty() {
                return Err(format!("{name}() takes at least 1 argument"));
            }
            let vals = eval_args(args, vars, fns, rng)?;
            // `min(list)` / `max(list)` fold a single list argument.
            let items: &[Value] = if vals.len() == 1 {
                match &vals[0] {
                    Value::List(l) => l,
                    v => std::slice::from_ref(v),
                }
            } else {
                &vals
            };
            if items.is_empty() {
                return Err(format!("{name}() on empty sequence"));
            }
            let mut best = &items[0];
            for v in &items[1..] {
                let ord = value_cmp(v, best)?;
                if (name == "min" && ord == std::cmp::Ordering::Less)
                    || (name == "max" && ord == std::cmp::Ordering::Greater)
                {
                    best = v;
                }
            }
            Ok(best.clone())
        }
        "round" => {
            arity(1)?;
            let vals = eval_args(args, vars, fns, rng)?;
            match &vals[0] {
                Value::Int(i) => Ok(Value::Int(*i)),
                Value::Float(f) => Ok(Value::Int(f.round() as i64)),
                _ => Err("round() argument must be a number".into()),
            }
        }
        "choice" => {
            arity(1)?;
            let vals = eval_args(args, vars, fns, rng)?;
            match &vals[0] {
                Value::List(items) if !items.is_empty() => {
                    let i = (next_u64(rng) % items.len() as u64) as usize;
                    Ok(items[i].clone())
                }
                Value::List(_) => Err("choice() on empty list".into()),
                _ => Err("choice() argument must be a list".into()),
            }
        }
        "sample" => {
            arity(2)?;
            let vals = eval_args(args, vars, fns, rng)?;
            let (Value::List(items), Some(n)) = (&vals[0], vals[1].as_int()) else {
                return Err("sample() takes (list, n)".into());
            };
            let mut pool = items.clone();
            let n = (n.max(0) as usize).min(pool.len());
            for i in 0..n {
                let j = i + (next_u64(rng) % (pool.len() - i) as u64) as usize;
                pool.swap(i, j);
            }
            pool.truncate(n);
            Ok(Value::List(pool))
        }
        "shuffle" => {
            arity(1)?;
            let vals = eval_args(args, vars, fns, rng)?;
            match &vals[0] {
                Value::List(items) => {
                    let mut out = items.clone();
                    for i in (1..out.len()).rev() {
                        let j = (next_u64(rng) % (i as u64 + 1)) as usize;
                        out.swap(i, j);
                    }
                    Ok(Value::List(out))
                }
                _ => Err("shuffle() argument must be a list".into()),
            }
        }
        "range" => {
            if args.is_empty() || args.len() > 3 {
                return Err("range() takes 1 to 3 arguments".into());
            }
            let vals = eval_args(args, vars, fns, rng)?;
            let ints: Vec<i64> = vals
                .iter()
                .map(|v| v.as_int().ok_or("range() bounds must be integers"))
                .collect::<Result<_, _>>()?;
            let (start, end, step) = match ints.as_slice() {
                [n] => (0, *n, 1),
                [a, b] => (*a, *b, 1),
                [a, b, s] => (*a, *b, *s),
                _ => unreachable!(),
            };
            if step == 0 {
                return Err("range() step cannot be 0".into());
            }
            let mut out = Vec::new();
            let mut cur = start;
            while (step > 0 && cur < end) || (step < 0 && cur > end) {
                out.push(Value::Int(cur));
                cur += step;
                if out.len() > 1_000_000 {
                    return Err("range() result too large".into());
                }
            }
            Ok(Value::List(out))
        }
        "sorted" => {
            arity(1)?;
            let vals = eval_args(args, vars, fns, rng)?;
            match &vals[0] {
                Value::List(items) => {
                    let mut sorted = items.clone();
                    sorted.sort_by(|a, b| {
                        value_cmp(a, b).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    Ok(Value::List(sorted))
                }
                _ => Err("sorted() argument must be a list".into()),
            }
        }
        "reversed" => {
            arity(1)?;
            let vals = eval_args(args, vars, fns, rng)?;
            match &vals[0] {
                Value::List(items) => {
                    let mut r = items.clone();
                    r.reverse();
                    Ok(Value::List(r))
                }
                Value::Str(s) => Ok(Value::Str(s.chars().rev().collect())),
                _ => Err("reversed() argument must be a list or string".into()),
            }
        }
        "keys" | "values" | "items" => {
            arity(1)?;
            let vals = eval_args(args, vars, fns, rng)?;
            match &vals[0] {
                Value::Dict(map) => Ok(match name {
                    "keys" => Value::List(
                        map.keys().cloned().map(Value::Str).collect(),
                    ),
                    "values" => Value::List(map.values().cloned().collect()),
                    _ => Value::List(
                        map.iter()
                            .map(|(k, v)| Value::List(vec![Value::Str(k.clone()), v.clone()]))
                            .collect(),
                    ),
                }),
                _ => Err(format!("{name}() argument must be a dict")),
            }
        }
        "has" => {
            arity(2)?;
            let vals = eval_args(args, vars, fns, rng)?;
            match (&vals[0], &vals[1]) {
                (Value::Dict(map), Value::Str(k)) => Ok(Value::Bool(map.contains_key(k))),
                (Value::List(items), v) => Ok(Value::Bool(items.iter().any(|i| values_eq_pub(i, v)))),
                (Value::Str(s), Value::Str(sub)) => Ok(Value::Bool(s.contains(sub.as_str()))),
                _ => Err("has() unsupported operand types".into()),
            }
        }
        "get" => {
            if args.len() < 2 || args.len() > 3 {
                return Err("get() takes 2 or 3 arguments (dict, key[, default])".into());
            }
            let vals = eval_args(args, vars, fns, rng)?;
            match &vals[0] {
                Value::Dict(map) => {
                    let key = vals[1].as_str().unwrap_or_default();
                    Ok(map
                        .get(key.as_str())
                        .cloned()
                        .unwrap_or_else(|| vals.get(2).cloned().unwrap_or(Value::None)))
                }
                _ => Err("get() first argument must be a dict".into()),
            }
        }
        // String helpers (also available as methods): first argument is
        // the receiver, the rest are method arguments.
        "upper" | "lower" | "strip" | "lstrip" | "rstrip" | "split" | "replace"
        | "startswith" | "endswith" | "count" | "find" | "index" | "join" => {
            let vals = eval_args(args, vars, fns, rng)?;
            let Some((obj, rest)) = vals.split_first() else {
                return Err(format!("{name}() takes at least 1 argument"));
            };
            eval_method(obj, name, rest)
        }
        _ => Err(format!("unknown function `{name}()`")),
    }
}

/// Shared equality for builtins/methods (mirrors `values_eq`).
#[allow(dead_code)]
fn values_eq_pub(a: &Value, b: &Value) -> bool {
    values_eq(a, b)
}

/// Ordering for `sorted`/`min`/`max`: numbers compare numerically,
/// strings lexicographically, bools false<true, lists lexicographically.
fn value_cmp(a: &Value, b: &Value) -> Result<std::cmp::Ordering, String> {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok(x.cmp(y)),
        (Value::Float(x), Value::Float(y)) => Ok(x.partial_cmp(y).unwrap_or(Ordering::Equal)),
        (Value::Int(x), Value::Float(y)) => Ok((*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)),
        (Value::Float(x), Value::Int(y)) => Ok(x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)),
        (Value::Str(x), Value::Str(y)) => Ok(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Ok(x.cmp(y)),
        (Value::List(x), Value::List(y)) => {
            for (xi, yi) in x.iter().zip(y.iter()) {
                let ord = value_cmp(xi, yi)?;
                if ord != Ordering::Equal {
                    return Ok(ord);
                }
            }
            Ok(x.len().cmp(&y.len()))
        }
        _ => Err(format!("cannot compare {a} and {b}")),
    }
}

/// Map a layer name from script text to a [`Layer`].
fn parse_layer(name: &str) -> Layer {
    match name {
        "background" => Layer::Background,
        "midground" => Layer::Midground,
        "overlay" => Layer::Overlay,
        _ => Layer::Foreground,
    }
}

/// Convert a script [`Value`] to plain JSON for the plugin ABI.
fn value_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Str(s) => serde_json::Value::String(s.clone()),
        Value::Int(i) => serde_json::json!(*i),
        Value::Float(f) => serde_json::json!(*f),
        Value::Bool(b) => serde_json::json!(*b),
        Value::None => serde_json::Value::Null,
        Value::List(items) => serde_json::Value::Array(items.iter().map(value_to_json).collect()),
        Value::Dict(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), value_to_json(v)))
                .collect(),
        ),
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

/// Binary operator evaluation, shared with the function interpreter
/// (`funcs.rs`).
pub(crate) fn eval_binop_pub(op: BinaryOp, l: Value, r: Value) -> Result<Value, String> {
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
            (Value::Dict(mut a), Value::Dict(b)) => {
                a.extend(b);
                Value::Dict(a)
            }
            _ => return Err("invalid operands for `+`".into()),
        },
        Sub => num_op(l, r, |a, b| a - b, |a, b| a - b)?,
        Mul => match (&l, &r) {
            // `"ab" * 3` → "ababab" (string repetition).
            (Value::Str(s), Value::Int(n)) | (Value::Int(n), Value::Str(s)) => {
                if *n <= 0 {
                    Value::Str(String::new())
                } else {
                    Value::Str(s.repeat(*n as usize))
                }
            }
            _ => num_op(l, r, |a, b| a * b, |a, b| a * b)?,
        },
        Div => {
            if is_zero(&r) {
                return Err("division by zero".into());
            }
            num_op(l, r, |a, b| a.wrapping_div(b), |a, b| a / b)?
        }
        Mod => {
            if is_zero(&r) {
                return Err("division by zero".into());
            }
            num_op(l, r, |a, b| a.wrapping_rem(b), |a, b| a % b)?
        }
        Eq => Value::Bool(values_eq(&l, &r)),
        Neq => Value::Bool(!values_eq(&l, &r)),
        Lt => Value::Bool(cmp(&l, &r)? == std::cmp::Ordering::Less),
        Gt => Value::Bool(cmp(&l, &r)? == std::cmp::Ordering::Greater),
        Le => Value::Bool(cmp(&l, &r)? != std::cmp::Ordering::Greater),
        Ge => Value::Bool(cmp(&l, &r)? != std::cmp::Ordering::Less),
        In => Value::Bool(contains(&l, &r)?),
        NotIn => Value::Bool(!contains(&l, &r)?),
        Is => Value::Bool(values_eq(&l, &r)),
        IsNot => Value::Bool(!values_eq(&l, &r)),
        And => Value::Bool(l.is_truthy() && r.is_truthy()),
        Or => Value::Bool(l.is_truthy() || r.is_truthy()),
    })
}

fn is_zero(v: &Value) -> bool {
    matches!(v, Value::Int(0)) || matches!(v, Value::Float(f) if *f == 0.0)
}

/// `a in b` membership: list items (by equality), string substring,
/// dict keys.
fn contains(item: &Value, container: &Value) -> Result<bool, String> {
    match container {
        Value::List(items) => Ok(items.iter().any(|v| values_eq(v, item))),
        Value::Str(s) => match item {
            Value::Str(sub) => Ok(s.contains(sub.as_str())),
            _ => Err("`in` on a string requires a string operand".into()),
        },
        Value::Dict(map) => match item {
            Value::Str(k) => Ok(map.contains_key(k)),
            _ => Err("`in` on a dict requires a string key".into()),
        },
        _ => Err("`in` requires a list, string or dict".into()),
    }
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
        (Value::List(a), Value::List(b)) => a == b,
        (Value::Dict(a), Value::Dict(b)) => a == b,
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
