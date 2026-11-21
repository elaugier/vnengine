//! Static analysis of `.vn` scripts, shared by `vnengine-pack --check-only`,
//! the WASM linter embedded in the VS Code extension, and any other host.
//!
//! Two entry points:
//! - [`lint_script`] checks an already-parsed [`Script`] and returns
//!   [`LintWarning`]s (message + an optional `needle` identifying the source
//!   token the warning is about).
//! - [`check_source`] parses source text, runs the lint and resolves each
//!   warning to a line/column, returning serializable [`Diagnostic`]s.
//!   Parse failures come back as a single `error` diagnostic.
//!
//! Everything reported here besides parse errors is a warning — dynamic
//! targets (`jump [expr]`, `show e [expr]`) legitimately defeat static
//! checks, so lint never fails a build.

use crate::ast::{ButtonAction, Definition, Expr, ScreenStmt, Stmt};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use vnengine_core::assets::{AUDIO_EXTS, IMAGE_EXTS};
use vnengine_core::EngineConfig;

/// All known positional anchors — `at` names outside this set must be
/// `transform` definitions.
const ANCHORS: &[&str] = &[
    "left",
    "centerleft",
    "center",
    "centerright",
    "right",
    "offscreenleft",
    "offscreenright",
];

/// Diagnostic severity, serialized lowercase for JSON consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// A positioned, serializable diagnostic — the common currency of
/// `--check-only --json` and the editor extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    /// 1-based line in the source file, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// 0-based column (in characters) of the offending token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col: Option<usize>,
    /// Length of the offending token in characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<usize>,
}

impl Diagnostic {
    fn error(message: String, line: Option<usize>) -> Self {
        Self {
            severity: Severity::Error,
            message,
            line,
            col: None,
            span: None,
        }
    }
}

/// A lint finding before position resolution. `needle` is the source token
/// the warning is about (a label, asset path, speaker id, ...); `def_site`
/// asks [`locate`] to prefer the token's *definition* line over its first
/// use (used for "unreachable label" reports). `file` is the
/// project-relative source file the warning originates from, when the
/// script was parsed with `Parser::parse_multi`.
#[derive(Debug, Clone)]
pub struct LintWarning {
    pub message: String,
    pub needle: Option<String>,
    pub def_site: bool,
    pub file: Option<String>,
}

impl LintWarning {
    fn usage(message: String, needle: impl Into<String>) -> Self {
        Self {
            message,
            needle: Some(needle.into()),
            def_site: false,
            file: None,
        }
    }

    fn definition(message: String, needle: impl Into<String>) -> Self {
        Self {
            message,
            needle: Some(needle.into()),
            def_site: true,
            file: None,
        }
    }
}

/// Knobs for hosts that cannot check everything — e.g. a `.vn` file opened
/// outside a project has no `project.toml` (no declared characters) and no
/// asset tree to validate paths against.
#[derive(Debug, Clone, Copy)]
pub struct LintOptions {
    /// Check `scene`/`show`/`play`/`voice` literals against the asset set.
    pub check_assets: bool,
    /// Warn on speakers that are neither `Character(...)` definitions nor
    /// `project.toml` characters.
    pub check_speakers: bool,
}

impl Default for LintOptions {
    fn default() -> Self {
        Self {
            check_assets: true,
            check_speakers: true,
        }
    }
}

/// Parse `source`, then lint it against `config` and the normalized asset
/// set (`dir/file`, `/`-separated). Pass `assets: None` to skip asset
/// existence checks entirely.
pub fn check_source(
    source: &str,
    config: &EngineConfig,
    assets: Option<&BTreeSet<String>>,
    opts: &LintOptions,
) -> Vec<Diagnostic> {
    let script = match crate::Parser::parse(source) {
        Ok(s) => s,
        Err(e) => {
            return vec![Diagnostic::error(e.message(), Some(e.line()))];
        }
    };
    lint_script(&script, config, assets, opts)
        .into_iter()
        .map(|w| {
            let pos = w
                .needle
                .as_deref()
                .and_then(|n| locate(source, n, w.def_site));
            Diagnostic {
                severity: Severity::Warning,
                message: w.message,
                line: pos.map(|p| p.0),
                col: pos.map(|p| p.1),
                span: pos.map(|p| p.2),
            }
        })
        .collect()
}

