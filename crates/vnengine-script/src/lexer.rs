//! Lexer for the `.vn` script language.
//!
//! The language is line-oriented and Ren'Py-inspired. Example:
//!
//! ```text
//! define e = Character("Eileen", color="#ff0000")
//!
//! label start:
//!     scene bg school with dissolve
//!     show eileen happy at left
//!     e "Hello, world!"
//!     menu:
//!         "Go left":
//!             $ went_left = True
//!             jump go_left
//!         "Go right":
//!             jump go_right
//! ```
//!
//! Indentation is significant: nested blocks must be indented more than their
//! parent.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Indent(usize),
    Newline,
    // Keywords
    Label,
    Define,
    Scene,
    Show,
    Hide,
    With,
    At,
    As,
    Menu,
    Jump,
    Call,
    Return,
    If,
    Elif,
    Else,
    Pause,
    Play,
    Stop,
    Voice,
    Set,       // `set` keyword (unused alias for `$`)
    Default,   // `default` keyword
    While,     // `while` keyword
    For,       // `for` keyword
    In,        // `in` keyword (for loops)
    Def,       // `def` keyword (user functions)
    Transform, // `transform` keyword
    Shake,     // `shake` keyword
    Break,     // `break` keyword
    Continue,  // `continue` keyword
    Is,        // `is` keyword (identity comparison)
    End,
    // Literals
    Ident(String),
    StringLit(String),
    IntLit(i64),
    FloatLit(f64),
    // Operators / punctuation
    Assign,        // =
    PlusAssign,    // +=
    MinusAssign,   // -=
    StarAssign,    // *=
    SlashAssign,   // /=
    PercentAssign, // %=
    Eq,            // ==
    Neq,           // !=
    Lt,
    Gt,
    Le,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    And,
    Or,
    Not,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Dot,
    Colon,
    Dollar, // `$` for inline expressions
    True,
    False,
    None,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone)]
pub struct Spanned {
    pub token: Token,
    pub line: usize,
    pub col: usize,
}

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum LexError {
    #[error("line {line}: {message}")]
    Generic { line: usize, message: String },
}

impl LexError {
    /// 1-based source line where the error occurred.
    pub fn line(&self) -> usize {
        match self {
            LexError::Generic { line, .. } => *line,
        }
    }
}

fn lex_error(line: usize, msg: impl Into<String>) -> LexError {
    LexError::Generic {
        line,
        message: msg.into(),
    }
}

impl Lexer {
    pub fn new(src: &str) -> Self {
        Self {
            chars: src.replace("\r\n", "\n").chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    pub fn tokenize(mut self) -> Result<Vec<Spanned>, LexError> {
        let mut out = Vec::new();
        let mut at_line_start = true;
        while self.pos < self.chars.len() {
            if at_line_start {
                // Compute indentation (spaces only; tabs not supported to keep
                // things simple and unambiguous).
                let mut indent = 0;
                while self.pos < self.chars.len() && self.chars[self.pos] == ' ' {
                    indent += 1;
                    self.advance();
                }
                // Blank line or comment-only line: skip without emitting indent.
                if self.pos >= self.chars.len()
                    || self.chars[self.pos] == '\n'
                    || self.chars[self.pos] == '#'
                {
                    // consume comment to end of line
                    while self.pos < self.chars.len() && self.chars[self.pos] != '\n' {
                        self.advance();
                    }
                    if self.pos < self.chars.len() {
                        self.advance(); // consume newline
                        self.line += 1;
                        self.col = 1;
                    }
                    continue;
                }
                out.push(Spanned {
                    token: Token::Indent(indent),
                    line: self.line,
                    col: 1,
                });
                at_line_start = false;
                continue;
            }

            let c = self.chars[self.pos];

            // End of line
            if c == '\n' {
                out.push(Spanned {
                    token: Token::Newline,
                    line: self.line,
                    col: self.col,
                });
                self.advance();
                self.line += 1;
                self.col = 1;
                at_line_start = true;
                continue;
            }

            // Whitespace within a line
            if c == ' ' || c == '\t' {
                self.advance();
                continue;
            }

            // Comments
            if c == '#' {
                while self.pos < self.chars.len() && self.chars[self.pos] != '\n' {
                    self.advance();
                }
                continue;
            }

            // Inline `$` expression
            if c == '$' {
                // The rest of the line is an expression statement; capture raw.
                self.advance();
                out.push(Spanned {
                    token: Token::Dollar,
                    line: self.line,
                    col: self.col,
                });
                continue;
            }

            // Strings
            if c == '"' || c == '\'' {
                let quote = c;
                self.advance();
                let mut s = String::new();
                while self.pos < self.chars.len() && self.chars[self.pos] != quote {
                    let ch = self.chars[self.pos];
                    if ch == '\\' && self.pos + 1 < self.chars.len() {
                        self.advance();
                        let e = self.chars[self.pos];
                        s.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            '\\' => '\\',
                            '"' => '"',
                            '\'' => '\'',
                            other => other,
                        });
                        self.advance();
                    } else {
                        s.push(ch);
                        self.advance();
                    }
                }
                if self.pos >= self.chars.len() {
                    return Err(lex_error(self.line, "unterminated string"));
                }
                self.advance(); // closing quote
                out.push(Spanned {
                    token: Token::StringLit(s),
                    line: self.line,
                    col: self.col,
                });
                continue;
            }

            // Numbers
            if c.is_ascii_digit() {
                let start = self.pos;
                while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
                    self.advance();
                }
                let mut is_float = false;
                if self.pos < self.chars.len()
                    && self.chars[self.pos] == '.'
                    && self.pos + 1 < self.chars.len()
                    && self.chars[self.pos + 1].is_ascii_digit()
                {
                    is_float = true;
                    self.advance();
                    while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
                        self.advance();
                    }
                }
                let text: String = self.chars[start..self.pos].iter().collect();
                let tok = if is_float {
                    Token::FloatLit(
                        text.parse()
                            .map_err(|_| lex_error(self.line, format!("bad float `{text}`")))?,
                    )
                } else {
                    Token::IntLit(
                        text.parse()
                            .map_err(|_| lex_error(self.line, format!("bad int `{text}`")))?,
                    )
                };
                out.push(Spanned {
                    token: tok,
                    line: self.line,
                    col: self.col,
                });
                continue;
            }

