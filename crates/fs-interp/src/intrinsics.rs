//! Pure-language `@builtins`: arrays, maps, strings, math, regex, JSON.
//!
//! These are the builtins the standard library uses that involve no geometry.
//! Anything not handled here is offered to the [`crate::kernel::Kernel`].

use crate::error::{Error, EvalResult};
use crate::value::{Map, Value};
use std::rc::Rc;

/// Returns `None` if `name` is not a pure intrinsic.
pub fn call(name: &str, args: &[Value]) -> Option<EvalResult<Value>> {
    let r = match name {
        // ---- arrays -------------------------------------------------------
        "@size" => size(args),
        "@resize" => resize(args),
        "@subArray" => sub_array(args),
        "@concatenateArrays" => concatenate_arrays(args),
        "@indexOf" => index_of(args),
        "@reverse" => reverse(args),
        "@range" => range(args),
        "@normalize" => normalize(args),
        // ---- maps ---------------------------------------------------------
        "@keys" => keys(args),
        "@values" => values(args),
        "@mergeMaps" => merge_maps(args),
        "@intersectMaps" => intersect_maps(args),
        // ---- strings ------------------------------------------------------
        "@length" => str_fn(args, |s| Ok(Value::Number(s.chars().count() as f64))),
        "@splitIntoCharacters" => str_fn(args, |s| {
            Ok(Value::array(
                s.chars().map(|c| Value::from(c.to_string())).collect(),
            ))
        }),
        "@stringToNumber" => str_fn(args, |s| {
            s.trim()
                .parse::<f64>()
                .map(Value::Number)
                .map_err(|_| Error::runtime(format!("`{s}` is not a number")))
        }),
        "@substring" => substring(args),
        "@startsWith" => str2(args, |s, p| Value::Bool(s.starts_with(p))),
        "@endsWith" => str2(args, |s, p| Value::Bool(s.ends_with(p))),
        "@indexOfString" => index_of_string(args),
        "@repeatString" => (|| {
            let s = arg_str(args, 0)?;
            let n = arg_num(args, 1)?;
            Ok(Value::from(s.repeat(n.max(0.0) as usize)))
        })(),
        "@match" => regex_match(args),
        "@replace" => regex_replace(args),
        "@indexOfRegexp" => index_of_regexp(args),
        "@splitByRegexp" => split_by_regexp(args),
        "@parseJson" => str_fn(args, |s| {
            serde_json::from_str::<serde_json::Value>(s)
                .map(json_to_value)
                .map_err(|e| Error::runtime(format!("invalid JSON: {e}")))
        }),
        // ---- math ---------------------------------------------------------
        "@sqrt" => math1(args, f64::sqrt),
        "@floor" => math1(args, f64::floor),
        "@ceil" => math1(args, f64::ceil),
        "@exp" => math1(args, f64::exp),
        "@log" => math1(args, f64::ln),
        "@sin" => math1(args, f64::sin),
        "@cos" => math1(args, f64::cos),
        "@tan" => math1(args, f64::tan),
        "@asin" => math1(args, f64::asin),
        "@acos" => math1(args, f64::acos),
        "@sinh" => math1(args, f64::sinh),
        "@cosh" => math1(args, f64::cosh),
        "@tanh" => math1(args, f64::tanh),
        "@asinh" => math1(args, f64::asinh),
        "@acosh" => math1(args, f64::acosh),
        "@atanh" => math1(args, f64::atanh),
        "@atan" => {
            if args.len() == 2 {
                math2(args, f64::atan2)
            } else {
                math1(args, f64::atan)
            }
        }
        "@hypot" => math2(args, f64::hypot),
        "@tolerantSort" => tolerant_sort(args),
        _ => return None,
    };
    Some(r)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn arg(args: &[Value], i: usize) -> EvalResult<&Value> {
    args.get(i)
        .ok_or_else(|| Error::runtime(format!("builtin expects at least {} arguments", i + 1)))
}

fn arg_num(args: &[Value], i: usize) -> EvalResult<f64> {
    arg(args, i)?
        .as_number()
        .ok_or_else(|| Error::type_error(format!("argument {} must be a number", i + 1)))
}

fn arg_str(args: &[Value], i: usize) -> EvalResult<&str> {
    arg(args, i)?
        .as_str()
        .ok_or_else(|| Error::type_error(format!("argument {} must be a string", i + 1)))
}

fn arg_array(args: &[Value], i: usize) -> EvalResult<&Rc<Vec<Value>>> {
    arg(args, i)?
        .as_array()
        .ok_or_else(|| Error::type_error(format!("argument {} must be an array", i + 1)))
}

fn arg_map(args: &[Value], i: usize) -> EvalResult<&Rc<Map>> {
    arg(args, i)?
        .as_map()
        .ok_or_else(|| Error::type_error(format!("argument {} must be a map", i + 1)))
}

fn arg_index(args: &[Value], i: usize) -> EvalResult<usize> {
    let n = arg_num(args, i)?;
    if n.fract() != 0.0 || n < 0.0 {
        return Err(Error::type_error(format!(
            "argument {} must be a non-negative integer",
            i + 1
        )));
    }
    Ok(n as usize)
}

fn math1(args: &[Value], f: fn(f64) -> f64) -> EvalResult<Value> {
    Ok(Value::Number(f(arg_num(args, 0)?)))
}

fn math2(args: &[Value], f: fn(f64, f64) -> f64) -> EvalResult<Value> {
    Ok(Value::Number(f(arg_num(args, 0)?, arg_num(args, 1)?)))
}

fn str_fn(args: &[Value], f: impl FnOnce(&str) -> EvalResult<Value>) -> EvalResult<Value> {
    f(arg_str(args, 0)?)
}

fn str2(args: &[Value], f: impl FnOnce(&str, &str) -> Value) -> EvalResult<Value> {
    Ok(f(arg_str(args, 0)?, arg_str(args, 1)?))
}

// ---------------------------------------------------------------------------
// arrays
// ---------------------------------------------------------------------------

fn size(args: &[Value]) -> EvalResult<Value> {
    let v = arg(args, 0)?;
    match v.untagged() {
        Value::Array(a) => Ok(Value::Number(a.len() as f64)),
        Value::Map(m) => Ok(Value::Number(m.len() as f64)),
        Value::Str(s) => Ok(Value::Number(s.chars().count() as f64)),
        other => Err(Error::type_error(format!(
            "@size: expected array or map, got {}",
            other.type_name()
        ))),
    }
}

/// `@resize(arr, newSize [, fill])` — keeps the array's type tag.
fn resize(args: &[Value]) -> EvalResult<Value> {
    let src = arg(args, 0)?;
    let arr = arg_array(args, 0)?;
    let n = arg_index(args, 1)?;
    let fill = args.get(2).cloned().unwrap_or(Value::Undefined);
    let mut out = (**arr).clone();
    out.resize(n, fill);
    Ok(src.retag(Value::array(out)))
}

fn sub_array(args: &[Value]) -> EvalResult<Value> {
    let src = arg(args, 0)?;
    let arr = arg_array(args, 0)?;
    let start = arg_index(args, 1)?;
    let end = if args.len() > 2 {
        arg_index(args, 2)?
    } else {
        arr.len()
    };
    if start > end || end > arr.len() {
        return Err(Error::runtime(format!(
            "@subArray: range {start}..{end} out of bounds for length {}",
            arr.len()
        )));
    }
    Ok(src.retag(Value::array(arr[start..end].to_vec())))
}

fn concatenate_arrays(args: &[Value]) -> EvalResult<Value> {
    let list = arg_array(args, 0)?;
    let mut out = Vec::new();
    for a in list.iter() {
        let inner = a
            .as_array()
            .ok_or_else(|| Error::type_error("@concatenateArrays: all elements must be arrays"))?;
        out.extend(inner.iter().cloned());
    }
    Ok(Value::array(out))
}

fn index_of(args: &[Value]) -> EvalResult<Value> {
    let arr = arg_array(args, 0)?;
    let needle = arg(args, 1)?;
    Ok(Value::Number(
        arr.iter()
            .position(|v| v == needle)
            .map(|i| i as f64)
            .unwrap_or(-1.0),
    ))
}

fn reverse(args: &[Value]) -> EvalResult<Value> {
    let src = arg(args, 0)?;
    let arr = arg_array(args, 0)?;
    Ok(src.retag(Value::array(arr.iter().rev().cloned().collect())))
}

/// `@range(start, end [, count])` — inclusive ends, `count` evenly spaced values.
fn range(args: &[Value]) -> EvalResult<Value> {
    let start = arg_num(args, 0)?;
    let end = arg_num(args, 1)?;
    let count = if args.len() > 2 {
        arg_index(args, 2)?
    } else {
        (end - start).abs() as usize + 1
    };
    let out = if count <= 1 {
        vec![Value::Number(start)]
    } else {
        (0..count)
            .map(|i| Value::Number(start + (end - start) * i as f64 / (count - 1) as f64))
            .collect()
    };
    Ok(Value::array(out))
}

fn normalize(args: &[Value]) -> EvalResult<Value> {
    let src = arg(args, 0)?;
    let arr = arg_array(args, 0)?;
    let nums: Vec<f64> = arr
        .iter()
        .map(|v| {
            v.as_number()
                .ok_or_else(|| Error::type_error("@normalize: expected numeric array"))
        })
        .collect::<EvalResult<_>>()?;
    let len = nums.iter().map(|x| x * x).sum::<f64>().sqrt();
    if len == 0.0 {
        return Err(Error::runtime("@normalize: zero-length vector"));
    }
    Ok(src.retag(Value::array(
        nums.iter().map(|x| Value::Number(x / len)).collect(),
    )))
}

/// `@tolerantSort(values, tolerance)` — indices that sort `values`, treating
/// numbers within `tolerance` as equal (stable).
fn tolerant_sort(args: &[Value]) -> EvalResult<Value> {
    let arr = arg_array(args, 0)?;
    let tol = arg_num(args, 1)?;
    let nums: Vec<f64> = arr
        .iter()
        .map(|v| {
            v.as_number()
                .ok_or_else(|| Error::type_error("@tolerantSort: expected numeric array"))
        })
        .collect::<EvalResult<_>>()?;
    let mut idx: Vec<usize> = (0..nums.len()).collect();
    idx.sort_by(|&a, &b| {
        if (nums[a] - nums[b]).abs() <= tol {
            std::cmp::Ordering::Equal
        } else {
            nums[a]
                .partial_cmp(&nums[b])
                .unwrap_or(std::cmp::Ordering::Equal)
        }
    });
    Ok(Value::array(
        idx.into_iter().map(|i| Value::Number(i as f64)).collect(),
    ))
}

// ---------------------------------------------------------------------------
// maps
// ---------------------------------------------------------------------------

fn keys(args: &[Value]) -> EvalResult<Value> {
    Ok(Value::array(arg_map(args, 0)?.keys().cloned().collect()))
}

fn values(args: &[Value]) -> EvalResult<Value> {
    Ok(Value::array(arg_map(args, 0)?.values().cloned().collect()))
}

/// `@mergeMaps(defaults, overrides)` — keys from the second win; the result
/// keeps the first map's type tag.
fn merge_maps(args: &[Value]) -> EvalResult<Value> {
    let src = arg(args, 0)?;
    let a = arg_map(args, 0)?;
    let b = arg_map(args, 1)?;
    let mut out = (**a).clone();
    for (k, v) in b.iter() {
        out.insert(k.clone(), v.clone());
    }
    Ok(src.retag(Value::map(out)))
}

/// `@intersectMaps([m1, m2, ...])` — keys present in every map, values from
/// the last one (the std `intersectMaps(maps is array)` form). The two-map
/// spelling `@intersectMaps(a, b)` is accepted as well.
fn intersect_maps(args: &[Value]) -> EvalResult<Value> {
    let maps: Vec<Rc<Map>> = if args.len() == 1 {
        let list = arg_array(args, 0)?;
        list.iter()
            .enumerate()
            .map(|(i, v)| {
                v.as_map()
                    .cloned()
                    .ok_or_else(|| Error::type_error(format!("element {i} must be a map")))
            })
            .collect::<EvalResult<_>>()?
    } else {
        (0..args.len())
            .map(|i| arg_map(args, i).cloned())
            .collect::<EvalResult<_>>()?
    };
    let Some(last) = maps.last() else {
        return Ok(Value::empty_map());
    };
    let out: Map = last
        .iter()
        .filter(|(k, _)| maps.iter().all(|m| m.contains_key(*k)))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Ok(Value::map(out))
}

// ---------------------------------------------------------------------------
// strings & regex
// ---------------------------------------------------------------------------

fn char_offset(s: &str, char_index: usize) -> usize {
    s.char_indices()
        .nth(char_index)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

fn substring(args: &[Value]) -> EvalResult<Value> {
    let s = arg_str(args, 0)?;
    let start = arg_index(args, 1)?;
    let end = if args.len() > 2 {
        arg_index(args, 2)?
    } else {
        s.chars().count()
    };
    if start > end || end > s.chars().count() {
        return Err(Error::runtime(format!(
            "@substring: range {start}..{end} out of bounds"
        )));
    }
    Ok(Value::from(&s[char_offset(s, start)..char_offset(s, end)]))
}

fn index_of_string(args: &[Value]) -> EvalResult<Value> {
    let s = arg_str(args, 0)?;
    let needle = arg_str(args, 1)?;
    let start = if args.len() > 2 {
        arg_index(args, 2)?
    } else {
        0
    };
    let from = char_offset(s, start);
    let found = s[from..]
        .find(needle)
        .map(|b| s[..from + b].chars().count() as f64)
        .unwrap_or(-1.0);
    Ok(Value::Number(found))
}

fn compile(pattern: &str) -> EvalResult<regex::Regex> {
    regex::Regex::new(pattern)
        .map_err(|e| Error::runtime(format!("invalid regular expression `{pattern}`: {e}")))
}

/// `@match(s, regExp)` → `{ "hasMatch" : bool, "captures" : [s, group1, ...] }`;
/// the pattern must match the entire string.
fn regex_match(args: &[Value]) -> EvalResult<Value> {
    let s = arg_str(args, 0)?;
    let re = compile(&format!("^(?:{})$", arg_str(args, 1)?))?;
    let mut out = Map::new();
    match re.captures(s) {
        Some(caps) => {
            out.insert("hasMatch".into(), Value::Bool(true));
            let captures: Vec<Value> = caps
                .iter()
                .map(|m| {
                    m.map(|m| Value::from(m.as_str()))
                        .unwrap_or(Value::from(""))
                })
                .collect();
            out.insert("captures".into(), Value::array(captures));
        }
        None => {
            out.insert("hasMatch".into(), Value::Bool(false));
            out.insert("captures".into(), Value::array(vec![Value::from(s)]));
        }
    }
    Ok(Value::map(out))
}

fn regex_replace(args: &[Value]) -> EvalResult<Value> {
    let s = arg_str(args, 0)?;
    let re = compile(arg_str(args, 1)?)?;
    let replacement = arg_str(args, 2)?;
    // FeatureScript (Java) uses `$1` for groups, as does the regex crate.
    Ok(Value::from(re.replace_all(s, replacement).into_owned()))
}

fn index_of_regexp(args: &[Value]) -> EvalResult<Value> {
    let s = arg_str(args, 0)?;
    let re = compile(arg_str(args, 1)?)?;
    let start = if args.len() > 2 {
        arg_index(args, 2)?
    } else {
        0
    };
    let from = char_offset(s, start);
    let found = re
        .find(&s[from..])
        .map(|m| s[..from + m.start()].chars().count() as f64)
        .unwrap_or(-1.0);
    Ok(Value::Number(found))
}

fn split_by_regexp(args: &[Value]) -> EvalResult<Value> {
    let s = arg_str(args, 0)?;
    let re = compile(arg_str(args, 1)?)?;
    Ok(Value::array(re.split(s).map(Value::from).collect()))
}

fn json_to_value(j: serde_json::Value) -> Value {
    use serde_json::Value as J;
    match j {
        J::Null => Value::Undefined,
        J::Bool(b) => Value::Bool(b),
        J::Number(n) => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
        J::String(s) => Value::from(s),
        J::Array(a) => Value::array(a.into_iter().map(json_to_value).collect()),
        J::Object(o) => Value::map(
            o.into_iter()
                .map(|(k, v)| (Value::from(k), json_to_value(v)))
                .collect(),
        ),
    }
}
