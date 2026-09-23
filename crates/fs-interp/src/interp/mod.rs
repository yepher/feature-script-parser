//! The tree-walking evaluator.
//!
//! * `mod.rs`  — the [`Interp`] struct, module loading, top-level
//!   declarations, function calls, overload resolution, `is`/`as`.
//! * `expr.rs` — expression evaluation and assignment targets.
//! * `stmt.rs` — statements, blocks, predicate/precondition mode.

mod expr;
mod stmt;

use crate::env::{Env, LazyGlobal, Lookup, ModuleEnv};
use crate::error::{Error, ErrorKind, EvalResult, Flow};
use crate::intrinsics;
use crate::kernel::{BuiltinResult, Kernel};
use crate::value::{EnumDef, FnKind, Function, Overload, TypeDef, Value};
use fs_syntax::ast::{Block, FunctionSig, ItemKind, Module, Stmt, StmtKind, Version};
use fs_syntax::{parse_module, Span};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

/// Supplies module source text for an import path.
pub trait ModuleLoader {
    fn source(&self, path: &str) -> Option<String>;
}

/// Loads `onshape/std/<name>.fs` from a directory (e.g. a checkout of the
/// standard library mirror). Other paths are taken relative to the same root.
pub struct DirLoader {
    pub root: PathBuf,
}

impl ModuleLoader for DirLoader {
    fn source(&self, path: &str) -> Option<String> {
        let rel = path.strip_prefix("onshape/std/").unwrap_or(path);
        std::fs::read_to_string(self.root.join(rel)).ok()
    }
}

/// In-memory modules, for tests and embedding.
#[derive(Default)]
pub struct MemoryLoader {
    pub modules: HashMap<String, String>,
    /// Consulted for paths not in `modules`.
    pub fallback: Option<Box<dyn ModuleLoader>>,
}

impl ModuleLoader for MemoryLoader {
    fn source(&self, path: &str) -> Option<String> {
        self.modules
            .get(path)
            .cloned()
            .or_else(|| self.fallback.as_ref().and_then(|f| f.source(path)))
    }
}

pub struct Interp {
    pub kernel: Box<dyn Kernel>,
    loader: Box<dyn ModuleLoader>,
    modules: HashMap<String, Rc<ModuleEnv>>,
    /// Call stack for error traces; also bounds recursion.
    call_stack: Vec<String>,
    pub max_call_depth: usize,
    /// Module whose code is currently executing (for `@getLanguageVersion`).
    current_module: Option<Rc<ModuleEnv>>,
    /// Bumped whenever a module is loaded; invalidates overload caches.
    generation: u64,
}

/// Builtin type names accepted by `is` / `as`.
const BUILTIN_TYPES: &[&str] = &[
    "number",
    "string",
    "boolean",
    "array",
    "map",
    "box",
    "function",
    "undefined",
];

impl Interp {
    pub fn new(kernel: Box<dyn Kernel>, loader: Box<dyn ModuleLoader>) -> Interp {
        Interp {
            kernel,
            loader,
            modules: HashMap::new(),
            call_stack: Vec::new(),
            max_call_depth: 2000,
            current_module: None,
            generation: 0,
        }
    }

    /// Convenience: a stub kernel and a std-library directory.
    pub fn with_std_dir(root: impl Into<PathBuf>) -> Interp {
        Interp::new(
            Box::new(crate::kernel::StubKernel::new()),
            Box::new(DirLoader { root: root.into() }),
        )
    }

    pub fn module(&self, path: &str) -> Option<&Rc<ModuleEnv>> {
        self.modules.get(path)
    }

    /// All loaded modules, in load order.
    pub fn modules(&self) -> Vec<Rc<ModuleEnv>> {
        let mut v: Vec<_> = self.modules.values().cloned().collect();
        v.sort_by(|a, b| a.path.cmp(&b.path));
        v
    }

    // ------------------------------------------------------------------
    // Module loading
    // ------------------------------------------------------------------

    /// Load (or fetch from cache) the module at an import path such as
    /// `onshape/std/geometry.fs`.
    pub fn load_module(&mut self, path: &str) -> EvalResult<Rc<ModuleEnv>> {
        if let Some(m) = self.modules.get(path) {
            return Ok(m.clone());
        }
        let src = self
            .loader
            .source(path)
            .ok_or_else(|| Error::new(ErrorKind::Load(format!("module not found: {path}"))))?;
        let ast = parse_module(&src)
            .map_err(|e| Error::new(ErrorKind::Load(e.render(path, &src).to_string())))?;
        self.load_parsed(path, Rc::new(ast))
    }

