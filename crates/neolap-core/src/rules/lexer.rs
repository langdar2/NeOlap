//! Tokenizer of the rule language.

use crate::{Error, Result};

/// Source position (1-based) for error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for Pos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}, column {}", self.line, self.col)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Bang,
    LBracket,
    RBracket,
    LParen,
    RParen,
    Comma,
    Colon,
    Semi,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub pos: Pos,
}

struct Lexer {
    chars: Vec<char>,
    i: usize,
    pos: Pos,
}

impl Lexer {
    fn peek(&self, off: usize) -> Option<char> {
        self.chars.get(self.i + off).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.i).copied()?;
        self.i += 1;
        if c == '\n' {
            self.pos.line += 1;
            self.pos.col = 1;
        } else {
            self.pos.col += 1;
        }
        Some(c)
    }

    fn err<T>(&self, pos: Pos, msg: &str) -> Result<T> {
        Err(Error::Parse(format!("{pos}: {msg}")))
    }
}

/// Splits rule text into tokens. `#` starts a comment up to the end of the line.
pub fn lex(src: &str) -> Result<Vec<Token>> {
    let mut lx = Lexer {
        chars: src.chars().collect(),
        i: 0,
        pos: Pos { line: 1, col: 1 },
    };
    let mut out = Vec::new();
    while let Some(c) = lx.peek(0) {
        let pos = lx.pos;
        let tok = match c {
            c if c.is_whitespace() => {
                lx.bump();
                continue;
            }
            '#' => {
                while lx.peek(0).is_some_and(|c| c != '\n') {
                    lx.bump();
                }
                continue;
            }
            '\'' | '"' => {
                lx.bump();
                let mut s = String::new();
                loop {
                    match lx.bump() {
                        None => return lx.err(pos, "unterminated string"),
                        Some(q) if q == c => {
                            if lx.peek(0) == Some(c) {
                                lx.bump();
                                s.push(c);
                            } else {
                                break;
                            }
                        }
                        Some(ch) => s.push(ch),
                    }
                }
                Tok::Str(s)
            }
            c if c.is_ascii_digit() => {
                let mut s = String::new();
                while lx.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                    s.extend(lx.bump());
                }
                if lx.peek(0) == Some('.') && lx.peek(1).is_some_and(|c| c.is_ascii_digit()) {
                    s.extend(lx.bump());
                    while lx.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                        s.extend(lx.bump());
                    }
                }
                if matches!(lx.peek(0), Some('e' | 'E')) {
                    let sign = matches!(lx.peek(1), Some('+' | '-'));
                    if lx
                        .peek(if sign { 2 } else { 1 })
                        .is_some_and(|c| c.is_ascii_digit())
                    {
                        s.extend(lx.bump());
                        if sign {
                            s.extend(lx.bump());
                        }
                        while lx.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                            s.extend(lx.bump());
                        }
                    }
                }
                match s.parse::<f64>() {
                    Ok(v) => Tok::Num(v),
                    Err(_) => return lx.err(pos, &format!("invalid number '{s}'")),
                }
            }
            c if c.is_alphabetic() || c == '_' => {
                let mut s = String::new();
                while lx.peek(0).is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    s.extend(lx.bump());
                }
                Tok::Ident(s)
            }
            _ => {
                lx.bump();
                match c {
                    '!' if lx.peek(0) == Some('=') => {
                        lx.bump();
                        Tok::Ne
                    }
                    '!' => Tok::Bang,
                    '<' if lx.peek(0) == Some('=') => {
                        lx.bump();
                        Tok::Le
                    }
                    '<' if lx.peek(0) == Some('>') => {
                        lx.bump();
                        Tok::Ne
                    }
                    '<' => Tok::Lt,
                    '>' if lx.peek(0) == Some('=') => {
                        lx.bump();
                        Tok::Ge
                    }
                    '>' => Tok::Gt,
                    '=' => Tok::Eq,
                    '[' => Tok::LBracket,
                    ']' => Tok::RBracket,
                    '(' => Tok::LParen,
                    ')' => Tok::RParen,
                    ',' => Tok::Comma,
                    ':' => Tok::Colon,
                    ';' => Tok::Semi,
                    '+' => Tok::Plus,
                    '-' => Tok::Minus,
                    '*' => Tok::Star,
                    '/' => Tok::Slash,
                    '^' => Tok::Caret,
                    other => return lx.err(pos, &format!("unexpected character '{other}'")),
                }
            }
        };
        out.push(Token { tok, pos });
    }
    out.push(Token {
        tok: Tok::Eof,
        pos: lx.pos,
    });
    Ok(out)
}
