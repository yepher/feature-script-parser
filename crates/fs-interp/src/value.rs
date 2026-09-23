//! The FeatureScript value model.
//!
//! FeatureScript has *value semantics* for arrays and maps (assignment
//! copies) and a single reference type, `box`. Arrays and maps are held in
//! `Rc` and cloned copy-on-write (`Rc::make_mut`), so passing them around is
//! cheap and mutation only copies when the value is shared.
//!
//! Custom types (`export type Foo typecheck canBeFoo;`) are *tags* layered on
//! top of an ordinary value: `{..} as Foo` yields [`Value::Tagged`], `x is Foo`
//! checks for the tag, `x as map` strips it. Enum members are values of their
//! enum type. Anything the geometry kernel owns (a `Context`, a `Query`
//! evaluated to real topology, a sketch) is a [`Value::Native`] handle.

use fs_syntax::ast::FunctionSig;
use indexmap::IndexMap;
use std::any::Any;
use std::cell::RefCell;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::env::{Env, ModuleEnv};

pub type Map = IndexMap<Value, Value>;
pub type Array = Vec<Value>;

#[derive(Clone)]
pub enum Value {
    Undefined,
    Bool(bool),
    Number(f64),
    Str(Rc<str>),
    Array(Rc<Array>),
    Map(Rc<Map>),
    Box(Rc<RefCell<Value>>),
    Function(Rc<Function>),
    /// A member of an enum, e.g. `EntityType.FACE`.
    Enum(EnumValue),
    /// An enum type used as a value (`EntityType`), so that `.FACE` works and
    /// `x is EntityType` can be checked.
    EnumType(Rc<EnumDef>),
    /// A custom type used as a value (only meaningful for `is` / `as`).
    Type(Rc<TypeDef>),
    /// A value carrying a custom type tag.
    Tagged(Rc<TypeDef>, Rc<Value>),
    /// A kernel-owned handle.
    Native(Rc<dyn NativeValue>),
}

/// A value owned by the kernel (a `Context`, a `Query` result set, ...).
pub trait NativeValue: Any + fmt::Debug {
    /// Name reported by `@internalType` and in error messages.
    fn type_name(&self) -> &str;
    fn as_any(&self) -> &dyn Any;
    /// Identity-based equality by default; override for value-like handles.
    fn eq_native(&self, other: &dyn NativeValue) -> bool {
        std::ptr::eq(
            self.as_any() as *const _ as *const u8,
            other.as_any() as *const _ as *const u8,
        )
    }
}

#[derive(Debug)]
pub struct EnumDef {
    pub name: String,
    pub members: Vec<String>,
    /// Which module declared it (for identity and error messages).
    pub module: String,
}

#[derive(Clone)]
pub struct EnumValue {
    pub def: Rc<EnumDef>,
    pub index: usize,
}

impl EnumValue {
    pub fn name(&self) -> &str {
        &self.def.members[self.index]
    }
}

#[derive(Debug)]
pub struct TypeDef {
    pub name: String,
    /// Name of the `typecheck` predicate, resolved lazily in the declaring module.
    pub typecheck: Option<String>,
    pub module: String,
}

/// A callable. Named module-level functions and predicates may have several
/// overloads; lambdas and `function(...)` expressions have exactly one.
pub struct Function {
    pub name: String,
    pub overloads: Vec<Rc<Overload>>,
}

pub struct Overload {
    pub sig: Rc<FunctionSig>,
    /// Captured environment for closures; module scope for declarations.
    pub env: Rc<Env>,
    pub module: Rc<ModuleEnv>,
    pub kind: FnKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FnKind {
    Function,
    /// `predicate`: body statements are checks; the call returns bool.
    Predicate,
    /// `x => expr` / `(a, b) => { ... }`.
    Lambda,
}

// ---------------------------------------------------------------------------
// Constructors & accessors
// ---------------------------------------------------------------------------

impl Value {
    pub fn str(s: impl Into<Rc<str>>) -> Value {
        Value::Str(s.into())
    }
    pub fn array(items: Vec<Value>) -> Value {
        Value::Array(Rc::new(items))
    }
    pub fn map(m: Map) -> Value {
        Value::Map(Rc::new(m))
    }
    pub fn empty_map() -> Value {
        Value::Map(Rc::new(Map::new()))
    }
    pub fn native<T: NativeValue>(v: T) -> Value {
        Value::Native(Rc::new(v))
    }