    /// Load a module from an already-parsed AST under the given path.
    pub fn load_parsed(&mut self, path: &str, ast: Rc<Module>) -> EvalResult<Rc<ModuleEnv>> {
        self.generation += 1;
        let module = ModuleEnv::new(path);
        // Register before evaluating so circular imports resolve to this
        // (partially initialised) module instead of recursing forever.
        self.modules.insert(path.to_string(), module.clone());
        *module.ast.borrow_mut() = Some(ast.clone());
        if let Some(h) = &ast.header {
            *module.language_version.borrow_mut() = match h.version {
                Version::Number(n) => Some(n),
                Version::Sparkle => None,
            };
        }
        let scope = Env::module_scope(module.clone());

        // Pass 1: functions, predicates, operators, enums, types. These need
        // no evaluation, so declaring them first lets circular imports find
        // them even while this module is still loading.
        for item in &ast.items {
            match &item.kind {
                ItemKind::Function(f) => {
                    let ov = self.make_overload(&f.sig, &scope, &module, FnKind::Function);
                    module.declare_overload(&f.name.name, ov, item.export);
                }
                ItemKind::Operator(o) => {
                    let ov = self.make_overload(&o.sig, &scope, &module, FnKind::Function);
                    module.declare_overload(&format!("operator{}", o.op), ov, item.export);
                }
                ItemKind::Predicate(p) => {
                    let sig = Rc::new(FunctionSig {
                        params: p.params.clone(),
                        returns: None,
                        precondition: p.precondition.clone(),
                        body: p.body.clone(),
                        span: p.span,
                    });
                    let ov = Rc::new(Overload {
                        sig,
                        env: scope.clone(),
                        module: module.clone(),
                        kind: FnKind::Predicate,
                    });
                    module.declare_overload(&p.name.name, ov, item.export);
                }
                ItemKind::Enum(e) => {
                    let def = Rc::new(EnumDef {
                        name: e.name.name.clone(),
                        members: e.members.iter().map(|m| m.name.name.clone()).collect(),
                        module: path.to_string(),
                    });
                    module.declare(&e.name.name, Value::EnumType(def), true, item.export);
                }
                ItemKind::Type(t) => {
                    let def = Rc::new(TypeDef {
                        name: t.name.name.clone(),
                        typecheck: t.typecheck.as_ref().map(|i| i.name.clone()),
                        module: path.to_string(),
                    });
                    module.declare(&t.name.name, Value::Type(def), true, item.export);
                }
                ItemKind::Import(_) | ItemKind::Const(_) | ItemKind::Var(_) => {}
            }
        }

        // Pass 2: imports.
        for item in &ast.items {
            if let ItemKind::Import(imp) = &item.kind {
                let target = imp.path().ok_or_else(|| {
                    Error::new(ErrorKind::Load("import without a path".into())).at(imp.span)
                })?;
                let dep = self.load_module(target)?;
                module.imports.borrow_mut().push(dep.clone());
                if item.export {
                    module.reexports.borrow_mut().push(dep);
                }
            }
        }

        // Pass 3: constants and variables are registered lazily and evaluated
        // on first use. This matches Onshape: the standard library both
        // forward-references constants declared later in the same file and
        // has constants whose initialisers would throw if evaluated (e.g.
        // `tolerance.fs` packages a precondition by calling a one-parameter
        // function with no arguments).
        for item in &ast.items {
            if let ItemKind::Const(v) | ItemKind::Var(v) = &item.kind {
                let is_const = matches!(item.kind, ItemKind::Const(_));
                module.declare_lazy(
                    &v.name.name,
                    LazyGlobal {
                        init: v.init.clone(),
                        ty: v.ty.clone(),
                        span: v.span,
                        evaluating: std::cell::Cell::new(false),
                    },
                    is_const,
                    item.export,
                );
            }
        }
        Ok(module)
    }

    /// Evaluate every top-level constant of a module now, returning the
    /// errors (name → error) for the ones that fail. Useful for diagnostics.
    pub fn force_all_globals(&mut self, module: &Rc<ModuleEnv>) -> Vec<(String, Error)> {
        let names: Vec<String> = module.globals.borrow().keys().cloned().collect();
        let mut failures = Vec::new();
        for name in names {
            if module.lazy(&name).is_some() {
                if let Err(e) = self.force_global(module, &name) {
                    failures.push((name, self.locate(e, &module.path)));
                }
            }
        }
        failures
    }

