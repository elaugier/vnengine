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
    /// User-defined functions.
    pub functions: Vec<FunctionDef>,
    /// Named ATL transform definitions (`transform name:`).
    #[serde(default)]
    pub transforms: Vec<TransformDef>,
    /// Composite image definitions (`image name:`).
    #[serde(default)]
    pub images: Vec<ImageDef>,
}

impl Script {
    /// Find a transform definition by name.
    pub fn find_transform(&self, name: &str) -> Option<&TransformDef> {
        self.transforms.iter().find(|t| t.name == name)
    }

    /// Find a composite image definition by name.
    pub fn find_image(&self, name: &str) -> Option<&ImageDef> {
        self.images.iter().find(|i| i.name == name)
    }
}

/// A named ATL transform: `transform <name>: <body>`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformDef {
    pub name: String,
    pub body: Vec<AtlStmt>,
}

/// A composite image definition: `image <name>: <layers>`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageDef {
    pub name: String,
    pub layers: Vec<ImageLayerDef>,
}

/// One layer of an `image name:` composite. `when` gates the layer on
/// an expression evaluated at show time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageLayerDef {
    pub image: Expr,
    #[serde(default)]
    pub when: Option<Expr>,
    #[serde(default)]
    pub offset: Option<(Expr, Expr)>,
    #[serde(default)]
    pub zoom: Option<Expr>,
    #[serde(default)]
    pub alpha: Option<Expr>,
}

/// A statement inside an ATL `transform` block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AtlStmt {
    /// `[warper] <duration> <prop> <value> [<prop> <value> ...]` —
    /// tween the properties to their targets.
    Warp {
        warper: String,
        duration: Expr,
        props: Vec<(String, Expr)>,
    },
    /// `<prop> <value> [<prop> <value> ...]` — instant set.
    Set { props: Vec<(String, Expr)> },
    /// `pause <seconds>` — wait.
    Pause { seconds: Expr },
    /// `time <t>` — hold until the track elapsed time reaches `t`.
    Time { at: Expr },
    /// `parallel:` — the block compiles to an extra concurrent track.
    Parallel { body: Vec<AtlStmt> },
    /// `choice [weight]:` — pick one child via the seeded PRNG.
    Choice { weight: Option<Expr>, body: Vec<AtlStmt> },
    /// `repeat [n]` — wraps the *preceding* sibling list; the parser
    /// turns it into a track-level repeat.
    Repeat { count: Option<Expr> },
    /// `on show|replace|hide:` — event block.
    On { event: String, body: Vec<AtlStmt> },
}

/// A user-defined function: `def name(params): body`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
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
    /// Optional parameters: `label name(a, b):` — bound as variables
    /// when the label is invoked via `call name(arg, ...)`.
    #[serde(default)]
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
}

