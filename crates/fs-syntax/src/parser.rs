//! Recursive-descent parser for FeatureScript, with a Pratt-style expression
//! parser.
//!
//! Operator precedence, loosest to tightest:
//!
//! | level | operators                              | assoc |
//! |-------|----------------------------------------|-------|
//! | 1     | `=` `+=` `-=` `*=` `/=` `%=` `~=` `^=` | right |
//! | 2     | `?:`                                   | right |
//! | 3     | `||` `??`                              | left  |
//! | 4     | `&&`                                   | left  |
//! | 5     | `==` `!=`                              | left  |
//! | 6     | `<` `<=` `>` `>=`                      | left  |
//! | 7     | `~`                                    | left  |
//! | 8     | `+` `-`                                | left  |
//! | 9     | `*` `/` `%`                            | left  |
//! | 10    | `^`                                    | right |
//! | 11    | `is` `as`                              | left  |
//! | 12    | unary `-` `!`                          |       |
//! | 13    | `.` `?.` `[]` `()` `->`                | left  |
//!
//! Onshape publishes no precedence table, so the placement of `is`/`as` and
//! `~` is inferred from code that the standard library compiles:
//! `parameter as Vector * meter` (splineUtils.fs) only makes sense as
//! `(parameter as Vector) * meter`, so `is`/`as` bind tighter than arithmetic.

use crate::ast::*;
use crate::error::{ParseError, ParseResult};
use crate::lexer::Lexer;
use crate::span::Span;
use crate::token::{Keyword, Token, TokenKind};

/// Parse a complete FeatureScript module.
pub fn parse_module(src: &str) -> ParseResult<Module> {
    let tokens = Lexer::new(src).keep_doc_comments(true).tokenize()?;
    Parser::new(tokens).module()
}

