# feature-script-parser

[See Manual](https://deepwiki.com/yepher/feature-script-parser)


A Rust front end for [Onshape FeatureScript](https://cad.onshape.com/FsDoc/): lexer, parser,
typed AST, pretty-printer and a small CLI. The goal is to take off-the-shelf FeatureScript
(the standard library, custom features) and run it against a non-Onshape geometry kernel;
this repository is the language side of that, with a clean seam where a kernel plugs in.

It parses all 265 files of the Onshape standard library (v2960) and round-trips them
(parse → print → parse gives an identical tree). Release build: ~6.7 MB of source in ~150 ms.

## Layout

```
crates/
  fs-syntax/    library: lexer, parser, AST, visitor, printer  (no I/O, optional serde)
    src/lexer.rs    hand-written tokenizer (spans, doc comments, ✨ version placeholder)
    src/parser.rs   recursive descent + precedence climbing; precedence table in the module docs
    src/ast.rs      the tree — plain data, every node has a Span, derives Serialize/Deserialize
    src/visit.rs    Visitor trait with default walk_* functions
    src/print.rs    canonical pretty-printer (used for round-trip testing)
    examples/kernel_calls.rs   lists opXxx / evXxx / @builtin calls in a file
    tests/corpus.rs            parses + round-trips the whole std library
  fscript/      CLI binary
```

## CLI

```
cargo build --release
./target/release/fscript check ../onshape-std-library-mirror     # parse everything, report failures
./target/release/fscript dump extrude.fs --json                  # AST as JSON (Rust Debug without --json)
./target/release/fscript tokens extrude.fs                       # token stream with line:col
./target/release/fscript stats ../onshape-std-library-mirror     # declaration counts, most-imported modules
cargo run --example kernel_calls -- ../onshape-std-library-mirror/extrude.fs
```

Parse errors print `file:line:col`, the offending line and a caret.

## Library

```rust
use fs_syntax::{parse_module, ast::*};

let module = parse_module(&src)?;
for item in &module.items {
    if let ItemKind::Function(f) = &item.kind {
        println!("{} ({} params)", f.name.name, f.sig.params.len());
    }
}
```

`fs_syntax::visit::Visitor` walks the tree; override only the hooks you need and call the
matching `walk_*` to recurse. `Expr::desugar_pipe` turns `a -> f(b)` into `f(a, b)`.

Feature-gate: `serde` (on by default) derives `Serialize`/`Deserialize` on every AST node.

## Language coverage

Everything the standard library uses:

* `FeatureScript <n>;` header (also the mirror's `✨` placeholder), `import` / `export import`
* `function`, `predicate`, `operator+`…, `type … typecheck …`, `enum`, `const`/`var`, all with
  optional `export`, `annotation { … }` and leading `/** doc */` (kept on items and enum members)
* `precondition { … }` and the brace-less `precondition expr;` form, `returns T`
* statements: `var`/`const` with `is T`, `if`/`else`, C-style `for`, `for (var k, v in …)`,
  `while`, `break`, `continue`, `return`, `throw`, `try [silent] { } catch [(e)] { }`, blocks,
  annotated statements inside preconditions
* expressions: full operator set including `~`, `^`, `??`, `?.`, `->` pipe calls, `is`/`as`,
  ternary, all compound assignments (`+=` … `~=` `||=` `&&=` `??=`), `box[]` dereference,
  `new box(x)`, `try(e)` / `try silent(e)`, `switch (e) { k : v }`, map/array literals,
  anonymous `function(...)`, lambdas `x => e` / `(a is T, b) returns R => { … }`, `@builtin` names

### Operator precedence

Onshape does not publish a precedence table. The parser's table (in `parser.rs`) follows the
usual C-family ordering, with two choices inferred from code the standard library compiles:

* `is` / `as` bind tighter than arithmetic — `parameter as Vector * meter` in `splineUtils.fs`
  only makes sense as `(parameter as Vector) * meter`.
* `~` (concatenation) sits just below `+`/`-`.

Because std-library code parenthesizes almost everything ambiguous, the exact placement rarely
matters for parsing; it will matter for evaluation, so both are called out in the docs and tests.

## Hooking up a kernel

The parser is deliberately kernel-agnostic. The intended next layer is an interpreter over
`ast::Module` where:

* language semantics (maps, arrays, boxes, units as `ValueWithUnits`, `is`/`as` type tags,
  preconditions, `try`/`throw`) are implemented once, in Rust;
* every `opXxx(context, id, definition)` and `evXxx(context, …)` call, and every `@builtin`,
  dispatches through a `Kernel` trait — `examples/kernel_calls.rs` shows how to find them.

`opExtrude`, `opBoolean`, `opFillet` … are the ~60 primitives the standard library is written
against; implementing those on stepkernel / your Parasolid bridge is what lets the rest of the
library (which is pure FeatureScript) run unmodified.

## Testing

```
cargo test                       # unit tests + corpus round-trip
FS_STD_CORPUS=/path/to/std cargo test --test corpus
```

The corpus test looks for `../onshape-std-library-mirror` next to this repo
([javawizard/onshape-std-library-mirror](https://github.com/javawizard/onshape-std-library-mirror),
`without-versions` branch) and is skipped with a note if it isn't there.

## License

MIT. The Onshape standard library used for testing is MIT-licensed by PTC Inc. and is not
included in this repository.
