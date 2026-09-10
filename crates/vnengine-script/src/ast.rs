//! Abstract syntax tree for the `.vn` script language.

use serde::{Deserialize, Serialize};

/// A complete parsed script.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Script {
    /// Top-level definitions (characters, constants).
    pub definitions: Vec<Definition>,
    /// Top-level `default` statements.
    pub defaults: Vec<DefaultStmt>,
    /// Labels keyed by name.
    pub labels: Vec<Label>,
}

/// A `default <name> = <expr>` declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultStmt {
    pub name: String,
    pub value: Expr,
}

impl Script {
    pub fn find_label(&self, name: &str) -> Option<&Label> {
        self.labels.iter().find(|l| l.name == name)
    }

    pub fn find_label_index(&self, name: &str) -> Option<usize> {
        self.labels.iter().position(|l| l.name == name)
    }
}

/// A top-level definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Definition {
    /// `define id = Character("Name", color=...)`
    Character {
        id: String,
        name: String,
        color: Option<[f32; 3]>,
    },
    /// `define id = <value>`
    Constant { id: String, value: Expr },
}

/// A label: a named block of statements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub name: String,
    pub body: Vec<Stmt>,
}

/// A statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Stmt {
    /// `scene <image> [with <transition>]`
    Scene {
        image: Option<String>,
        transition: Option<TransitionSpec>,
    },
    /// `show <character> <image> [at <anchor>] [with <transition>]`
    Show {
        character: String,
        image: String,
        anchor: Option<AnchorExpr>,
        position: Option<(Expr, Expr)>,
        scale: Option<Expr>,
        z: Option<Expr>,
        transition: Option<TransitionSpec>,
    },
    /// `hide <character> [with <transition>]`
    Hide {
        character: String,
        transition: Option<TransitionSpec>,
    },
    /// `<speaker> "<text>"` or `"<text>"` (narration)
    Say {
        speaker: Option<String>,
        text: String,
        voice: Option<String>,
    },
    /// `menu [prompt]: <choices>`
    Menu {
        prompt: Option<String>,
        choices: Vec<MenuChoice>,
    },
    /// `jump <label>`
    Jump { label: String },
    /// `call <label>`
    Call { label: String },
    /// `return`
    Return,
    /// `if <cond>: ... [elif <cond>: ...] [else: ...]`
    If {
        branches: Vec<(Expr, Vec<Stmt>)>,
        else_branch: Option<Vec<Stmt>>,
    },
    /// `pause <seconds>`
    Pause { seconds: Expr },
    /// `play music "<track>" [fade <s>]`
    PlayMusic { track: String, fade: f32 },
    /// `stop music [fade <s>]`
    StopMusic { fade: f32 },
    /// `play sfx "<track>" [volume <v>]`
    PlaySfx { track: String, volume: f32 },
    /// `voice "<track>"`
    PlayVoice { track: String },
    /// `stop voice`
    StopVoice,
    /// `$ <name> = <expr>` or `set <name> = <expr>`
    Assign { name: String, value: Expr },
    /// `$ <name> += <expr>` etc.
    AssignOp { name: String, op: BinaryOp, value: Expr },
    /// `default <name> = <expr>` — only assigns if the variable doesn't exist.
    Default { name: String, value: Expr },
    /// `while <cond>: <body>`
    While { cond: Expr, body: Vec<Stmt> },
    /// `for <var> in <iterable>: <body>`
    For { var: String, iterable: Expr, body: Vec<Stmt> },
    /// `transform <character> <props>`
    Transform {
        character: String,
        rotation: Option<Expr>,
        scale: Option<Expr>,
        offset: Option<(Expr, Expr)>,
        opacity: Option<Expr>,
        layer: Option<String>,
    },
    /// `end`
    End,
}

/// A choice inside a menu.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuChoice {
    pub text: String,
    pub condition: Option<Expr>,
    pub body: Vec<Stmt>,
}

/// A transition with an optional duration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionSpec {
    pub name: String,
    pub duration: Option<f32>,
}

/// An anchor expression (may be a literal keyword or an expression).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnchorExpr {
    Left,
    CenterLeft,
    Center,
    CenterRight,
    Right,
    OffscreenLeft,
    OffscreenRight,
    Expr(Expr),
}

/// An expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    Var(String),
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    None,
    /// List literal: `[expr, expr, ...]`
    List(Vec<Expr>),
    /// Index access: `expr[expr]`
    Index { collection: Box<Expr>, index: Box<Expr> },
    UnaryOp { op: UnaryOp, operand: Box<Expr> },
    BinaryOp {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}