/// Parse a single expression (useful for tests and REPL-style tools).
pub fn parse_expr(src: &str) -> ParseResult<Expr> {
    let tokens = Lexer::new(src).keep_doc_comments(false).tokenize()?;
    let mut p = Parser::new(tokens);
    let e = p.expr()?;
    p.expect(&TokenKind::Eof)?;
    Ok(e)
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Doc comment seen since the last real token was consumed.
    pending_doc: Option<String>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        let mut p = Parser {
            tokens,
            pos: 0,
            pending_doc: None,
        };
        p.skip_docs();
        p
    }

    // ------------------------------------------------------------------
    // Token stream helpers
    // ------------------------------------------------------------------

    fn skip_docs(&mut self) {
        while let TokenKind::DocComment(text) = &self.tokens[self.pos].kind {
            self.pending_doc = Some(text.clone());
            self.pos += 1;
        }
    }

    fn take_doc(&mut self) -> Option<String> {
        self.pending_doc.take()
    }

    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    /// Peek `n` tokens ahead, skipping doc comments.
    fn peek_nth(&self, n: usize) -> &TokenKind {
        let mut i = self.pos;
        let mut remaining = n;
        loop {
            if let TokenKind::DocComment(_) = self.tokens[i].kind {
                i += 1;
                continue;
            }
            if remaining == 0 {
                return &self.tokens[i].kind;
            }
            remaining -= 1;
            if self.tokens[i].kind == TokenKind::Eof {
                return &self.tokens[i].kind;
            }
            i += 1;
        }
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn prev_span(&self) -> Span {
        let mut i = self.pos;
        while i > 0 {
            i -= 1;
            if !matches!(self.tokens[i].kind, TokenKind::DocComment(_)) {
                return self.tokens[i].span;
            }
        }
        self.tokens[0].span
    }

    fn advance(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if t.kind != TokenKind::Eof {
            self.pos += 1;
        }
        self.pending_doc = None;
        self.skip_docs();
        t
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.peek() == kind
    }

    fn at_kw(&self, kw: Keyword) -> bool {
        matches!(self.peek(), TokenKind::Kw(k) if *k == kw)
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, kw: Keyword) -> bool {
        if self.at_kw(kw) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn error<T>(&self, msg: impl Into<String>) -> ParseResult<T> {
        Err(ParseError::new(self.span(), msg))
    }

    fn expect(&mut self, kind: &TokenKind) -> ParseResult<Token> {
        if self.at(kind) {
            Ok(self.advance())
        } else {
            self.error(format!("expected {kind}, found {}", self.peek()))
        }
    }

    fn expect_kw(&mut self, kw: Keyword) -> ParseResult<Token> {
        self.expect(&TokenKind::Kw(kw))
    }

    fn ident_text(kind: &TokenKind) -> Option<&str> {
        match kind {
            TokenKind::Ident(s) => Some(s.as_str()),
            TokenKind::Kw(k) if k.is_soft() => Some(k.as_str()),
            _ => None,
        }
    }

    fn expect_ident(&mut self) -> ParseResult<Ident> {
        if let Some(name) = Self::ident_text(self.peek()) {
            let name = name.to_string();
            let span = self.span();
            self.advance();
            Ok(Ident { name, span })
        } else {
            self.error(format!("expected identifier, found {}", self.peek()))
        }
    }

    // ------------------------------------------------------------------
    // Module & items
    // ------------------------------------------------------------------

    pub fn module(&mut self) -> ParseResult<Module> {
        let start = self.span();
        let header = if self.at_kw(Keyword::FeatureScript) {
            Some(self.version_header()?)
        } else {
            None
        };
        let mut items = Vec::new();
        while !self.at(&TokenKind::Eof) {
            items.push(self.item()?);
        }
        let end = self.prev_span();
        Ok(Module {
            header,
            items,
            span: start.to(end),
        })
    }

    fn version_header(&mut self) -> ParseResult<VersionHeader> {
        let start = self.span();
        self.expect_kw(Keyword::FeatureScript)?;
        let version = match self.peek().clone() {
            TokenKind::Number(n) => {
                self.advance();
                let v = n
                    .parse::<f64>()
                    .map_err(|_| ParseError::new(start, "invalid version number"))?;
                Version::Number(v as u64)
            }
            TokenKind::Sparkle => {
                self.advance();
                Version::Sparkle
            }
            other => {
                return self.error(format!(
                    "expected version number after `FeatureScript`, found {other}"
                ))
            }
        };
        self.expect(&TokenKind::Semi)?;
        Ok(VersionHeader {
            version,
            span: start.to(self.prev_span()),
        })
    }

    fn item(&mut self) -> ParseResult<Item> {
        let doc = self.take_doc();
        let start = self.span();
        let annotation = if self.at_kw(Keyword::Annotation) {
            Some(self.annotation()?)
        } else {
            None
        };
        // A doc comment may also sit between the annotation and the declaration.
        let doc = self.take_doc().or(doc);
        let export = self.eat_kw(Keyword::Export);
        let kind = match self.peek() {
            TokenKind::Kw(Keyword::Import) => ItemKind::Import(self.import()?),
            TokenKind::Kw(Keyword::Function) => ItemKind::Function(self.function_decl()?),
            TokenKind::Kw(Keyword::Predicate) => ItemKind::Predicate(self.predicate_decl()?),
            TokenKind::Kw(Keyword::Operator) => ItemKind::Operator(self.operator_decl()?),
            TokenKind::Kw(Keyword::Type) => ItemKind::Type(self.type_decl()?),
            TokenKind::Kw(Keyword::Enum) => ItemKind::Enum(self.enum_decl()?),
            TokenKind::Kw(Keyword::Const) => {
                self.advance();
                ItemKind::Const(self.var_decl_rest(start)?)
            }
            TokenKind::Kw(Keyword::Var) => {
                self.advance();
                ItemKind::Var(self.var_decl_rest(start)?)
            }
            other => return self.error(format!("expected a declaration, found {other}")),
        };
        Ok(Item {
            doc,
            annotation,
            export,
            kind,
            span: start.to(self.prev_span()),
        })
    }

    fn annotation(&mut self) -> ParseResult<Annotation> {
        let start = self.span();
        self.expect_kw(Keyword::Annotation)?;
        let entries = self.map_body()?;
        Ok(Annotation {
            entries,
            span: start.to(self.prev_span()),
        })
    }

    fn import(&mut self) -> ParseResult<Import> {
        let start = self.span();
        self.expect_kw(Keyword::Import)?;
        self.expect(&TokenKind::LParen)?;
        let mut args = Vec::new();
        while !self.at(&TokenKind::RParen) {
            let a_start = self.span();
            let name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let value = self.expr()?;
            args.push(NamedArg {
                name,
                value,
                span: a_start.to(self.prev_span()),
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Semi)?;
        Ok(Import {
            args,
            span: start.to(self.prev_span()),
        })
    }

    fn function_decl(&mut self) -> ParseResult<FunctionDecl> {
        let start = self.span();
        self.expect_kw(Keyword::Function)?;
        let name = self.expect_ident()?;
        let sig = self.function_sig()?;
        Ok(FunctionDecl {
            name,
            sig,
            span: start.to(self.prev_span()),
        })
    }

    /// `(params) [returns T] [precondition Block] Block`
    fn function_sig(&mut self) -> ParseResult<FunctionSig> {
        let start = self.span();
        let params = self.params()?;
        let returns = if self.eat_kw(Keyword::Returns) {
            Some(self.type_ref()?)
        } else {
            None
        };
        let precondition = if self.eat_kw(Keyword::Precondition) {
            Some(self.precondition_body()?)
        } else {
            None
        };
        let body = self.block()?;
        Ok(FunctionSig {
            params,
            returns,
            precondition,
            body,
            span: start.to(self.prev_span()),
        })
    }

    /// `precondition { ... }` or the brace-less `precondition expr;` form; the
    /// latter is wrapped in a single-statement block.
    fn precondition_body(&mut self) -> ParseResult<Block> {
        if self.at(&TokenKind::LBrace) {
            return self.block();
        }
        let stmt = self.stmt()?;
        let span = stmt.span;
        Ok(Block {
            stmts: vec![stmt],
            span,
        })
    }

    fn params(&mut self) -> ParseResult<Vec<Param>> {
        self.expect(&TokenKind::LParen)?;
        let mut params = Vec::new();
        while !self.at(&TokenKind::RParen) {
            let start = self.span();
            let name = self.expect_ident()?;
            let ty = if self.eat_kw(Keyword::Is) {
                Some(self.type_ref()?)
            } else {
                None
            };
            params.push(Param {
                name,
                ty,
                span: start.to(self.prev_span()),
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen)?;
        Ok(params)
    }

    fn type_ref(&mut self) -> ParseResult<TypeRef> {
        let span = self.span();
        let name = match self.peek() {
            TokenKind::Kw(Keyword::Function) => "function".to_string(),
            TokenKind::Kw(Keyword::Undefined) => "undefined".to_string(),
            k => match Self::ident_text(k) {
                Some(s) => s.to_string(),
                None => return self.error(format!("expected type name, found {k}")),
            },
        };
        self.advance();
        Ok(TypeRef { name, span })
    }

    fn predicate_decl(&mut self) -> ParseResult<PredicateDecl> {
        let start = self.span();
        self.expect_kw(Keyword::Predicate)?;
        let name = self.expect_ident()?;
        let params = self.params()?;
        let precondition = if self.eat_kw(Keyword::Precondition) {
            Some(self.precondition_body()?)
        } else {
            None
        };
        let body = self.block()?;
        Ok(PredicateDecl {
            name,
            params,
            precondition,
            body,
            span: start.to(self.prev_span()),
        })
    }

    fn operator_decl(&mut self) -> ParseResult<OperatorDecl> {
        let start = self.span();
        self.expect_kw(Keyword::Operator)?;
        use TokenKind::*;
        let op = match self.peek() {
            Plus => "+",
            Minus => "-",
            Star => "*",
            Slash => "/",
            Percent => "%",
            Caret => "^",
            Tilde => "~",
            Bang => "!",
            EqEq => "==",
            BangEq => "!=",
            Lt => "<",
            LtEq => "<=",
            Gt => ">",
            GtEq => ">=",
            other => return self.error(format!("expected operator symbol, found {other}")),
        }
        .to_string();
        self.advance();
        let sig = self.function_sig()?;
        Ok(OperatorDecl {
            op,
            sig,
            span: start.to(self.prev_span()),
        })
    }

    fn type_decl(&mut self) -> ParseResult<TypeDecl> {
        let start = self.span();
        self.expect_kw(Keyword::Type)?;
        let name = self.expect_ident()?;
        let typecheck = if self.eat_kw(Keyword::Typecheck) {
            Some(self.expect_ident()?)
        } else {
            None
        };
        self.expect(&TokenKind::Semi)?;
        Ok(TypeDecl {
            name,
            typecheck,
            span: start.to(self.prev_span()),
        })
    }

    fn enum_decl(&mut self) -> ParseResult<EnumDecl> {
        let start = self.span();
        self.expect_kw(Keyword::Enum)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let mut members = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            let doc = self.take_doc();
            let m_start = self.span();
            let annotation = if self.at_kw(Keyword::Annotation) {
                Some(self.annotation()?)
            } else {
                None
            };
            let doc = self.take_doc().or(doc);
            let name = self.expect_ident()?;
            members.push(EnumMember {
                doc,
                annotation,
                name,
                span: m_start.to(self.prev_span()),
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace)?;
        // Some files terminate the enum with `;`, most don't.
        self.eat(&TokenKind::Semi);
        Ok(EnumDecl {
            name,
            members,
            span: start.to(self.prev_span()),
        })
    }

    /// After `var` / `const` has been consumed: `name [is T] [= expr] ;`
    fn var_decl_rest(&mut self, start: Span) -> ParseResult<VarDecl> {
        let name = self.expect_ident()?;
        let ty = if self.eat_kw(Keyword::Is) {
            Some(self.type_ref()?)
        } else {
            None
        };
        let init = if self.eat(&TokenKind::Eq) {
            Some(self.expr()?)
        } else {
            None
        };
        self.expect(&TokenKind::Semi)?;
        Ok(VarDecl {
            name,
            ty,
            init,
            span: start.to(self.prev_span()),
        })
    }

    // ------------------------------------------------------------------
    // Statements
    // ------------------------------------------------------------------

    fn block(&mut self) -> ParseResult<Block> {
        let start = self.span();
        self.expect(&TokenKind::LBrace)?;
        let mut stmts = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            if self.at(&TokenKind::Eof) {
                return self.error("unexpected end of file inside block");
            }
            stmts.push(self.stmt()?);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Block {
            stmts,
            span: start.to(self.prev_span()),
        })
    }

    fn stmt(&mut self) -> ParseResult<Stmt> {
        let start = self.span();
        let annotation = if self.at_kw(Keyword::Annotation) {
            Some(self.annotation()?)
        } else {
            None
        };
        let kind = self.stmt_kind(start)?;
        Ok(Stmt {
            annotation,
            kind,
            span: start.to(self.prev_span()),
        })
    }

    fn boxed_stmt(&mut self) -> ParseResult<Box<Stmt>> {
        Ok(Box::new(self.stmt()?))
    }

    fn stmt_kind(&mut self, start: Span) -> ParseResult<StmtKind> {
        use Keyword as K;
        match self.peek() {
            TokenKind::LBrace => Ok(StmtKind::Block(self.block()?)),
            TokenKind::Semi => {
                self.advance();
                Ok(StmtKind::Empty)
            }
            TokenKind::Kw(K::Var) => {
                self.advance();
                Ok(StmtKind::Var(self.var_decl_rest(start)?))
            }
            TokenKind::Kw(K::Const) => {
                self.advance();
                Ok(StmtKind::Const(self.var_decl_rest(start)?))
            }
            TokenKind::Kw(K::If) => {
                self.advance();
                self.expect(&TokenKind::LParen)?;
                let cond = self.expr()?;
                self.expect(&TokenKind::RParen)?;
                let then = self.boxed_stmt()?;
                let else_ = if self.eat_kw(K::Else) {
                    Some(self.boxed_stmt()?)
                } else {
                    None
                };
                Ok(StmtKind::If { cond, then, else_ })
            }
            TokenKind::Kw(K::While) => {
                self.advance();
                self.expect(&TokenKind::LParen)?;
                let cond = self.expr()?;
                self.expect(&TokenKind::RParen)?;
                let body = self.boxed_stmt()?;
                Ok(StmtKind::While { cond, body })
            }
            TokenKind::Kw(K::For) => self.for_stmt(),
            TokenKind::Kw(K::Break) => {
                self.advance();
                self.expect(&TokenKind::Semi)?;
                Ok(StmtKind::Break)
            }
            TokenKind::Kw(K::Continue) => {
                self.advance();
                self.expect(&TokenKind::Semi)?;
                Ok(StmtKind::Continue)
            }
            TokenKind::Kw(K::Return) => {
                self.advance();
                let value = if self.at(&TokenKind::Semi) {
                    None
                } else {
                    Some(self.expr()?)
                };
                self.expect(&TokenKind::Semi)?;
                Ok(StmtKind::Return(value))
            }
            TokenKind::Kw(K::Throw) => {
                self.advance();
                let value = self.expr()?;
                self.expect(&TokenKind::Semi)?;
                Ok(StmtKind::Throw(value))
            }
            // `try { ... }` / `try silent { ... }` are statements; `try(expr)`
            // and `try silent(expr)` are expressions and fall through.
            TokenKind::Kw(K::Try) if self.try_is_statement() => {
                self.advance();
                let silent = self.eat_kw(K::Silent);
                let body = self.block()?;
                let catch = if self.at_kw(K::Catch) {
                    let c_start = self.span();
                    self.advance();
                    let param = if self.eat(&TokenKind::LParen) {
                        let p = self.expect_ident()?;
                        self.expect(&TokenKind::RParen)?;
                        Some(p)
                    } else {
                        None
                    };
                    let body = self.block()?;
                    Some(CatchClause {
                        param,
                        body,
                        span: c_start.to(self.prev_span()),
                    })
                } else {
                    None
                };
                Ok(StmtKind::Try {
                    silent,
                    body,
                    catch,
                })
            }
            _ => {
                let e = self.expr()?;
                self.expect(&TokenKind::Semi)?;
                Ok(StmtKind::Expr(e))
            }
        }
    }

    fn try_is_statement(&self) -> bool {
        match self.peek_nth(1) {
            TokenKind::LBrace => true,
            TokenKind::Kw(Keyword::Silent) => matches!(self.peek_nth(2), TokenKind::LBrace),
            _ => false,
        }
    }

    fn for_stmt(&mut self) -> ParseResult<StmtKind> {
        self.expect_kw(Keyword::For)?;
        self.expect(&TokenKind::LParen)?;
        let init_start = self.span();

        // for-in: `var name in`, `var key, value in`, and (rarely) `name in`.
        let declared = self.at_kw(Keyword::Var) || self.at_kw(Keyword::Const);
        let off = usize::from(declared);
        let is_for_in = match self.peek_nth(off + 1) {
            TokenKind::Kw(Keyword::In) => true,
            TokenKind::Comma => matches!(self.peek_nth(off + 3), TokenKind::Kw(Keyword::In)),
            _ => false,
        } && self.at_ident_nth(off);
        if is_for_in {
            if declared {
                self.advance();
            }
            let first = self.expect_ident()?;
            let (key, value) = if self.eat(&TokenKind::Comma) {
                let second = self.expect_ident()?;
                (Some(first), second)
            } else {
                (None, first)
            };
            self.expect_kw(Keyword::In)?;
            let iter = self.expr()?;
            self.expect(&TokenKind::RParen)?;
            let body = self.boxed_stmt()?;
            return Ok(StmtKind::ForIn {
                declared,
                key,
                value,
                iter,
                body,
            });
        }

        // C-style.
        let init = if self.at(&TokenKind::Semi) {
            self.advance();
            None
        } else if self.at_kw(Keyword::Var) || self.at_kw(Keyword::Const) {
            self.advance();
            Some(ForInit::Var(self.var_decl_rest(init_start)?)) // consumes the `;`
        } else {
            let e = self.expr()?;
            self.expect(&TokenKind::Semi)?;
            Some(ForInit::Expr(e))
        };
        let cond = if self.at(&TokenKind::Semi) {
            None
        } else {
            Some(self.expr()?)
        };
        self.expect(&TokenKind::Semi)?;
        let step = if self.at(&TokenKind::RParen) {
            None
        } else {
            Some(self.expr()?)
        };
        self.expect(&TokenKind::RParen)?;
        let body = self.boxed_stmt()?;
        Ok(StmtKind::For {
            init,
            cond,
            step,
            body,
        })
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    pub fn expr(&mut self) -> ParseResult<Expr> {
        self.assignment()
    }

    fn mk(&self, start: Span, kind: ExprKind) -> Expr {
        Expr {
            kind,
            span: start.to(self.prev_span()),
        }
    }

    fn assignment(&mut self) -> ParseResult<Expr> {
        let start = self.span();
        let lhs = self.ternary()?;
        use TokenKind::*;
        let op = match self.peek() {
            Eq => AssignOp::Assign,
            PlusEq => AssignOp::Add,
            MinusEq => AssignOp::Sub,
            StarEq => AssignOp::Mul,
            SlashEq => AssignOp::Div,
            PercentEq => AssignOp::Mod,
            TildeEq => AssignOp::Concat,
            CaretEq => AssignOp::Pow,
            PipePipeEq => AssignOp::Or,
            AmpAmpEq => AssignOp::And,
            QuestionQuestionEq => AssignOp::Coalesce,
            _ => return Ok(lhs),
        };
        if !is_assignable(&lhs) {
            return self.error("invalid assignment target");
        }
        self.advance();
        let value = self.assignment()?;
        Ok(self.mk(
            start,
            ExprKind::Assign {
                op,
                target: Box::new(lhs),
                value: Box::new(value),
            },
        ))
    }

    fn ternary(&mut self) -> ParseResult<Expr> {
        let start = self.span();
        let cond = self.binary(0)?;
        if self.eat(&TokenKind::Question) {
            let then = self.assignment()?;
            self.expect(&TokenKind::Colon)?;
            let else_ = self.assignment()?;
            return Ok(self.mk(
                start,
                ExprKind::Ternary {
                    cond: Box::new(cond),
                    then: Box::new(then),
                    else_: Box::new(else_),
                },
            ));
        }
        Ok(cond)
    }

    /// Binary precedence level of the current token, if it is a binary operator.
    fn binary_op(&self) -> Option<(u8, BinOrType)> {
        use BinOrType::Bin;
        use BinaryOp as B;
        use TokenKind as T;
        Some(match self.peek() {
            T::PipePipe => (1, Bin(B::Or)),
            T::QuestionQuestion => (1, Bin(B::Coalesce)),
            T::AmpAmp => (2, Bin(B::And)),
            T::EqEq => (3, Bin(B::Eq)),
            T::BangEq => (3, Bin(B::Ne)),
            T::Lt => (4, Bin(B::Lt)),
            T::LtEq => (4, Bin(B::Le)),
            T::Gt => (4, Bin(B::Gt)),
            T::GtEq => (4, Bin(B::Ge)),
            T::Tilde => (5, Bin(B::Concat)),
            T::Plus => (6, Bin(B::Add)),
            T::Minus => (6, Bin(B::Sub)),
            T::Star => (7, Bin(B::Mul)),
            T::Slash => (7, Bin(B::Div)),
            T::Percent => (7, Bin(B::Mod)),
            T::Caret => (8, Bin(B::Pow)),
            T::Kw(Keyword::Is) => (9, BinOrType::Is),
            T::Kw(Keyword::As) => (9, BinOrType::As),
            _ => return None,
        })
    }

    /// Precedence climbing over binary levels (see module docs).
    fn binary(&mut self, min_level: u8) -> ParseResult<Expr> {
        let start = self.span();
        let mut lhs = self.unary()?;
        while let Some((level, op)) = self.binary_op() {
            if level < min_level {
                break;
            }
            self.advance();
            match op {
                BinOrType::Is => {
                    let ty = self.type_ref()?;
                    lhs = self.mk(
                        start,
                        ExprKind::Is {
                            expr: Box::new(lhs),
                            ty,
                        },
                    );
                }
                BinOrType::As => {
                    let ty = self.type_ref()?;
                    lhs = self.mk(
                        start,
                        ExprKind::As {
                            expr: Box::new(lhs),
                            ty,
                        },
                    );
                }
                BinOrType::Bin(op) => {
                    // `^` is right-associative; everything else is left.
                    let next_min = if op == BinaryOp::Pow {
                        level
                    } else {
                        level + 1
                    };
                    let rhs = self.binary(next_min)?;
                    lhs = self.mk(
                        start,
                        ExprKind::Binary {
                            op,
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                        },
                    );
                }
            }
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> ParseResult<Expr> {
        let start = self.span();
        let op = match self.peek() {
            TokenKind::Minus => Some(UnaryOp::Neg),
            TokenKind::Bang => Some(UnaryOp::Not),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let expr = self.unary()?;
            return Ok(self.mk(
                start,
                ExprKind::Unary {
                    op,
                    expr: Box::new(expr),
                },
            ));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> ParseResult<Expr> {
        let start = self.span();
        let mut e = self.primary()?;
        loop {
            match self.peek() {
                TokenKind::Dot | TokenKind::QuestionDot => {
                    let safe = self.at(&TokenKind::QuestionDot);
                    self.advance();
                    let name = self.expect_ident()?;
                    e = self.mk(
                        start,
                        ExprKind::Member {
                            object: Box::new(e),
                            name,
                            safe,
                        },
                    );
                }
                TokenKind::LBracket => {
                    self.advance();
                    if self.eat(&TokenKind::RBracket) {
                        e = self.mk(start, ExprKind::Deref(Box::new(e)));
                        continue;
                    }
                    let index = self.expr()?;
                    self.expect(&TokenKind::RBracket)?;
                    e = self.mk(
                        start,
                        ExprKind::Index {
                            object: Box::new(e),
                            index: Box::new(index),
                        },
                    );
                }
                TokenKind::LParen => {
                    let args = self.call_args()?;
                    e = self.mk(
                        start,
                        ExprKind::Call {
                            callee: Box::new(e),
                            args,
                        },
                    );
                }
                TokenKind::Arrow => {
                    self.advance();
                    // The right-hand side is a callee (identifier, possibly with
                    // member access) followed by an argument list.
                    let c_start = self.span();
                    let mut callee = self.primary()?;
                    while matches!(self.peek(), TokenKind::Dot) {
                        self.advance();
                        let name = self.expect_ident()?;
                        callee = self.mk(
                            c_start,
                            ExprKind::Member {
                                object: Box::new(callee),
                                name,
                                safe: false,
                            },
                        );
                    }
                    if !self.at(&TokenKind::LParen) {
                        return self.error("expected call arguments after `->`");
                    }
                    let args = self.call_args()?;
                    let call = self.mk(
                        c_start,
                        ExprKind::Call {
                            callee: Box::new(callee),
                            args,
                        },
                    );
                    e = self.mk(
                        start,
                        ExprKind::Pipe {
                            receiver: Box::new(e),
                            call: Box::new(call),
                        },
                    );
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn call_args(&mut self) -> ParseResult<Vec<Expr>> {
        self.expect(&TokenKind::LParen)?;
        let mut args = Vec::new();
        while !self.at(&TokenKind::RParen) {
            args.push(self.expr()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen)?;
        Ok(args)
    }

    /// Does the token stream at `(` look like `(a, b is Type, c) =>`?
    fn at_paren_lambda(&self) -> bool {
        let mut i = 1;
        loop {
            match self.peek_nth(i) {
                TokenKind::RParen => {
                    return match self.peek_nth(i + 1) {
                        TokenKind::FatArrow => true,
                        TokenKind::Kw(Keyword::Returns) => {
                            matches!(self.peek_nth(i + 3), TokenKind::FatArrow)
                        }
                        _ => false,
                    }
                }
                k if Self::ident_text(k).is_some() => {
                    i += 1;
                    if matches!(self.peek_nth(i), TokenKind::Kw(Keyword::Is)) {
                        i += 2; // `is Type`
                    }
                    match self.peek_nth(i) {
                        TokenKind::Comma => i += 1,
                        TokenKind::RParen => {}
                        _ => return false,
                    }
                }
                _ => return false,
            }
        }
    }

    /// After `=>`: an expression, or a `{ ... }` block. A `{` is a block
    /// unless it reads as a map literal (`{}` or `{ key : value, ... }`).
    fn lambda_body(&mut self) -> ParseResult<LambdaBody> {
        if self.at(&TokenKind::LBrace) && !self.brace_is_map_literal() {
            return Ok(LambdaBody::Block(self.block()?));
        }
        Ok(LambdaBody::Expr(self.assignment()?))
    }

    /// Heuristic lookahead at a `{`: scan to the first top-level `:` (map)
    /// or `;` (block), ignoring `:` that pairs with a `?`.
    fn brace_is_map_literal(&self) -> bool {
        use TokenKind::*;
        match self.peek_nth(1) {
            RBrace => return true,
            Kw(k)
                if !k.is_soft()
                    && !matches!(
                        k,
                        Keyword::True
                            | Keyword::False
                            | Keyword::Undefined
                            | Keyword::Function
                            | Keyword::Try
                    ) =>
            {
                return false
            }
            _ => {}
        }
        let mut depth = 0i32;
        let mut open_ternaries = 0i32;
        let mut i = 1;
        loop {
            match self.peek_nth(i) {
                Eof => return false,
                LParen | LBracket | LBrace => depth += 1,
                RParen | RBracket | RBrace => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                Question if depth == 0 => open_ternaries += 1,
                Colon if depth == 0 => {
                    if open_ternaries > 0 {
                        open_ternaries -= 1;
                    } else {
                        return true;
                    }
                }
                Semi if depth == 0 => return false,
                Eq | PlusEq | MinusEq | StarEq | SlashEq | PercentEq | CaretEq | TildeEq
                | PipePipeEq | AmpAmpEq | QuestionQuestionEq
                    if depth == 0 =>
                {
                    return false
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn primary(&mut self) -> ParseResult<Expr> {
        let start = self.span();
        use Keyword as K;
        let tok = self.peek().clone();
        match tok {
            TokenKind::Number(text) => {
                self.advance();
                let v = text
                    .parse::<f64>()
                    .map_err(|_| ParseError::new(start, format!("invalid number `{text}`")))?;
                Ok(self.mk(start, ExprKind::Number(v)))
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(self.mk(start, ExprKind::Str(s)))
            }
            TokenKind::Kw(K::True) => {
                self.advance();
                Ok(self.mk(start, ExprKind::Bool(true)))
            }
            TokenKind::Kw(K::False) => {
                self.advance();
                Ok(self.mk(start, ExprKind::Bool(false)))
            }
            TokenKind::Kw(K::Undefined) => {
                self.advance();
                Ok(self.mk(start, ExprKind::Undefined))
            }
            TokenKind::LParen => {
                if self.at_paren_lambda() {
                    let params = self.params()?;
                    let returns = if self.eat_kw(Keyword::Returns) {
                        Some(self.type_ref()?)
                    } else {
                        None
                    };
                    self.expect(&TokenKind::FatArrow)?;
                    let body = self.lambda_body()?;
                    return Ok(self.mk(
                        start,
                        ExprKind::Lambda {
                            params,
                            returns,
                            body: Box::new(body),
                        },
                    ));
                }
                self.advance();
                let inner = self.expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(self.mk(start, ExprKind::Paren(Box::new(inner))))
            }
            TokenKind::LBracket => {
                self.advance();
                let mut items = Vec::new();
                while !self.at(&TokenKind::RBracket) {
                    items.push(self.expr()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RBracket)?;
                Ok(self.mk(start, ExprKind::Array(items)))
            }
            TokenKind::LBrace => {
                let entries = self.map_body()?;
                Ok(self.mk(start, ExprKind::Map(entries)))
            }
            TokenKind::Kw(K::Function) => {
                self.advance();
                let sig = self.function_sig()?;
                Ok(self.mk(start, ExprKind::Function(sig)))
            }
            TokenKind::Kw(K::Try) => {
                self.advance();
                let silent = self.eat_kw(K::Silent);
                self.expect(&TokenKind::LParen)?;
                let expr = self.expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(self.mk(
                    start,
                    ExprKind::Try {
                        silent,
                        expr: Box::new(expr),
                    },
                ))
            }
            TokenKind::Kw(K::Switch) if matches!(self.peek_nth(1), TokenKind::LParen) => {
                self.advance();
                self.expect(&TokenKind::LParen)?;
                let scrutinee = self.expr()?;
                self.expect(&TokenKind::RParen)?;
                let cases = self.map_body()?;
                Ok(self.mk(
                    start,
                    ExprKind::Switch {
                        scrutinee: Box::new(scrutinee),
                        cases,
                    },
                ))
            }
            TokenKind::Kw(K::New) if self.at_ident_nth(1) => {
                self.advance();
                let inner = self.postfix()?;
                Ok(self.mk(start, ExprKind::New(Box::new(inner))))
            }
            k if Self::ident_text(&k).is_some() => {
                let name = self.expect_ident()?;
                if self.eat(&TokenKind::FatArrow) {
                    let param = Param {
                        span: name.span,
                        name,
                        ty: None,
                    };
                    let body = self.lambda_body()?;
                    return Ok(self.mk(
                        start,
                        ExprKind::Lambda {
                            params: vec![param],
                            returns: None,
                            body: Box::new(body),
                        },
                    ));
                }
                Ok(self.mk(start, ExprKind::Ident(name.name)))
            }
            other => self.error(format!("expected expression, found {other}")),
        }
    }

    fn at_ident_nth(&self, n: usize) -> bool {
        Self::ident_text(self.peek_nth(n)).is_some()
    }

    /// `{ key : value, ... }` — shared by map literals, annotations and switch.
    fn map_body(&mut self) -> ParseResult<Vec<MapEntry>> {
        self.expect(&TokenKind::LBrace)?;
        let mut entries = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            let start = self.span();
            let key = self.expr()?;
            self.expect(&TokenKind::Colon)?;
            let value = self.expr()?;
            entries.push(MapEntry {
                key,
                value,
                span: start.to(self.prev_span()),
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(entries)
    }
}

enum BinOrType {
    Bin(BinaryOp),
    Is,
    As,
}

fn is_assignable(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Ident(_)
        | ExprKind::Member { .. }
        | ExprKind::Index { .. }
        | ExprKind::Deref(_) => true,
        ExprKind::Paren(inner) => is_assignable(inner),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(src: &str) -> Expr {
        parse_expr(src).unwrap_or_else(|e| panic!("{src}: {e}"))
    }

    fn module(src: &str) -> Module {
        parse_module(src).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn precedence_arith() {
        let e = expr("1 + 2 * 3 ^ 2 ^ 2");
        // (1 + (2 * (3 ^ (2 ^ 2))))
        let ExprKind::Binary {
            op: BinaryOp::Add,
            rhs,
            ..
        } = &e.kind
        else {
            panic!()
        };
        let ExprKind::Binary {
            op: BinaryOp::Mul,
            rhs,
            ..
        } = &rhs.kind
        else {
            panic!()
        };
        let ExprKind::Binary {
            op: BinaryOp::Pow,
            rhs,
            ..
        } = &rhs.kind
        else {
            panic!()
        };
        assert!(matches!(
            rhs.kind,
            ExprKind::Binary {
                op: BinaryOp::Pow,
                ..
            }
        ));
    }

    #[test]
    fn is_and_as_bind_tighter_than_arithmetic() {
        let e = expr("value is map && x is undefined");
        let ExprKind::Binary {
            op: BinaryOp::And,
            lhs,
            rhs,
        } = &e.kind
        else {
            panic!()
        };
        assert!(matches!(lhs.kind, ExprKind::Is { .. }));
        let ExprKind::Is { ty, .. } = &rhs.kind else {
            panic!()
        };
        assert_eq!(ty.name, "undefined");
        // (parameter as Vector) * meter
        let e = expr("parameter as Vector * meter");
        let ExprKind::Binary {
            op: BinaryOp::Mul,
            lhs,
            ..
        } = &e.kind
        else {
            panic!()
        };
        assert!(matches!(lhs.kind, ExprKind::As { .. }));
        // !(x is boolean) is what people write; unparenthesised `!x is boolean` is `(!x) is boolean`.
        assert!(matches!(expr("!x is boolean").kind, ExprKind::Is { .. }));
        assert!(matches!(
            expr("x ||= y").kind,
            ExprKind::Assign {
                op: AssignOp::Or,
                ..
            }
        ));
    }

    #[test]
    fn pipe_desugars() {
        let e = expr("ALL_BODIES -> qBodyType(BodyType.SOLID) -> qFilter(x)");
        let d = e.desugar_pipe();
        let ExprKind::Call { callee, args } = &d.kind else {
            panic!()
        };
        assert!(matches!(&callee.kind, ExprKind::Ident(n) if n == "qFilter"));
        assert_eq!(args.len(), 2);
        assert!(matches!(args[0].kind, ExprKind::Pipe { .. }));
    }

    #[test]
    fn lambdas() {
        assert!(matches!(expr("x => x + 1").kind, ExprKind::Lambda { .. }));
        let ExprKind::Lambda { params, .. } =
            expr("(soFar, next) => soFar->mergeMaps([next], true)").kind
        else {
            panic!()
        };
        assert_eq!(params.len(), 2);
        // Parenthesised expression is not a lambda.
        assert!(matches!(expr("(a) + 1").kind, ExprKind::Binary { .. }));
    }

    #[test]
    fn try_expr_and_stmt() {
        assert!(matches!(
            expr("try silent(f(x))").kind,
            ExprKind::Try { silent: true, .. }
        ));
        let m = module("function f() { try { g(); } catch (e) { throw e; } try silent { h(); } }");
        let ItemKind::Function(f) = &m.items[0].kind else {
            panic!()
        };
        assert!(matches!(
            &f.sig.body.stmts[0].kind,
            StmtKind::Try {
                silent: false,
                catch: Some(_),
                ..
            }
        ));
        assert!(matches!(
            &f.sig.body.stmts[1].kind,
            StmtKind::Try {
                silent: true,
                catch: None,
                ..
            }
        ));
    }

    #[test]
    fn typed_lambda_block_body_and_deref() {
        let e = expr("(index is number, data is map) => { \"index\" : index, \"data\" : data }");
        let ExprKind::Lambda { params, body, .. } = &e.kind else {
            panic!()
        };
        assert_eq!(params[0].ty.as_ref().unwrap().name, "number");
        let e = expr("(c is Context) returns boolean => { return true; }");
        let ExprKind::Lambda {
            returns: Some(r), ..
        } = &e.kind
        else {
            panic!()
        };
        assert_eq!(r.name, "boolean");
        assert!(matches!(**body, LambdaBody::Expr(_)));
        let e = expr("q => { const a = f(q); return a; }");
        let ExprKind::Lambda { body, .. } = &e.kind else {
            panic!()
        };
        assert!(matches!(**body, LambdaBody::Block(_)));
        let e = expr("x => { y = a ? b : c; }");
        let ExprKind::Lambda { body, .. } = &e.kind else {
            panic!()
        };
        assert!(matches!(**body, LambdaBody::Block(_)));
        assert!(matches!(expr("counter[]").kind, ExprKind::Deref(_)));
        assert!(matches!(
            expr("counter[] = 1").kind,
            ExprKind::Assign { .. }
        ));
        let m = module("function f(v) precondition @size(v) == 3; { return v; } function g() { for (e in xs) {} } predicate p(a, b) precondition a.unit == b.unit; { a < b; }");
        let ItemKind::Function(f) = &m.items[0].kind else {
            panic!()
        };
        assert_eq!(f.sig.precondition.as_ref().unwrap().stmts.len(), 1);
        let ItemKind::Function(g) = &m.items[1].kind else {
            panic!()
        };
        assert!(matches!(
            &g.sig.body.stmts[0].kind,
            StmtKind::ForIn {
                declared: false,
                ..
            }
        ));
        let ItemKind::Predicate(p) = &m.items[2].kind else {
            panic!()
        };
        assert!(p.precondition.is_some());
    }

    #[test]
    fn switch_and_new() {
        assert!(matches!(
            expr("switch (k) { A.X : 0, A.Y : 1 }").kind,
            ExprKind::Switch { .. }
        ));
        assert!(matches!(expr("new box({})").kind, ExprKind::New(_)));
    }

    #[test]
    fn feature_definition() {
        let src = r#"
FeatureScript 2960;
import(path : "onshape/std/geometry.fs", version : "2960.0");

/** Doc for extrude. */
annotation { "Feature Type Name" : "Extrude" }
export const extrude = defineFeature(function(context is Context, id is Id, definition is map)
    precondition
    {
        annotation { "Name" : "Entities", "Filter" : EntityType.FACE && ConstructionObject.NO }
        definition.entities is Query;
        isLength(definition.depth, LENGTH_BOUNDS);
    }
    {
        opExtrude(context, id + "extrude", { "entities" : definition.entities });
    });

export enum Kind { annotation { "Name" : "A" } A, B }
export type Foo typecheck canBeFoo;
export operator+(a is Foo, b is Foo) returns Foo { return a; }
export predicate canBeFoo(value) { value is map; for (var k, v in value) v is number; }
"#;
        let m = module(src);
        assert_eq!(m.header.as_ref().unwrap().version, Version::Number(2960));
        assert_eq!(m.items.len(), 6);
        let it = &m.items[1];
        assert!(it.export);
        assert_eq!(it.doc.as_deref(), Some(" Doc for extrude. "));
        assert!(it.annotation.is_some());
        let ItemKind::Const(v) = &it.kind else {
            panic!()
        };
        let ExprKind::Call { args, .. } = &v.init.as_ref().unwrap().kind else {
            panic!()
        };
        let ExprKind::Function(sig) = &args[0].kind else {
            panic!()
        };
        let pre = sig.precondition.as_ref().unwrap();
        assert!(pre.stmts[0].annotation.is_some());
        assert!(
            matches!(&pre.stmts[0].kind, StmtKind::Expr(e) if matches!(e.kind, ExprKind::Is { .. }))
        );
        let ItemKind::Import(i) = &m.items[0].kind else {
            panic!()
        };
        assert_eq!(i.path(), Some("onshape/std/geometry.fs"));
        let ItemKind::Enum(e) = &m.items[2].kind else {
            panic!()
        };
        assert!(e.members[0].annotation.is_some());
        assert_eq!(e.members[1].name.name, "B");
        let ItemKind::Operator(o) = &m.items[4].kind else {
            panic!()
        };
        assert_eq!(o.op, "+");
    }

    #[test]
    fn for_forms() {
        let m = module(
            "function f() { for (var i = 0; i < n; i += 1) {} for (var x in xs) g(x); for (var k, v in m) {} for (;;) break; }",
        );
        let ItemKind::Function(f) = &m.items[0].kind else {
            panic!()
        };
        assert!(matches!(
            &f.sig.body.stmts[0].kind,
            StmtKind::For {
                init: Some(ForInit::Var(_)),
                ..
            }
        ));
        assert!(matches!(
            &f.sig.body.stmts[1].kind,
            StmtKind::ForIn {
                key: None,
                declared: true,
                ..
            }
        ));
        assert!(matches!(
            &f.sig.body.stmts[2].kind,
            StmtKind::ForIn { key: Some(_), .. }
        ));
        assert!(matches!(
            &f.sig.body.stmts[3].kind,
            StmtKind::For {
                init: None,
                cond: None,
                step: None,
                ..
            }
        ));
    }

    #[test]
    fn error_message_has_position() {
        let e = parse_module("function f() { var x = ; }").unwrap_err();
        assert!(e.message.contains("expected expression"));
        assert_eq!(e.span.start, 23);
    }

    #[test]
    fn json_roundtrip() {
        let m = module("FeatureScript ✨; export const X = 1 * meter;");
        let json = serde_json::to_string(&m).unwrap();
        let back: Module = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }
}
