//! `fs-syntax` — a lexer, parser and AST for Onshape FeatureScript.
//!
//! ```
//! let module = fs_syntax::parse_module("FeatureScript 2960; export const X = 1 * meter;").unwrap();
//! assert_eq!(module.items.len(), 1);
//! ```
//!
//! The crate has no opinion about *evaluating* FeatureScript; it produces a
//! plain [`ast::Module`] that an interpreter (or a transpiler targeting your
//! own geometry kernel) can walk. See the `visit` module for a generic
//! walker.

pub mod ast;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod print;
pub mod span;
pub mod token;
pub mod visit;

pub use ast::Module;
pub use error::{ParseError, ParseResult};
pub use parser::{parse_expr, parse_module};
pub use print::{print_expr, print_module};
pub use span::{LineCol, LineIndex, Span};
