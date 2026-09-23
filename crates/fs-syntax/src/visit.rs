//! A generic read-only AST walker.
//!
//! Implement [`Visitor`] and override the hooks you care about; every hook
//! defaults to walking its children via the free `walk_*` functions, so an
//! override can decide whether to recurse by calling `walk_*` itself.

use crate::ast::*;

pub trait Visitor {
    fn visit_module(&mut self, m: &Module) {
        walk_module(self, m)
    }
    fn visit_item(&mut self, item: &Item) {
        walk_item(self, item)
    }
    fn visit_annotation(&mut self, a: &Annotation) {
        walk_annotation(self, a)
    }
    fn visit_function_sig(&mut self, sig: &FunctionSig) {
        walk_function_sig(self, sig)
    }
    fn visit_block(&mut self, b: &Block) {
        walk_block(self, b)
    }
    fn visit_stmt(&mut self, s: &Stmt) {
        walk_stmt(self, s)
    }
    fn visit_var_decl(&mut self, v: &VarDecl) {
        walk_var_decl(self, v)
    }
    fn visit_expr(&mut self, e: &Expr) {
        walk_expr(self, e)
    }
}

pub fn walk_module<V: Visitor + ?Sized>(v: &mut V, m: &Module) {
    for item in &m.items {
        v.visit_item(item);
    }
}

pub fn walk_item<V: Visitor + ?Sized>(v: &mut V, item: &Item) {
    if let Some(a) = &item.annotation {
        v.visit_annotation(a);
    }
    match &item.kind {
        ItemKind::Import(i) => {
            for a in &i.args {
                v.visit_expr(&a.value);
            }
        }
        ItemKind::Function(f) => v.visit_function_sig(&f.sig),
        ItemKind::Operator(o) => v.visit_function_sig(&o.sig),
        ItemKind::Predicate(p) => v.visit_block(&p.body),
        ItemKind::Type(_) => {}
        ItemKind::Enum(e) => {
            for m in &e.members {
                if let Some(a) = &m.annotation {
                    v.visit_annotation(a);
                }
            }
        }
        ItemKind::Const(d) | ItemKind::Var(d) => v.visit_var_decl(d),
    }
}

pub fn walk_annotation<V: Visitor + ?Sized>(v: &mut V, a: &Annotation) {
    for e in &a.entries {
        v.visit_expr(&e.key);
        v.visit_expr(&e.value);
    }
}

pub fn walk_function_sig<V: Visitor + ?Sized>(v: &mut V, sig: &FunctionSig) {
    if let Some(p) = &sig.precondition {
        v.visit_block(p);
    }
    v.visit_block(&sig.body);
}

pub fn walk_block<V: Visitor + ?Sized>(v: &mut V, b: &Block) {
    for s in &b.stmts {
        v.visit_stmt(s);
    }
}

pub fn walk_var_decl<V: Visitor + ?Sized>(v: &mut V, d: &VarDecl) {
    if let Some(e) = &d.init {
        v.visit_expr(e);
    }
}

pub fn walk_stmt<V: Visitor + ?Sized>(v: &mut V, s: &Stmt) {
    if let Some(a) = &s.annotation {
        v.visit_annotation(a);
    }
    match &s.kind {
        StmtKind::Var(d) | StmtKind::Const(d) => v.visit_var_decl(d),
        StmtKind::Expr(e) | StmtKind::Throw(e) => v.visit_expr(e),
        StmtKind::Block(b) => v.visit_block(b),
        StmtKind::If { cond, then, else_ } => {
            v.visit_expr(cond);
            v.visit_stmt(then);
            if let Some(e) = else_ {
                v.visit_stmt(e);
            }
        }
        StmtKind::For {
            init,
            cond,
            step,
            body,
        } => {
            match init {
                Some(ForInit::Var(d)) => v.visit_var_decl(d),
                Some(ForInit::Expr(e)) => v.visit_expr(e),
                None => {}
            }
            if let Some(c) = cond {
                v.visit_expr(c);
            }
            if let Some(st) = step {
                v.visit_expr(st);
            }
            v.visit_stmt(body);
        }
        StmtKind::ForIn { iter, body, .. } => {
            v.visit_expr(iter);
            v.visit_stmt(body);
        }
        StmtKind::While { cond, body } => {
            v.visit_expr(cond);
            v.visit_stmt(body);
        }
        StmtKind::Return(e) => {
            if let Some(e) = e {
                v.visit_expr(e);
            }
        }
        StmtKind::Try { body, catch, .. } => {
            v.visit_block(body);
            if let Some(c) = catch {
                v.visit_block(&c.body);
            }
        }
        StmtKind::Break | StmtKind::Continue | StmtKind::Empty => {}
    }
}

pub fn walk_expr<V: Visitor + ?Sized>(v: &mut V, e: &Expr) {
    match &e.kind {
        ExprKind::Number(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Undefined
        | ExprKind::Ident(_) => {}
        ExprKind::Array(items) => {
            for i in items {
                v.visit_expr(i);
            }
        }
        ExprKind::Map(entries) | ExprKind::Switch { cases: entries, .. } => {
            if let ExprKind::Switch { scrutinee, .. } = &e.kind {
                v.visit_expr(scrutinee);
            }
            for en in entries {
                v.visit_expr(&en.key);
                v.visit_expr(&en.value);
            }
        }
        ExprKind::Paren(inner) | ExprKind::New(inner) | ExprKind::Deref(inner) => {
            v.visit_expr(inner)
        }
        ExprKind::Unary { expr, .. } | ExprKind::Try { expr, .. } => v.visit_expr(expr),
        ExprKind::Binary { lhs, rhs, .. } => {
            v.visit_expr(lhs);
            v.visit_expr(rhs);
        }
        ExprKind::Assign { target, value, .. } => {
            v.visit_expr(target);
            v.visit_expr(value);
        }
        ExprKind::Ternary { cond, then, else_ } => {
            v.visit_expr(cond);
            v.visit_expr(then);
            v.visit_expr(else_);
        }
        ExprKind::Is { expr, .. } | ExprKind::As { expr, .. } => v.visit_expr(expr),
        ExprKind::Member { object, .. } => v.visit_expr(object),
        ExprKind::Index { object, index } => {
            v.visit_expr(object);
            v.visit_expr(index);
        }
        ExprKind::Call { callee, args } => {
            v.visit_expr(callee);
            for a in args {
                v.visit_expr(a);
            }
        }
        ExprKind::Pipe { receiver, call } => {
            v.visit_expr(receiver);
            v.visit_expr(call);
        }
        ExprKind::Lambda { body, .. } => match &**body {
            LambdaBody::Expr(e) => v.visit_expr(e),
            LambdaBody::Block(b) => v.visit_block(b),
        },
        ExprKind::Function(sig) => v.visit_function_sig(sig),
    }
}
