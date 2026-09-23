//! Pretty-printer: turns an AST back into FeatureScript source.
//!
//! Output is canonical rather than faithful (ordinary comments are dropped, spacing is
//! normalized, every binary sub-expression is parenthesized as needed by
//! precedence). Its main job is testing: `parse(print(parse(src)))` must equal
//! `parse(src)` for every file in the standard library.

use crate::ast::*;
use std::fmt::Write;

pub fn print_module(m: &Module) -> String {
    let mut p = Printer {
        out: String::new(),
        indent: 0,
    };
    p.module(m);
    p.out
}

pub fn print_expr(e: &Expr) -> String {
    let mut p = Printer {
        out: String::new(),
        indent: 0,
    };
    p.expr(e);
    p.out
}

struct Printer {
    out: String,
    indent: usize,
}

/// Precedence of an expression's outermost operator (higher binds tighter);
/// mirrors the table in `parser.rs`.
fn prec(e: &Expr) -> u8 {
    use ExprKind::*;
    match &e.kind {
        Assign { .. } => 1,
        Ternary { .. } => 2,
        Lambda { .. } => 2,
        Binary { op, .. } => match op {
            BinaryOp::Or | BinaryOp::Coalesce => 3,
            BinaryOp::And => 4,
            BinaryOp::Eq | BinaryOp::Ne => 5,
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => 6,
            BinaryOp::Concat => 7,
            BinaryOp::Add | BinaryOp::Sub => 8,
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => 9,
            BinaryOp::Pow => 10,
        },
        Is { .. } | As { .. } => 11,
        Unary { .. } => 12,
        _ => 13,
    }
}

impl Printer {
    fn nl(&mut self) {
        self.out.push('\n');
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
    }

    fn module(&mut self, m: &Module) {
        if let Some(h) = &m.header {
            match h.version {
                Version::Number(n) => write!(self.out, "FeatureScript {n};").unwrap(),
                Version::Sparkle => self.out.push_str("FeatureScript ✨;"),
            }
            self.nl();
        }
        for item in &m.items {
            self.item(item);
            self.nl();
        }
    }

    fn annotation(&mut self, a: &Annotation) {
        self.out.push_str("annotation ");
        self.map_entries(&a.entries);
        self.nl();
    }

    fn doc(&mut self, doc: &Option<String>) {
        if let Some(d) = doc {
            write!(self.out, "/**{d}*/").unwrap();
            self.nl();
        }
    }

    fn item(&mut self, item: &Item) {
        self.doc(&item.doc);
        if let Some(a) = &item.annotation {
            self.annotation(a);
        }
        if item.export {
            self.out.push_str("export ");
        }
        match &item.kind {
            ItemKind::Import(i) => {
                self.out.push_str("import(");
                for (n, a) in i.args.iter().enumerate() {
                    if n > 0 {
                        self.out.push_str(", ");
                    }
                    write!(self.out, "{} : ", a.name.name).unwrap();
                    self.expr(&a.value);
                }
                self.out.push_str(");");
            }
            ItemKind::Function(f) => {
                write!(self.out, "function {}", f.name.name).unwrap();
                self.sig(&f.sig);
            }
            ItemKind::Operator(o) => {
                write!(self.out, "operator{}", o.op).unwrap();
                self.sig(&o.sig);
            }
            ItemKind::Predicate(p) => {
                write!(self.out, "predicate {}", p.name.name).unwrap();
                self.params(&p.params);
                if let Some(pre) = &p.precondition {
                    self.nl();
                    self.out.push_str("precondition");
                    self.nl();
                    self.block(pre);
                }
                self.nl();
                self.block(&p.body);
            }
            ItemKind::Type(t) => {
                write!(self.out, "type {}", t.name.name).unwrap();
                if let Some(tc) = &t.typecheck {
                    write!(self.out, " typecheck {}", tc.name).unwrap();
                }
                self.out.push(';');
            }
            ItemKind::Enum(e) => {
                write!(self.out, "enum {}", e.name.name).unwrap();
                self.nl();
                self.out.push('{');
                self.indent += 1;
                for (n, m) in e.members.iter().enumerate() {
                    self.nl();
                    self.doc(&m.doc);
                    if let Some(a) = &m.annotation {
                        self.annotation(a);
                    }
                    self.out.push_str(&m.name.name);
                    if n + 1 < e.members.len() {
                        self.out.push(',');
                    }
                }
                self.indent -= 1;
                self.nl();
                self.out.push('}');
            }
            ItemKind::Const(v) => {
                self.out.push_str("const ");
                self.var_decl(v);
            }
            ItemKind::Var(v) => {
                self.out.push_str("var ");
                self.var_decl(v);
            }
        }
    }