/// A statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Stmt {
    /// `scene <image> [with <transition>] [onlayer <layer>]` —
    /// `image` is an expression evaluating to a string (or `None`
    /// to clear).
    Scene {
        image: Option<Expr>,
        transition: Option<TransitionSpec>,
        /// Target layer (`onlayer <layer>`); `None` = default background.
        #[serde(default)]
        layer: Option<String>,
    },
    /// `show <character> <image> [at <anchor>|<transform>,...] [layer=<layer>] [with <transition>]`
    /// `image` is an expression evaluating to a string; `show e [expr]`
    /// appends the evaluated suffix to the character name. `at` accepts
    /// named anchors (`left`, `right`...) and named ATL transforms.
    Show {
        character: String,
        image: Expr,
        anchor: Option<AnchorExpr>,
        position: Option<(Expr, Expr)>,
        scale: Option<Expr>,
        z: Option<Expr>,
        layer: Option<String>,
        transition: Option<TransitionSpec>,
        /// ATL transform names from `at <name>` (`at left` stays in
        /// `anchor`; unknown names are treated as transforms).
        #[serde(default)]
        at: Vec<String>,
    },
    /// `hide <character> [with <transition>]` or bare `hide [with <transition>]`
    /// (equivalent to `hide all`).
    Hide {
        character: Option<String>,
        transition: Option<TransitionSpec>,
    },
    /// `<speaker> "<text>"` or `"<text>"` (narration)
    Say {
        speaker: Option<String>,
        text: String,
        voice: Option<Expr>,
        side_image: Option<Expr>,
        transition: Option<TransitionSpec>,
    },
    /// `menu [prompt] [with <trans>] [set <var>]: <choices>` — `set`
    /// stores the chosen index (0-based) into `<var>` on selection.
    Menu {
        prompt: Option<Expr>,
        #[serde(default)]
        transition: Option<TransitionSpec>,
        #[serde(default)]
        set_var: Option<String>,
        choices: Vec<MenuChoice>,
    },
    /// `jump <expr>` — the expression must evaluate to a label name.
    Jump { label: Expr },
    /// `call <expr>` — `call name` invokes a zero-arg user function or
    /// jumps to the label; `call f(args)` (parsed as a call expression)
    /// invokes a function with arguments; any other expression is
    /// evaluated to a label name.
    Call { label: Expr },
    /// `return` — exits the innermost `call` frame.
    Return,
    /// `return <expr>` — exits a user function with a value. Inside a
    /// label `call` the value is discarded.
    ReturnValue { value: Expr },
    /// `with <transition>` — standalone transition over the current scene.
    With { transition: TransitionSpec },
    /// `break` — exit the innermost loop.
    Break,
    /// `continue` — skip to the next loop iteration.
    Continue,
    /// `if <cond>: ... [elif <cond>: ...] [else: ...]`
    If {
        branches: Vec<(Expr, Vec<Stmt>)>,
        else_branch: Option<Vec<Stmt>>,
    },
    /// `pause [<seconds>]` — without a duration, waits for player input.
    Pause { seconds: Option<Expr> },
    /// `play music <expr> [fade <s>]`
    PlayMusic { track: Expr, fade: f32 },
    /// `stop music [fade <s>]`
    StopMusic { fade: f32 },
    /// `play sfx <expr> [volume <v>]`
    PlaySfx { track: Expr, volume: f32 },
    /// `voice <expr>`
    PlayVoice { track: Expr },
    /// `stop voice`
    StopVoice,
    /// `$ <name> = <expr>` or `set <name> = <expr>`
    Assign { name: String, value: Expr },
    /// `$ <name> += <expr>` etc.
    AssignOp {
        name: String,
        op: BinaryOp,
        value: Expr,
    },
    /// `default <name> = <expr>` — only assigns if the variable doesn't exist.
    Default { name: String, value: Expr },
    /// `while <cond>: <body>`
    While { cond: Expr, body: Vec<Stmt> },
    /// `for <var> in <iterable>: <body>`
    For {
        var: String,
        iterable: Expr,
        body: Vec<Stmt>,
    },
    /// `transform <character> <props>`
    Transform {
        character: String,
        rotation: Option<Expr>,
        scale: Option<Expr>,
        offset: Option<(Expr, Expr)>,
        opacity: Option<Expr>,
        layer: Option<String>,
    },
    /// light <name> x=<e> y=<e> radius=<e> intensity=<e> [color=#hex]
    Light {
        name: String,
        x: Expr,
        y: Expr,
        radius: Expr,
        intensity: Expr,
        color: Option<[f32; 3]>,
    },
    /// unlight <name>
    Unlight { name: String },
    /// ambient <r> <g> <b>
    Ambient { r: Expr, g: Expr, b: Expr },
    /// music layer "<n>" ["<track>"] [volume=<e>] [fade=<e>] — or `off`
    /// to remove the layer.
    MusicLayer {
        name: String,
        track: Option<Expr>,
        volume: Option<Expr>,
        fade: Option<Expr>,
        off: bool,
    },
    /// camera [x=<e>] [y=<e>] [zoom=<e>] [over=<e>]
    Camera {
        x: Option<Expr>,
        y: Option<Expr>,
        zoom: Option<Expr>,
        /// Animation duration in seconds (`over=`); absent = instant.
        over: Option<Expr>,
    },
    /// `def <name>(<params>): <body>` — user-defined function.
    Def {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    /// `shake intensity=<expr> duration=<expr>`
    Shake { intensity: Expr, duration: Expr },
    /// `plugin <name> [<expr>, ...]` — send a command to a WASM plugin.
    Plugin { name: String, args: Vec<Expr> },
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
    /// Dict literal: `{ "key": expr, ... }` (keys may be idents or strings).
    Dict(Vec<(String, Expr)>),
    /// Index access: `expr[expr]` — lists/strings by int, dicts by string.
    Index {
        collection: Box<Expr>,
        index: Box<Expr>,
    },
    /// Slice access: `expr[start:end]` — bounds optional (`a[:]`).
    Slice {
        collection: Box<Expr>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
    },
    /// Attribute access: `expr.name` — dict key lookup sugar.
    Attr {
        object: Box<Expr>,
        name: String,
    },
    /// Method call: `expr.name(args...)` — read-only methods on
    /// lists/strings/dicts (e.g. `name.upper()`, `items.count(x)`).
    MethodCall {
        object: Box<Expr>,
        name: String,
        args: Vec<Expr>,
    },
    /// Builtin or user-function call: `random()`, `len(x)`, `f(y)`.
    Call {
        name: String,
        args: Vec<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
    },
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
    /// `a in b` — membership (list items, string substring, dict keys).
    In,
    /// `a not in b`.
    NotIn,
    /// `a is b` — identity-ish comparison (mainly `x is None`).
    Is,
    /// `a is not b`.
    IsNot,
    And,
    Or,
}
