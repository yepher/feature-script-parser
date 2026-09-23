//! Language-core tests using in-memory modules (no standard library needed).

use fs_interp::{Interp, MemoryLoader, StubKernel, Value};

/// Build an interpreter with `main` as `test/main.fs` plus extra modules.
fn interp(modules: &[(&str, &str)]) -> Interp {
    let mut loader = MemoryLoader::default();
    for (path, src) in modules {
        loader.modules.insert(path.to_string(), src.to_string());
    }
    Interp::new(Box::new(StubKernel::new()), Box::new(loader))
}

/// Load `main` and evaluate `expr` in it.
fn eval(main: &str, expr: &str) -> Value {
    let mut i = interp(&[("test/main.fs", main)]);
    let m = i
        .load_module("test/main.fs")
        .unwrap_or_else(|e| panic!("load: {e}"));
    i.eval_in(&m, expr)
        .unwrap_or_else(|e| panic!("eval `{expr}`: {e}"))
}

fn eval_err(main: &str, expr: &str) -> String {
    let mut i = interp(&[("test/main.fs", main)]);
    let m = i
        .load_module("test/main.fs")
        .unwrap_or_else(|e| panic!("load: {e}"));
    match i.eval_in(&m, expr) {
        Ok(v) => panic!("expected error, got {v}"),
        Err(e) => e.to_string(),
    }
}

fn num(v: &Value) -> f64 {
    v.as_number()
        .unwrap_or_else(|| panic!("not a number: {v:?}"))
}

#[test]
fn arithmetic_and_precedence() {
    assert_eq!(num(&eval("", "1 + 2 * 3 ^ 2")), 19.0);
    assert_eq!(num(&eval("", "2 ^ 3 ^ 2")), 512.0);
    assert_eq!(num(&eval("", "7 % 3")), 1.0);
    assert_eq!(num(&eval("", "-(2 + 3)")), -5.0);
    assert_eq!(
        eval("", "\"a\" ~ 1 ~ true ~ undefined").as_str(),
        Some("a1trueundefined")
    );
    assert_eq!(
        eval("", "1 < 2 && !(3 == 4) || false").as_bool(),
        Some(true)
    );
    assert_eq!(num(&eval("", "undefined ?? 5")), 5.0);
    assert_eq!(num(&eval("", "inf > 1e308 ? 1 : 0")), 1.0);
}

#[test]
fn value_semantics_for_arrays_and_maps() {
    let src = r#"
        function f() {
            var a = [1, 2, 3];
            var b = a;
            b[0] = 99;
            var m = { "x" : { "y" : 1 } };
            var n = m;
            n.x.y = 2;
            return [a[0], b[0], m.x.y, n.x.y];
        }
        function g(arr is array) { arr[0] = -1; return arr; }
        function h() { var a = [5]; g(a); return a[0]; }
    "#;
    let v = eval(src, "f()");
    let items: Vec<f64> = v.as_array().unwrap().iter().map(num).collect();
    assert_eq!(items, [1.0, 99.0, 1.0, 2.0]);
    assert_eq!(num(&eval(src, "h()")), 5.0);
}

#[test]
fn boxes_are_references() {
    let src = r#"
        function bump(b is box) { b[] = b[] + 1; }
        function f() {
            var b = new box(1);
            var alias = b;
            bump(b);
            bump(alias);
            return b[];
        }
    "#;
    assert_eq!(num(&eval(src, "f()")), 3.0);
}

#[test]
fn overloads_by_parameter_type_and_specificity() {
    let src = r#"
        function describe(x is number) { return "number"; }
        function describe(x is string) { return "string"; }
        function describe(x) { return "other"; }
        function pair(a is array, b) { return "array-any"; }
        function pair(a is array, b is array) { return "array-array"; }
        function pair(a, b is array) { return "any-array"; }
    "#;
    assert_eq!(eval(src, "describe(1)").as_str(), Some("number"));
    assert_eq!(eval(src, "describe(\"s\")").as_str(), Some("string"));
    assert_eq!(eval(src, "describe([])").as_str(), Some("other"));
    assert_eq!(eval(src, "pair([], [])").as_str(), Some("array-array"));
    assert_eq!(eval(src, "pair([], 1)").as_str(), Some("array-any"));
    assert_eq!(eval(src, "pair(1, [])").as_str(), Some("any-array"));
    assert!(eval_err(src, "pair(1, 2)").contains("no overload of `pair`"));
}