    fn var_decl(&mut self, v: &VarDecl) {
        self.out.push_str(&v.name.name);
        if let Some(t) = &v.ty {
            write!(self.out, " is {}", t.name).unwrap();
        }
        if let Some(e) = &v.init {
            self.out.push_str(" = ");
            self.expr(e);
        }
        self.out.push(';');
    }

    fn params(&mut self, params: &[Param]) {
        self.out.push('(');
        for (n, p) in params.iter().enumerate() {
            if n > 0 {
                self.out.push_str(", ");
            }
            self.out.push_str(&p.name.name);
            if let Some(t) = &p.ty {
                write!(self.out, " is {}", t.name).unwrap();
            }
        }
        self.out.push(')');
    }

    fn sig(&mut self, sig: &FunctionSig) {
        self.params(&sig.params);
        if let Some(r) = &sig.returns {
            write!(self.out, " returns {}", r.name).unwrap();
        }
        if let Some(pre) = &sig.precondition {
            self.nl();
            self.out.push_str("precondition");
            self.nl();
            self.block(pre);
        }
        self.nl();
        self.block(&sig.body);
    }

    fn block(&mut self, b: &Block) {
        self.out.push('{');
        self.indent += 1;
        for s in &b.stmts {
            self.nl();
            self.stmt(s);
        }
        self.indent -= 1;
        self.nl();
        self.out.push('}');
    }

    /// A statement in body position (after `if (...)`, `for (...)` etc.).
    fn body(&mut self, s: &Stmt) {
        self.indent += 1;
        self.nl();
        self.stmt(s);
        self.indent -= 1;
    }

    fn stmt(&mut self, s: &Stmt) {
        if let Some(a) = &s.annotation {
            self.annotation(a);
        }
        match &s.kind {
            StmtKind::Var(v) => {
                self.out.push_str("var ");
                self.var_decl(v);
            }
            StmtKind::Const(v) => {
                self.out.push_str("const ");
                self.var_decl(v);
            }
            StmtKind::Expr(e) => {
                self.expr(e);
                self.out.push(';');
            }
            StmtKind::Block(b) => self.block(b),
            StmtKind::If { cond, then, else_ } => {
                self.out.push_str("if (");
                self.expr(cond);
                self.out.push(')');
                self.body(then);
                if let Some(e) = else_ {
                    self.nl();
                    self.out.push_str("else");
                    self.body(e);
                }
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                self.out.push_str("for (");
                match init {
                    Some(ForInit::Var(v)) => {
                        self.out.push_str("var ");
                        self.var_decl(v);
                    }
                    Some(ForInit::Expr(e)) => {
                        self.expr(e);
                        self.out.push(';');
                    }
                    None => self.out.push(';'),
                }
                if let Some(c) = cond {
                    self.out.push(' ');
                    self.expr(c);
                }
                self.out.push(';');
                if let Some(st) = step {
                    self.out.push(' ');
                    self.expr(st);
                }
                self.out.push(')');
                self.body(body);
            }
            StmtKind::ForIn {
                declared,
                key,
                value,
                iter,
                body,
            } => {
                self.out.push_str("for (");
                if *declared {
                    self.out.push_str("var ");
                }
                if let Some(k) = key {
                    write!(self.out, "{}, ", k.name).unwrap();
                }
                write!(self.out, "{} in ", value.name).unwrap();
                self.expr(iter);
                self.out.push(')');
                self.body(body);
            }
            StmtKind::While { cond, body } => {
                self.out.push_str("while (");
                self.expr(cond);
                self.out.push(')');
                self.body(body);
            }
            StmtKind::Break => self.out.push_str("break;"),
            StmtKind::Continue => self.out.push_str("continue;"),
            StmtKind::Return(None) => self.out.push_str("return;"),
            StmtKind::Return(Some(e)) => {
                self.out.push_str("return ");
                self.expr(e);
                self.out.push(';');
            }
            StmtKind::Throw(e) => {
                self.out.push_str("throw ");
                self.expr(e);
                self.out.push(';');
            }
            StmtKind::Try {
                silent,
                body,
                catch,
            } => {
                self.out
                    .push_str(if *silent { "try silent" } else { "try" });
                self.nl();
                self.block(body);
                if let Some(c) = catch {
                    self.nl();
                    self.out.push_str("catch");
                    if let Some(p) = &c.param {
                        write!(self.out, " ({})", p.name).unwrap();
                    }
                    self.nl();
                    self.block(&c.body);
                }
            }
            StmtKind::Empty => self.out.push(';'),
        }
    }

    fn map_entries(&mut self, entries: &[MapEntry]) {
        self.out.push_str("{ ");
        for (n, e) in entries.iter().enumerate() {
            if n > 0 {
                self.out.push_str(", ");
            }
            self.expr(&e.key);
            self.out.push_str(" : ");
            self.expr(&e.value);
        }
        if entries.is_empty() {
            self.out.pop();
        }
        self.out.push('}');
    }

