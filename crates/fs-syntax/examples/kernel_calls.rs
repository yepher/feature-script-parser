//! Lists the kernel-facing calls a FeatureScript file makes: `opXxx(...)`
//! (modeling operations), `evXxx(...)` (evaluations) and `@builtin(...)`.
//!
//! This is the shape of the seam between the parser and a geometry kernel:
//! walk the AST, find the calls, and dispatch them to your own implementation.
//!
//! ```text
//! cargo run --example kernel_calls -- ../onshape-std-library-mirror/extrude.fs
//! ```

use fs_syntax::ast::{Expr, ExprKind};
use fs_syntax::visit::{walk_expr, Visitor};
use fs_syntax::{parse_module, LineIndex};
use std::collections::BTreeMap;

#[derive(Default)]
struct KernelCalls {
    counts: BTreeMap<String, usize>,
    first_seen: BTreeMap<String, u32>,
}

impl Visitor for KernelCalls {
    fn visit_expr(&mut self, e: &Expr) {
        // `a -> f(b)` is `f(a, b)`; look at the plain call form.
        let e = e.desugar_pipe();
        if let ExprKind::Call { callee, .. } = &e.kind {
            if let ExprKind::Ident(name) = &callee.kind {
                let is_kernel = name.starts_with('@')
                    || (name.starts_with("op") && name[2..].starts_with(char::is_uppercase))
                    || (name.starts_with("ev") && name[2..].starts_with(char::is_uppercase));
                if is_kernel {
                    *self.counts.entry(name.clone()).or_default() += 1;
                    self.first_seen.entry(name.clone()).or_insert(e.span.start);
                }
            }
        }
        walk_expr(self, &e);
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: kernel_calls <file.fs>");
    let src = std::fs::read_to_string(&path).expect("read file");
    let module = match parse_module(&src) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{}", e.render(&path, &src));
            std::process::exit(1);
        }
    };
    let index = LineIndex::new(&src);
    let mut v = KernelCalls::default();
    v.visit_module(&module);
    println!("{:<40} {:>5}  first use", "call", "count");
    for (name, n) in &v.counts {
        println!(
            "{name:<40} {n:>5}  line {}",
            index.line_col(v.first_seen[name]).line
        );
    }
}