#[test]
fn custom_types_tags_and_operators() {
    let src = r#"
        export type Money typecheck canBeMoney;
        export predicate canBeMoney(value) { value is map; value.cents is number; }
        export function money(cents is number) returns Money { return { "cents" : cents } as Money; }
        export operator+(a is Money, b is Money) returns Money { return money(a.cents + b.cents); }
        export operator*(a is Money, k is number) returns Money { return money(a.cents * k); }
        export operator*(k is number, a is Money) returns Money { return a * k; }
        export operator<(a is Money, b is Money) { return a.cents < b.cents; }
        export operator-(a is Money) returns Money { return money(-a.cents); }
    "#;
    assert_eq!(num(&eval(src, "(money(5) + 2 * money(10)).cents")), 25.0);
    assert_eq!(eval(src, "money(1) is Money").as_bool(), Some(true));
    assert_eq!(eval(src, "money(1) is map").as_bool(), Some(true));
    assert_eq!(
        eval(src, "(money(1) as map) is Money").as_bool(),
        Some(false)
    );
    assert_eq!(
        eval(
            src,
            "money(1) < money(2) && money(2) >= money(2) && money(3) > money(1)"
        )
        .as_bool(),
        Some(true)
    );
    assert_eq!(num(&eval(src, "(-money(4)).cents")), -4.0);
    assert!(eval_err(src, "{ \"cents\" : \"x\" } as Money").contains("typecheck"));
    // Structural equality ignores tags.
    assert_eq!(
        eval(src, "money(1) == { \"cents\" : 1 }").as_bool(),
        Some(true)
    );
}

#[test]
fn enums() {
    let src = r#"
        export enum Color { RED, GREEN, BLUE }
        function name(c is Color) { return switch (c) { Color.RED : "r", Color.GREEN : "g", Color.BLUE : "b" }; }
    "#;
    assert_eq!(eval(src, "name(Color.GREEN)").as_str(), Some("g"));
    assert_eq!(eval(src, "Color.RED is Color").as_bool(), Some(true));
    assert_eq!(eval(src, "Color.RED == Color.BLUE").as_bool(), Some(false));
    assert_eq!(eval(src, "\"\" ~ Color.BLUE").as_str(), Some("BLUE"));
    assert_eq!(
        eval(src, "{ Color.RED : 1 }[Color.RED]").as_number(),
        Some(1.0)
    );
}

#[test]
fn preconditions_and_predicates() {
    let src = r#"
        export predicate isPositive(x) { x is number; x > 0; }
        function root(x) precondition isPositive(x); { return x ^ 0.5; }
        function ranged(x) precondition { x is number; if (x > 10) x < 20; } { return x; }
    "#;
    assert_eq!(num(&eval(src, "root(16)")), 4.0);
    assert!(eval_err(src, "root(-1)").contains("precondition of `root` failed"));
    assert_eq!(eval(src, "isPositive(\"no\")").as_bool(), Some(false));
    assert_eq!(num(&eval(src, "ranged(15)")), 15.0);
    assert!(eval_err(src, "ranged(25)").contains("precondition"));
}

#[test]
fn try_catch_throw() {
    let src = r#"
        function risky(x) { if (x < 0) throw { "message" : "negative" }; return x; }
        function guarded(x) {
            try { return risky(x); }
            catch (e) { return e.message; }
        }
        function silent(x) { try silent { risky(x); } return "ok"; }
    "#;
    assert_eq!(num(&eval(src, "guarded(1)")), 1.0);
    assert_eq!(eval(src, "guarded(-1)").as_str(), Some("negative"));
    assert_eq!(eval(src, "silent(-1)").as_str(), Some("ok"));
    assert!(eval(src, "try(risky(-1))").is_undefined());
    assert_eq!(num(&eval(src, "try silent(risky(2))")), 2.0);
    assert!(eval_err(src, "risky(-5)").contains("uncaught throw"));
}