/// Multi-file variant of [`check_source`]: parses `entry_src` with
/// `Parser::parse_multi`, resolving `import` directives through
/// `resolver(resolved_path, importing_file)`. Returns diagnostics paired
/// with the project-relative file they were found in (parse errors carry
/// the file via [`crate::parser::ParseError::file`]; lint warnings carry
/// the file their AST item was parsed from and are only searched for
/// there).
pub fn check_source_multi(
    entry_src: &str,
    entry_rel: &str,
    mut resolver: impl FnMut(&str, &str) -> Result<String, String>,
    config: &EngineConfig,
    assets: Option<&BTreeSet<String>>,
    opts: &LintOptions,
) -> Vec<(String, Diagnostic)> {
    // Record every source file the resolver reads so we can locate
    // warning needles in the right file.
    let mut sources: Vec<(String, String)> = vec![(
        crate::parser::normalize_rel_path(entry_rel),
        entry_src.to_string(),
    )];
    let mut recording = |resolved: &str, cur: &str| -> Result<String, String> {
        let content = resolver(resolved, cur)?;
        sources.push((resolved.to_string(), content.clone()));
        Ok(content)
    };
    let script = match crate::Parser::parse_multi(entry_src, entry_rel, &mut recording) {
        Ok(s) => s,
        Err(e) => {
            return vec![(
                e.file().unwrap_or(entry_rel).to_string(),
                Diagnostic::error(e.message(), Some(e.line())),
            )];
        }
    };
    lint_script(&script, config, assets, opts)
        .into_iter()
        .map(|w| {
            let needle = w.needle.clone().unwrap_or_default();
            // The warning's file of origin is authoritative — searching
            // other files would misattribute needles that appear in
            // several sources. Warnings without recorded provenance
            // (single-file parses) scan the whole tree as before.
            let found: Option<(String, usize, usize, usize)> = match &w.file {
                Some(file) => sources
                    .iter()
                    .find(|(f, _)| f == file)
                    .and_then(|(_, src)| locate(src, &needle, w.def_site))
                    .map(|p| (file.clone(), p.0, p.1, p.2)),
                None => sources.iter().find_map(|(file, src)| {
                    locate(src, &needle, w.def_site).map(|p| (file.clone(), p.0, p.1, p.2))
                }),
            };
            let (file, pos) = found
                .map(|(f, l, c, s)| (f, (Some(l), Some(c), Some(s))))
                .unwrap_or_else(|| {
                    (
                        w.file.clone().unwrap_or_else(|| entry_rel.to_string()),
                        (None, None, None),
                    )
                });
            (
                file,
                Diagnostic {
                    severity: Severity::Warning,
                    message: w.message,
                    line: pos.0,
                    col: pos.1,
                    span: pos.2,
                },
            )
        })
        .collect()
}

/// Lint `script` against the optional asset set and engine config.
/// `assets: None` disables all asset-existence checks.
pub fn lint_script(
    script: &crate::ast::Script,
    config: &EngineConfig,
    assets: Option<&BTreeSet<String>>,
    opts: &LintOptions,
) -> Vec<LintWarning> {
    let mut w = Vec::new();
    let l = LintCtx::new(script, config, assets, opts);

    // --- referenced-label / unreachable-label bookkeeping ---
    let mut referenced: BTreeSet<String> = BTreeSet::new();

    for label in &script.labels {
        let base = w.len();
        lint_body(&l, &label.body, &mut w, &mut referenced);
        // Every statement in a body comes from the label's own file
        // (`import` is top-level only), so warnings raised here inherit
        // the label's recorded origin.
        for warning in &mut w[base..] {
            warning.file.clone_from(&label.file);
        }
    }
    for screen in &script.screens {
        let base = w.len();
        lint_screen_body(&l, &screen.body, &mut w, &mut referenced);
        for warning in &mut w[base..] {
            warning.file.clone_from(&screen.file);
        }
    }

    for label in &script.labels {
        if label.name != "start" && !referenced.contains(&label.name) {
            w.push(LintWarning {
                file: label.file.clone(),
                ..LintWarning::definition(
                    format!(
                        "unreachable label `{}` (no jump/call targets it)",
                        label.name
                    ),
                    label.name.clone(),
                )
            });
        }
    }
    w
}

