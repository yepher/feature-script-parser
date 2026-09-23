//! Expression evaluation.

use super::{return_block, Interp};
use crate::env::Env;
use crate::error::{Error, EvalResult};
use crate::value::{FnKind, Map, Value};
use fs_syntax::ast::{
    AssignOp, BinaryOp, Expr, ExprKind, FunctionSig, LambdaBody, MapEntry, UnaryOp,
};
use std::rc::Rc;

impl Interp {
    pub(crate) fn eval(&mut self, e: &Expr, env: &Rc<Env>) -> EvalResult<Value> {
        self.eval_inner(e, env).map_err(|err| err.at(e.span))
    }

    fn eval_inner(&mut self, e: &Expr, env: &Rc<Env>) -> EvalResult<Value> {
        use ExprKind::*;
        match &e.kind {
            Number(n) => Ok(Value::Number(*n)),
            Str(s) => Ok(Value::from(s.as_str())),
            Bool(b) => Ok(Value::Bool(*b)),
            Undefined => Ok(Value::Undefined),
            Ident(name) => self.eval_ident(name, env),
            Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for i in items {
                    out.push(self.eval(i, env)?);
                }
                Ok(Value::array(out))
            }
            Map(entries) => Ok(Value::map(self.eval_map(entries, env)?)),
            Paren(inner) => self.eval(inner, env),
            Unary { op, expr } => {
                let v = self.eval(expr, env)?;
                self.unary(*op, v, env)
            }
            Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, env),
            Assign { op, target, value } => {
                let rhs = self.eval(value, env)?;
                let new = match op {
                    AssignOp::Assign => rhs,
                    _ => {
                        let current = self.eval(target, env)?;
                        let bin = match op {
                            AssignOp::Add => BinaryOp::Add,
                            AssignOp::Sub => BinaryOp::Sub,
                            AssignOp::Mul => BinaryOp::Mul,
                            AssignOp::Div => BinaryOp::Div,
                            AssignOp::Mod => BinaryOp::Mod,
                            AssignOp::Concat => BinaryOp::Concat,
                            AssignOp::Pow => BinaryOp::Pow,
                            AssignOp::Or => BinaryOp::Or,
                            AssignOp::And => BinaryOp::And,
                            AssignOp::Coalesce => BinaryOp::Coalesce,
                            AssignOp::Assign => unreachable!(),
                        };
                        self.binary_values(bin, current, rhs, env)?
                    }
                };
                self.assign_to(target, new.clone(), env)?;
                Ok(new)
            }
            Ternary { cond, then, else_ } => {
                if self.eval_bool(cond, env)? {
                    self.eval(then, env)
                } else {
                    self.eval(else_, env)
                }
            }
            Is { expr, ty } => {
                let v = self.eval(expr, env)?;
                Ok(Value::Bool(self.check_type(&v, &ty.name, env)?))
            }
            As { expr, ty } => {
                let v = self.eval(expr, env)?;
                self.cast(v, &ty.name, env)
            }
            Member { object, name, safe } => {
                let obj = self.eval(object, env)?;
                if *safe && obj.is_undefined() {
                    return Ok(Value::Undefined);
                }
                self.member(&obj, &name.name)
            }
            Index { object, index } => {
                let obj = self.eval(object, env)?;
                let idx = self.eval(index, env)?;
                self.index(&obj, &idx)
            }
            Deref(inner) => {
                let b = self.eval(inner, env)?;
                match b.untagged() {
                    Value::Box(cell) => Ok(cell.borrow().clone()),
                    other => Err(Error::type_error(format!(
                        "`[]` applied to a {}, not a box",
                        other.type_name()
                    ))),
                }
            }
            Call { callee, args } => {
                let mut argv = Vec::with_capacity(args.len());
                for a in args {
                    argv.push(self.eval(a, env)?);
                }
                self.eval_call(callee, argv, env, e)
            }
            Pipe { .. } => {
                let d = e.desugar_pipe();
                self.eval_inner(&d, env)
            }
            Lambda {
                params,
                returns,
                body,
            } => {
                let block = match &**body {
                    LambdaBody::Expr(x) => return_block(x),
                    LambdaBody::Block(b) => b.clone(),
                };
                let sig = Rc::new(FunctionSig {
                    params: params.clone(),
                    returns: returns.clone(),
                    precondition: None,
                    body: block,
                    span: e.span,
                });
                Ok(self.make_closure("<lambda>", sig, env, FnKind::Lambda))
            }
            Function(sig) => {
                Ok(self.make_closure("<function>", Rc::new(sig.clone()), env, FnKind::Function))
            }
            Try { expr, .. } => match self.eval(expr, env) {
                Ok(v) => Ok(v),
                Err(_) => Ok(Value::Undefined),
            },
            Switch { scrutinee, cases } => {
                let key = self.eval(scrutinee, env)?;
                for case in cases {
                    let k = self.eval(&case.key, env)?;
                    if k == key {
                        return self.eval(&case.value, env);
                    }
                }
                Ok(Value::Undefined)
            }
            New(inner) => {
                // Only `new box(value)` exists.
                if let ExprKind::Call { callee, args } = &inner.kind {
                    if matches!(&callee.kind, ExprKind::Ident(n) if n == "box") {
                        let v = match args.first() {
                            Some(a) => self.eval(a, env)?,
                            None => Value::Undefined,
                        };
                        return Ok(Value::Box(Rc::new(std::cell::RefCell::new(v))));
                    }
                }
                Err(Error::runtime("only `new box(value)` is supported"))
            }
        }
    }

    fn eval_ident(&mut self, name: &str, env: &Rc<Env>) -> EvalResult<Value> {
        if name.starts_with('@') {
            return Err(Error::runtime(format!("builtin `{name}` must be called")));
        }
        // Named functions may be overloaded across modules; present the
        // merged overload set so a function value behaves like a call would.
        if let Some(v) = self.lookup(env, name)? {
            if v.as_function().is_some() && !env.is_local(name) {
                let overloads = self.overloads(env, name);
                if overloads.len() > 1 {
                    return Ok(Value::Function(Rc::new(crate::value::Function {
                        name: name.to_string(),
                        overloads: (*overloads).clone(),
                    })));
                }
            }
            return Ok(v);
        }
        // Builtin globals.
        if name == "inf" {
            return Ok(Value::Number(f64::INFINITY));
        }
        Err(Error::name(format!("undefined identifier `{name}`")))
    }

    pub(crate) fn eval_bool(&mut self, e: &Expr, env: &Rc<Env>) -> EvalResult<bool> {
        let v = self.eval(e, env)?;
        v.as_bool().ok_or_else(|| {
            Error::type_error(format!("expected a boolean, got {}", v.type_name())).at(e.span)
        })
    }

    /// Map literal. A bare identifier key is a string key, as in JavaScript:
    /// `{ endBound : BoundingType.BLIND }` means `{ "endBound" : ... }`.
    pub(crate) fn eval_map(&mut self, entries: &[MapEntry], env: &Rc<Env>) -> EvalResult<Map> {
        let mut m = Map::with_capacity(entries.len());
        for en in entries {
            let k = match &en.key.kind {
                ExprKind::Ident(name) => Value::from(name.as_str()),
                _ => self.eval(&en.key, env)?,
            };
            let v = self.eval(&en.value, env)?;
            m.insert(k, v);
        }
        Ok(m)
    }

    fn eval_call(
        &mut self,
        callee: &Expr,
        args: Vec<Value>,
        env: &Rc<Env>,
        call: &Expr,
    ) -> EvalResult<Value> {
        if let ExprKind::Ident(name) = &callee.kind {
            if name.starts_with('@') {
                return self.call_builtin(name, args, call.span);
            }
            // Local variables shadow module-level functions.
            if !env.is_local(name) {
                let overloads = self.overloads(env, name);
                if !overloads.is_empty() {
                    return self.call_overloads(name, &overloads, args, Some(call.span));
                }
            }
        }
        let f = self.eval(callee, env)?;
        self.call_value(&f, args, Some(call.span))
    }

    // ------------------------------------------------------------------
    // Member / index
    // ------------------------------------------------------------------

    fn member(&mut self, obj: &Value, name: &str) -> EvalResult<Value> {
        match obj.untagged() {
            Value::Map(m) => Ok(m
                .get(&Value::from(name))
                .cloned()
                .unwrap_or(Value::Undefined)),
            Value::EnumType(def) => match def.members.iter().position(|m| m == name) {
                Some(index) => Ok(Value::Enum(crate::value::EnumValue {
                    def: def.clone(),
                    index,
                })),
                None => Err(Error::name(format!(
                    "enum `{}` has no member `{name}`",
                    def.name
                ))),
            },
            Value::Undefined => Err(Error::type_error(format!(
                "cannot read `.{name}` of undefined"
            ))),
            other => Err(Error::type_error(format!(
                "cannot read `.{name}` of a {}",
                other.type_name()
            ))),
        }
    }

    fn index(&mut self, obj: &Value, idx: &Value) -> EvalResult<Value> {
        match obj.untagged() {
            Value::Array(a) => {
                let i = idx
                    .as_number()
                    .ok_or_else(|| Error::type_error("array index must be a number"))?;
                if i.fract() != 0.0 || i < 0.0 || i as usize >= a.len() {
                    return Err(Error::runtime(format!(
                        "array index {} out of bounds (length {})",
                        i,
                        a.len()
                    )));
                }
                Ok(a[i as usize].clone())
            }
            Value::Map(m) => Ok(m.get(idx).cloned().unwrap_or(Value::Undefined)),
            Value::Undefined => Err(Error::type_error("cannot index undefined")),
            other => Err(Error::type_error(format!(
                "cannot index a {}",
                other.type_name()
            ))),
        }
    }

    // ------------------------------------------------------------------
    // Assignment (value semantics: rebuild the path back to the variable)
    // ------------------------------------------------------------------

    pub(crate) fn assign_to(
        &mut self,
        target: &Expr,
        value: Value,
        env: &Rc<Env>,
    ) -> EvalResult<()> {
        match &target.kind {
            ExprKind::Ident(name) => env.assign(name, value).map_err(|e| e.at(target.span)),
            ExprKind::Paren(inner) => self.assign_to(inner, value, env),
            ExprKind::Deref(inner) => {
                let b = self.eval(inner, env)?;
                match b.untagged() {
                    Value::Box(cell) => {
                        *cell.borrow_mut() = value;
                        Ok(())
                    }
                    other => Err(Error::type_error(format!(
                        "`[]` applied to a {}, not a box",
                        other.type_name()
                    ))
                    .at(target.span)),
                }
            }
            ExprKind::Member { object, name, .. } => {
                let base = self.eval(object, env)?;
                let updated = set_key(base, Value::from(name.name.as_str()), value)
                    .map_err(|e| e.at(target.span))?;
                self.assign_to(object, updated, env)
            }
            ExprKind::Index { object, index } => {
                let base = self.eval(object, env)?;
                let key = self.eval(index, env)?;
                let updated = set_key(base, key, value).map_err(|e| e.at(target.span))?;
                self.assign_to(object, updated, env)
            }
            _ => Err(Error::runtime("invalid assignment target").at(target.span)),
        }
    }

    // ------------------------------------------------------------------
    // Operators
    // ------------------------------------------------------------------

    fn unary(&mut self, op: UnaryOp, v: Value, env: &Rc<Env>) -> EvalResult<Value> {
        match op {
            UnaryOp::Not => v
                .as_bool()
                .map(|b| Value::Bool(!b))
                .ok_or_else(|| Error::type_error(format!("`!` applied to a {}", v.type_name()))),
            UnaryOp::Neg => {
                if let Some(n) = v.as_number() {
                    return Ok(Value::Number(-n));
                }
                let overloads = self.overloads(env, "operator-");
                let unary: Vec<_> = overloads
                    .iter()
                    .filter(|o| o.sig.params.len() == 1)
                    .cloned()
                    .collect();
                if unary.is_empty() {
                    return Err(Error::type_error(format!(
                        "unary `-` applied to a {}",
                        v.type_name()
                    )));
                }
                self.call_overloads("operator-", &unary, vec![v], None)
            }
        }
    }

    fn binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr, env: &Rc<Env>) -> EvalResult<Value> {
        // Short-circuit operators.
        match op {
            BinaryOp::And => {
                if !self.eval_bool(lhs, env)? {
                    return Ok(Value::Bool(false));
                }
                return Ok(Value::Bool(self.eval_bool(rhs, env)?));
            }
            BinaryOp::Or => {
                if self.eval_bool(lhs, env)? {
                    return Ok(Value::Bool(true));
                }
                return Ok(Value::Bool(self.eval_bool(rhs, env)?));
            }
            BinaryOp::Coalesce => {
                let l = self.eval(lhs, env)?;
                if !l.is_undefined() {
                    return Ok(l);
                }
                return self.eval(rhs, env);
            }
            _ => {}
        }
        let l = self.eval(lhs, env)?;
        let r = self.eval(rhs, env)?;
        self.binary_values(op, l, r, env)
    }

    pub(crate) fn binary_values(
        &mut self,
        op: BinaryOp,
        l: Value,
        r: Value,
        env: &Rc<Env>,
    ) -> EvalResult<Value> {
        use BinaryOp::*;
        match op {
            Eq => return Ok(Value::Bool(l == r)),
            Ne => return Ok(Value::Bool(l != r)),
            Concat => {
                return Ok(Value::from(format!(
                    "{}{}",
                    l.to_fs_string(),
                    r.to_fs_string()
                )))
            }
            And | Or | Coalesce => {
                // Only reachable through compound assignment (`||=` etc.).
                let a = l.as_bool();
                let b = r.as_bool();
                return match (op, a, b) {
                    (And, Some(a), Some(b)) => Ok(Value::Bool(a && b)),
                    (Or, Some(a), Some(b)) => Ok(Value::Bool(a || b)),
                    (Coalesce, _, _) => Ok(if l.is_undefined() { r } else { l }),
                    _ => Err(Error::type_error("logical operator applied to non-boolean")),
                };
            }
            _ => {}
        }

        // Numbers and strings have builtin behaviour.
        if let (Some(a), Some(b)) = (l.as_number(), r.as_number()) {
            return Ok(match op {
                Add => Value::Number(a + b),
                Sub => Value::Number(a - b),
                Mul => Value::Number(a * b),
                Div => Value::Number(a / b),
                Mod => Value::Number(a % b),
                Pow => Value::Number(a.powf(b)),
                Lt => Value::Bool(a < b),
                Le => Value::Bool(a <= b),
                Gt => Value::Bool(a > b),
                Ge => Value::Bool(a >= b),
                _ => unreachable!(),
            });
        }
        if let (Some(a), Some(b)) = (l.as_str(), r.as_str()) {
            match op {
                Lt => return Ok(Value::Bool(a < b)),
                Le => return Ok(Value::Bool(a <= b)),
                Gt => return Ok(Value::Bool(a > b)),
                Ge => return Ok(Value::Bool(a >= b)),
                _ => {}
            }
        }

        // Everything else goes through user-defined operator overloads.
        // `>` / `<=` / `>=` are derived from `operator<`.
        let (name, args, negate) = match op {
            Add => ("operator+", vec![l, r], false),
            Sub => ("operator-", vec![l, r], false),
            Mul => ("operator*", vec![l, r], false),
            Div => ("operator/", vec![l, r], false),
            Mod => ("operator%", vec![l, r], false),
            Pow => ("operator^", vec![l, r], false),
            Lt => ("operator<", vec![l, r], false),
            Gt => ("operator<", vec![r, l], false),
            Le => ("operator<", vec![r, l], true),
            Ge => ("operator<", vec![l, r], true),
            _ => unreachable!(),
        };
        let overloads = self.overloads(env, name);
        let binary: Vec<_> = overloads
            .iter()
            .filter(|o| o.sig.params.len() == 2)
            .cloned()
            .collect();
        if binary.is_empty() {
            return Err(Error::type_error(format!(
                "`{}` is not defined for {} and {}",
                op.symbol(),
                args[0].type_name(),
                args[1].type_name()
            )));
        }
        let types = (args[0].type_name(), args[1].type_name());
        let result = self
            .call_overloads(name, &binary, args, None)
            .map_err(|e| match e.kind {
                crate::error::ErrorKind::Type(_) if e.trace.is_empty() => {
                    Error::type_error(format!(
                        "`{}` is not defined for {} and {}",
                        op.symbol(),
                        types.0,
                        types.1
                    ))
                }
                _ => e,
            })?;
        if negate {
            let b = result
                .as_bool()
                .ok_or_else(|| Error::type_error("operator< must return a boolean"))?;
            Ok(Value::Bool(!b))
        } else {
            Ok(result)
        }
    }
}

/// Return `container` with `key` set to `value`, preserving type tags.
/// Arrays accept in-range integer indices only.
fn set_key(container: Value, key: Value, value: Value) -> EvalResult<Value> {
    let tags = container.clone();
    match container.untagged().clone() {
        Value::Map(mut m) => {
            Rc::make_mut(&mut m).insert(key, value);
            Ok(tags.retag(Value::Map(m)))
        }
        Value::Array(mut a) => {
            let i = key
                .as_number()
                .ok_or_else(|| Error::type_error("array index must be a number"))?;
            if i.fract() != 0.0 || i < 0.0 || i as usize >= a.len() {
                return Err(Error::runtime(format!(
                    "array index {} out of bounds (length {})",
                    i,
                    a.len()
                )));
            }
            Rc::make_mut(&mut a)[i as usize] = value;
            Ok(tags.retag(Value::Array(a)))
        }
        Value::Undefined => Err(Error::type_error("cannot set a member of undefined")),
        other => Err(Error::type_error(format!(
            "cannot set a member of a {}",
            other.type_name()
        ))),
    }
}
