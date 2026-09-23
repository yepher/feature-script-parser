//! Errors and non-local control flow.

use crate::value::Value;
use fs_syntax::Span;
use std::fmt;

/// Why evaluation of a statement stopped early.
#[derive(Debug)]
pub enum Flow {
    Return(Value),
    Break,
    Continue,
    Error(Error),
}

impl From<Error> for Flow {
    fn from(e: Error) -> Self {
        Flow::Error(e)
    }
}

pub type EvalResult<T> = Result<T, Error>;
pub type FlowResult<T> = Result<T, Flow>;

#[derive(Debug, Clone)]
pub struct Error {
    pub kind: ErrorKind,
    /// Where it happened; filled in by the innermost expression that knows.
    pub span: Option<Span>,
    pub module: Option<String>,
    /// Call stack, innermost first, as `function@module`.
    pub trace: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ErrorKind {
    /// A FeatureScript `throw`.
    Thrown(Value),
    /// Precondition or predicate check failed.
    Precondition(String),
    Type(String),
    Name(String),
    /// A `@builtin` neither the interpreter nor the kernel implements.
    Unimplemented(String),
    /// Module could not be loaded (I/O or parse).
    Load(String),
    Runtime(String),
}

impl Error {
    /// Whether FeatureScript `try` / `catch` may swallow this error. Thrown
    /// values and ordinary runtime faults are catchable, as in Onshape; a
    /// missing builtin or a module that failed to load is an interpreter /
    /// kernel gap, and hiding it would turn "not implemented" into silent
    /// empty results, so those always propagate.
    pub fn is_catchable(&self) -> bool {
        !matches!(self.kind, ErrorKind::Unimplemented(_) | ErrorKind::Load(_))
    }

    pub fn new(kind: ErrorKind) -> Error {
        Error {
            kind,
            span: None,
            module: None,
            trace: Vec::new(),
        }
    }
    pub fn runtime(msg: impl Into<String>) -> Error {
        Error::new(ErrorKind::Runtime(msg.into()))
    }
    pub fn type_error(msg: impl Into<String>) -> Error {
        Error::new(ErrorKind::Type(msg.into()))
    }
    pub fn name(msg: impl Into<String>) -> Error {
        Error::new(ErrorKind::Name(msg.into()))
    }
    pub fn precondition(msg: impl Into<String>) -> Error {
        Error::new(ErrorKind::Precondition(msg.into()))
    }
    pub fn unimplemented(name: impl Into<String>) -> Error {
        Error::new(ErrorKind::Unimplemented(name.into()))
    }
    pub fn thrown(v: Value) -> Error {
        Error::new(ErrorKind::Thrown(v))
    }

    /// Prefix the message with the builtin that raised it (`@evArea: ...`).
    pub fn prefixed(mut self, name: &str) -> Error {
        use ErrorKind::*;
        self.kind = match self.kind {
            Precondition(m) => Precondition(format!("{name}: {m}")),
            Type(m) => Type(format!("{name}: {m}")),
            Name(m) => Name(format!("{name}: {m}")),
            Unimplemented(m) => Unimplemented(format!("{name}: {m}")),
            Load(m) => Load(format!("{name}: {m}")),
            Runtime(m) => Runtime(format!("{name}: {m}")),
            other => other,
        };
        self
    }

    pub fn at(mut self, span: Span) -> Error {
        if self.span.is_none() {
            self.span = Some(span);
        }
        self
    }

    /// The value a `catch (e)` clause binds.
    pub fn catch_value(&self) -> Value {
        match &self.kind {
            ErrorKind::Thrown(v) => v.clone(),
            other => Value::from(other.to_string()),
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::Thrown(v) => write!(f, "uncaught throw: {v}"),
            ErrorKind::Precondition(m) => write!(f, "precondition failed: {m}"),
            ErrorKind::Type(m) => write!(f, "type error: {m}"),
            ErrorKind::Name(m) => write!(f, "name error: {m}"),
            ErrorKind::Unimplemented(m) => write!(f, "builtin not implemented: {m}"),
            ErrorKind::Load(m) => write!(f, "load error: {m}"),
            ErrorKind::Runtime(m) => write!(f, "runtime error: {m}"),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)?;
        if let (Some(m), Some(s)) = (&self.module, &self.span) {
            write!(f, " ({m} @ {s})")?;
        }
        for frame in &self.trace {
            write!(f, "\n    in {frame}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}
