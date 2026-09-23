//! `fs-interp` — a tree-walking interpreter for Onshape FeatureScript.
//!
//! The language (values, maps/arrays with value semantics, boxes, custom
//! type tags, enums, overloaded functions and operators, preconditions,
//! `try`/`throw`, module imports) is implemented here once. Everything that
//! touches geometry reaches the outside world through `@builtin` calls, which
//! are routed to a [`Kernel`]; see `kernel.rs`.
//!
//! ```no_run
//! use fs_interp::Interp;
//!
//! let mut interp = Interp::with_std_dir("../feature_script_std");
//! let math = interp.load_module("onshape/std/math.fs").unwrap();
//! let v = interp.eval_in(&math, "clamp(15, 0, 10)").unwrap();
//! assert_eq!(v.as_number(), Some(10.0));
//! ```

pub mod env;
pub mod error;
pub mod interp;
pub mod intrinsics;
pub mod kernel;
pub mod matrix;
pub mod value;

pub use error::{Error, ErrorKind, EvalResult};
pub use interp::{DirLoader, Interp, MemoryLoader, ModuleLoader};
pub use kernel::{BuiltinResult, Host, Kernel, StubKernel};
pub use value::{NativeValue, Value};
