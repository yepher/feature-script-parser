//! Statement execution.
//!
//! Two modes share one walker: ordinary execution, and *checking* mode used
//! for `precondition` blocks and `predicate` bodies, where every expression
//! statement must evaluate to `true`.

use super::Interp;
use crate::env::Env;
use crate::error::{Error, Flow, FlowResult};
use crate::value::Value;
use fs_syntax::ast::{Block, ForInit, Stmt, StmtKind, VarDecl};
use std::rc::Rc;

impl Interp {
    pub(crate) fn exec_block(&mut self, block: &Block, env: &Rc<Env>) -> FlowResult<()> {
        let inner = Env::child(env);
        for s in &block.stmts {
            self.exec(s, &inner)?;
        }
        Ok(())
    }

    /// Checking mode: `Ok(false)` as soon as a check fails.
    pub(crate) fn exec_stmts_checking(
        &mut self,
        stmts: &[Stmt],
        env: &Rc<Env>,
    ) -> FlowResult<bool> {
        for s in stmts {
            if !self.exec_checking(s, env)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn exec_checking(&mut self, s: &Stmt, env: &Rc<Env>) -> FlowResult<bool> {
        match &s.kind {
            StmtKind::Expr(e) => {
                let v = self.eval(e, env)?;
                match v.as_bool() {
                    Some(b) => Ok(b),
                    // An assignment or call used for effect inside a predicate.
                    None if v.is_undefined() => Ok(true),
                    None => Err(Error::type_error(format!(
                        "predicate statement must be boolean, got {}",
                        v.type_name()
                    ))
                    .at(e.span)
                    .into()),
                }
            }
            StmtKind::Block(b) => {
                let inner = Env::child(env);
                self.exec_stmts_checking(&b.stmts, &inner)
            }
            StmtKind::If { cond, then, else_ } => {
                if self.eval_bool(cond, env)? {
                    self.exec_checking(then, env)
                } else if let Some(e) = else_ {
                    self.exec_checking(e, env)
                } else {
                    Ok(true)
                }
            }
            StmtKind::ForIn { .. } | StmtKind::For { .. } | StmtKind::While { .. } => {
                self.exec_loop(s, env, true)
            }
            // Declarations and control flow behave as in ordinary execution.
            _ => {
                self.exec(s, env)?;
                Ok(true)
            }
        }
    }

    pub(crate) fn exec(&mut self, s: &Stmt, env: &Rc<Env>) -> FlowResult<()> {
        match &s.kind {
            StmtKind::Empty => Ok(()),
            StmtKind::Var(v) => self.declare(v, env, false),
            StmtKind::Const(v) => self.declare(v, env, true),
            StmtKind::Expr(e) => {
                self.eval(e, env)?;
                Ok(())
            }
            StmtKind::Block(b) => self.exec_block(b, env),
            StmtKind::If { cond, then, else_ } => {
                if self.eval_bool(cond, env)? {
                    self.exec(then, env)
                } else if let Some(e) = else_ {
                    self.exec(e, env)
                } else {
                    Ok(())
                }
            }
            StmtKind::For { .. } | StmtKind::ForIn { .. } | StmtKind::While { .. } => {
                self.exec_loop(s, env, false).map(|_| ())
            }
            StmtKind::Break => Err(Flow::Break),
            StmtKind::Continue => Err(Flow::Continue),
            StmtKind::Return(e) => {
                let v = match e {
                    Some(e) => self.eval(e, env)?,
                    None => Value::Undefined,
                };
                Err(Flow::Return(v))
            }
            StmtKind::Throw(e) => {
                let v = self.eval(e, env)?;
                Err(Flow::Error(Error::thrown(v).at(s.span)))
            }
            StmtKind::Try { body, catch, .. } => match self.exec_block(body, env) {
                Err(Flow::Error(err)) if err.is_catchable() => {
                    if let Some(c) = catch {
                        let inner = Env::child(env);
                        if let Some(p) = &c.param {
                            inner.declare(&p.name, err.catch_value(), false);
                        }
                        self.exec_block(&c.body, &inner)
                    } else {
                        Ok(())
                    }
                }
                other => other,
            },
        }
    }

    fn declare(&mut self, v: &VarDecl, env: &Rc<Env>, is_const: bool) -> FlowResult<()> {
        let value = match &v.init {
            Some(e) => self.eval(e, env)?,
            None => Value::Undefined,
        };
        if let Some(ty) = &v.ty {
            if !self.check_type(&value, &ty.name, env)? {
                return Err(Error::type_error(format!(
                    "`{}` is declared `is {}` but got {}",
                    v.name.name,
                    ty.name,
                    value.type_name()
                ))
                .at(v.span)
                .into());
            }
        }
        env.declare(&v.name.name, value, is_const);
        Ok(())
    }

    /// Runs a loop in either mode. In checking mode the result is whether all
    /// iterations passed; otherwise it is always `true`.
    fn exec_loop(&mut self, s: &Stmt, env: &Rc<Env>, checking: bool) -> FlowResult<bool> {
        // Runs the body once; maps break/continue to a loop decision.
        // Returns Ok(Some(passed)) to keep going, Ok(None) to break.
        let run_body =
            |this: &mut Interp, body: &Stmt, scope: &Rc<Env>| -> FlowResult<Option<bool>> {
                let r = if checking {
                    this.exec_checking(body, scope)
                } else {
                    this.exec(body, scope).map(|_| true)
                };
                match r {
                    Ok(passed) => Ok(Some(passed)),
                    Err(Flow::Break) => Ok(None),
                    Err(Flow::Continue) => Ok(Some(true)),
                    Err(other) => Err(other),
                }
            };

        match &s.kind {
            StmtKind::While { cond, body } => {
                loop {
                    if !self.eval_bool(cond, env)? {
                        break;
                    }
                    let scope = Env::child(env);
                    match run_body(self, body, &scope)? {
                        Some(true) => {}
                        Some(false) => return Ok(false),
                        None => break,
                    }
                }
                Ok(true)
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                let loop_env = Env::child(env);
                match init {
                    Some(ForInit::Var(v)) => self.declare(v, &loop_env, false)?,
                    Some(ForInit::Expr(e)) => {
                        self.eval(e, &loop_env)?;
                    }
                    None => {}
                }
                loop {
                    if let Some(c) = cond {
                        if !self.eval_bool(c, &loop_env)? {
                            break;
                        }
                    }
                    let scope = Env::child(&loop_env);
                    match run_body(self, body, &scope)? {
                        Some(true) => {}
                        Some(false) => return Ok(false),
                        None => break,
                    }
                    if let Some(st) = step {
                        self.eval(st, &loop_env)?;
                    }
                }
                Ok(true)
            }
            StmtKind::ForIn {
                key,
                value,
                iter,
                body,
                ..
            } => {
                let container = self.eval(iter, env)?;
                let entries: Vec<(Value, Value)> = match container.untagged() {
                    Value::Array(a) => a
                        .iter()
                        .enumerate()
                        .map(|(i, v)| (Value::Number(i as f64), v.clone()))
                        .collect(),
                    Value::Map(m) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                    other => {
                        return Err(Error::type_error(format!(
                            "cannot iterate over a {}",
                            other.type_name()
                        ))
                        .at(iter.span)
                        .into())
                    }
                };
                let is_map = container.as_map().is_some();
                for (k, v) in entries {
                    let scope = Env::child(env);
                    match key {
                        Some(kname) => {
                            scope.declare(&kname.name, k, false);
                            scope.declare(&value.name, v, false);
                        }
                        // `for (var entry in map)` yields `{ key, value }` maps.
                        None if is_map => {
                            let mut entry = crate::value::Map::new();
                            entry.insert(Value::from("key"), k);
                            entry.insert(Value::from("value"), v);
                            scope.declare(&value.name, Value::map(entry), false);
                        }
                        None => scope.declare(&value.name, v, false),
                    }
                    match run_body(self, body, &scope)? {
                        Some(true) => {}
                        Some(false) => return Ok(false),
                        None => break,
                    }
                }
                Ok(true)
            }
            _ => unreachable!("exec_loop called on a non-loop"),
        }
    }
}
