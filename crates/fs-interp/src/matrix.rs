//! `@matrix*` intrinsics. A FeatureScript matrix is an array of equal-length
//! rows of numbers; a vector is a flat array of numbers.

#![allow(clippy::needless_range_loop)]

use crate::error::{Error, EvalResult};
use crate::value::Value;

type M = Vec<Vec<f64>>;

pub fn call(name: &str, args: &[Value]) -> Option<EvalResult<Value>> {
    let r = match name {
        "@isMatrix" => Ok(Value::Bool(
            args.first().is_some_and(|v| to_matrix(v).is_ok()),
        )),
        "@matrixIdentity" => (|| {
            let n = arg_num(args, 0)? as usize;
            Ok(from_matrix(
                (0..n)
                    .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
                    .collect(),
            ))
        })(),
        "@matrixSum" => elementwise(args, |a, b| a + b),
        "@matrixDifference" => elementwise(args, |a, b| a - b),
        "@matrixCwiseProduct" => elementwise(args, |a, b| a * b),
        "@matrixNegate" => arg_matrix(args, 0)
            .map(|m| from_matrix(m.iter().map(|r| r.iter().map(|x| -x).collect()).collect())),
        "@matrixTranspose" => arg_matrix(args, 0).map(|m| from_matrix(transpose(&m))),
        "@matrixMultiply" => multiply(args),
        "@matrixSquaredNorm" => {
            arg_matrix(args, 0).map(|m| Value::Number(m.iter().flatten().map(|x| x * x).sum()))
        }
        "@matrixDeterminant" => {
            arg_matrix(args, 0).and_then(|m| determinant(&m).map(Value::Number))
        }
        "@matrixInverse" => arg_matrix(args, 0).and_then(|m| inverse(&m).map(from_matrix)),
        "@matrixRotation3d" => rotation3d(args),
        _ => return None,
    };
    Some(r)
}

fn arg_num(args: &[Value], i: usize) -> EvalResult<f64> {
    args.get(i)
        .and_then(|v| v.as_number())
        .ok_or_else(|| Error::type_error(format!("argument {} must be a number", i + 1)))
}

fn to_vector(v: &Value) -> EvalResult<Vec<f64>> {
    let a = v
        .as_array()
        .ok_or_else(|| Error::type_error("expected a numeric array"))?;
    a.iter()
        .map(|x| {
            x.as_number()
                .ok_or_else(|| Error::type_error("expected a numeric array"))
        })
        .collect()
}

fn to_matrix(v: &Value) -> EvalResult<M> {
    let rows = v
        .as_array()
        .ok_or_else(|| Error::type_error("expected a matrix (array of rows)"))?;
    let m: M = rows.iter().map(to_vector).collect::<EvalResult<_>>()?;
    if let Some(first) = m.first() {
        if m.iter().any(|r| r.len() != first.len()) {
            return Err(Error::type_error(
                "matrix rows must all have the same length",
            ));
        }
    }
    Ok(m)
}

fn arg_matrix(args: &[Value], i: usize) -> EvalResult<M> {
    to_matrix(
        args.get(i)
            .ok_or_else(|| Error::runtime("missing matrix argument"))?,
    )
}

fn from_vector(v: Vec<f64>) -> Value {
    Value::array(v.into_iter().map(Value::Number).collect())
}

fn from_matrix(m: M) -> Value {
    Value::array(m.into_iter().map(from_vector).collect())
}

fn transpose(m: &M) -> M {
    if m.is_empty() {
        return Vec::new();
    }
    (0..m[0].len())
        .map(|j| m.iter().map(|r| r[j]).collect())
        .collect()
}

fn elementwise(args: &[Value], f: fn(f64, f64) -> f64) -> EvalResult<Value> {
    let a = arg_matrix(args, 0)?;
    let b = arg_matrix(args, 1)?;
    if a.len() != b.len() || a.first().map(|r| r.len()) != b.first().map(|r| r.len()) {
        return Err(Error::runtime("matrix sizes do not match"));
    }
    Ok(from_matrix(
        a.iter()
            .zip(&b)
            .map(|(ra, rb)| ra.iter().zip(rb).map(|(x, y)| f(*x, *y)).collect())
            .collect(),
    ))
}

