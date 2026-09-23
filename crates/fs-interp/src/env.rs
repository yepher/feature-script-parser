//! Lexical environments and module namespaces.
//!
//! Name lookup order: enclosing block scopes → the module's own top-level
//! declarations → exported declarations of imported modules (following
//! `export import` chains). Imports are resolved by reference rather than by
//! copying names, which is what makes the standard library's circular imports
//! work without any special casing.

use crate::error::{Error, EvalResult};
use crate::value::{Function, Overload, Value};
use fs_syntax::ast::{Expr, Module, TypeRef};
use indexmap::IndexMap;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub struct Slot {
    pub value: Value,
    pub is_const: bool,
    /// Set while a top-level constant has not been evaluated yet.
    /// FeatureScript allows forward references between top-level constants,
    /// so they are initialised on first use rather than in source order.
    pub lazy: Option<Rc<LazyGlobal>>,
}

pub struct LazyGlobal {
    pub init: Option<Expr>,
    pub ty: Option<TypeRef>,
    pub span: fs_syntax::Span,
    /// Cycle guard.
    pub evaluating: Cell<bool>,
}

/// A shared, cached overload set.
pub type Overloads = Rc<Vec<Rc<Overload>>>;

/// Result of a module-level name lookup.
pub enum Lookup {
    Value(Value),
    /// The global exists but has not been initialised; force it in `module`.
    Lazy(Rc<ModuleEnv>, String),
}

/// A module's top-level namespace.
pub struct ModuleEnv {
    /// Canonical import path, e.g. `onshape/std/units.fs`.
    pub path: String,
    pub globals: RefCell<IndexMap<String, Slot>>,
    pub exports: RefCell<HashSet<String>>,
    pub imports: RefCell<Vec<Rc<ModuleEnv>>>,
    /// Subset of `imports` declared with `export import`.
    pub reexports: RefCell<Vec<Rc<ModuleEnv>>>,
    pub ast: RefCell<Option<Rc<Module>>>,
    /// The `FeatureScript <n>;` header, if any.
    pub language_version: RefCell<Option<u64>>,
    /// Cached overload sets, tagged with the interpreter's load generation.
    overload_cache: RefCell<HashMap<String, (u64, Overloads)>>,
}

impl ModuleEnv {
    pub fn new(path: &str) -> Rc<ModuleEnv> {
        Rc::new(ModuleEnv {
            path: path.to_string(),
            globals: RefCell::new(IndexMap::new()),
            exports: RefCell::new(HashSet::new()),
            imports: RefCell::new(Vec::new()),
            reexports: RefCell::new(Vec::new()),
            ast: RefCell::new(None),
            language_version: RefCell::new(None),
            overload_cache: RefCell::new(HashMap::new()),
        })
    }

    pub fn declare(&self, name: &str, value: Value, is_const: bool, export: bool) {
        self.globals.borrow_mut().insert(
            name.to_string(),
            Slot {
                value,
                is_const,
                lazy: None,
            },
        );
        if export {
            self.exports.borrow_mut().insert(name.to_string());
        }
    }

    pub fn declare_lazy(&self, name: &str, lazy: LazyGlobal, is_const: bool, export: bool) {
        self.globals.borrow_mut().insert(
            name.to_string(),
            Slot {
                value: Value::Undefined,
                is_const,
                lazy: Some(Rc::new(lazy)),
            },
        );
        if export {
            self.exports.borrow_mut().insert(name.to_string());
        }
    }

    /// Replace a lazy slot with its computed value.
    pub fn resolve_lazy(&self, name: &str, value: Value) {
        if let Some(slot) = self.globals.borrow_mut().get_mut(name) {
            slot.value = value;
            slot.lazy = None;
        }
    }

    pub fn lazy(&self, name: &str) -> Option<Rc<LazyGlobal>> {
        self.globals.borrow().get(name).and_then(|s| s.lazy.clone())
    }

    /// Add an overload to a named function/predicate/operator, creating the
    /// function value on first use.
    pub fn declare_overload(&self, name: &str, overload: Rc<Overload>, export: bool) {
        let mut globals = self.globals.borrow_mut();
        let existing = globals
            .get(name)
            .and_then(|s| s.value.as_function().cloned());
        let func = match existing {
            Some(f) => {
                let mut overloads = f.overloads.clone();
                overloads.push(overload);
                Rc::new(Function {
                    name: name.to_string(),
                    overloads,
                })
            }
            None => Rc::new(Function {
                name: name.to_string(),
                overloads: vec![overload],
            }),
        };
        globals.insert(
            name.to_string(),
            Slot {
                value: Value::Function(func),
                is_const: true,
                lazy: None,
            },
        );
        if export {
            self.exports.borrow_mut().insert(name.to_string());
        }
    }

    /// Look a name up in this module, then in its imports' exports.
    pub fn lookup(self: &Rc<Self>, name: &str) -> Option<Lookup> {
        if let Some(s) = self.globals.borrow().get(name) {
            return Some(if s.lazy.is_some() {
                Lookup::Lazy(self.clone(), name.to_string())
            } else {
                Lookup::Value(s.value.clone())
            });
        }
        let mut visited = HashSet::new();
        for import in self.imports.borrow().iter() {
            if let Some(v) = import.lookup_export(name, &mut visited) {
                return Some(v);
            }
        }
        None
    }