/// Find `needle` in `source`, returning `(line, col, len)` — 1-based line,
/// 0-based character column, needle length in characters. `def_site`
/// prefers definition lines (`label|screen|transform|image|achievement|
/// define|default|def <needle>`); usage needles skip those headers so the
/// first hit is the use site.
fn locate(source: &str, needle: &str, def_site: bool) -> Option<(usize, usize, usize)> {
    let is_def = |line: &str| {
        let t = line.trim_start();
        DEF_HEADERS
            .iter()
            .any(|h| t.starts_with(h) && t[h.len()..].starts_with(char::is_whitespace) || t == *h)
    };
    if def_site {
        for (i, line) in source.lines().enumerate() {
            if is_def(line) {
                if let Some(col) = find_word(line, needle) {
                    return Some((i + 1, col, needle.chars().count()));
                }
            }
        }
    }
    let mut fallback = None;
    for (i, line) in source.lines().enumerate() {
        if let Some(col) = find_word(line, needle) {
            let hit = (i + 1, col, needle.chars().count());
            if is_def(line) {
                fallback.get_or_insert(hit);
            } else {
                return Some(hit);
            }
        }
    }
    fallback
}

/// Header keywords whose lines count as "definition sites".
const DEF_HEADERS: &[&str] = &[
    "label",
    "screen",
    "transform",
    "image",
    "achievement",
    "define",
    "default",
    "def",
];

/// `true` when `c` can be part of an identifier/path token.
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '/' || c == '.' || c == '-'
}

/// First occurrence of `needle` in `line` as a whole word (character
/// column), or `None`.
fn find_word(line: &str, needle: &str) -> Option<usize> {
    let chars: Vec<char> = line.chars().collect();
    let nlen = needle.chars().count();
    if nlen == 0 || chars.len() < nlen {
        return None;
    }
    for start in 0..=chars.len() - nlen {
        let slice: String = chars[start..start + nlen].iter().collect();
        if slice != needle {
            continue;
        }
        let before_ok = start == 0 || !word_char(chars[start - 1]);
        let after_ok = start + nlen >= chars.len() || !word_char(chars[start + nlen]);
        if before_ok && after_ok {
            return Some(start);
        }
    }
    None
}

struct LintCtx<'a> {
    script: &'a crate::ast::Script,
    config: &'a EngineConfig,
    assets: Option<&'a BTreeSet<String>>,
    /// Names that can be used as variables — a `jump foo` where `foo` is
    /// a defined variable is dynamic and therefore unchecked.
    vars: BTreeSet<String>,
    labels: BTreeSet<String>,
    characters: BTreeSet<String>,
    screens: BTreeSet<String>,
    transforms: BTreeSet<String>,
    achievements: BTreeSet<String>,
}

impl<'a> LintCtx<'a> {
    fn new(
        script: &'a crate::ast::Script,
        config: &'a EngineConfig,
        assets: Option<&'a BTreeSet<String>>,
        opts: &LintOptions,
    ) -> Self {
        let mut vars = BTreeSet::new();
        let mut characters = BTreeSet::new();
        for d in &script.definitions {
            match d {
                Definition::Character { id, .. } => {
                    characters.insert(id.clone());
                    vars.insert(id.clone());
                }
                Definition::Constant { id, .. } => {
                    vars.insert(id.clone());
                }
            }
        }
        for c in &config.characters {
            characters.insert(c.id.clone());
            vars.insert(c.id.clone());
        }
        for d in &script.defaults {
            vars.insert(d.name.clone());
        }
        for f in &script.functions {
            vars.insert(f.name.clone());
        }
        let mut ctx = Self {
            script,
            config,
            assets,
            vars,
            labels: script.labels.iter().map(|l| l.name.clone()).collect(),
            characters,
            screens: script.screens.iter().map(|s| s.name.clone()).collect(),
            transforms: script.transforms.iter().map(|t| t.name.clone()).collect(),
            achievements: script.achievements.iter().map(|a| a.name.clone()).collect(),
        };
        // Gather variables bound anywhere in the script (assignments,
        // loop vars, `set`/`input`/`confirm` targets, label params).
        for label in &script.labels {
            for p in &label.params {
                ctx.vars.insert(p.clone());
            }
            collect_vars(&label.body, &mut ctx.vars);
        }
        if !opts.check_speakers {
            // No project.toml characters to compare against: treat every
            // speaker used as acceptable instead of warning on all of them.
            for label in &script.labels {
                collect_speakers(&label.body, &mut ctx.characters);
            }
        }
        ctx
    }