    /// Evaluate a lazily-declared top-level constant, if not done already.
    fn force_global(&mut self, module: &Rc<ModuleEnv>, name: &str) -> EvalResult<Value> {
        let Some(lazy) = module.lazy(name) else {
            return Ok(match module.lookup(name) {
                Some(Lookup::Value(v)) => v,
                _ => Value::Undefined,
            });
        };
        if lazy.evaluating.get() {
            return Err(Error::runtime(format!(
                "constant `{name}` depends on itself during initialisation"
            ))
            .at(lazy.span));
        }
        lazy.evaluating.set(true);
        let scope = Env::module_scope(module.clone());
        let saved = self.current_module.replace(module.clone());
        let result = (|| {
            let value = match &lazy.init {
                Some(e) => self.eval(e, &scope)?,
                None => Value::Undefined,
            };
            if let Some(ty) = &lazy.ty {
                if !self.check_type(&value, &ty.name, &scope)? {
                    return Err(
                        Error::type_error(format!("`{name}` is not a {}", ty.name)).at(lazy.span)
                    );
                }
            }
            Ok(value)
        })();
        self.current_module = saved;
        lazy.evaluating.set(false);
        let value = result?;
        module.resolve_lazy(name, value.clone());
        Ok(value)
    }

    /// Resolve a name from a scope, forcing lazy globals.
    pub(crate) fn lookup(&mut self, env: &Rc<Env>, name: &str) -> EvalResult<Option<Value>> {
        match env.lookup(name) {
            None => Ok(None),
            Some(Lookup::Value(v)) => Ok(Some(v)),
            Some(Lookup::Lazy(module, name)) => self.force_global(&module, &name).map(Some),
        }
    }

    pub(crate) fn overloads(&self, env: &Rc<Env>, name: &str) -> Rc<Vec<Rc<Overload>>> {
        env.module.collect_overloads(name, self.generation)
    }

    fn make_overload(
        &self,
        sig: &FunctionSig,
        env: &Rc<Env>,
        module: &Rc<ModuleEnv>,
        kind: FnKind,
    ) -> Rc<Overload> {
        Rc::new(Overload {
            sig: Rc::new(sig.clone()),
            env: env.clone(),
            module: module.clone(),
            kind,
        })
    }

    fn locate(&self, mut e: Error, module: &str) -> Error {
        if e.module.is_none() {
            e.module = Some(module.to_string());
        }
        e
    }

    // ------------------------------------------------------------------
    // Public evaluation entry points
    // ------------------------------------------------------------------

    /// Evaluate an expression in the top-level scope of a loaded module.
    pub fn eval_in(&mut self, module: &Rc<ModuleEnv>, src: &str) -> EvalResult<Value> {
        let expr = fs_syntax::parse_expr(src)
            .map_err(|e| Error::new(ErrorKind::Load(e.render("<expr>", src).to_string())))?;
        let scope = Env::module_scope(module.clone());
        let saved = self.current_module.replace(module.clone());
        let r = self.eval(&expr, &scope);
        self.current_module = saved;
        r
    }

    /// Call a function value with arguments.
    pub fn call(&mut self, func: &Value, args: Vec<Value>) -> EvalResult<Value> {
        self.call_value(func, args, None)
    }

    // ------------------------------------------------------------------
    // Calls
    // ------------------------------------------------------------------

    pub(crate) fn call_value(
        &mut self,
        callee: &Value,
        args: Vec<Value>,
        span: Option<Span>,
    ) -> EvalResult<Value> {
        match callee.untagged() {
            Value::Function(f) => self.call_overloads(&f.name, &f.overloads, args, span),
            other => Err(Error::type_error(format!(
                "cannot call a {}",
                other.type_name()
            ))),
        }
    }

    /// Pick the best overload for `args` and run it.
    pub(crate) fn call_overloads(
        &mut self,
        name: &str,
        overloads: &[Rc<Overload>],
        args: Vec<Value>,
        span: Option<Span>,
    ) -> EvalResult<Value> {
        let chosen = self.select_overload(overloads, &args)?.ok_or_else(|| {
            let types: Vec<String> = args.iter().map(|a| a.type_name()).collect();
            Error::type_error(format!(
                "no overload of `{name}` accepts ({})",
                types.join(", ")
            ))
        })?;
        self.invoke(name, &chosen, args, span)
    }

