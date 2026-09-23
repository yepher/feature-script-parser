//! The kernel seam.
//!
//! Every geometry-facing operation in the FeatureScript standard library
//! bottoms out in an `@builtin` call: `opExtrude` is FeatureScript that
//! validates its arguments and then calls `@opExtrude(context, id, definition)`;
//! `evPlane` calls `@evPlane`; a `Context` is whatever `@newContext` returns.
//! So a geometry kernel plugs in by implementing [`Kernel::builtin`] for those
//! names. Pure-language builtins (`@size`, `@sqrt`, `@match`, ...) are handled
//! by the interpreter before the kernel is consulted (see `intrinsics.rs`).

use crate::error::EvalResult;
use crate::value::{NativeValue, Value};
use std::any::Any;
use std::cell::RefCell;

/// The interpreter as seen from inside a kernel builtin: a way to call
/// FeatureScript functions (std constructors such as `plane`, `vector`,
/// `qTransient`), read globals (`meter`, `QueryType`) and apply type tags,
/// so a kernel can return values that satisfy the std library's typecheck
/// predicates without re-implementing them.
///
/// Names resolve in the module that made the builtin call, then in the
/// exports of every loaded module. While a builtin runs, the kernel itself
/// is detached from the interpreter, so a host call that reaches another
/// `@builtin` fails with `Unimplemented`.
pub trait Host {
    fn call(&mut self, name: &str, args: Vec<Value>) -> EvalResult<Value>;
    fn global(&mut self, name: &str) -> EvalResult<Value>;
    /// `value as Type` (runs the typecheck predicate and tags).
    fn cast(&mut self, value: Value, type_name: &str) -> EvalResult<Value>;
    /// `value is Type`.
    fn is_type(&mut self, value: &Value, type_name: &str) -> EvalResult<bool>;
}

/// Outcome of asking a kernel to handle a builtin.
pub enum BuiltinResult {
    Value(Value),
    /// The kernel does not implement this builtin. The interpreter reports it
    /// as an `Unimplemented` error naming the builtin.
    NotHandled,
    /// The kernel raised a FeatureScript-visible error (becomes a `throw` of
    /// the given value, so `try` can catch it).
    Throw(Value),
    /// Fatal kernel failure with a message.
    Error(String),
}

pub trait Kernel {
    /// Handle `@name(args...)`. `name` includes the leading `@`.
    fn builtin(&mut self, name: &str, args: &[Value], host: &mut dyn Host) -> BuiltinResult;

    /// Called for `@print` output. Defaults to stdout.
    fn print(&mut self, text: &str) {
        print!("{text}");
    }

    /// For downcasting a `Box<dyn Kernel>` back to the concrete type
    /// (e.g. to read a [`StubKernel`]'s recorded calls). Implement as `self`.
    fn as_any(&self) -> &dyn Any;
}

/// A `Context` handle produced by [`StubKernel`].
#[derive(Debug)]
pub struct StubContext {
    pub version: f64,
}

impl NativeValue for StubContext {
    fn type_name(&self) -> &str {
        "Context"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A record of one builtin the stub was asked for but does not implement.
#[derive(Debug, Clone)]
pub struct RecordedCall {
    pub name: String,
    pub args: Vec<Value>,
}

/// A kernel with no geometry. It understands just enough for the pure parts
/// of the standard library to load (`@newContext`, `@isContext`, version
/// queries) and records every other builtin it is asked for, returning
/// `NotHandled` so the interpreter raises `Unimplemented`.
///
/// Useful for tests and for discovering which builtins a feature needs.
#[derive(Default)]
pub struct StubKernel {
    pub calls: RefCell<Vec<RecordedCall>>,
    pub output: RefCell<String>,
    /// Version reported by `@getCurrentVersion` / accepted by `@isAtVersionOrLater`.
    pub version: f64,
}

impl StubKernel {
    pub fn new() -> Self {
        StubKernel {
            version: 2960.0,
            ..Default::default()
        }
    }
}

impl Kernel for StubKernel {
    fn builtin(&mut self, name: &str, args: &[Value], _host: &mut dyn Host) -> BuiltinResult {
        match name {
            "@newContext" => {
                let version = args
                    .first()
                    .and_then(enum_or_number)
                    .unwrap_or(self.version);
                BuiltinResult::Value(Value::native(StubContext { version }))
            }
            "@isContext" => BuiltinResult::Value(Value::Bool(
                args.first()
                    .is_some_and(|v| v.as_native::<StubContext>().is_some()),
            )),
            "@getCurrentVersion" => {
                let v = args
                    .first()
                    .and_then(|c| c.as_native::<StubContext>())
                    .map(|c| c.version)
                    .unwrap_or(self.version);
                BuiltinResult::Value(Value::Number(v))
            }
            // `@isAtVersionOrLater(context, introduced)`: everything is "at
            // or later" than the newest std release we know about.
            "@isAtVersionOrLater" => BuiltinResult::Value(Value::Bool(true)),
            "@clampContextVersion" => {
                BuiltinResult::Value(args.first().cloned().unwrap_or(Value::Undefined))
            }
            _ => {
                self.calls.borrow_mut().push(RecordedCall {
                    name: name.to_string(),
                    args: args.to_vec(),
                });
                BuiltinResult::NotHandled
            }
        }
    }

    fn print(&mut self, text: &str) {
        self.output.borrow_mut().push_str(text);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Enum members of `FeatureScriptVersionNumber` are passed to `@newContext`;
/// accept either those or a plain number.
fn enum_or_number(v: &Value) -> Option<f64> {
    match v.untagged() {
        Value::Number(n) => Some(*n),
        Value::Enum(e) => e
            .name()
            .trim_start_matches('V')
            .split('_')
            .next()
            .and_then(|s| s.parse().ok()),
        _ => None,
    }
}

/// Stands in for the real kernel while it is detached during a builtin call.
pub(crate) struct DetachedKernel;

impl Kernel for DetachedKernel {
    fn builtin(&mut self, _name: &str, _args: &[Value], _host: &mut dyn Host) -> BuiltinResult {
        BuiltinResult::NotHandled
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