    /// `true` when `dir/name` or `dir/name.<ext>` exists in the index —
    /// mirrors `AssetSource::read_any`. Asset checks are skipped when no
    /// asset set was provided.
    fn has_asset(&self, dir: &str, name: &str, exts: &[&str]) -> bool {
        let Some(assets) = self.assets else {
            return true;
        };
        let rel = format!("{dir}/{name}");
        if assets.contains(&rel) {
            return true;
        }
        exts.iter().any(|e| assets.contains(&format!("{rel}.{e}")))
    }

    /// Evaluate a *literal* name expression like `eval_str` would for
    /// static analysis: `Str` always, `Var` only when it cannot be a
    /// variable (a bare ident is a literal name by convention).
    fn literal<'b>(&self, e: &'b Expr) -> Option<&'b str> {
        match e {
            Expr::Str(s) => Some(s),
            Expr::Var(n) if !self.vars.contains(n) => Some(n),
            _ => None,
        }
    }

    fn check_label_target(&self, e: &Expr, w: &mut Vec<LintWarning>, refs: &mut BTreeSet<String>) {
        let Some(name) = self.literal(e) else { return };
        if !self.labels.contains(name) {
            w.push(LintWarning::usage(
                format!("jump/call to unknown label `{name}`"),
                name,
            ));
        } else {
            refs.insert(name.to_string());
        }
    }

    fn check_screen_target(&self, e: &Expr, w: &mut Vec<LintWarning>) {
        let Some(name) = self.literal(e) else { return };
        if !self.screens.contains(name) {
            w.push(LintWarning::usage(format!("unknown screen `{name}`"), name));
        }
    }

    fn check_audio(&self, e: &Expr, dir: &str, w: &mut Vec<LintWarning>) {
        let Some(track) = self.literal(e) else { return };
        if !self.has_asset(dir, track, AUDIO_EXTS) {
            w.push(LintWarning::usage(
                format!("missing audio asset `{dir}/{track}`"),
                track,
            ));
        }
    }

    fn check_image(&self, e: &Expr, w: &mut Vec<LintWarning>) {
        let Some(img) = self.literal(e) else { return };
        // Composite `image <name>:` defs count as resolvable too.
        if self.script.find_image(img).is_some() {
            return;
        }
        if !self.has_asset(&self.config.image_dir, img, IMAGE_EXTS) {
            w.push(LintWarning::usage(
                format!("missing image asset `{img}`"),
                img,
            ));
        }
    }
}

fn collect_vars(body: &[Stmt], vars: &mut BTreeSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::Assign { target, .. } | Stmt::AssignOp { target, .. } => {
                if let Some(root) = target_root(target) {
                    vars.insert(root);
                }
            }
            Stmt::Default { name, .. } => {
                vars.insert(name.clone());
            }
            Stmt::For { var, body, .. } => {
                vars.insert(var.clone());
                collect_vars(body, vars);
            }
            Stmt::If {
                branches,
                else_branch,
            } => {
                for (_, b) in branches {
                    collect_vars(b, vars);
                }
                if let Some(b) = else_branch {
                    collect_vars(b, vars);
                }
            }
            Stmt::While { body, .. } => collect_vars(body, vars),
            Stmt::Menu {
                set_var: Some(v),
                choices,
                ..
            } => {
                vars.insert(v.clone());
                for c in choices {
                    collect_vars(&c.body, vars);
                }
            }
            Stmt::Menu { choices, .. } => {
                for c in choices {
                    collect_vars(&c.body, vars);
                }
            }
            Stmt::CallScreen {
                set_var: Some(v), ..
            } => {
                vars.insert(v.clone());
            }
            Stmt::Input { var, .. } | Stmt::Confirm { var, .. } => {
                vars.insert(var.clone());
            }
            _ => {}
        }
    }
}

/// Root variable name of an assignment target (`d.k[0] = v` → `d`).
fn target_root(target: &Expr) -> Option<String> {
    match target {
        Expr::Var(n) => Some(n.clone()),
        Expr::Attr { object, .. }
        | Expr::Index {
            collection: object, ..
        } => target_root(object),
        _ => None,
    }
}