    /// The value with all custom type tags removed.
    pub fn untagged(&self) -> &Value {
        let mut v = self;
        while let Value::Tagged(_, inner) = v {
            v = inner;
        }
        v
    }

    /// Tags applied to this value, outermost first.
    pub fn tags(&self) -> Vec<Rc<TypeDef>> {
        let mut out = Vec::new();
        let mut v = self;
        while let Value::Tagged(t, inner) = v {
            out.push(t.clone());
            v = inner;
        }
        out
    }

    pub fn has_tag(&self, ty: &TypeDef) -> bool {
        self.tags()
            .iter()
            .any(|t| t.name == ty.name && t.module == ty.module)
    }

    /// Re-apply `self`'s tags (outermost first) onto `inner`.
    pub fn retag(&self, inner: Value) -> Value {
        let tags = self.tags();
        tags.into_iter()
            .rev()
            .fold(inner, |acc, t| Value::Tagged(t, Rc::new(acc)))
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self.untagged(), Value::Undefined)
    }

    pub fn as_number(&self) -> Option<f64> {
        match self.untagged() {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self.untagged() {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self.untagged() {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&Rc<Array>> {
        match self.untagged() {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_map(&self) -> Option<&Rc<Map>> {
        match self.untagged() {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_function(&self) -> Option<&Rc<Function>> {
        match self.untagged() {
            Value::Function(f) => Some(f),
            _ => None,
        }
    }
    pub fn as_native<T: NativeValue>(&self) -> Option<&T> {
        match self.untagged() {
            Value::Native(n) => n.as_any().downcast_ref::<T>(),
            _ => None,
        }
    }

    /// Builtin type name as FeatureScript reports it (`number`, `map`, ...).
    pub fn type_name(&self) -> String {
        match self.untagged() {
            Value::Undefined => "undefined".into(),
            Value::Bool(_) => "boolean".into(),
            Value::Number(_) => "number".into(),
            Value::Str(_) => "string".into(),
            Value::Array(_) => "array".into(),
            Value::Map(_) => "map".into(),
            Value::Box(_) => "box".into(),
            Value::Function(_) => "function".into(),
            Value::Enum(e) => e.def.name.clone(),
            Value::EnumType(e) => format!("enum {}", e.name),
            Value::Type(t) => format!("type {}", t.name),
            Value::Native(n) => n.type_name().to_string(),
            Value::Tagged(..) => unreachable!(),
        }
    }

    /// Map/array member lookup by key; `None` when absent.
    pub fn get(&self, key: &Value) -> Option<Value> {
        match self.untagged() {
            Value::Map(m) => m.get(key).cloned(),
            Value::Array(a) => {
                let i = key.as_number()?;
                if i.fract() != 0.0 || i < 0.0 {
                    return None;
                }
                a.get(i as usize).cloned()
            }
            _ => None,
        }
    }

    /// FeatureScript's `"" ~ value` conversion.
    pub fn to_fs_string(&self) -> String {
        format!("{self}")
    }
}

// ---------------------------------------------------------------------------
// Equality / hashing (needed for map keys)
// ---------------------------------------------------------------------------

fn canonical_bits(n: f64) -> u64 {
    if n == 0.0 {
        0
    } else if n.is_nan() {
        u64::MAX
    } else {
        n.to_bits()
    }
}

impl PartialEq for Value {
    /// Structural equality. Type tags are ignored, matching FeatureScript,
    /// where `[1, 2] == vector(1, 2)` is true.
    fn eq(&self, other: &Value) -> bool {
        use Value::*;
        match (self.untagged(), other.untagged()) {
            (Undefined, Undefined) => true,
            (Bool(a), Bool(b)) => a == b,
            (Number(a), Number(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (Array(a), Array(b)) => a == b,
            (Map(a), Map(b)) => a.len() == b.len() && a.iter().all(|(k, v)| b.get(k) == Some(v)),
            (Box(a), Box(b)) => Rc::ptr_eq(a, b),
            (Function(a), Function(b)) => Rc::ptr_eq(a, b),
            (Enum(a), Enum(b)) => Rc::ptr_eq(&a.def, &b.def) && a.index == b.index,
            (EnumType(a), EnumType(b)) => Rc::ptr_eq(a, b),
            (Type(a), Type(b)) => Rc::ptr_eq(a, b),
            (Native(a), Native(b)) => a.eq_native(&**b),
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        use Value::*;
        match self.untagged() {
            Undefined => 0u8.hash(state),
            Bool(b) => {
                1u8.hash(state);
                b.hash(state)
            }
            Number(n) => {
                2u8.hash(state);
                canonical_bits(*n).hash(state)
            }
            Str(s) => {
                3u8.hash(state);
                s.hash(state)
            }
            Array(a) => {
                4u8.hash(state);
                for v in a.iter() {
                    v.hash(state)
                }
            }
            Map(m) => {
                5u8.hash(state);
                // Order-independent.
                let mut acc: u64 = 0;
                for (k, v) in m.iter() {
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    k.hash(&mut h);
                    v.hash(&mut h);
                    acc = acc.wrapping_add(h.finish());
                }
                acc.hash(state)
            }
            Box(b) => (Rc::as_ptr(b) as usize).hash(state),
            Function(f) => (Rc::as_ptr(f) as usize).hash(state),
            Enum(e) => {
                6u8.hash(state);
                (Rc::as_ptr(&e.def) as usize).hash(state);
                e.index.hash(state)
            }
            EnumType(e) => (Rc::as_ptr(e) as usize).hash(state),
            Type(t) => (Rc::as_ptr(t) as usize).hash(state),
            Native(n) => (Rc::as_ptr(n) as *const u8 as usize).hash(state),
            Tagged(..) => unreachable!(),
        }
    }
}

// ---------------------------------------------------------------------------
// Display (FeatureScript's toString conventions)
// ---------------------------------------------------------------------------

pub fn format_number(n: f64) -> String {
    if n.is_nan() {
        "NaN".into()
    } else if n.is_infinite() {
        if n > 0.0 {
            "inf".into()
        } else {
            "-inf".into()
        }
    } else if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Value::*;
        match self {
            Undefined => write!(f, "undefined"),
            Bool(b) => write!(f, "{b}"),
            Number(n) => write!(f, "{}", format_number(*n)),
            Str(s) => write!(f, "{s}"),
            Array(a) => {
                write!(f, "[")?;
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, " {} ", DisplayNested(v))?;
                }
                write!(f, "]")
            }
            Map(m) => {
                write!(f, "{{")?;
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, " {} : {} ", DisplayNested(k), DisplayNested(v))?;
                }
                write!(f, "}}")
            }
            Box(b) => write!(f, "box({})", DisplayNested(&b.borrow())),
            Function(func) => write!(f, "function {}", func.name),
            Enum(e) => write!(f, "{}", e.name()),
            EnumType(e) => write!(f, "{}", e.name),
            Type(t) => write!(f, "{}", t.name),
            Tagged(t, inner) => match &**inner {
                // Tagged containers print with their type name, like Onshape.
                Map(_) | Array(_) => write!(f, "{} {}", t.name, inner),
                other => write!(f, "{other}"),
            },
            Native(n) => write!(f, "<{}>", n.type_name()),
        }
    }
}

/// Strings inside containers print quoted.
struct DisplayNested<'a>(&'a Value);

impl fmt::Display for DisplayNested<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.untagged() {
            Value::Str(s) => write!(f, "\"{s}\""),
            _ => write!(f, "{}", self.0),
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Tagged(t, inner) => write!(f, "({:?} as {})", inner, t.name),
            _ => write!(f, "{self}"),
        }
    }
}

impl fmt::Debug for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "function {} ({} overloads)",
            self.name,
            self.overloads.len()
        )
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Number(n)
    }
}
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.into())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s.into())
    }
}
