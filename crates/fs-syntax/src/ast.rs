//! Abstract syntax tree for FeatureScript.
//!
//! Every node carries a [`Span`] pointing back into the source text. The tree
//! is deliberately plain data (no interning, no arenas) so it is easy to walk
//! from an interpreter or to serialize with serde.

use crate::span::Span;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

macro_rules! node {
    ($(#[$m:meta])* pub struct $name:ident { $($(#[$fm:meta])* pub $f:ident : $t:ty),* $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq)]
        #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
        pub struct $name { $($(#[$fm])* pub $f: $t),* }
    };
    ($(#[$m:meta])* pub enum $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq)]
        #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
        #[allow(clippy::large_enum_variant)]
        pub enum $name { $($body)* }
    };
}

// ---------------------------------------------------------------------------
// Module level
// ---------------------------------------------------------------------------

node! {
    /// A whole `.fs` file.
    pub struct Module {
        /// The `FeatureScript <version>;` header, if present.
        pub header: Option<VersionHeader>,
        pub items: Vec<Item>,
        pub span: Span,
    }
}

node! {
    pub struct VersionHeader {
        pub version: Version,
        pub span: Span,
    }
}

node! {
    pub enum Version {
        Number(u64),
        /// The `✨` placeholder used by the std-library mirror's `without-versions` branch.
        Sparkle,
    }
}

node! {
    /// A simple identifier with its location.
    pub struct Ident {
        pub name: String,
        pub span: Span,
    }
}

node! {
    /// A top-level declaration, with its leading doc comment / annotation.
    pub struct Item {
        /// Text of an immediately preceding `/** ... */` comment.
        pub doc: Option<String>,
        pub annotation: Option<Annotation>,
        pub export: bool,
        pub kind: ItemKind,
        pub span: Span,
    }
}

node! {
    pub enum ItemKind {
        Import(Import),
        Function(FunctionDecl),
        Predicate(PredicateDecl),
        Operator(OperatorDecl),
        Type(TypeDecl),
        Enum(EnumDecl),
        Const(VarDecl),
        Var(VarDecl),
    }
}

node! {
    /// `annotation { "Name" : "Extrude", ... }` — the payload is a map literal.
    pub struct Annotation {
        pub entries: Vec<MapEntry>,
        pub span: Span,
    }
}

node! {
    /// `import(path : "onshape/std/geometry.fs", version : "1234.0");`
    pub struct Import {
        pub args: Vec<NamedArg>,
        pub span: Span,
    }
}

impl Import {
    fn string_arg(&self, name: &str) -> Option<&str> {
        self.args
            .iter()
            .find(|a| a.name.name == name)
            .and_then(|a| match &a.value.kind {
                ExprKind::Str(s) => Some(s.as_str()),
                _ => None,
            })
    }
    pub fn path(&self) -> Option<&str> {
        self.string_arg("path")
    }
    pub fn version(&self) -> Option<&str> {
        self.string_arg("version")
    }
}

node! {
    /// `name : value` inside an import.
    pub struct NamedArg {
        pub name: Ident,
        pub value: Expr,
        pub span: Span,
    }
}

node! {
    pub struct FunctionDecl {
        pub name: Ident,
        pub sig: FunctionSig,
        pub span: Span,
    }
}

node! {
    /// Parameter list, return type, precondition and body — shared by named
    /// functions, operators and anonymous `function(...)` expressions.
    pub struct FunctionSig {
        pub params: Vec<Param>,
        pub returns: Option<TypeRef>,
        pub precondition: Option<Block>,
        pub body: Block,
        pub span: Span,
    }
}

node! {
    pub struct Param {
        pub name: Ident,
        /// `is Type` constraint.
        pub ty: Option<TypeRef>,
        pub span: Span,
    }
}

node! {
    /// A type name as used after `is`, `as`, `returns`.
    pub struct TypeRef {
        pub name: String,
        pub span: Span,
    }
}

node! {
    pub struct PredicateDecl {
        pub name: Ident,
        pub params: Vec<Param>,
        pub precondition: Option<Block>,
        pub body: Block,
        pub span: Span,
    }
}

node! {
    /// `export operator+(a is Foo, b is Foo) returns Foo { ... }`
    pub struct OperatorDecl {
        /// The operator symbol, e.g. `+`, `-`, `*`, `/`, `==`.
        pub op: String,
        pub sig: FunctionSig,
        pub span: Span,
    }
}

node! {
    /// `export type Foo typecheck canBeFoo;`
    pub struct TypeDecl {
        pub name: Ident,
        pub typecheck: Option<Ident>,
        pub span: Span,
    }
}

node! {
    pub struct EnumDecl {
        pub name: Ident,
        pub members: Vec<EnumMember>,
        pub span: Span,
    }
}

node! {
    pub struct EnumMember {
        pub doc: Option<String>,
        pub annotation: Option<Annotation>,
        pub name: Ident,
        pub span: Span,
    }
}

node! {
    /// `var x is Type = expr;` / `const X = expr;` — used both at top level
    /// and as a statement.
    pub struct VarDecl {
        pub name: Ident,
        pub ty: Option<TypeRef>,
        pub init: Option<Expr>,
        pub span: Span,
    }
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

node! {
    pub struct Block {
        pub stmts: Vec<Stmt>,
        pub span: Span,
    }
}

node! {
    pub struct Stmt {
        /// Annotations may precede statements inside `precondition` blocks
        /// (`annotation { "Name" : "Depth" } isLength(definition.depth, ...);`).
        pub annotation: Option<Annotation>,
        pub kind: StmtKind,
        pub span: Span,
    }
}

node! {
    pub enum StmtKind {
        Var(VarDecl),
        Const(VarDecl),
        /// Expression statement; includes assignments, calls and the
        /// `definition.x is Query;` checks found in preconditions.
        Expr(Expr),
        Block(Block),
        If {
            cond: Expr,
            then: Box<Stmt>,
            #[cfg_attr(feature = "serde", serde(rename = "else"))]
            else_: Option<Box<Stmt>>,
        },
        /// C-style `for (init; cond; step) body`.
        For {
            init: Option<ForInit>,
            cond: Option<Expr>,
            step: Option<Expr>,
            body: Box<Stmt>,
        },
        /// `for (var value in iter)` / `for (var key, value in iter)`.
        /// `declared` is false for the rare `for (value in iter)` form.
        ForIn {
            declared: bool,
            key: Option<Ident>,
            value: Ident,
            iter: Expr,
            body: Box<Stmt>,
        },
        While {
            cond: Expr,
            body: Box<Stmt>,
        },
        Break,
        Continue,
        Return(Option<Expr>),
        Throw(Expr),
        /// `try [silent] { ... } [catch [(e)] { ... }]`
        Try {
            silent: bool,
            body: Block,
            catch: Option<CatchClause>,
        },
        /// A lone `;`.
        Empty,
    }
}

node! {
    pub enum ForInit {
        Var(VarDecl),
        Expr(Expr),
    }
}

node! {
    pub struct CatchClause {
        pub param: Option<Ident>,
        pub body: Block,
        pub span: Span,
    }
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

node! {
    pub struct Expr {
        pub kind: ExprKind,
        pub span: Span,
    }
}

node! {
    pub enum ExprKind {
        Number(f64),
        Str(String),
        Bool(bool),
        Undefined,
        Ident(String),
        Array(Vec<Expr>),
        Map(Vec<MapEntry>),
        /// `(expr)` — kept so spans and printing round-trip.
        Paren(Box<Expr>),
        Unary {
            op: UnaryOp,
            expr: Box<Expr>,
        },
        Binary {
            op: BinaryOp,
            lhs: Box<Expr>,
            rhs: Box<Expr>,
        },
        Assign {
            op: AssignOp,
            target: Box<Expr>,
            value: Box<Expr>,
        },
        Ternary {
            cond: Box<Expr>,
            then: Box<Expr>,
            #[cfg_attr(feature = "serde", serde(rename = "else"))]
            else_: Box<Expr>,
        },
        /// `expr is Type`
        Is {
            expr: Box<Expr>,
            ty: TypeRef,
        },
        /// `expr as Type`
        As {
            expr: Box<Expr>,
            ty: TypeRef,
        },
        /// `object.name` or `object?.name`
        Member {
            object: Box<Expr>,
            name: Ident,
            safe: bool,
        },
        Index {
            object: Box<Expr>,
            index: Box<Expr>,
        },
        Call {
            callee: Box<Expr>,
            args: Vec<Expr>,
        },
        /// `receiver -> f(args)`, which means `f(receiver, args)`. `call` is
        /// always an [`ExprKind::Call`]; see [`Expr::desugar_pipe`].
        Pipe {
            receiver: Box<Expr>,
            call: Box<Expr>,
        },
        /// `x => expr` / `(a is Type, b) => expr` / `x => { stmts }`
        Lambda {
            params: Vec<Param>,
            /// Optional `returns Type` between the parameter list and `=>`.
            returns: Option<TypeRef>,
            body: Box<LambdaBody>,
        },
        /// `boxed[]` — the contents of a `box`.
        Deref(Box<Expr>),
        /// Anonymous `function(params) [returns T] [precondition {..}] { ... }`
        Function(FunctionSig),
        /// `try(expr)` / `try silent(expr)` — evaluates to `undefined` on error.
        Try {
            silent: bool,
            expr: Box<Expr>,
        },
        /// `switch (scrutinee) { key : value, ... }`
        Switch {
            scrutinee: Box<Expr>,
            cases: Vec<MapEntry>,
        },
        /// `new box(value)`
        New(Box<Expr>),
    }
}

node! {
    pub enum LambdaBody {
        Expr(Expr),
        Block(Block),
    }
}

node! {
    pub struct MapEntry {
        pub key: Expr,
        pub value: Expr,
        pub span: Span,
    }
}

node! {
    #[derive(Copy, Eq, Hash)]
    pub enum UnaryOp {
        Neg,
        Not,
    }
}

node! {
    #[derive(Copy, Eq, Hash)]
    pub enum BinaryOp {
        Add,
        Sub,
        Mul,
        Div,
        Mod,
        Pow,
        /// `~` string concatenation
        Concat,
        Eq,
        Ne,
        Lt,
        Le,
        Gt,
        Ge,
        And,
        Or,
        /// `??`
        Coalesce,
    }
}

node! {
    #[derive(Copy, Eq, Hash)]
    pub enum AssignOp {
        Assign,
        Add,
        Sub,
        Mul,
        Div,
        Mod,
        Concat,
        Pow,
        /// `||=`
        Or,
        /// `&&=`
        And,
        /// `??=`
        Coalesce,
    }
}

impl BinaryOp {
    pub fn symbol(self) -> &'static str {
        use BinaryOp::*;
        match self {
            Add => "+",
            Sub => "-",
            Mul => "*",
            Div => "/",
            Mod => "%",
            Pow => "^",
            Concat => "~",
            Eq => "==",
            Ne => "!=",
            Lt => "<",
            Le => "<=",
            Gt => ">",
            Ge => ">=",
            And => "&&",
            Or => "||",
            Coalesce => "??",
        }
    }
}

impl AssignOp {
    pub fn symbol(self) -> &'static str {
        use AssignOp::*;
        match self {
            Assign => "=",
            Add => "+=",
            Sub => "-=",
            Mul => "*=",
            Div => "/=",
            Mod => "%=",
            Concat => "~=",
            Pow => "^=",
            Or => "||=",
            And => "&&=",
            Coalesce => "??=",
        }
    }
}

impl Expr {
    /// For a [`ExprKind::Pipe`], return the equivalent plain call with the
    /// receiver inserted as the first argument. Other expressions are
    /// returned unchanged (cloned).
    pub fn desugar_pipe(&self) -> Expr {
        if let ExprKind::Pipe { receiver, call } = &self.kind {
            if let ExprKind::Call { callee, args } = &call.kind {
                let mut new_args = Vec::with_capacity(args.len() + 1);
                new_args.push((**receiver).clone());
                new_args.extend(args.iter().cloned());
                return Expr {
                    kind: ExprKind::Call {
                        callee: callee.clone(),
                        args: new_args,
                    },
                    span: self.span,
                };
            }
        }
        self.clone()
    }
}

impl Item {
    /// The declared name, when the item has one.
    pub fn name(&self) -> Option<&str> {
        match &self.kind {
            ItemKind::Import(_) => None,
            ItemKind::Function(f) => Some(&f.name.name),
            ItemKind::Predicate(p) => Some(&p.name.name),
            ItemKind::Operator(o) => Some(&o.op),
            ItemKind::Type(t) => Some(&t.name.name),
            ItemKind::Enum(e) => Some(&e.name.name),
            ItemKind::Const(v) | ItemKind::Var(v) => Some(&v.name.name),
        }
    }
}