/// All speaker ids used in `Say` statements — used to disable speaker
/// checks when no `project.toml` character list is available.
fn collect_speakers(body: &[Stmt], out: &mut BTreeSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::Say {
                speaker: Some(s), ..
            } => {
                out.insert(s.clone());
            }
            Stmt::Menu { choices, .. } => {
                for c in choices {
                    collect_speakers(&c.body, out);
                }
            }
            Stmt::If {
                branches,
                else_branch,
            } => {
                for (_, b) in branches {
                    collect_speakers(b, out);
                }
                if let Some(b) = else_branch {
                    collect_speakers(b, out);
                }
            }
            Stmt::While { body, .. } | Stmt::For { body, .. } => collect_speakers(body, out),
            _ => {}
        }
    }
}

fn lint_body(l: &LintCtx, body: &[Stmt], w: &mut Vec<LintWarning>, refs: &mut BTreeSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::Jump { label } | Stmt::Call { label } => l.check_label_target(label, w, refs),
            Stmt::Say { speaker, voice, .. } => {
                if let Some(sp) = speaker {
                    if !l.characters.contains(sp) {
                        w.push(LintWarning::usage(
                            format!("speaker `{sp}` is not a defined Character"),
                            sp.clone(),
                        ));
                    }
                }
                if let Some(v) = voice {
                    l.check_audio(v, &l.config.voice_dir, w);
                }
            }
            Stmt::Scene { image: Some(e), .. } => {
                l.check_image(e, w);
            }
            Stmt::Show { image, at, .. } => {
                l.check_image(image, w);
                for name in at {
                    if !ANCHORS.contains(&name.as_str()) && !l.transforms.contains(name) {
                        w.push(LintWarning::usage(
                            format!("unknown transform `{name}` in `at`"),
                            name.clone(),
                        ));
                    }
                }
            }
            Stmt::Menu { choices, .. } => {
                for c in choices {
                    lint_body(l, &c.body, w, refs);
                }
            }
            Stmt::If {
                branches,
                else_branch,
            } => {
                for (_, b) in branches {
                    lint_body(l, b, w, refs);
                }
                if let Some(b) = else_branch {
                    lint_body(l, b, w, refs);
                }
            }
            Stmt::While { body, .. } | Stmt::For { body, .. } => lint_body(l, body, w, refs),
            Stmt::ShowScreen { screen }
            | Stmt::HideScreen { screen }
            | Stmt::CallScreen { screen, .. } => l.check_screen_target(screen, w),
            Stmt::PlayMusic { track, .. } | Stmt::PlaySfx { track, .. } => {
                l.check_audio(track, &l.config.audio_dir, w)
            }
            Stmt::PlayVoice { track } => l.check_audio(track, &l.config.voice_dir, w),
            Stmt::MusicLayer { track: Some(t), .. } => {
                l.check_audio(t, &l.config.audio_dir, w);
            }
            Stmt::PlayChannel { track, .. } | Stmt::QueueChannel { track, .. } => {
                l.check_audio(track, &l.config.audio_dir, w)
            }
            Stmt::Grant { name } => {
                if let Some(n) = l.literal(name) {
                    if !l.achievements.contains(n) {
                        w.push(LintWarning::usage(
                            format!("grant of undeclared achievement `{n}`"),
                            n,
                        ));
                    }
                }
            }
            _ => {}
        }
    }
}

fn lint_screen_body(
    l: &LintCtx,
    body: &[ScreenStmt],
    w: &mut Vec<LintWarning>,
    refs: &mut BTreeSet<String>,
) {
    for stmt in body {
        match stmt {
            ScreenStmt::If { body, .. } | ScreenStmt::For { body, .. } => {
                lint_screen_body(l, body, w, refs)
            }
            ScreenStmt::Image { image, .. } => {
                l.check_image(image, w);
            }
            ScreenStmt::Button { actions, .. } => {
                for a in actions {
                    match a {
                        ButtonAction::Jump(e) => l.check_label_target(e, w, refs),
                        ButtonAction::ShowScreen(e) | ButtonAction::HideScreen(e) => {
                            l.check_screen_target(e, w)
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}