            // Identifiers / keywords
            if c.is_alphabetic() || c == '_' {
                let start = self.pos;
                while self.pos < self.chars.len()
                    && (self.chars[self.pos].is_alphanumeric() || self.chars[self.pos] == '_')
                {
                    self.advance();
                }
                let word: String = self.chars[start..self.pos].iter().collect();
                let tok = match word.as_str() {
                    "label" => Token::Label,
                    "define" => Token::Define,
                    "scene" => Token::Scene,
                    "show" => Token::Show,
                    "hide" => Token::Hide,
                    "with" => Token::With,
                    "at" => Token::At,
                    "as" => Token::As,
                    "menu" => Token::Menu,
                    "jump" => Token::Jump,
                    "call" => Token::Call,
                    "return" => Token::Return,
                    "if" => Token::If,
                    "elif" => Token::Elif,
                    "else" => Token::Else,
                    "pause" => Token::Pause,
                    "play" => Token::Play,
                    "stop" => Token::Stop,
                    "voice" => Token::Voice,
                    "set" => Token::Set,
                    "default" => Token::Default,
                    "while" => Token::While,
                    "for" => Token::For,
                    "in" => Token::In,
                    "def" => Token::Def,
                    "transform" => Token::Transform,
                    "shake" => Token::Shake,
                    "break" => Token::Break,
                    "continue" => Token::Continue,
                    "is" => Token::Is,
                    "end" => Token::End,
                    "and" => Token::And,
                    "or" => Token::Or,
                    "not" => Token::Not,
                    "true" | "True" => Token::True,
                    "false" | "False" => Token::False,
                    "none" | "None" => Token::None,
                    other => Token::Ident(other.to_string()),
                };
                out.push(Spanned {
                    token: tok,
                    line: self.line,
                    col: self.col,
                });
                continue;
            }

            // Operators / punctuation
            let two = if self.pos + 1 < self.chars.len() {
                Some(format!("{}{}", c, self.chars[self.pos + 1]))
            } else {
                None
            };
            if let Some(two) = two {
                let tok = match two.as_str() {
                    "==" => Some(Token::Eq),
                    "!=" => Some(Token::Neq),
                    "<=" => Some(Token::Le),
                    ">=" => Some(Token::Ge),
                    "+=" => Some(Token::PlusAssign),
                    "-=" => Some(Token::MinusAssign),
                    "*=" => Some(Token::StarAssign),
                    "/=" => Some(Token::SlashAssign),
                    "%=" => Some(Token::PercentAssign),
                    _ => None,
                };
                if let Some(tok) = tok {
                    self.advance();
                    self.advance();
                    out.push(Spanned {
                        token: tok,
                        line: self.line,
                        col: self.col,
                    });
                    continue;
                }
            }
            let single = match c {
                '=' => Token::Assign,
                '<' => Token::Lt,
                '>' => Token::Gt,
                '+' => Token::Plus,
                '-' => Token::Minus,
                '*' => Token::Star,
                '/' => Token::Slash,
                '%' => Token::Percent,
                '(' => Token::LParen,
                ')' => Token::RParen,
                '[' => Token::LBracket,
                ']' => Token::RBracket,
                '{' => Token::LBrace,
                '}' => Token::RBrace,
                ',' => Token::Comma,
                '.' => Token::Dot,
                ':' => Token::Colon,
                _ => {
                    return Err(lex_error(self.line, format!("unexpected character `{c}`")));
                }
            };
            self.advance();
            out.push(Spanned {
                token: single,
                line: self.line,
                col: self.col,
            });
        }

        // Ensure trailing newline for parser simplicity.
        if out
            .last()
            .map(|s| !matches!(s.token, Token::Newline))
            .unwrap_or(true)
        {
            out.push(Spanned {
                token: Token::Newline,
                line: self.line,
                col: self.col,
            });
        }
        Ok(out)
    }

    fn advance(&mut self) {
        self.pos += 1;
        self.col += 1;
    }
}
