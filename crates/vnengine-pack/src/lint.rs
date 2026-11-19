//! Static analysis of parsed `.vn` scripts, run by `validate`/`build`:
//! unknown labels/screens/transforms/achievements, undefined speakers,
//! missing literal asset files, and unreachable labels.
//!
//! Everything reported here is a warning — dynamic targets
//! (`jump [expr]`, `show e [expr]`) legitimately defeat static checks,
//! so lint never fails a build.

use std::collections::BTreeSet;
use vnengine_core::assets::{AUDIO_EXTS, IMAGE_EXTS};
use vnengine_core::EngineConfig;
use vnengine_script::ast::{ButtonAction, Definition, Expr, ScreenStmt, Stmt};

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

/// Lint `script` against the normalized asset set (`dir/file`,
/// `/`-separated, relative to the project root) and engine config.
/// Returns human-readable warnings.
pub fn lint_script(
    script: &vnengine_script::ast::Script,
    config: &EngineConfig,
    assets: &BTreeSet<String>,
) -> Vec<String> {
    let mut w = Vec::new();
    let l = LintCtx::new(script, config, assets);

    // --- referenced-label / unreachable-label bookkeeping ---
    let mut referenced: BTreeSet<String> = BTreeSet::new();

    for label in &script.labels {
        lint_body(&l, &label.body, &mut w, &mut referenced);
    }
    for screen in &script.screens {
        lint_screen_body(&l, &screen.body, &mut w, &mut referenced);
    }

    for label in &script.labels {
        if label.name != "start" && !referenced.contains(&label.name) {
            w.push(format!(
                "unreachable label `{}` (no jump/call targets it)",
                label.name
            ));
        }
    }
    w
}

struct LintCtx<'a> {
    script: &'a vnengine_script::ast::Script,
    config: &'a EngineConfig,
    assets: &'a BTreeSet<String>,
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
        script: &'a vnengine_script::ast::Script,
        config: &'a EngineConfig,
        assets: &'a BTreeSet<String>,
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
        ctx
    }

    /// `true` when `dir/name` or `dir/name.<ext>` exists in the index —
    /// mirrors `AssetSource::read_any`.
    fn has_asset(&self, dir: &str, name: &str, exts: &[&str]) -> bool {
        let rel = format!("{dir}/{name}");
        if self.assets.contains(&rel) {
            return true;
        }
        exts.iter()
            .any(|e| self.assets.contains(&format!("{rel}.{e}")))
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

    fn check_label_target(&self, e: &Expr, w: &mut Vec<String>, refs: &mut BTreeSet<String>) {
        let Some(name) = self.literal(e) else { return };
        if !self.labels.contains(name) {
            w.push(format!("jump/call to unknown label `{name}`"));
        } else {
            refs.insert(name.to_string());
        }
    }

    fn check_screen_target(&self, e: &Expr, w: &mut Vec<String>) {
        let Some(name) = self.literal(e) else { return };
        if !self.screens.contains(name) {
            w.push(format!("unknown screen `{name}`"));
        }
    }

    fn check_audio(&self, e: &Expr, dir: &str, w: &mut Vec<String>) {
        let Some(track) = self.literal(e) else { return };
        if !self.has_asset(dir, track, AUDIO_EXTS) {
            w.push(format!("missing audio asset `{dir}/{track}`"));
        }
    }

    fn check_image(&self, e: &Expr, w: &mut Vec<String>) {
        let Some(img) = self.literal(e) else { return };
        // Composite `image <name>:` defs count as resolvable too.
        if self.script.find_image(img).is_some() {
            return;
        }
        if !self.has_asset(&self.config.image_dir, img, IMAGE_EXTS) {
            w.push(format!("missing image asset `{img}`"));
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
                set_var, choices, ..
            } => {
                if let Some(v) = set_var {
                    vars.insert(v.clone());
                }
                for c in choices {
                    collect_vars(&c.body, vars);
                }
            }
            Stmt::CallScreen { set_var, .. } => {
                if let Some(v) = set_var {
                    vars.insert(v.clone());
                }
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

fn lint_body(l: &LintCtx, body: &[Stmt], w: &mut Vec<String>, refs: &mut BTreeSet<String>) {
    for stmt in body {
        match stmt {
            Stmt::Jump { label } | Stmt::Call { label } => l.check_label_target(label, w, refs),
            Stmt::Say { speaker, voice, .. } => {
                if let Some(sp) = speaker {
                    if !l.characters.contains(sp) {
                        w.push(format!("speaker `{sp}` is not a defined Character"));
                    }
                }
                if let Some(v) = voice {
                    l.check_audio(v, &l.config.voice_dir, w);
                }
            }
            Stmt::Scene { image, .. } => {
                if let Some(e) = image {
                    l.check_image(e, w);
                }
            }
            Stmt::Show { image, at, .. } => {
                l.check_image(image, w);
                for name in at {
                    if !ANCHORS.contains(&name.as_str()) && !l.transforms.contains(name) {
                        w.push(format!("unknown transform `{name}` in `at`"));
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
            Stmt::MusicLayer { track, .. } => {
                if let Some(t) = track {
                    l.check_audio(t, &l.config.audio_dir, w);
                }
            }
            Stmt::PlayChannel { track, .. } | Stmt::QueueChannel { track, .. } => {
                l.check_audio(track, &l.config.audio_dir, w)
            }
            Stmt::Grant { name } => {
                if let Some(n) = l.literal(name) {
                    if !l.achievements.contains(n) {
                        w.push(format!("grant of undeclared achievement `{n}`"));
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
    w: &mut Vec<String>,
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