    fn lookup_export(
        self: &Rc<Self>,
        name: &str,
        visited: &mut HashSet<*const ModuleEnv>,
    ) -> Option<Lookup> {
        if !visited.insert(Rc::as_ptr(self)) {
            return None;
        }
        if self.exports.borrow().contains(name) {
            if let Some(s) = self.globals.borrow().get(name) {
                return Some(if s.lazy.is_some() {
                    Lookup::Lazy(self.clone(), name.to_string())
                } else {
                    Lookup::Value(s.value.clone())
                });
            }
        }
        for re in self.reexports.borrow().iter() {
            if let Some(v) = re.lookup_export(name, visited) {
                return Some(v);
            }
        }
        None
    }

    /// All overloads of a function name visible from this module: its own
    /// first, then every imported module's exported overloads. Used for
    /// operator overloads (`operator*` is defined in a dozen std modules).
    /// Results are cached per `generation` (bumped whenever a module loads).
    pub fn collect_overloads(&self, name: &str, generation: u64) -> Overloads {
        if let Some((gen, cached)) = self.overload_cache.borrow().get(name) {
            if *gen == generation {
                return cached.clone();
            }
        }
        let mut out = Vec::new();
        let mut visited: HashSet<*const ModuleEnv> = HashSet::new();
        visited.insert(self as *const _);
        if let Some(s) = self.globals.borrow().get(name) {
            if let Some(f) = s.value.as_function() {
                out.extend(f.overloads.iter().cloned());
            }
        }
        for import in self.imports.borrow().iter() {
            import.collect_exported_overloads(name, &mut out, &mut visited);
        }
        let out = Rc::new(out);
        self.overload_cache
            .borrow_mut()
            .insert(name.to_string(), (generation, out.clone()));
        out
    }

    fn collect_exported_overloads(
        &self,
        name: &str,
        out: &mut Vec<Rc<Overload>>,
        visited: &mut HashSet<*const ModuleEnv>,
    ) {
        if !visited.insert(self as *const _) {
            return;
        }
        if self.exports.borrow().contains(name) {
            if let Some(s) = self.globals.borrow().get(name) {
                if let Some(f) = s.value.as_function() {
                    out.extend(f.overloads.iter().cloned());
                }
            }
        }
        for re in self.reexports.borrow().iter() {
            re.collect_exported_overloads(name, out, visited);
        }
    }

    pub fn assign_global(&self, name: &str, value: Value) -> EvalResult<bool> {
        let mut globals = self.globals.borrow_mut();
        match globals.get_mut(name) {
            Some(slot) if slot.is_const => {
                Err(Error::runtime(format!("cannot assign to const `{name}`")))
            }
            Some(slot) => {
                slot.value = value;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

/// A block/function scope.
pub struct Env {
    vars: RefCell<IndexMap<String, Slot>>,
    parent: Option<Rc<Env>>,
    pub module: Rc<ModuleEnv>,
}

impl Env {
    pub fn module_scope(module: Rc<ModuleEnv>) -> Rc<Env> {
        Rc::new(Env {
            vars: RefCell::new(IndexMap::new()),
            parent: None,
            module,
        })
    }

    pub fn child(parent: &Rc<Env>) -> Rc<Env> {
        Rc::new(Env {
            vars: RefCell::new(IndexMap::new()),
            parent: Some(parent.clone()),
            module: parent.module.clone(),
        })
    }

    pub fn declare(&self, name: &str, value: Value, is_const: bool) {
        self.vars.borrow_mut().insert(
            name.to_string(),
            Slot {
                value,
                is_const,
                lazy: None,
            },
        );
    }

    /// Block-scope lookup, falling back to the module namespace. A `Lazy`
    /// result must be forced through `Interp::lookup`.
    pub fn lookup(&self, name: &str) -> Option<Lookup> {
        let mut env = self;
        loop {
            if let Some(s) = env.vars.borrow().get(name) {
                return Some(Lookup::Value(s.value.clone()));
            }
            match &env.parent {
                Some(p) => env = p,
                None => return env.module.lookup(name),
            }
        }
    }

    /// Is `name` bound in a block scope (as opposed to a module global)?
    pub fn is_local(&self, name: &str) -> bool {
        let mut env = self;
        loop {
            if env.vars.borrow().contains_key(name) {
                return true;
            }
            match &env.parent {
                Some(p) => env = p,
                None => return false,
            }
        }
    }

    pub fn assign(&self, name: &str, value: Value) -> EvalResult<()> {
        let mut env = self;
        loop {
            {
                let mut vars = env.vars.borrow_mut();
                if let Some(slot) = vars.get_mut(name) {
                    if slot.is_const {
                        return Err(Error::runtime(format!("cannot assign to const `{name}`")));
                    }
                    slot.value = value;
                    return Ok(());
                }
            }
            match &env.parent {
                Some(p) => env = p,
                None => {
                    if env.module.assign_global(name, value)? {
                        return Ok(());
                    }
                    return Err(Error::name(format!(
                        "assignment to undeclared variable `{name}`"
                    )));
                }
            }
        }
    }
}