    fn string(&mut self, s: &str) {
        self.out.push('"');
        for c in s.chars() {
            match c {
                '"' => self.out.push_str("\\\""),
                '\\' => self.out.push_str("\\\\"),
                '\n' => self.out.push_str("\\n"),
                '\t' => self.out.push_str("\\t"),
                '\r' => self.out.push_str("\\r"),
                c => self.out.push(c),
            }
        }
        self.out.push('"');
    }

    /// Print `e` parenthesized if it binds looser than `min`.
    fn sub(&mut self, e: &Expr, min: u8) {
        if prec(e) < min {
            self.out.push('(');
            self.expr(e);
            self.out.push(')');
        } else {
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &Expr) {
        use ExprKind::*;
        match &e.kind {
            Number(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    write!(self.out, "{}", *n as i64).unwrap();
                } else {
                    write!(self.out, "{n:?}").unwrap();
                }
            }
            Str(s) => self.string(s),
            Bool(b) => write!(self.out, "{b}").unwrap(),
            Undefined => self.out.push_str("undefined"),
            Ident(name) => self.out.push_str(name),
            Array(items) => {
                self.out.push('[');
                for (n, i) in items.iter().enumerate() {
                    if n > 0 {
                        self.out.push_str(", ");
                    }
                    self.expr(i);
                }
                self.out.push(']');
            }
            Map(entries) => self.map_entries(entries),
            Paren(inner) => {
                self.out.push('(');
                self.expr(inner);
                self.out.push(')');
            }
            Unary { op, expr } => {
                self.out.push(match op {
                    UnaryOp::Neg => '-',
                    UnaryOp::Not => '!',
                });
                self.sub(expr, 12);
            }
            Binary { op, lhs, rhs } => {
                let p = prec(e);
                // Left-assoc: rhs needs one more; `^` is right-assoc.
                let (lmin, rmin) = if *op == BinaryOp::Pow {
                    (p + 1, p)
                } else {
                    (p, p + 1)
                };
                self.sub(lhs, lmin);
                write!(self.out, " {} ", op.symbol()).unwrap();
                self.sub(rhs, rmin);
            }
            Assign { op, target, value } => {
                self.sub(target, 13);
                write!(self.out, " {} ", op.symbol()).unwrap();
                self.sub(value, 1);
            }
            Ternary { cond, then, else_ } => {
                self.sub(cond, 3);
                self.out.push_str(" ? ");
                self.sub(then, 1);
                self.out.push_str(" : ");
                self.sub(else_, 1);
            }
            Is { expr, ty } => {
                self.sub(expr, 11);
                write!(self.out, " is {}", ty.name).unwrap();
            }
            As { expr, ty } => {
                self.sub(expr, 11);
                write!(self.out, " as {}", ty.name).unwrap();
            }
            Member { object, name, safe } => {
                self.sub(object, 13);
                self.out.push_str(if *safe { "?." } else { "." });
                self.out.push_str(&name.name);
            }
            Index { object, index } => {
                self.sub(object, 13);
                self.out.push('[');
                self.expr(index);
                self.out.push(']');
            }
            Deref(inner) => {
                self.sub(inner, 13);
                self.out.push_str("[]");
            }
            Call { callee, args } => {
                self.sub(callee, 13);
                self.args(args);
            }
            Pipe { receiver, call } => {
                self.sub(receiver, 13);
                self.out.push_str(" -> ");
                self.expr(call);
            }
            Lambda {
                params,
                returns,
                body,
            } => {
                self.params(params);
                if let Some(r) = returns {
                    write!(self.out, " returns {}", r.name).unwrap();
                }
                self.out.push_str(" => ");
                match &**body {
                    LambdaBody::Expr(e) => self.sub(e, 1),
                    LambdaBody::Block(b) => self.block(b),
                }
            }
            Function(sig) => {
                self.out.push_str("function");
                self.sig(sig);
            }
            Try { silent, expr } => {
                self.out
                    .push_str(if *silent { "try silent(" } else { "try(" });
                self.expr(expr);
                self.out.push(')');
            }
            Switch { scrutinee, cases } => {
                self.out.push_str("switch (");
                self.expr(scrutinee);
                self.out.push_str(") ");
                self.map_entries(cases);
            }
            New(inner) => {
                self.out.push_str("new ");
                self.expr(inner);
            }
        }
    }

    fn args(&mut self, args: &[Expr]) {
        self.out.push('(');
        for (n, a) in args.iter().enumerate() {
            if n > 0 {
                self.out.push_str(", ");
            }
            self.expr(a);
        }
        self.out.push(')');
    }
}