    /// Overload resolution: arity must match and every `is Type` parameter
    /// constraint must hold; among candidates the one satisfying the most
    /// typed parameters wins, ties going to the earliest declaration.
    fn select_overload(
        &mut self,
        overloads: &[Rc<Overload>],
        args: &[Value],
    ) -> EvalResult<Option<Rc<Overload>>> {
        let mut best: Option<(usize, Rc<Overload>)> = None;
        for ov in overloads {
            if ov.sig.params.len() != args.len() {
                continue;
            }
            let mut score = 0usize;
            let mut ok = true;
            for (p, a) in ov.sig.params.iter().zip(args) {
                if let Some(ty) = &p.ty {
                    if self.check_type(a, &ty.name, &ov.env)? {
                        score += 1;
                    } else {
                        ok = false;
                        break;
                    }
                }
            }
            if ok && best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, ov.clone()));
            }
        }
        Ok(best.map(|(_, ov)| ov))
    }

    fn invoke(
        &mut self,
        name: &str,
        ov: &Rc<Overload>,
        args: Vec<Value>,
        span: Option<Span>,
    ) -> EvalResult<Value> {
        if self.call_stack.len() >= self.max_call_depth {
            return Err(Error::runtime(format!(
                "call depth exceeded ({}) calling `{name}`",
                self.max_call_depth
            )));
        }
        let scope = Env::child(&ov.env);
        for (p, a) in ov.sig.params.iter().zip(args) {
            scope.declare(&p.name.name, a, false);
        }
        self.call_stack.push(format!("{name}@{}", ov.module.path));
        let saved = self.current_module.replace(ov.module.clone());
        let result = self.run_body(name, ov, &scope);
        self.current_module = saved;
        self.call_stack.pop();
        result.map_err(|mut e| {
            if let Some(s) = span {
                e = e.at(s);
            }
            if e.trace.len() < 32 {
                e.trace.push(format!("{name}@{}", ov.module.path));
            }
            e
        })
    }

    fn run_body(&mut self, name: &str, ov: &Rc<Overload>, scope: &Rc<Env>) -> EvalResult<Value> {
        if let Some(pre) = &ov.sig.precondition {
            if !self.check_block(pre, scope)? {
                return Err(
                    Error::precondition(format!("precondition of `{name}` failed")).at(pre.span),
                );
            }
        }
        match ov.kind {
            FnKind::Predicate => Ok(Value::Bool(self.check_block(&ov.sig.body, scope)?)),
            FnKind::Function | FnKind::Lambda => match self.exec_block(&ov.sig.body, scope) {
                Ok(()) => Ok(Value::Undefined),
                Err(Flow::Return(v)) => Ok(v),
                Err(Flow::Error(e)) => Err(e),
                Err(Flow::Break) | Err(Flow::Continue) => {
                    Err(Error::runtime("`break`/`continue` outside of a loop"))
                }
            },
        }
    }

    /// Run a predicate or precondition block: every expression statement
    /// must evaluate to `true`. Returns `false` on the first failure.
    pub(crate) fn check_block(&mut self, block: &Block, scope: &Rc<Env>) -> EvalResult<bool> {
        let inner = Env::child(scope);
        match self.exec_stmts_checking(&block.stmts, &inner) {
            Ok(ok) => Ok(ok),
            Err(Flow::Error(e)) => Err(e),
            Err(Flow::Return(_)) => Err(Error::runtime("`return` inside a predicate")),
            Err(Flow::Break) | Err(Flow::Continue) => {
                Err(Error::runtime("`break`/`continue` outside of a loop"))
            }
        }
    }

    /// `@name(args)`: intrinsics first, then the kernel.
    pub(crate) fn call_builtin(
        &mut self,
        name: &str,
        args: Vec<Value>,
        span: Span,
    ) -> EvalResult<Value> {
        match name {
            "@print" => {
                let text = args.first().map(|v| v.to_fs_string()).unwrap_or_default();
                self.kernel.print(&text);
                return Ok(Value::Undefined);
            }
            "@getLanguageVersion" => {
                let v = self
                    .current_module
                    .as_ref()
                    .and_then(|m| *m.language_version.borrow());
                return Ok(v
                    .map(|n| Value::Number(n as f64))
                    .unwrap_or(Value::Undefined));
            }
            "@internalType" => {
                return Ok(Value::from(
                    args.first().map(|v| v.type_name()).unwrap_or_default(),
                ));
            }
            _ => {}
        }
        if let Some(r) = intrinsics::call(name, &args).or_else(|| crate::matrix::call(name, &args))
        {
            return r.map_err(|e| e.at(span));
        }
        match self.kernel.builtin(name, &args) {
            BuiltinResult::Value(v) => Ok(v),
            BuiltinResult::NotHandled => Err(Error::unimplemented(name).at(span)),
            BuiltinResult::Throw(v) => Err(Error::thrown(v).at(span)),
            BuiltinResult::Error(msg) => Err(Error::runtime(format!("{name}: {msg}")).at(span)),
        }
    }

    // ------------------------------------------------------------------
    // Types
    // ------------------------------------------------------------------

    /// `value is <type_name>`, resolving custom types/enums in `scope`.
    pub(crate) fn check_type(
        &mut self,
        value: &Value,
        type_name: &str,
        scope: &Rc<Env>,
    ) -> EvalResult<bool> {
        if BUILTIN_TYPES.contains(&type_name) {
            return Ok(value.type_name() == type_name);
        }
        match self.lookup(scope, type_name)? {
            Some(Value::Type(t)) => Ok(value.has_tag(&t)),
            Some(Value::EnumType(def)) => {
                Ok(matches!(value.untagged(), Value::Enum(e) if Rc::ptr_eq(&e.def, &def)))
            }
            Some(other) => Err(Error::type_error(format!(
                "`{type_name}` is not a type (it is a {})",
                other.type_name()
            ))),
            None => Err(Error::name(format!("unknown type `{type_name}`"))),
        }
    }

    /// `value as <type_name>`: builtin types strip tags; custom types run the
    /// typecheck predicate and add the tag.
    pub(crate) fn cast(
        &mut self,
        value: Value,
        type_name: &str,
        scope: &Rc<Env>,
    ) -> EvalResult<Value> {
        if BUILTIN_TYPES.contains(&type_name) {
            let inner = value.untagged().clone();
            if inner.type_name() != type_name {
                return Err(Error::type_error(format!(
                    "cannot cast {} to {type_name}",
                    inner.type_name()
                )));
            }
            return Ok(inner);
        }
        match self.lookup(scope, type_name)? {
            Some(Value::Type(t)) => {
                if value.has_tag(&t) {
                    return Ok(value);
                }
                if let Some(pred_name) = &t.typecheck {
                    let decl_module = self.modules.get(&t.module).cloned().ok_or_else(|| {
                        Error::name(format!(
                            "module `{}` for type `{}` not loaded",
                            t.module, t.name
                        ))
                    })?;
                    let decl_scope = Env::module_scope(decl_module);
                    let pred = self.lookup(&decl_scope, pred_name)?.ok_or_else(|| {
                        Error::name(format!("typecheck predicate `{pred_name}` not found"))
                    })?;
                    let ok = self.call_value(&pred, vec![value.clone()], None)?;
                    if ok.as_bool() != Some(true) {
                        return Err(Error::type_error(format!(
                            "value does not satisfy typecheck `{pred_name}` for `{}`: {value:?}",
                            t.name
                        )));
                    }
                }
                Ok(Value::Tagged(t, Rc::new(value)))
            }
            Some(Value::EnumType(_)) => Err(Error::type_error(format!(
                "cannot cast to enum `{type_name}`"
            ))),
            Some(other) => Err(Error::type_error(format!(
                "`{type_name}` is not a type (it is a {})",
                other.type_name()
            ))),
            None => Err(Error::name(format!("unknown type `{type_name}`"))),
        }
    }

    /// Build a closure for `function(...) {...}` or a lambda.
    pub(crate) fn make_closure(
        &self,
        name: &str,
        sig: Rc<FunctionSig>,
        env: &Rc<Env>,
        kind: FnKind,
    ) -> Value {
        Value::Function(Rc::new(Function {
            name: name.to_string(),
            overloads: vec![Rc::new(Overload {
                sig,
                env: env.clone(),
                module: env.module.clone(),
                kind,
            })],
        }))
    }
}

/// Wrap a lambda's expression body as `{ return expr; }`.
pub(crate) fn return_block(expr: &fs_syntax::ast::Expr) -> Block {
    Block {
        stmts: vec![Stmt {
            annotation: None,
            kind: StmtKind::Return(Some(expr.clone())),
            span: expr.span,
        }],
        span: expr.span,
    }
}
