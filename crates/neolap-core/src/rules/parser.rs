//! Pratt parser: rule text → AST with positions.

use super::lexer::{Pos, Tok, Token, lex};
use crate::{Error, MemberId, Result};

/// On which cells a rule applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// No prefix: base cells and aggregates (the cell itself is calculated).
    All,
    /// `N:` base cells only; aggregates sum the rule results.
    Base,
    /// `C:` aggregates only.
    Aggregate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// `dim:member` inside `[...]`. `dim`, `fixed` and `desc` are filled in by name resolution.
#[derive(Debug, Clone)]
pub struct RefItem {
    pub dim_name: Option<String>,
    pub member: Expr,
    pub dim: usize,
    pub fixed: Option<MemberId>,
    pub desc: bool,
}

#[derive(Debug, Clone)]
pub struct CellRef {
    pub items: Vec<RefItem>,
    pub pos: Pos,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Num(f64),
    Str(String),
    Null,
    Continue,
    /// `!dim`: name of the current member; the index is resolved later.
    Dim(String, usize),
    Cell(CellRef),
    Unary(UnOp, Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call {
        name: String,
        args: Vec<Expr>,
        pos: Pos,
    },
}

#[derive(Debug, Clone)]
pub struct RuleAst {
    pub scope: Scope,
    pub target: CellRef,
    pub expr: Expr,
    pub feed: Vec<CellRef>,
    pub line: usize,
}

struct Parser {
    toks: Vec<Token>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.i].tok
    }

    fn peek_at(&self, off: usize) -> &Tok {
        &self.toks[(self.i + off).min(self.toks.len() - 1)].tok
    }

    fn pos(&self) -> Pos {
        self.toks[self.i].pos
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.i].tok.clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }

    fn err<T>(&self, msg: &str) -> Result<T> {
        Err(Error::Parse(format!("{}: {msg}", self.pos())))
    }

    fn expect(&mut self, t: Tok, what: &str) -> Result<()> {
        if *self.peek() == t {
            self.bump();
            Ok(())
        } else {
            self.err(&format!("expected {what}, found {}", describe(self.peek())))
        }
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s.eq_ignore_ascii_case(kw))
    }

    fn rule(&mut self) -> Result<RuleAst> {
        let line = self.pos().line;
        let mut scope = Scope::All;
        if let (Tok::Ident(s), Tok::Colon, Tok::LBracket) =
            (self.peek(), self.peek_at(1), self.peek_at(2))
        {
            scope = match s.to_ascii_uppercase().as_str() {
                "N" => Scope::Base,
                "C" => Scope::Aggregate,
                _ => return self.err(&format!("unknown rule prefix '{s}:' (use N: or C:)")),
            };
            self.bump();
            self.bump();
        }
        let target = self.cell_ref()?;
        self.expect(Tok::Eq, "'=' after the rule target")?;
        let expr = self.expr(0)?;
        let mut feed = Vec::new();
        if self.is_kw("FEED") {
            self.bump();
            loop {
                feed.push(self.cell_ref()?);
                if *self.peek() == Tok::Comma {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        match self.peek() {
            Tok::Semi => {
                self.bump();
            }
            Tok::Eof => {}
            t => {
                return self.err(&format!(
                    "expected ';' at the end of the rule, found {}",
                    describe(t)
                ));
            }
        }
        Ok(RuleAst {
            scope,
            target,
            expr,
            feed,
            line,
        })
    }

    fn cell_ref(&mut self) -> Result<CellRef> {
        let pos = self.pos();
        self.expect(Tok::LBracket, "'['")?;
        let mut items = Vec::new();
        loop {
            let dim_name = match (self.peek().clone(), self.peek_at(1)) {
                (Tok::Str(s) | Tok::Ident(s), Tok::Colon) => {
                    self.bump();
                    self.bump();
                    Some(s)
                }
                _ => None,
            };
            let member = self.primary()?;
            items.push(RefItem {
                dim_name,
                member,
                dim: usize::MAX,
                fixed: None,
                desc: false,
            });
            match self.peek() {
                Tok::Comma => {
                    self.bump();
                }
                Tok::RBracket => {
                    self.bump();
                    break;
                }
                t => return self.err(&format!("expected ',' or ']', found {}", describe(t))),
            }
        }
        Ok(CellRef { items, pos })
    }

    fn expr(&mut self, min_bp: u8) -> Result<Expr> {
        let mut lhs = match self.peek() {
            Tok::Minus => {
                self.bump();
                Expr::Unary(UnOp::Neg, Box::new(self.expr(13)?))
            }
            Tok::Plus => {
                self.bump();
                self.expr(13)?
            }
            _ if self.is_kw("NOT") => {
                self.bump();
                Expr::Unary(UnOp::Not, Box::new(self.expr(5)?))
            }
            _ => self.primary()?,
        };
        loop {
            let (op, lbp, rbp) = match self.peek() {
                Tok::Ident(s) if s.eq_ignore_ascii_case("OR") => (BinOp::Or, 1, 2),
                Tok::Ident(s) if s.eq_ignore_ascii_case("AND") => (BinOp::And, 3, 4),
                Tok::Eq => (BinOp::Eq, 7, 8),
                Tok::Ne => (BinOp::Ne, 7, 8),
                Tok::Lt => (BinOp::Lt, 7, 8),
                Tok::Le => (BinOp::Le, 7, 8),
                Tok::Gt => (BinOp::Gt, 7, 8),
                Tok::Ge => (BinOp::Ge, 7, 8),
                Tok::Plus => (BinOp::Add, 9, 10),
                Tok::Minus => (BinOp::Sub, 9, 10),
                Tok::Star => (BinOp::Mul, 11, 12),
                Tok::Slash => (BinOp::Div, 11, 12),
                Tok::Caret => (BinOp::Pow, 16, 15),
                _ => break,
            };
            if lbp < min_bp {
                break;
            }
            self.bump();
            let rhs = self.expr(rbp)?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn primary(&mut self) -> Result<Expr> {
        let pos = self.pos();
        match self.peek().clone() {
            Tok::Num(v) => {
                self.bump();
                Ok(Expr::Num(v))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            Tok::Bang => {
                self.bump();
                match self.bump() {
                    Tok::Ident(name) => Ok(Expr::Dim(name, usize::MAX)),
                    _ => Err(Error::Parse(format!(
                        "{pos}: expected a dimension name after '!'"
                    ))),
                }
            }
            Tok::LBracket => Ok(Expr::Cell(self.cell_ref()?)),
            Tok::LParen => {
                self.bump();
                let e = self.expr(0)?;
                self.expect(Tok::RParen, "')'")?;
                Ok(e)
            }
            Tok::Ident(name) => {
                self.bump();
                if *self.peek() == Tok::LParen {
                    self.bump();
                    let mut args = Vec::new();
                    if *self.peek() != Tok::RParen {
                        loop {
                            args.push(self.expr(0)?);
                            if *self.peek() == Tok::Comma {
                                self.bump();
                            } else {
                                break;
                            }
                        }
                    }
                    self.expect(Tok::RParen, "')'")?;
                    let name = name.to_ascii_uppercase();
                    if name == "CONTINUE" && args.is_empty() {
                        return Ok(Expr::Continue);
                    }
                    return Ok(Expr::Call { name, args, pos });
                }
                match name.to_ascii_uppercase().as_str() {
                    "NULL" => Ok(Expr::Null),
                    "CONTINUE" => Ok(Expr::Continue),
                    _ => Err(Error::Parse(format!(
                        "{pos}: unknown identifier '{name}' (member names are written ['…'])"
                    ))),
                }
            }
            t => self.err(&format!("expected a value, found {}", describe(&t))),
        }
    }
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Num(v) => format!("number {v}"),
        Tok::Str(s) => format!("string '{s}'"),
        Tok::Ident(s) => format!("'{s}'"),
        Tok::Eof => "end of text".into(),
        other => format!("'{}'", symbol(other)),
    }
}

fn symbol(t: &Tok) -> &'static str {
    match t {
        Tok::Bang => "!",
        Tok::LBracket => "[",
        Tok::RBracket => "]",
        Tok::LParen => "(",
        Tok::RParen => ")",
        Tok::Comma => ",",
        Tok::Colon => ":",
        Tok::Semi => ";",
        Tok::Eq => "=",
        Tok::Ne => "<>",
        Tok::Lt => "<",
        Tok::Le => "<=",
        Tok::Gt => ">",
        Tok::Ge => ">=",
        Tok::Plus => "+",
        Tok::Minus => "-",
        Tok::Star => "*",
        Tok::Slash => "/",
        Tok::Caret => "^",
        _ => "?",
    }
}

/// Parses the complete rule text; the first syntax error aborts with its position.
pub fn parse(src: &str) -> Result<Vec<RuleAst>> {
    let mut p = Parser {
        toks: lex(src)?,
        i: 0,
    };
    let mut rules = Vec::new();
    while *p.peek() != Tok::Eof {
        if *p.peek() == Tok::Semi {
            p.bump();
            continue;
        }
        rules.push(p.rule()?);
    }
    Ok(rules)
}
