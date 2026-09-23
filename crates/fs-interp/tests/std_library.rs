//! Runs real standard-library code through the interpreter. Needs the std
//! corpus (see fs-syntax/tests/corpus.rs for how it is located).

use fs_interp::{ErrorKind, Interp, StubKernel};
use std::path::PathBuf;

fn corpus_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FS_STD_CORPUS") {
        return Some(PathBuf::from(p));
    }
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    [
        "../../../feature_script_std",
        "../../../onshape-std-library-mirror",
    ]
    .iter()
    .map(|c| here.join(c))
    .find(|c| c.is_dir())
}

fn std_interp() -> Option<Interp> {
    let dir = corpus_dir()?;
    Some(Interp::with_std_dir(dir))
}

#[test]
fn whole_library_loads_and_constants_evaluate() {
    let Some(mut interp) = std_interp() else {
        eprintln!("corpus not found; set FS_STD_CORPUS");
        return;
    };
    interp
        .load_module("onshape/std/geometry.fs")
        .unwrap_or_else(|e| panic!("{e}"));
    let modules = interp.modules();
    assert!(modules.len() > 200, "only {} modules loaded", modules.len());
    let mut failures = Vec::new();
    for m in &modules {
        for (name, err) in interp.force_all_globals(m) {
            failures.push((m.path.clone(), name, err));
        }
    }
    // Known: tolerance.fs "packages" preconditions by calling a one-parameter
    // function with no arguments; Onshape never evaluates those constants.
    let unexpected: Vec<String> = failures
        .iter()
        .filter(|(path, _, err)| {
            !(path.ends_with("tolerance.fs") && matches!(err.kind, ErrorKind::Type(_)))
        })
        .map(|(p, n, e)| format!("{p}:{n}: {e}"))
        .collect();
    assert!(
        unexpected.is_empty(),
        "unexpected failures:\n{}",
        unexpected.join("\n")
    );
    assert_eq!(failures.len(), 3);
}

#[test]
fn std_library_expressions() {
    let Some(mut interp) = std_interp() else {
        return;
    };
    let geometry = interp.load_module("onshape/std/geometry.fs").unwrap();
    let cases: &[(&str, &str)] = &[
        ("clamp(15, 0, 10)", "10"),
        ("(2 * meter + 3 * inch).value", "2.0762"),
        ("(90 * degree) < (2 * radian)", "true"),
        ("tolerantEquals(1 * inch, 25.4 * millimeter)", "true"),
        ("toString(vector(1, 2, 3) + vector(1, 1, 1))", "[ 2 , 3 , 4 ]"),
        ("norm(vector(3, 4))", "5"),
        ("dot(vector(1, 2), vector(3, 4))", "11"),
        ("toString(cross(vector(1, 0, 0), vector(0, 1, 0)))", "[ 0 , 0 , 1 ]"),
        ("toString(mapArray([1, 2, 3], x => x * 10))", "[ 10 , 20 , 30 ]"),
        ("toString(filter([1, 2, 3, 4], x => x % 2 == 0))", "[ 2 , 4 ]"),
        ("toString(reverse([1, 2, 3]))", "[ 3 , 2 , 1 ]"),
        ("toString(sort([3, 1, 2], (a, b) => a - b))", "[ 1 , 2 , 3 ]"),
        ("match(\"abc123\", \"([a-z]+)([0-9]+)\").captures[1]", "abc"),
        ("roundToPrecision(3.14159, 2)", "3.14"),
        ("determinant(matrix([[1, 2], [3, 4]]))", "-2"),
        ("toString(inverse(matrix([[2, 0], [0, 4]])))", "[ [ 0.5 , 0 ] , [ 0 , 0.25 ] ]"),
        ("isLength(3 * meter, LENGTH_BOUNDS)", "true"),
        ("BooleanOperationType.UNION == BooleanOperationType.UNION", "true"),
        ("toString(coordSystem(WORLD_ORIGIN, X_DIRECTION, Z_DIRECTION).xAxis)", "[ 1 , 0 , 0 ]"),
        ("toString(range(0, 1, 5))", "[ 0 , 0.25 , 0.5 , 0.75 , 1 ]"),
        ("size(evaluateSpline)", "<error>"), // functions have no size: a type error, not a crash
        ("regenError(ErrorStringEnum.BOOLEAN_NEED_ONE_SOLID).message == ErrorStringEnum.BOOLEAN_NEED_ONE_SOLID", "true"),
        ("newContext() is Context", "true"),
    ];
    let mut failures = Vec::new();
    for (expr, expected) in cases {
        let got = match interp.eval_in(&geometry, expr) {
            Ok(v) => v.to_string(),
            Err(_) if *expected == "<error>" => continue,
            Err(e) => format!("<error> {e}"),
        };
        if got != *expected {
            failures.push(format!(
                "{expr}\n    expected: {expected}\n    got:      {got}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn rotation_through_transform_module() {
    let Some(mut interp) = std_interp() else {
        return;
    };
    let g = interp.load_module("onshape/std/geometry.fs").unwrap();
    let v = interp
        .eval_in(&g, "rotationAround(line(vector(0, 0, 0) * meter, vector(0, 0, 1)), 90 * degree) * (vector(1, 0, 0) * meter)")
        .unwrap();
    let comps: Vec<f64> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.get(&"value".into()).unwrap().as_number().unwrap())
        .collect();
    assert!(
        comps[0].abs() < 1e-12 && (comps[1] - 1.0).abs() < 1e-12 && comps[2].abs() < 1e-12,
        "{comps:?}"
    );
}

#[test]
fn stub_kernel_records_geometry_builtins() {
    let Some(mut interp) = std_interp() else {
        return;
    };
    let g = interp.load_module("onshape/std/geometry.fs").unwrap();
    // opPoint goes straight to @opPoint, which the stub does not implement.
    let err = interp
        .eval_in(
            &g,
            "opPoint(newContext(), makeId(\"p\"), { \"point\" : vector(0, 0, 0) * meter })",
        )
        .unwrap_err();
    assert!(
        matches!(err.kind, ErrorKind::Unimplemented(ref b) if b == "@opPoint"),
        "{err}"
    );
    let stub = interp.kernel.as_any().downcast_ref::<StubKernel>().unwrap();
    assert!(stub.calls.borrow().iter().any(|c| c.name == "@opPoint"));
}