#[test]
fn loops_and_control_flow() {
    let src = r#"
        function sumTo(n) { var s = 0; for (var i = 1; i <= n; i += 1) { if (i == 4) continue; s += i; if (i > 6) break; } return s; }
        function keys(m) { var out = []; for (var k, v in m) out = append(out, k ~ "=" ~ v); return out; }
        function entries(m) { var out = []; for (var e in m) out = append(out, e.key ~ ":" ~ e.value); return out; }
        function append(a is array, x) { return @resize(a, @size(a) + 1, x); }
        function countdown(n) { var c = 0; while (n > 0) { n -= 1; c += 1; } return c; }
    "#;
    assert_eq!(
        num(&eval(src, "sumTo(10)")),
        1.0 + 2.0 + 3.0 + 5.0 + 6.0 + 7.0
    );
    let v = eval(src, "keys({ \"a\" : 1, \"b\" : 2 })");
    let items: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(items, ["a=1", "b=2"]);
    let v = eval(src, "entries({ \"a\" : 1 })");
    assert_eq!(v.as_array().unwrap()[0].as_str(), Some("a:1"));
    assert_eq!(num(&eval(src, "countdown(7)")), 7.0);
}

#[test]
fn closures_lambdas_and_pipes() {
    let src = r#"
        function map(arr is array, f is function) { var out = []; for (var x in arr) out = @resize(out, @size(out) + 1, f(x)); return out; }
        function adder(n) { return x => x + n; }
        function twice(f is function) { return (x) => f(f(x)); }
        function blockLambda() { return (a, b) => { const s = a + b; return s * 2; }; }
        function pipeline() { return [1, 2, 3] -> map(x => x * 2) -> map(adder(1)); }
    "#;
    let v = eval(src, "pipeline()");
    let items: Vec<f64> = v.as_array().unwrap().iter().map(num).collect();
    assert_eq!(items, [3.0, 5.0, 7.0]);
    assert_eq!(num(&eval(src, "twice(adder(10))(1)")), 21.0);
    assert_eq!(num(&eval(src, "blockLambda()(1, 2)")), 6.0);
    assert_eq!(num(&eval(src, "(function(x) { return x * x; })(9)")), 81.0);
}

#[test]
fn maps_member_access_and_bare_keys() {
    let src = r#"
        function f() {
            var m = { endBound : 1, "quoted" : 2 };
            m.added = 3;
            m["idx"] = 4;
            return [m.endBound, m.quoted, m.added, m.idx, m.missing == undefined, m?.missing?.deeper == undefined];
        }
    "#;
    let v = eval(src, "f()");
    let a = v.as_array().unwrap();
    assert_eq!(num(&a[0]), 1.0);
    assert_eq!(num(&a[1]), 2.0);
    assert_eq!(num(&a[2]), 3.0);
    assert_eq!(num(&a[3]), 4.0);
    assert_eq!(a[4].as_bool(), Some(true));
    assert_eq!(a[5].as_bool(), Some(true));
    assert!(eval_err("", "undefined.x").contains("undefined"));
    // Assigning undefined removes the key.
    assert_eq!(
        num(&eval(
            "function g() { var m = { \"a\" : 1, \"b\" : 2 }; m.a = undefined; return @size(m); }",
            "g()"
        )),
        1.0
    );
}

#[test]
fn lazy_forward_referenced_constants() {
    let src = r#"
        const A = { "b" : B };
        const B = 42;
        export const BAD = @noSuchBuiltin();
        function useA() { return A.b; }
    "#;
    // Loads fine even though BAD would fail: constants evaluate on use.
    assert_eq!(num(&eval(src, "useA()")), 42.0);
    assert!(eval_err(src, "BAD").contains("@noSuchBuiltin"));
    assert!(eval_err("const X = Y; const Y = X;", "X").contains("depends on itself"));
}

#[test]
fn imports_exports_and_reexports() {
    let a = "export function fromA() { return \"a\"; } function hidden() { return 0; } export const A_CONST = 1;";
    let b = "export import(path : \"test/a.fs\", version : \"0\"); export function fromB() { return fromA() ~ \"b\"; }";
    let main = "import(path : \"test/b.fs\", version : \"0\");";
    let mut i = interp(&[("test/a.fs", a), ("test/b.fs", b), ("test/main.fs", main)]);
    let m = i.load_module("test/main.fs").unwrap();
    assert_eq!(i.eval_in(&m, "fromB()").unwrap().as_str(), Some("ab"));
    assert_eq!(i.eval_in(&m, "fromA()").unwrap().as_str(), Some("a"));
    assert_eq!(num(&i.eval_in(&m, "A_CONST").unwrap()), 1.0);
    assert!(i
        .eval_in(&m, "hidden()")
        .unwrap_err()
        .to_string()
        .contains("undefined identifier"));
}

