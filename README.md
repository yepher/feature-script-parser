# feature-script-parser

A Rust front end and interpreter for [Onshape FeatureScript](https://cad.onshape.com/FsDoc/):
lexer, parser, typed AST, pretty-printer, a tree-walking interpreter with a pluggable geometry
kernel, and a small CLI. The goal is to take off-the-shelf FeatureScript (the standard library,
custom features) and run it against a non-Onshape geometry kernel; this repository is the
language side of that, with a clean seam where a kernel plugs in.

Status: the parser handles all 265 files of the Onshape standard library (v2960) and round-trips
them (parse → print → parse gives an identical tree; ~6.7 MB in ~150 ms). The interpreter loads
the whole standard library (`geometry.fs` and its 251 transitive imports) in ~230 ms and evaluates
857 of its 860 top-level constants with a stub kernel; units, vectors, matrices, transforms,
coordinate systems, bounds specs, containers, strings and error handling all run as the unmodified
std-library code. The three constants that fail are documented below.

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
  fs-interp/    library: tree-walking interpreter + Kernel trait  (depends only on fs-syntax)
    src/value.rs    Value model: numbers/strings/arrays/maps (value semantics), box, functions,
                    enums, custom type tags, kernel-owned Native handles
    src/env.rs      block scopes and module namespaces; imports resolved by reference
    src/interp/     evaluator: module loading, overload resolution, is/as, expressions, statements
    src/intrinsics.rs  pure @builtins (arrays, maps, strings, regex, JSON, math)
    src/matrix.rs   @matrix* builtins
    src/kernel.rs   the Kernel trait, StubKernel
    tests/language.rs      language-core tests on in-memory modules
    tests/std_library.rs   runs real std-library code
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

./target/release/fscript eval --std ../onshape-std-library-mirror '(2 * meter + 3 * inch).value'
./target/release/fscript eval --std ../onshape-std-library-mirror -m vector.fs 'norm(vector(3, 4))'
./target/release/fscript std-check --std ../onshape-std-library-mirror     # evaluate every std constant
```

Parse errors print `file:line:col`, the offending line and a caret. Runtime errors carry the
module, span and a call trace.

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

## Interpreter

```rust
use fs_interp::Interp;

let mut interp = Interp::with_std_dir("../onshape-std-library-mirror");   // StubKernel
let geometry = interp.load_module("onshape/std/geometry.fs")?;
let v = interp.eval_in(&geometry, "norm(vector(3, 4) * meter)")?;
println!("{v}");   // ValueWithUnits { "value" : 5 , "unit" : UnitSpec { "meter" : 1 } }
```

What is implemented, all learned from what the standard library actually does:

* **Values.** Arrays and maps have value semantics (assignment and parameter passing copy);
  `box` is the one reference type. They are `Rc`-backed and copy-on-write, so copies are cheap.
  Custom types (`export type Foo typecheck canBeFoo;`) are tags: `x as Foo` runs the typecheck
  predicate and tags, `x is Foo` checks the tag, `x as map` strips it, and equality ignores tags.
  Enum members are values of their enum type. Kernel-owned things are `Value::Native` handles.
* **Functions.** Named functions, predicates and operators are overload sets, merged across
  every module visible from the call site (`operator*` is defined in a dozen std modules).
  Resolution: arity must match, every `is Type` parameter must hold, the candidate satisfying
  the most typed parameters wins, ties go to the earliest declaration. Preconditions and
  predicate bodies run in "checking mode" where each expression statement must be `true`.
  Lambdas and `function(...)` expressions are closures.
* **Modules.** `import` binds by reference to the imported module's exports (following
  `export import` chains), which is what makes the std library's circular imports work.
  Top-level constants are lazy: the std library forward-references constants declared later in
  the same file, and has constants whose initialisers would throw if ever evaluated.
* **Map literals** accept bare identifier keys: `{ endBound : BoundingType.BLIND }` means
  `{ "endBound" : ... }`. The std library relies on this everywhere.
* **Builtins.** `@size`, `@resize`, `@keys`, `@match`, `@sqrt`, `@matrixMultiply`, … are
  implemented in Rust (`intrinsics.rs`, `matrix.rs`). Everything else is handed to the kernel.

### Hooking up a kernel

Every geometry operation in the standard library bottoms out in an `@builtin`: `opExtrude` is
FeatureScript that validates its `definition` map and then calls
`@opExtrude(context, id, definition)`; `evPlane` calls `@evPlane`; a `Context` is whatever
`@newContext` returns. So a kernel is one trait:

```rust
pub trait Kernel {
    fn builtin(&mut self, name: &str, args: &[Value], host: &mut dyn Host) -> BuiltinResult;
    fn print(&mut self, text: &str) { .. }
    fn as_any(&self) -> &dyn std::any::Any;
}

pub trait Host {                      // the interpreter, callable from inside a builtin
    fn call(&mut self, name: &str, args: Vec<Value>) -> EvalResult<Value>;   // any visible std function
    fn global(&mut self, name: &str) -> EvalResult<Value>;                  // e.g. `meter`
    fn cast(&mut self, value: Value, type_name: &str) -> EvalResult<Value>;  // `value as Plane`
    fn is_type(&mut self, value: &Value, type_name: &str) -> EvalResult<bool>;
}
```

The kernel is detached from the interpreter while a builtin runs, and gets a `Host` instead so
it can build std-typed results the canonical way — `plane(origin, normal, x)`,
`vector(x, y, z) * meter`, `qTransient(id)`, `{..} as ValueWithUnits` — rather than
hand-assembling maps.

`StubKernel` implements `@newContext` / `@isContext` / version queries and records every other
builtin it is asked for; `fscript std-check` and `examples/kernel_calls.rs` tell you which
builtins a given feature needs. The adapter for a real kernel belongs in its own crate depending on `fs-interp`, so this
crate stays free of geometry dependencies; `fs_script_runner` (sibling checkout) is the
gkernel adapter and runs the std `extrude` feature end to end. Arguments arrive as plain `Value`s: `definition` is a map, queries are
the maps `qUnion`/`qCreatedBy` build, lengths are `ValueWithUnits` maps with a `value` in metres.

### Known gaps

* `tolerance.fs` defines three constants by calling a one-parameter function with no arguments
  (Onshape only inspects those functions' preconditions for the UI and never evaluates them);
  they raise a type error if forced. `fscript std-check` reports exactly these three.
* Overload resolution is a best guess: Onshape does not document its rule. If a std function
  ever picks the wrong overload, `select_overload` in `interp/mod.rs` is the place to look.
* `@matrixSvd`, `@approximateSpline`, `@clusterPoints` and the sketch (`@sk*`) builtins are not
  implemented; they go to the kernel.
* Annotations are parsed but not evaluated (Onshape only uses them for the feature UI).
* No `returns` type checking, and `@getLanguageVersion` reports the module's header version.

## Testing

```
cargo test                       # parser unit tests, corpus round-trip, interpreter language + std tests
FS_STD_CORPUS=/path/to/std cargo test
```

The corpus test looks for `../onshape-std-library-mirror` next to this repo
([javawizard/onshape-std-library-mirror](https://github.com/javawizard/onshape-std-library-mirror),
`without-versions` branch) and is skipped with a note if it isn't there.

## License

MIT. The Onshape standard library used for testing is MIT-licensed by PTC Inc. and is not
included in this repository.