/// `@matrixMultiply` accepts matrix×matrix, matrix×vector, matrix×scalar and
/// scalar×matrix.
fn multiply(args: &[Value]) -> EvalResult<Value> {
    let a = args
        .first()
        .ok_or_else(|| Error::runtime("missing argument"))?;
    let b = args
        .get(1)
        .ok_or_else(|| Error::runtime("missing argument"))?;
    if let Some(s) = a.as_number() {
        let m = to_matrix(b)?;
        return Ok(from_matrix(
            m.iter()
                .map(|r| r.iter().map(|x| x * s).collect())
                .collect(),
        ));
    }
    let m = to_matrix(a)?;
    if let Some(s) = b.as_number() {
        return Ok(from_matrix(
            m.iter()
                .map(|r| r.iter().map(|x| x * s).collect())
                .collect(),
        ));
    }
    // Vector (flat) or matrix (nested)?
    let is_vector = b
        .as_array()
        .is_some_and(|arr| arr.first().is_some_and(|x| x.as_number().is_some()));
    if is_vector {
        let v = to_vector(b)?;
        if m.first().is_some_and(|r| r.len() != v.len()) {
            return Err(Error::runtime("matrix/vector size mismatch"));
        }
        return Ok(from_vector(
            m.iter()
                .map(|r| r.iter().zip(&v).map(|(x, y)| x * y).sum())
                .collect(),
        ));
    }
    let n = to_matrix(b)?;
    let inner = m.first().map(|r| r.len()).unwrap_or(0);
    if inner != n.len() {
        return Err(Error::runtime("matrix size mismatch"));
    }
    let cols = n.first().map(|r| r.len()).unwrap_or(0);
    Ok(from_matrix(
        m.iter()
            .map(|r| {
                (0..cols)
                    .map(|j| (0..inner).map(|k| r[k] * n[k][j]).sum())
                    .collect()
            })
            .collect(),
    ))
}

fn determinant(m: &M) -> EvalResult<f64> {
    let n = m.len();
    if m.iter().any(|r| r.len() != n) {
        return Err(Error::runtime("determinant requires a square matrix"));
    }
    // LU with partial pivoting.
    let mut a = m.clone();
    let mut det = 1.0;
    for i in 0..n {
        let p = (i..n)
            .max_by(|&x, &y| a[x][i].abs().partial_cmp(&a[y][i].abs()).unwrap())
            .unwrap();
        if a[p][i] == 0.0 {
            return Ok(0.0);
        }
        if p != i {
            a.swap(p, i);
            det = -det;
        }
        det *= a[i][i];
        for r in i + 1..n {
            let f = a[r][i] / a[i][i];
            for c in i..n {
                a[r][c] -= f * a[i][c];
            }
        }
    }
    Ok(det)
}

/// Gauss-Jordan inverse; singular matrices yield infinities/NaNs, matching
/// the documented FeatureScript behaviour.
fn inverse(m: &M) -> EvalResult<M> {
    let n = m.len();
    if m.iter().any(|r| r.len() != n) {
        return Err(Error::runtime("inverse requires a square matrix"));
    }
    let mut a: M = m
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut row = r.clone();
            row.extend((0..n).map(|j| if i == j { 1.0 } else { 0.0 }));
            row
        })
        .collect();
    for i in 0..n {
        let p = (i..n)
            .max_by(|&x, &y| a[x][i].abs().partial_cmp(&a[y][i].abs()).unwrap())
            .unwrap();
        a.swap(p, i);
        let pivot = a[i][i];
        for c in 0..2 * n {
            a[i][c] /= pivot;
        }
        for r in 0..n {
            if r != i {
                let f = a[r][i];
                for c in 0..2 * n {
                    a[r][c] -= f * a[i][c];
                }
            }
        }
    }
    Ok(a.into_iter().map(|r| r[n..].to_vec()).collect())
}

/// `@matrixRotation3d(axis, angleRadians)` — Rodrigues' rotation matrix.
fn rotation3d(args: &[Value]) -> EvalResult<Value> {
    let axis = to_vector(args.first().ok_or_else(|| Error::runtime("missing axis"))?)?;
    let angle = arg_num(args, 1)?;
    if axis.len() != 3 {
        return Err(Error::runtime("rotation axis must have 3 components"));
    }
    let len = axis.iter().map(|x| x * x).sum::<f64>().sqrt();
    if len == 0.0 {
        return Err(Error::runtime("rotation axis must be non-zero"));
    }
    let (x, y, z) = (axis[0] / len, axis[1] / len, axis[2] / len);
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    Ok(from_matrix(vec![
        vec![t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        vec![t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        vec![t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]))
}