#[test]
fn circular_imports() {
    let a = "import(path : \"test/b.fs\", version : \"0\"); export function fa(n) { return n <= 0 ? 0 : fb(n - 1) + 1; }";
    let b = "import(path : \"test/a.fs\", version : \"0\"); export function fb(n) { return n <= 0 ? 0 : fa(n - 1) + 1; }";
    let mut i = interp(&[("test/a.fs", a), ("test/b.fs", b)]);
    let m = i.load_module("test/a.fs").unwrap();
    assert_eq!(num(&i.eval_in(&m, "fa(5)").unwrap()), 5.0);
}

#[test]
fn builtins_route_to_kernel_and_report_unimplemented() {
    let src = "function f() { return @opExtrude(1, 2, 3); }";
    let err = eval_err(src, "f()");
    assert!(err.contains("builtin not implemented: @opExtrude"), "{err}");
    assert!(err.contains("in f@test/main.fs"), "{err}");
}

#[test]
fn intrinsics() {
    assert_eq!(num(&eval("", "@size([1,2,3]) + @size({\"a\":1})")), 4.0);
    assert_eq!(eval("", "@keys({\"a\":1,\"b\":2})[1]").as_str(), Some("b"));
    assert_eq!(
        eval("", "@match(\"ab12\", \"([a-z]+)([0-9]+)\").captures[2]").as_str(),
        Some("12")
    );
    assert_eq!(
        eval("", "@replace(\"a.b.c\", \"\\\\.\", \"_\")").as_str(),
        Some("a_b_c")
    );
    assert_eq!(eval("", "@substring(\"hello\", 1, 3)").as_str(), Some("el"));
    assert_eq!(num(&eval("", "@floor(2.7) + @ceil(2.1) + @sqrt(16)")), 9.0);
    assert_eq!(
        num(&eval("", "@parseJson(\"{\\\"x\\\": [1, 2]}\").x[1]")),
        2.0
    );
    assert_eq!(num(&eval("", "@matrixDeterminant([[1, 2], [3, 4]])")), -2.0);
    assert_eq!(num(&eval("", "@indexOf([5, 6, 7], 7)")), 2.0);
}

#[test]
fn print_goes_to_kernel() {
    let mut i = interp(&[(
        "test/main.fs",
        "function p() { @print(\"hi\"); @print(1 + 1); }",
    )]);
    let m = i.load_module("test/main.fs").unwrap();
    i.eval_in(&m, "p()").unwrap();
    let stub = i.kernel.as_any().downcast_ref::<StubKernel>().unwrap();
    assert_eq!(stub.output.borrow().as_str(), "hi2");
}

#[test]
fn intersect_maps_takes_an_array_of_maps() {
    assert_eq!(
        num(&eval("", "@size(@intersectMaps([{a:0, b:1}, {a:0, b:2}]))")),
        2.0
    );
    assert_eq!(
        num(&eval("", "@intersectMaps([{a:0, b:1}, {a:0, b:2}]).b")),
        2.0
    );
    assert_eq!(num(&eval("", "@size(@intersectMaps([{a:0}, {b:1}]))")), 0.0);
    assert_eq!(
        num(&eval("", "@size(@intersectMaps({a:0, c:1}, {a:5}))")),
        1.0
    );
}

#[test]
fn try_does_not_hide_unimplemented_builtins() {
    let mut i = interp(&[(
        "test/main.fs",
        "function a() { return try(@noSuchBuiltin(1)); }
         function b() { try { @noSuchBuiltin(1); } catch { return 1; } return 2; }
         function c() { return try({}.x.y); }",
    )]);
    let m = i.load_module("test/main.fs").unwrap();
    assert!(i.eval_in(&m, "a()").is_err());
    assert!(i.eval_in(&m, "b()").is_err());
    assert!(i.eval_in(&m, "c()").unwrap().is_undefined());
}
