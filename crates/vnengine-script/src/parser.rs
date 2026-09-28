//! Recursive-descent, indentation-sensitive parser for `.vn` scripts.

use crate::ast::*;
use crate::lexer::{Lexer, Spanned, Token};
use std::collections::HashSet;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("line {line}: {message}")]
    Generic { line: usize, message: String },
    #[error("line {line}: unexpected token {got}, expected {expected}")]
    Unexpected {
        line: usize,
        got: String,
        expected: String,
    },
}

/// Split a token slice on top-level commas (commas inside parens,
/// brackets or braces don't split). An empty input yields no chunks.
fn split_commas(toks: &[Spanned]) -> Vec<&[Spanned]> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, t) in toks.iter().enumerate() {
        match t.token {
            Token::LParen | Token::LBracket => depth += 1,
            Token::RParen | Token::RBracket => depth -= 1,
            Token::Comma if depth == 0 => {
                if start < i {
                    out.push(&toks[start..i]);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < toks.len() {
        out.push(&toks[start..]);
    }
    out
}

fn err(line: usize, msg: impl Into<String>) -> ParseError {
    ParseError::Generic {
        line,
        message: msg.into(),
    }
}

/// Collect `key=<tokens>` pairs starting at `start`. A value runs until
/// the next `ident =` pair or end of line.
fn kv_pairs(
    toks: &[Spanned],
    start: usize,
    line_no: usize,
) -> Result<Vec<(String, Vec<Spanned>)>, ParseError> {
    let mut out = Vec::new();
    let mut i = start;
    while i < toks.len() {
        let key = expect_ident(&toks[i], line_no, "property name")?;
        i += 1;
        if i >= toks.len() || !matches!(toks[i].token, Token::Assign) {
            return Err(err(line_no, "expected `=` after property name"));
        }
        i += 1;
        let mut value = Vec::new();
        while i < toks.len() {
            if matches!(toks[i].token, Token::Ident(_))
                && i + 1 < toks.len()
                && matches!(toks[i + 1].token, Token::Assign)
            {
                break;
            }
            value.push(toks[i].clone());
            i += 1;
        }
        out.push((key, value));
    }
    Ok(out)
}

pub struct Parser {
    tokens: Vec<Spanned>,
    pos: usize,
}

/// A line of tokens with its indentation level.
struct Line {
    indent: usize,
    tokens: Vec<Spanned>,
    line_no: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Spanned>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub fn parse(src: &str) -> Result<Script, ParseError> {
        let tokens = Lexer::new(src)
            .tokenize()
            .map_err(|e| err(0, e.to_string()))?;
        let mut p = Parser::new(tokens);
        p.parse_script()
    }

    /// Parse a single expression string (for testing and REPL use).
    pub fn parse_expr(src: &str) -> Expr {
        let tokens = Lexer::new(src).tokenize().expect("lex error");
        // Skip leading Indent and trailing Newline tokens.
        let start = tokens
            .iter()
            .position(|t| !matches!(t.token, Token::Indent(_)))
            .unwrap_or(0);
        let end = tokens
            .iter()
            .rposition(|t| !matches!(t.token, Token::Newline))
            .unwrap_or(tokens.len().saturating_sub(1))
            + 1;
        let slice = &tokens[start..end];
        parse_expr_with_len(slice, 1).expect("parse error").0
    }

    fn peek(&self) -> Option<&Spanned> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<&Spanned> {
        let t = self.tokens.get(self.pos);
        self.pos += 1;
        t
    }

    fn parse_script(&mut self) -> Result<Script, ParseError> {
        let mut script = Script::default();
        let lines = self.collect_lines()?;

        let mut idx = 0;
        while idx < lines.len() {
            let line = &lines[idx];
            if line.tokens.is_empty() {
                idx += 1;
                continue;
            }
            match &line.tokens[0].token {
                Token::Define => {
                    let def = self.parse_definition(line)?;
                    script.definitions.push(def);
                    idx += 1;
                }
                Token::Default => {
                    let (d, _) = self.parse_default_stmt(&line.tokens, line.line_no)?;
                    script.defaults.push(d);
                    idx += 1;
                }
                Token::Def => {
                    let (func, consumed) = self.parse_def_stmt(&lines[idx..])?;
                    script.functions.push(func);
                    idx += consumed;
                }
                Token::Label => {
                    let (label, consumed) = self.parse_label(&lines[idx..])?;
                    script.labels.push(label);
                    idx += consumed;
                }
                _ => {
                    return Err(err(
                        line.line_no,
                        format!("unexpected top-level token {:?}", line.tokens[0].token),
                    ));
                }
            }
        }
        Ok(script)
    }

    /// Flatten the token stream into logical lines, each with its indent level.
    fn collect_lines(&mut self) -> Result<Vec<Line>, ParseError> {
        let mut lines = Vec::new();
        let mut current_indent = 0usize;
        let mut current: Vec<Spanned> = Vec::new();
        let mut line_no = 1;
        let mut started = false;

        while let Some(tok) = self.peek().cloned() {
            match &tok.token {
                Token::Indent(n) => {
                    current_indent = *n;
                    self.next();
                }
                Token::Newline => {
                    self.next();
                    if started {
                        lines.push(Line {
                            indent: current_indent,
                            tokens: std::mem::take(&mut current),
                            line_no,
                        });
                    }
                    current_indent = 0;
                    started = false;
                    line_no += 1;
                }
                _ => {
                    if !started {
                        // first token of the line: record its line number
                        line_no = tok.line;
                    }
                    started = true;
                    current.push(tok);
                    self.next();
                }
            }
        }
        if started {
            lines.push(Line {
                indent: current_indent,
                tokens: current,
                line_no,
            });
        }
        Ok(lines)
    }

    /// Parse `default <name> = <expr>`.
    fn parse_default_stmt(
        &self,
        toks: &[Spanned],
        line_no: usize,
    ) -> Result<(DefaultStmt, usize), ParseError> {
        // toks[0] = Default
        let name = expect_ident(&toks[1], line_no, "variable name")?;
        expect_token(&toks[2], Token::Assign, line_no, "=")?;
        let value = self.parse_expr_tokens(&toks[3..], line_no)?;
        Ok((DefaultStmt { name, value }, 1))
    }

    fn parse_definition(&self, line: &Line) -> Result<Definition, ParseError> {
        // define <ident> = Character("Name" [, color=(r,g,b)])
        // define <ident> = <expr>
        let toks = &line.tokens;
        // toks[0] = Define
        let id = expect_ident(&toks[1], line.line_no, "definition name")?;
        expect_token(&toks[2], Token::Assign, line.line_no, "=")?;
        // Character(...) form
        if let Some(t) = toks.get(3) {
            if matches!(t.token, Token::Ident(ref s) if s == "Character") {
                return self.parse_character_def(&id, &toks[3..], line.line_no);
            }
        }
        // Otherwise parse an expression for the constant value.
        let expr = self.parse_expr_tokens(&toks[3..], line.line_no)?;
        Ok(Definition::Constant { id, value: expr })
    }

    fn parse_character_def(
        &self,
        id: &str,
        toks: &[Spanned],
        line_no: usize,
    ) -> Result<Definition, ParseError> {
        // Character ( "Name" [, color = (r, g, b)] )
        // toks[0] = Ident("Character")
        expect_token(&toks[1], Token::LParen, line_no, "(")?;
        let name = expect_string(&toks[2], line_no, "character name")?;
        let mut color = None;
        let mut i = 3;
        while i < toks.len() && matches!(toks[i].token, Token::Comma) {
            i += 1;
            // expect ident "color"
            if let Some(t) = toks.get(i) {
                if let Token::Ident(ref s) = t.token {
                    if s == "color" {
                        i += 1;
                        expect_token(&toks[i], Token::Assign, line_no, "=")?;
                        i += 1;
                        // color = (r, g, b)  or  color = "#rrggbb"
                        if matches!(toks[i].token, Token::LParen) {
                            i += 1;
                            let r = parse_color_component(&toks[i], line_no)?;
                            i += 1;
                            expect_token(&toks[i], Token::Comma, line_no, ",")?;
                            i += 1;
                            let g = parse_color_component(&toks[i], line_no)?;
                            i += 1;
                            expect_token(&toks[i], Token::Comma, line_no, ",")?;
                            i += 1;
                            let b = parse_color_component(&toks[i], line_no)?;
                            i += 1;
                            expect_token(&toks[i], Token::RParen, line_no, ")")?;
                            i += 1;
                            color = Some([r, g, b]);
                        } else if let Token::StringLit(ref s) = toks[i].token {
                            color = parse_hex_color(s, line_no)?;
                            i += 1;
                        } else {
                            return Err(err(line_no, "expected color value"));
                        }
                        continue;
                    }
                }
            }
            // Unknown keyword argument: skip until comma or close.
            while i < toks.len() && !matches!(toks[i].token, Token::Comma | Token::RParen) {
                i += 1;
            }
        }
        Ok(Definition::Character {
            id: id.to_string(),
            name,
            color,
        })
    }

    /// Parse a label starting at `lines[0]`. Returns the label and the number
    /// of logical lines consumed.
    fn parse_label(&self, lines: &[Line]) -> Result<(Label, usize), ParseError> {
        let first = &lines[0];
        // label <name>:
        let name = expect_ident(&first.tokens[1], first.line_no, "label name")?;
        expect_token(&first.tokens[2], Token::Colon, first.line_no, ":")?;
        let base_indent = first.indent;
        let mut consumed = 1;
        let mut body = Vec::new();
        while consumed < lines.len() {
            let line = &lines[consumed];
            if line.tokens.is_empty() {
                consumed += 1;
                continue;
            }
            if line.indent <= base_indent {
                break;
            }
            let (stmt, extra) = self.parse_stmt(&lines[consumed..], line.indent)?;
            body.push(stmt);
            consumed += extra;
        }
        Ok((Label { name, body }, consumed))
    }

    /// Parse a single statement starting at `lines[0]`, which has indent
    /// `indent`. Returns the statement and how many logical lines it spans.
    fn parse_stmt(&self, lines: &[Line], indent: usize) -> Result<(Stmt, usize), ParseError> {
        let line = &lines[0];
        let toks = &line.tokens;
        if toks.is_empty() {
            return Err(err(line.line_no, "empty statement"));
        }
        let line_no = line.line_no;
        let head = &toks[0].token;
        match head {
            Token::Scene => self.parse_scene_stmt(toks, line_no),
            Token::Show => self.parse_show_stmt(toks, line_no),
            Token::Hide => self.parse_hide_stmt(toks, line_no),
            Token::Menu => self.parse_menu_stmt(lines, indent, line_no),
            Token::Jump => {
                let label = expect_ident(&toks[1], line_no, "label name")?;
                Ok((Stmt::Jump { label }, 1))
            }
            Token::Call => {
                let label = expect_ident(&toks[1], line_no, "label name")?;
                Ok((Stmt::Call { label }, 1))
            }
            Token::Return => Ok((Stmt::Return, 1)),
            Token::If => self.parse_if_stmt(lines, indent, line_no),
            Token::While => self.parse_while_stmt(lines, indent, line_no),
            Token::For => self.parse_for_stmt(lines, indent, line_no),
            Token::Transform => self.parse_transform_stmt(lines, indent, line_no),
            Token::Shake => self.parse_shake_stmt(toks, line_no),
            Token::Pause => {
                let expr = self.parse_expr_tokens(&toks[1..], line_no)?;
                Ok((Stmt::Pause { seconds: expr }, 1))
            }
            Token::Play => self.parse_play_stmt(toks, line_no),
            Token::Stop => self.parse_stop_stmt(toks, line_no),
            Token::Voice => {
                let track = expect_string(&toks[1], line_no, "voice track")?;
                Ok((Stmt::PlayVoice { track }, 1))
            }
            Token::End => Ok((Stmt::End, 1)),
            Token::Default => {
                let (d, _) = self.parse_default_stmt(toks, line_no)?;
                Ok((
                    Stmt::Default {
                        name: d.name,
                        value: d.value,
                    },
                    1,
                ))
            }
            Token::Dollar => {
                // $ name = expr  OR  $ name += expr  etc.
                let name = expect_ident(&toks[1], line_no, "variable name")?;
                if let Some(op) = compound_assign_op(&toks[2].token) {
                    let value = self.parse_expr_tokens(&toks[3..], line_no)?;
                    return Ok((Stmt::AssignOp { name, op, value }, 1));
                }
                expect_token(&toks[2], Token::Assign, line_no, "=")?;
                let value = self.parse_expr_tokens(&toks[3..], line_no)?;
                Ok((Stmt::Assign { name, value }, 1))
            }
            Token::Set => {
                let name = expect_ident(&toks[1], line_no, "variable name")?;
                if let Some(op) = compound_assign_op(&toks[2].token) {
                    let value = self.parse_expr_tokens(&toks[3..], line_no)?;
                    return Ok((Stmt::AssignOp { name, op, value }, 1));
                }
                expect_token(&toks[2], Token::Assign, line_no, "=")?;
                let value = self.parse_expr_tokens(&toks[3..], line_no)?;
                Ok((Stmt::Assign { name, value }, 1))
            }
            Token::Ident(s) if s == "plugin" => {
                // `plugin <name> [<expr>, <expr>, ...]`
                let name = expect_ident(&toks[1], line_no, "plugin name")?;
                let mut args = Vec::new();
                for chunk in split_commas(&toks[2..]) {
                    args.push(self.parse_expr_tokens(chunk, line_no)?);
                }
                Ok((Stmt::Plugin { name, args }, 1))
            }
            Token::Ident(s) if s == "light" => {
                // `light <name> x=<e> y=<e> radius=<e> intensity=<e> [color=#hex]`
                let name = match &toks.get(1).map(|t| &t.token) {
                    Some(Token::Ident(n)) | Some(Token::StringLit(n)) => n.clone(),
                    _ => return Err(err(line_no, "expected light name")),
                };
                let mut x = None;
                let mut y = None;
                let mut radius = None;
                let mut intensity = None;
                let mut color = None;
                for (k, vt) in kv_pairs(toks, 2, line_no)? {
                    match k.as_str() {
                        "x" => x = Some(self.parse_expr_tokens(&vt, line_no)?),
                        "y" => y = Some(self.parse_expr_tokens(&vt, line_no)?),
                        "radius" => radius = Some(self.parse_expr_tokens(&vt, line_no)?),
                        "intensity" => intensity = Some(self.parse_expr_tokens(&vt, line_no)?),
                        "color" => {
                            if let Some(Token::StringLit(h)) = vt.first().map(|t| &t.token) {
                                color = parse_hex_color(h, line_no)?;
                            }
                        }
                        _ => {}
                    }
                }
                let (x, y, radius, intensity) = match (x, y, radius, intensity) {
                    (Some(x), Some(y), Some(r), Some(i)) => (x, y, r, i),
                    _ => {
                        return Err(err(
                            line_no,
                            "light requires x=, y=, radius= and intensity=",
                        ))
                    }
                };
                Ok((
                    Stmt::Light {
                        name,
                        x,
                        y,
                        radius,
                        intensity,
                        color,
                    },
                    1,
                ))
            }
            Token::Ident(s) if s == "music" => {
                // `music layer "<n>" ["<track>"] [volume=<e>] [fade=<e>]`
                // `music layer "<n>" off [fade=<e>]`
                let sub = expect_ident(&toks[1], line_no, "`layer`")?;
                if sub != "layer" {
                    return Err(err(line_no, "expected `music layer`"));
                }
                let name = match toks.get(2).map(|t| &t.token) {
                    Some(Token::Ident(n)) | Some(Token::StringLit(n)) => n.clone(),
                    _ => return Err(err(line_no, "expected layer name")),
                };
                let mut i = 3;
                let mut track = None;
                let mut off = false;
                // Optional positional track (string) or `off` keyword.
                match toks.get(i).map(|t| &t.token) {
                    Some(Token::StringLit(t)) => {
                        track = Some(t.clone());
                        i += 1;
                    }
                    Some(Token::Ident(t)) if t == "off" => {
                        off = true;
                        i += 1;
                    }
                    _ => {}
                }
                let mut volume = None;
                let mut fade = None;
                for (k, vt) in kv_pairs(toks, i, line_no)? {
                    match k.as_str() {
                        "volume" => volume = Some(self.parse_expr_tokens(&vt, line_no)?),
                        "fade" => fade = Some(self.parse_expr_tokens(&vt, line_no)?),
                        _ => {}
                    }
                }
                Ok((
                    Stmt::MusicLayer {
                        name,
                        track,
                        volume,
                        fade,
                        off,
                    },
                    1,
                ))
            }
            Token::Ident(s) if s == "unlight" => {
                let name = match &toks.get(1).map(|t| &t.token) {
                    Some(Token::Ident(n)) | Some(Token::StringLit(n)) => n.clone(),
                    _ => return Err(err(line_no, "expected light name")),
                };
                Ok((Stmt::Unlight { name }, 1))
            }
            Token::Ident(s) if s == "ambient" => {
                // `ambient <r> <g> <b>` (three positional expressions)
                if toks.len() < 4 {
                    return Err(err(line_no, "ambient requires r g b"));
                }
                Ok((
                    Stmt::Ambient {
                        r: self.parse_expr_tokens(&toks[1..2], line_no)?,
                        g: self.parse_expr_tokens(&toks[2..3], line_no)?,
                        b: self.parse_expr_tokens(&toks[3..4], line_no)?,
                    },
                    1,
                ))
            }
            Token::Ident(s) if s == "camera" => {
                // `camera [x=<e>] [y=<e>] [zoom=<e>] [over=<e>]`
                let mut x = None;
                let mut y = None;
                let mut zoom = None;
                let mut over = None;
                for (k, vt) in kv_pairs(toks, 1, line_no)? {
                    match k.as_str() {
                        "x" => x = Some(self.parse_expr_tokens(&vt, line_no)?),
                        "y" => y = Some(self.parse_expr_tokens(&vt, line_no)?),
                        "zoom" => zoom = Some(self.parse_expr_tokens(&vt, line_no)?),
                        "over" => over = Some(self.parse_expr_tokens(&vt, line_no)?),
                        _ => {}
                    }
                }
                Ok((Stmt::Camera { x, y, zoom, over }, 1))
            }
            Token::Ident(s) if s == "stop" && toks.len() >= 2 => {
                // `stop voice` (lowercase ident fallback)
                if let Token::Ident(ref v) = toks[1].token {
                    if v == "voice" {
                        return Ok((Stmt::StopVoice, 1));
                    }
                }
                self.parse_say_stmt(toks, line_no)
            }
            Token::StringLit(text) => {
                // Narration: "text"
                Ok((
                    Stmt::Say {
                        speaker: None,
                        text: text.clone(),
                        voice: None,
                        side_image: None,
                    },
                    1,
                ))
            }
            Token::Ident(_) => {
                // Speaker line: <id> "text" [voice "track"]
                self.parse_say_stmt(toks, line_no)
            }
            _ => Err(err(line_no, format!("unexpected statement token {head:?}"))),
        }
    }

    fn parse_scene_stmt(
        &self,
        toks: &[Spanned],
        line_no: usize,
    ) -> Result<(Stmt, usize), ParseError> {
        // scene <image...> [with <transition>]
        // or `scene` alone (clear)
        let mut image_parts = Vec::new();
        let mut transition = None;
        let mut i = 1;
        // Collect image tokens until `with` keyword or end.
        while i < toks.len() {
            match &toks[i].token {
                Token::Ident(s) if s == "with" => break,
                Token::StringLit(s) => {
                    image_parts.push(s.clone());
                    i += 1;
                }
                Token::Ident(s) => {
                    image_parts.push(s.clone());
                    i += 1;
                }
                _ => break,
            }
        }
        let image = if image_parts.is_empty() {
            None
        } else {
            Some(image_parts.join(" "))
        };
        if let Some(t) = toks.get(i) {
            if matches!(t.token, Token::With) {
                i += 1;
                let (name, dur) = self.parse_transition(&toks[i..], line_no)?;
                transition = Some(TransitionSpec {
                    name,
                    duration: dur,
                });
            }
        }
        Ok((Stmt::Scene { image, transition }, 1))
    }

    fn parse_show_stmt(
        &self,
        toks: &[Spanned],
        line_no: usize,
    ) -> Result<(Stmt, usize), ParseError> {
        // show <character> <image> [at <anchor>] [with <transition>]
        // image may be a single ident or "char expr" (we join idents with space).
        let character = expect_ident(&toks[1], line_no, "character id")?;
        let mut i = 2;
        // Collect image tokens until a keyword (at/with) or end.
        let mut image_parts = Vec::new();
        // Prepend the character name so the full path is "character expression".
        image_parts.push(character.clone());
        while i < toks.len() {
            match &toks[i].token {
                Token::At | Token::With => break,
                Token::Ident(s) if s == "as" => break,
                Token::StringLit(s) => {
                    image_parts.push(s.clone());
                    i += 1;
                }
                Token::Ident(s) => {
                    image_parts.push(s.clone());
                    i += 1;
                }
                _ => break,
            }
        }
        let image = image_parts.join(" ");
        let mut anchor = None;
        let mut position = None;
        let mut scale = None;
        let mut z = None;
        let mut transition = None;
        while i < toks.len() {
            match &toks[i].token {
                Token::At => {
                    i += 1;
                    if let Some(t) = toks.get(i) {
                        if let Token::LParen = t.token {
                            // at (x, y)
                            let slice = &toks[i + 1..];
                            let (x, nx) = parse_expr_with_len(slice, line_no)?;
                            i += 1 + nx;
                            expect_token(&toks[i], Token::Comma, line_no, ",")?;
                            i += 1;
                            let slice = &toks[i..];
                            let (y, ny) = parse_expr_with_len(slice, line_no)?;
                            i += ny;
                            expect_token(&toks[i], Token::RParen, line_no, ")")?;
                            i += 1;
                            position = Some((x, y));
                        } else if let Token::Ident(ref a) = t.token {
                            anchor = Some(parse_anchor_ident(a, line_no)?);
                            i += 1;
                        } else {
                            return Err(err(line_no, "expected anchor after `at`"));
                        }
                    }
                }
                Token::With => {
                    i += 1;
                    let (name, dur) = self.parse_transition(&toks[i..], line_no)?;
                    transition = Some(TransitionSpec {
                        name,
                        duration: dur,
                    });
                    // consume transition token(s): single ident, optionally
                    // followed by a float duration.
                    i += 1;
                    if let Some(t) = toks.get(i) {
                        if let Token::FloatLit(_) | Token::IntLit(_) = t.token {
                            i += 1;
                        }
                    }
                }
                Token::Ident(s) if s == "scale" => {
                    i += 1;
                    let e = self.parse_expr_tokens(&toks[i..], line_no)?;
                    scale = Some(e);
                    i = toks.len();
                }
                Token::Ident(s) if s == "z" => {
                    i += 1;
                    let e = self.parse_expr_tokens(&toks[i..], line_no)?;
                    z = Some(e);
                    i = toks.len();
                }
                _ => break,
            }
        }
        Ok((
            Stmt::Show {
                character,
                image,
                anchor,
                position,
                scale,
                z,
                transition,
            },
            1,
        ))
    }

    fn parse_hide_stmt(
        &self,
        toks: &[Spanned],
        line_no: usize,
    ) -> Result<(Stmt, usize), ParseError> {
        let character = expect_ident(&toks[1], line_no, "character id")?;
        let mut transition = None;
        if let Some(t) = toks.get(2) {
            if matches!(t.token, Token::With) {
                let (name, dur) = self.parse_transition(&toks[3..], line_no)?;
                transition = Some(TransitionSpec {
                    name,
                    duration: dur,
                });
            }
        }
        Ok((
            Stmt::Hide {
                character,
                transition,
            },
            1,
        ))
    }

    fn parse_transition(
        &self,
        toks: &[Spanned],
        line_no: usize,
    ) -> Result<(String, Option<f32>), ParseError> {
        if toks.is_empty() {
            return Err(err(line_no, "expected transition name after `with`"));
        }
        let name = match &toks[0].token {
            Token::Ident(s) => s.clone(),
            Token::StringLit(s) => s.clone(),
            _ => return Err(err(line_no, "expected transition name")),
        };
        let mut duration = None;
        if let Some(t) = toks.get(1) {
            match t.token {
                Token::FloatLit(f) => duration = Some(f as f32),
                Token::IntLit(n) => duration = Some(n as f32),
                _ => {}
            }
        }
        Ok((name, duration))
    }

    fn parse_menu_stmt(
        &self,
        lines: &[Line],
        indent: usize,
        line_no: usize,
    ) -> Result<(Stmt, usize), ParseError> {
        // menu [prompt]:
        //     "choice text" [if cond]:
        //         <body>
        let first = &lines[0];
        let toks = &first.tokens;
        let mut prompt = None;
        if toks.len() >= 2 {
            if let Token::StringLit(ref s) = toks[1].token {
                prompt = Some(s.clone());
            }
        }
        // expect colon
        let colon_idx = if prompt.is_some() { 2 } else { 1 };
        if let Some(t) = toks.get(colon_idx) {
            expect_token(t, Token::Colon, line_no, ":")?;
        } else {
            return Err(err(line_no, "menu requires `:`"));
        }
        let base_indent = indent;
        let mut consumed = 1;
        let mut choices = Vec::new();
        while consumed < lines.len() {
            let line = &lines[consumed];
            if line.tokens.is_empty() {
                consumed += 1;
                continue;
            }
            if line.indent <= base_indent {
                break;
            }
            // Each choice is at indent > base_indent. Its body is at indent >
            // choice's indent.
            let choice_indent = line.indent;
            let text = expect_string(&line.tokens[0], line.line_no, "choice text")?;
            let mut condition = None;
            let mut j = 1;
            if let Some(t) = line.tokens.get(j) {
                if matches!(t.token, Token::If) {
                    j += 1;
                    let cond_toks: Vec<Spanned> = line.tokens[j..]
                        .iter()
                        .take_while(|t| !matches!(t.token, Token::Colon))
                        .cloned()
                        .collect();
                    condition = Some(self.parse_expr_tokens(&cond_toks, line.line_no)?);
                    j += cond_toks.len();
                }
            }
            // expect colon
            if let Some(t) = line.tokens.get(j) {
                expect_token(t, Token::Colon, line.line_no, ":")?;
            }
            consumed += 1;
            // Parse the choice body.
            let mut body = Vec::new();
            while consumed < lines.len() {
                let bl = &lines[consumed];
                if bl.tokens.is_empty() {
                    consumed += 1;
                    continue;
                }
                if bl.indent <= choice_indent {
                    break;
                }
                let (stmt, extra) = self.parse_stmt(&lines[consumed..], bl.indent)?;
                body.push(stmt);
                consumed += extra;
            }
            choices.push(MenuChoice {
                text,
                condition,
                body,
            });
        }
        Ok((Stmt::Menu { prompt, choices }, consumed))
    }

    fn parse_if_stmt(
        &self,
        lines: &[Line],
        indent: usize,
        _line_no: usize,
    ) -> Result<(Stmt, usize), ParseError> {
        // if <cond>:
        //     body
        // elif <cond>:
        //     body
        // else:
        //     body
        let mut branches = Vec::new();
        let mut else_branch = None;
        let mut consumed = 0;

        // First branch: if
        let first = &lines[0];
        let cond_toks: Vec<Spanned> = first.tokens[1..]
            .iter()
            .take_while(|t| !matches!(t.token, Token::Colon))
            .cloned()
            .collect();
        let cond = self.parse_expr_tokens(&cond_toks, first.line_no)?;
        expect_token(
            &first.tokens[1 + cond_toks.len()],
            Token::Colon,
            first.line_no,
            ":",
        )?;
        let base_indent = indent;
        consumed += 1;
        let body = self.parse_block(lines, base_indent, &mut consumed)?;
        branches.push((cond, body));

        // elif / else chains
        while consumed < lines.len() {
            let line = &lines[consumed];
            if line.tokens.is_empty() {
                consumed += 1;
                continue;
            }
            if line.indent != base_indent {
                break;
            }
            match &line.tokens[0].token {
                Token::Elif => {
                    let cond_toks: Vec<Spanned> = line.tokens[1..]
                        .iter()
                        .take_while(|t| !matches!(t.token, Token::Colon))
                        .cloned()
                        .collect();
                    let cond = self.parse_expr_tokens(&cond_toks, line.line_no)?;
                    expect_token(
                        &line.tokens[1 + cond_toks.len()],
                        Token::Colon,
                        line.line_no,
                        ":",
                    )?;
                    consumed += 1;
                    let body = self.parse_block(lines, base_indent, &mut consumed)?;
                    branches.push((cond, body));
                }
                Token::Else => {
                    expect_token(&line.tokens[1], Token::Colon, line.line_no, ":")?;
                    consumed += 1;
                    else_branch = Some(self.parse_block(lines, base_indent, &mut consumed)?);
                    break;
                }
                _ => break,
            }
        }
        Ok((
            Stmt::If {
                branches,
                else_branch,
            },
            consumed,
        ))
    }

    fn parse_while_stmt(
        &self,
        lines: &[Line],
        indent: usize,
        _line_no: usize,
    ) -> Result<(Stmt, usize), ParseError> {
        // while <cond>:
        //     body
        let first = &lines[0];
        let cond_toks: Vec<Spanned> = first.tokens[1..]
            .iter()
            .take_while(|t| !matches!(t.token, Token::Colon))
            .cloned()
            .collect();
        let cond = self.parse_expr_tokens(&cond_toks, first.line_no)?;
        expect_token(
            &first.tokens[1 + cond_toks.len()],
            Token::Colon,
            first.line_no,
            ":",
        )?;
        let base_indent = indent;
        let mut consumed = 1;
        let body = self.parse_block(lines, base_indent, &mut consumed)?;
        Ok((Stmt::While { cond, body }, consumed))
    }

    fn parse_for_stmt(
        &self,
        lines: &[Line],
        indent: usize,
        line_no: usize,
    ) -> Result<(Stmt, usize), ParseError> {
        // for <var> in <iterable>:
        //     body
        let first = &lines[0];
        let var = expect_ident(&first.tokens[1], line_no, "loop variable")?;
        expect_token(&first.tokens[2], Token::In, line_no, "in")?;
        let iter_toks: Vec<Spanned> = first.tokens[3..]
            .iter()
            .take_while(|t| !matches!(t.token, Token::Colon))
            .cloned()
            .collect();
        let iterable = self.parse_expr_tokens(&iter_toks, first.line_no)?;
        expect_token(
            &first.tokens[3 + iter_toks.len()],
            Token::Colon,
            first.line_no,
            ":",
        )?;
        let base_indent = indent;
        let mut consumed = 1;
        let body = self.parse_block(lines, base_indent, &mut consumed)?;
        Ok((
            Stmt::For {
                var,
                iterable,
                body,
            },
            consumed,
        ))
    }

    fn parse_def_stmt(&self, lines: &[Line]) -> Result<(FunctionDef, usize), ParseError> {
        // def <name>(<params>):
        //     body
        let first = &lines[0];
        let line_no = first.line_no;
        let name = expect_ident(&first.tokens[1], line_no, "function name")?;
        // Parse parameters inside parentheses.
        let mut params = Vec::new();
        let mut i = 2;
        if i < first.tokens.len() && matches!(first.tokens[i].token, Token::LParen) {
            i += 1;
            while i < first.tokens.len() && !matches!(first.tokens[i].token, Token::RParen) {
                if matches!(first.tokens[i].token, Token::Comma) {
                    i += 1;
                    continue;
                }
                let p = expect_ident(&first.tokens[i], line_no, "parameter name")?;
                params.push(p);
                i += 1;
            }
            // Skip the RParen
            i += 1;
        }
        // Expect colon at the end.
        if i < first.tokens.len() {
            expect_token(&first.tokens[i], Token::Colon, line_no, ":")?;
        }

        let base_indent = first.indent;
        let mut consumed = 1;
        let body = self.parse_block(lines, base_indent, &mut consumed)?;
        Ok((FunctionDef { name, params, body }, consumed))
    }

    fn parse_shake_stmt(
        &self,
        toks: &[Spanned],
        line_no: usize,
    ) -> Result<(Stmt, usize), ParseError> {
        // shake intensity=<expr> duration=<expr>
        let mut intensity = None;
        let mut duration = None;
        let mut i = 1;
        while i < toks.len() {
            let prop = expect_ident(&toks[i], line_no, "property name")?;
            i += 1;
            if i >= toks.len() {
                return Err(err(line_no, "expected `=` after property name"));
            }
            expect_token(&toks[i], Token::Assign, line_no, "=")?;
            i += 1;
            let mut value_toks = Vec::new();
            while i < toks.len() {
                if matches!(toks[i].token, Token::Ident(_))
                    && i + 1 < toks.len()
                    && matches!(toks[i + 1].token, Token::Assign)
                {
                    break;
                }
                value_toks.push(toks[i].clone());
                i += 1;
            }
            match prop.as_str() {
                "intensity" => {
                    intensity = Some(self.parse_expr_tokens(&value_toks, line_no)?);
                }
                "duration" => {
                    duration = Some(self.parse_expr_tokens(&value_toks, line_no)?);
                }
                _ => {}
            }
        }
        let intensity = intensity.ok_or_else(|| err(line_no, "shake requires intensity="))?;
        let duration = duration.ok_or_else(|| err(line_no, "shake requires duration="))?;
        Ok((
            Stmt::Shake {
                intensity,
                duration,
            },
            1,
        ))
    }

    fn parse_transform_stmt(
        &self,
        lines: &[Line],
        _indent: usize,
        line_no: usize,
    ) -> Result<(Stmt, usize), ParseError> {
        // transform <character> prop=value prop=value ...
        let first = &lines[0];
        let character = expect_ident(&first.tokens[1], line_no, "character name")?;

        let mut rotation = None;
        let mut scale = None;
        let mut offset = None;
        let mut opacity = None;
        let mut layer = None;

        let mut i = 2;
        while i < first.tokens.len() {
            let prop = expect_ident(&first.tokens[i], line_no, "property name")?;
            i += 1;
            if i >= first.tokens.len() {
                return Err(err(line_no, "expected `=` after property name"));
            }
            expect_token(&first.tokens[i], Token::Assign, line_no, "=")?;
            i += 1;

            // Parse value expression(s) until next property or end of line.
            let mut value_toks = Vec::new();
            while i < first.tokens.len() {
                if matches!(first.tokens[i].token, Token::Ident(_))
                    && i + 1 < first.tokens.len()
                    && matches!(first.tokens[i + 1].token, Token::Assign)
                {
                    break;
                }
                value_toks.push(first.tokens[i].clone());
                i += 1;
            }

            match prop.as_str() {
                "rotation" => {
                    rotation = Some(self.parse_expr_tokens(&value_toks, line_no)?);
                }
                "scale" => {
                    scale = Some(self.parse_expr_tokens(&value_toks, line_no)?);
                }
                "opacity" => {
                    opacity = Some(self.parse_expr_tokens(&value_toks, line_no)?);
                }
                "layer" => {
                    if let Some(Token::Ident(name)) = value_toks.first().map(|t| &t.token) {
                        layer = Some(name.clone());
                    }
                }
                // offset = (x, y) or offset = x, y
                "offset" if value_toks.len() >= 2 => {
                    // Try parsing as (x, y) or x, y
                    let mut toks = value_toks.clone();
                    // Remove parens if present
                    if matches!(toks.first().map(|t| &t.token), Some(Token::LParen)) {
                        toks.remove(0);
                    }
                    if matches!(toks.last().map(|t| &t.token), Some(Token::RParen)) {
                        toks.pop();
                    }
                    // Split on comma
                    if let Some(comma_pos) =
                        toks.iter().position(|t| matches!(t.token, Token::Comma))
                    {
                        let x_toks = &toks[..comma_pos];
                        let y_toks = &toks[comma_pos + 1..];
                        let x = self.parse_expr_tokens(x_toks, line_no)?;
                        let y = self.parse_expr_tokens(y_toks, line_no)?;
                        offset = Some((x, y));
                    }
                }
                _ => {} // Unknown property, ignore
            }
        }

        Ok((
            Stmt::Transform {
                character,
                rotation,
                scale,
                offset,
                opacity,
                layer,
            },
            1,
        ))
    }

    /// Parse a block of statements indented more than `parent_indent`.
    /// Updates `consumed` with the number of lines consumed.
    fn parse_block(
        &self,
        lines: &[Line],
        parent_indent: usize,
        consumed: &mut usize,
    ) -> Result<Vec<Stmt>, ParseError> {
        let mut body = Vec::new();
        while *consumed < lines.len() {
            let line = &lines[*consumed];
            if line.tokens.is_empty() {
                *consumed += 1;
                continue;
            }
            if line.indent <= parent_indent {
                break;
            }
            let (stmt, extra) = self.parse_stmt(&lines[*consumed..], line.indent)?;
            body.push(stmt);
            *consumed += extra;
        }
        Ok(body)
    }

    fn parse_play_stmt(
        &self,
        toks: &[Spanned],
        line_no: usize,
    ) -> Result<(Stmt, usize), ParseError> {
        // play music "track" [fade <s>]
        // play sfx "track" [volume <v>]
        let kind = expect_ident(&toks[1], line_no, "play kind (music/sfx)")?;
        let track = expect_string(&toks[2], line_no, "track path")?;
        match kind.as_str() {
            "music" => {
                let mut fade = 0.0;
                if let Some(t) = toks.get(3) {
                    if let Token::Ident(ref s) = t.token {
                        if s == "fade" {
                            if let Some(d) = toks.get(4) {
                                fade = number_token(d, line_no)? as f32;
                            }
                        }
                    }
                }
                Ok((Stmt::PlayMusic { track, fade }, 1))
            }
            "sfx" => {
                let mut volume = 1.0;
                if let Some(t) = toks.get(3) {
                    if let Token::Ident(ref s) = t.token {
                        if s == "volume" {
                            if let Some(v) = toks.get(4) {
                                volume = number_token(v, line_no)? as f32;
                            }
                        }
                    }
                }
                Ok((Stmt::PlaySfx { track, volume }, 1))
            }
            _ => Err(err(line_no, format!("unknown play kind `{kind}`"))),
        }
    }

    fn parse_stop_stmt(
        &self,
        toks: &[Spanned],
        line_no: usize,
    ) -> Result<(Stmt, usize), ParseError> {
        // stop music [fade <s>]
        // stop voice
        let kind = match &toks[1].token {
            Token::Ident(s) => s.clone(),
            Token::Voice => "voice".to_string(),
            other => {
                return Err(ParseError::Unexpected {
                    line: line_no,
                    got: format!("{other:?}"),
                    expected: "stop kind (music/voice)".to_string(),
                });
            }
        };
        match kind.as_str() {
            "music" => {
                let mut fade = 0.0;
                if let Some(t) = toks.get(2) {
                    if let Token::Ident(ref s) = t.token {
                        if s == "fade" {
                            if let Some(d) = toks.get(3) {
                                fade = number_token(d, line_no)? as f32;
                            }
                        }
                    }
                }
                Ok((Stmt::StopMusic { fade }, 1))
            }
            "voice" => Ok((Stmt::StopVoice, 1)),
            _ => Err(err(line_no, format!("unknown stop kind `{kind}`"))),
        }
    }

    fn parse_say_stmt(
        &self,
        toks: &[Spanned],
        line_no: usize,
    ) -> Result<(Stmt, usize), ParseError> {
        // <speaker> "text" [voice "track"] [side "image"]
        let speaker = expect_ident(&toks[0], line_no, "speaker id")?;
        let text = expect_string(&toks[1], line_no, "dialogue text")?;
        let mut voice = None;
        let mut side_image = None;
        let mut i = 2;
        while i < toks.len() {
            match &toks[i].token {
                Token::Voice => {
                    voice = Some(expect_string(&toks[i + 1], line_no, "voice track")?);
                    i += 2;
                }
                Token::Ident(s) if s == "side" => {
                    side_image = Some(expect_string(&toks[i + 1], line_no, "side image")?);
                    i += 2;
                }
                _ => break,
            }
        }
        Ok((
            Stmt::Say {
                speaker: Some(speaker),
                text,
                voice,
                side_image,
            },
            1,
        ))
    }

    // ---- Expression parsing ----

    fn parse_expr_tokens(&self, toks: &[Spanned], line_no: usize) -> Result<Expr, ParseError> {
        if toks.is_empty() {
            return Err(err(line_no, "expected expression"));
        }
        let mut p = ExprParser {
            tokens: toks,
            pos: 0,
            line_no,
        };
        let e = p.parse_expr()?;
        if p.pos != toks.len() {
            return Err(err(
                line_no,
                format!("trailing tokens in expression at position {}", p.pos),
            ));
        }
        Ok(e)
    }
}

/// Parse an expression from a token slice, returning the expression and the
/// number of tokens consumed.
fn parse_expr_with_len(toks: &[Spanned], line_no: usize) -> Result<(Expr, usize), ParseError> {
    if toks.is_empty() {
        return Err(err(line_no, "expected expression"));
    }
    let mut p = ExprParser {
        tokens: toks,
        pos: 0,
        line_no,
    };
    let e = p.parse_expr()?;
    Ok((e, p.pos))
}

struct ExprParser<'a> {
    tokens: &'a [Spanned],
    pos: usize,
    line_no: usize,
}

impl<'a> ExprParser<'a> {
    fn peek(&self) -> Option<&Spanned> {
        self.tokens.get(self.pos)
    }
    fn next(&mut self) -> Option<&Spanned> {
        let t = self.tokens.get(self.pos);
        self.pos += 1;
        t
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_and()?;
        while let Some(t) = self.peek() {
            if matches!(t.token, Token::Or) {
                self.next();
                let rhs = self.parse_and()?;
                lhs = Expr::BinaryOp {
                    op: BinaryOp::Or,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_cmp()?;
        while let Some(t) = self.peek() {
            if matches!(t.token, Token::And) {
                self.next();
                let rhs = self.parse_cmp()?;
                lhs = Expr::BinaryOp {
                    op: BinaryOp::And,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_cmp(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_add()?;
        if let Some(t) = self.peek() {
            let op = match &t.token {
                Token::Eq => Some(BinaryOp::Eq),
                Token::Neq => Some(BinaryOp::Neq),
                Token::Lt => Some(BinaryOp::Lt),
                Token::Gt => Some(BinaryOp::Gt),
                Token::Le => Some(BinaryOp::Le),
                Token::Ge => Some(BinaryOp::Ge),
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let rhs = self.parse_add()?;
                return Ok(Expr::BinaryOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                });
            }
        }
        Ok(lhs)
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_mul()?;
        while let Some(t) = self.peek() {
            let op = match &t.token {
                Token::Plus => Some(BinaryOp::Add),
                Token::Minus => Some(BinaryOp::Sub),
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let rhs = self.parse_mul()?;
                lhs = Expr::BinaryOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        while let Some(t) = self.peek() {
            let op = match &t.token {
                Token::Star => Some(BinaryOp::Mul),
                Token::Slash => Some(BinaryOp::Div),
                Token::Percent => Some(BinaryOp::Mod),
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let rhs = self.parse_unary()?;
                lhs = Expr::BinaryOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if let Some(t) = self.peek() {
            match &t.token {
                Token::Minus => {
                    self.next();
                    let operand = self.parse_unary()?;
                    return Ok(Expr::UnaryOp {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    });
                }
                Token::Not => {
                    self.next();
                    let operand = self.parse_unary()?;
                    return Ok(Expr::UnaryOp {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    });
                }
                _ => {}
            }
        }
        self.parse_postfix()
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let line_no = self.line_no;
        let t = self
            .next()
            .ok_or_else(|| err(line_no, "expected expression"))?;
        let expr = match &t.token {
            Token::IntLit(n) => Expr::Int(*n),
            Token::FloatLit(f) => Expr::Float(*f),
            Token::StringLit(s) => Expr::Str(s.clone()),
            Token::True => Expr::Bool(true),
            Token::False => Expr::Bool(false),
            Token::None => Expr::None,
            Token::Ident(s) => {
                // `name(args...)` is a builtin call, otherwise a variable.
                let name = s.clone();
                if matches!(self.peek().map(|t| &t.token), Some(Token::LParen)) {
                    self.next(); // consume `(`
                    let mut args = Vec::new();
                    if !matches!(self.peek().map(|t| &t.token), Some(Token::RParen)) {
                        loop {
                            args.push(self.parse_expr()?);
                            match self.next().map(|t| &t.token) {
                                Some(Token::Comma) => continue,
                                Some(Token::RParen) => break,
                                _ => {
                                    return Err(err(line_no, "expected `,` or `)`"));
                                }
                            }
                        }
                    } else {
                        self.next(); // consume `)`
                    }
                    Expr::Call { name, args }
                } else {
                    Expr::Var(name)
                }
            }
            Token::LParen => {
                let e = self.parse_expr()?;
                let close = self.next().ok_or_else(|| err(line_no, "expected `)`"))?;
                expect_token(close, Token::RParen, line_no, ")")?;
                e
            }
            Token::LBracket => {
                // List literal: [expr, expr, ...]
                let mut items = Vec::new();
                if let Some(next) = self.peek() {
                    if next.token != Token::RBracket {
                        loop {
                            let e = self.parse_expr()?;
                            items.push(e);
                            if let Some(next) = self.peek() {
                                if next.token == Token::Comma {
                                    self.next();
                                    continue;
                                }
                            }
                            break;
                        }
                    }
                }
                let close = self.next().ok_or_else(|| err(line_no, "expected `]`"))?;
                expect_token(close, Token::RBracket, line_no, "]")?;
                Expr::List(items)
            }
            other => {
                return Err(err(
                    line_no,
                    format!("unexpected token in expression: {other:?}"),
                ));
            }
        };
        Ok(expr)
    }

    /// Parse postfix index access: expr[expr]
    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            let is_lbracket = self
                .peek()
                .map(|t| t.token == Token::LBracket)
                .unwrap_or(false);
            if !is_lbracket {
                break;
            }
            self.next(); // consume [
            let index = self.parse_expr()?;
            let line_no = self.line_no;
            let close = self.next().ok_or_else(|| err(line_no, "expected `]`"))?;
            expect_token(close, Token::RBracket, line_no, "]")?;
            expr = Expr::Index {
                collection: Box::new(expr),
                index: Box::new(index),
            };
        }
        Ok(expr)
    }
}

// ---- helpers ----

fn expect_ident(t: &Spanned, line_no: usize, what: &str) -> Result<String, ParseError> {
    match &t.token {
        Token::Ident(s) => Ok(s.clone()),
        other => Err(ParseError::Unexpected {
            line: line_no,
            got: format!("{other:?}"),
            expected: what.to_string(),
        }),
    }
}

/// Map a compound assignment token to its binary operator.
fn compound_assign_op(token: &Token) -> Option<BinaryOp> {
    match token {
        Token::PlusAssign => Some(BinaryOp::Add),
        Token::MinusAssign => Some(BinaryOp::Sub),
        Token::StarAssign => Some(BinaryOp::Mul),
        Token::SlashAssign => Some(BinaryOp::Div),
        Token::PercentAssign => Some(BinaryOp::Mod),
        _ => None,
    }
}

fn expect_string(t: &Spanned, line_no: usize, what: &str) -> Result<String, ParseError> {
    match &t.token {
        Token::StringLit(s) => Ok(s.clone()),
        other => Err(ParseError::Unexpected {
            line: line_no,
            got: format!("{other:?}"),
            expected: what.to_string(),
        }),
    }
}

fn expect_token(
    t: &Spanned,
    expected: Token,
    line_no: usize,
    what: &str,
) -> Result<(), ParseError> {
    if t.token == expected {
        Ok(())
    } else {
        Err(ParseError::Unexpected {
            line: line_no,
            got: format!("{:?}", t.token),
            expected: what.to_string(),
        })
    }
}

fn number_token(t: &Spanned, line_no: usize) -> Result<f64, ParseError> {
    match &t.token {
        Token::IntLit(n) => Ok(*n as f64),
        Token::FloatLit(f) => Ok(*f),
        other => Err(err(line_no, format!("expected number, got {other:?}"))),
    }
}

fn parse_color_component(t: &Spanned, line_no: usize) -> Result<f32, ParseError> {
    match &t.token {
        Token::IntLit(n) => Ok(*n as f32 / 255.0),
        Token::FloatLit(f) => Ok(*f as f32),
        other => Err(err(
            line_no,
            format!("expected color component, got {other:?}"),
        )),
    }
}

fn parse_hex_color(s: &str, line_no: usize) -> Result<Option<[f32; 3]>, ParseError> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return Err(err(line_no, "expected #rrggbb color"));
    }
    let r = u8::from_str_radix(&s[0..2], 16).map_err(|_| err(line_no, "bad hex color"))?;
    let g = u8::from_str_radix(&s[2..4], 16).map_err(|_| err(line_no, "bad hex color"))?;
    let b = u8::from_str_radix(&s[4..6], 16).map_err(|_| err(line_no, "bad hex color"))?;
    Ok(Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]))
}

fn parse_anchor_ident(s: &str, line_no: usize) -> Result<AnchorExpr, ParseError> {
    match s {
        "left" => Ok(AnchorExpr::Left),
        "center" => Ok(AnchorExpr::Center),
        "centerleft" => Ok(AnchorExpr::CenterLeft),
        "centerright" => Ok(AnchorExpr::CenterRight),
        "right" => Ok(AnchorExpr::Right),
        "offscreenleft" => Ok(AnchorExpr::OffscreenLeft),
        "offscreenright" => Ok(AnchorExpr::OffscreenRight),
        _ => Err(err(line_no, format!("unknown anchor `{s}`"))),
    }
}

// Silence unused warnings for helper sets we keep for completeness.
#[allow(dead_code)]
fn _silence() {
    let _ = HashSet::<()>::new();
}
