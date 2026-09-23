//! `fscript` — command-line front end for the FeatureScript parser.
//!
//! ```text
//! fscript check <path>...          parse files / directories, report failures
//! fscript dump <file> [--json]     print the AST
//! fscript tokens <file>            print the token stream
//! fscript stats <path>...          summarize declarations across files
//! fscript eval --std <dir> [-m module.fs] <expr>   evaluate an expression against the std library
//! fscript std-check --std <dir> [--verbose]        load the std library, evaluate every constant
//! ```

use fs_syntax::ast::{ItemKind, Module};
use fs_syntax::lexer::Lexer;
use fs_syntax::parse_module;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    // Piping into `head` closes stdout early; that is not an error worth a panic.
    #[cfg(unix)]
    unsafe {
        extern "C" {
            fn signal(sig: i32, handler: usize) -> usize;
        }
        signal(13 /* SIGPIPE */, 0 /* SIG_DFL */);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let rest = &args[1..];
    let result = match cmd.as_str() {
        "check" => check(rest),
        "dump" => dump(rest),
        "tokens" => tokens(rest),
        "stats" => stats(rest),
        "eval" => eval(rest),
        "std-check" => std_check(rest),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(true)
        }
        other => {
            eprintln!("unknown command `{other}`\n{USAGE}");
            Ok(false)
        }
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

const USAGE: &str = "usage:
  fscript check <path>...          parse files/directories, report failures (exit 1 if any)
  fscript dump <file> [--json]     print the AST (Rust Debug, or JSON with --json)
  fscript tokens <file>            print the token stream
  fscript stats <path>...          count declarations across files
  fscript eval --std <dir> [-m <module.fs>] <expr>
                                   evaluate <expr> in a std-library module's scope (default geometry.fs)
  fscript std-check --std <dir> [--verbose]
                                   load the whole std library and evaluate every top-level constant";

/// Expand files and directories into a sorted list of `.fs` files.
fn collect_files(paths: &[String]) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths {
        let path = Path::new(p);
        if path.is_dir() {
            walk_dir(path, &mut out)?;
        } else {
            out.push(path.to_path_buf());
        }
    }
    out.sort();
    Ok(out)
}

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if entry.file_name() != ".git" {
                walk_dir(&path, out)?;
            }
        } else if path.extension().is_some_and(|e| e == "fs") {
            out.push(path);
        }
    }
    Ok(())
}

fn read(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

fn check(paths: &[String]) -> std::io::Result<bool> {
    if paths.is_empty() {
        eprintln!("check: no paths given");
        return Ok(false);
    }
    let files = collect_files(paths)?;
    let mut failed = 0usize;
    let start = std::time::Instant::now();
    let mut bytes = 0usize;
    for file in &files {
        let src = read(file)?;
        bytes += src.len();
        if let Err(e) = parse_module(&src) {
            failed += 1;
            eprintln!("{}", e.render(&file.display().to_string(), &src));
        }
    }
    let elapsed = start.elapsed();
    println!(
        "{} files, {} ok, {} failed ({:.1} KB in {:.0} ms)",
        files.len(),
        files.len() - failed,
        failed,
        bytes as f64 / 1024.0,
        elapsed.as_secs_f64() * 1000.0
    );
    Ok(failed == 0)
}

fn dump(args: &[String]) -> std::io::Result<bool> {
    let json = args.iter().any(|a| a == "--json");
    let Some(file) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("dump: expected a file");
        return Ok(false);
    };
    let src = read(Path::new(file))?;
    match parse_module(&src) {
        Ok(module) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&module).expect("serialize")
                );
            } else {
                println!("{module:#?}");
            }
            Ok(true)
        }
        Err(e) => {
            eprintln!("{}", e.render(file, &src));
            Ok(false)
        }
    }
}

fn tokens(args: &[String]) -> std::io::Result<bool> {
    let Some(file) = args.first() else {
        eprintln!("tokens: expected a file");
        return Ok(false);
    };
    let src = read(Path::new(file))?;
    let index = fs_syntax::LineIndex::new(&src);
    match Lexer::new(&src).tokenize() {
        Ok(toks) => {
            for t in toks {
                println!(
                    "{:>6}  {:?}",
                    index.line_col(t.span.start).to_string(),
                    t.kind
                );
            }
            Ok(true)
        }
        Err(e) => {
            eprintln!("{}", e.render(file, &src));
            Ok(false)
        }
    }
}

fn stats(paths: &[String]) -> std::io::Result<bool> {
    let files = collect_files(paths)?;
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut exported = 0usize;
    let mut failed = 0usize;
    let mut imports: BTreeMap<String, usize> = BTreeMap::new();
    for file in &files {
        let src = read(file)?;
        let module: Module = match parse_module(&src) {
            Ok(m) => m,
            Err(_) => {
                failed += 1;
                continue;
            }
        };
        for item in &module.items {
            let key = match &item.kind {
                ItemKind::Import(i) => {
                    if let Some(p) = i.path() {
                        *imports.entry(p.to_string()).or_default() += 1;
                    }
                    "import"
                }
                ItemKind::Function(_) => "function",
                ItemKind::Predicate(_) => "predicate",
                ItemKind::Operator(_) => "operator",
                ItemKind::Type(_) => "type",
                ItemKind::Enum(_) => "enum",
                ItemKind::Const(_) => "const",
                ItemKind::Var(_) => "var",
            };
            *counts.entry(key).or_default() += 1;
            if item.export {
                exported += 1;
            }
        }
    }
    println!("files: {} ({} failed to parse)", files.len(), failed);
    for (k, n) in &counts {
        println!("{k:>10}: {n}");
    }
    println!("  exported: {exported}");
    let mut top: Vec<_> = imports.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("most imported modules:");
    for (p, n) in top.iter().take(10) {
        println!("{n:>6}  {p}");
    }
    Ok(failed == 0)
}

/// `--std <dir>` plus remaining positional arguments.
fn std_args(args: &[String]) -> Result<(String, Vec<String>, Vec<String>), String> {
    let mut std = None;
    let mut positional = Vec::new();
    let mut flags = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--std" => {
                std = args.get(i + 1).cloned();
                i += 2;
            }
            f if f.starts_with('-') && f != "-m" => {
                flags.push(f.to_string());
                i += 1;
            }
            "-m" => {
                flags.push(format!(
                    "-m={}",
                    args.get(i + 1).cloned().unwrap_or_default()
                ));
                i += 2;
            }
            other => {
                positional.push(other.to_string());
                i += 1;
            }
        }
    }
    let std = std.ok_or("missing --std <dir> (a checkout of the Onshape std library)")?;
    Ok((std, positional, flags))
}

fn eval(args: &[String]) -> std::io::Result<bool> {
    let (std, positional, flags) = match std_args(args) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("eval: {e}");
            return Ok(false);
        }
    };
    let module = flags
        .iter()
        .find_map(|f| f.strip_prefix("-m="))
        .map(|m| {
            if m.contains('/') {
                m.to_string()
            } else {
                format!("onshape/std/{m}")
            }
        })
        .unwrap_or_else(|| "onshape/std/geometry.fs".to_string());
    let expr = positional.join(" ");
    let mut interp = fs_interp::Interp::with_std_dir(&std);
    let m = match interp.load_module(&module) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("load failed: {e}");
            return Ok(false);
        }
    };
    match interp.eval_in(&m, &expr) {
        Ok(v) => {
            println!("{v}");
            Ok(true)
        }
        Err(e) => {
            eprintln!("error: {e}");
            Ok(false)
        }
    }
}

fn std_check(args: &[String]) -> std::io::Result<bool> {
    let (std, _, flags) = match std_args(args) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("std-check: {e}");
            return Ok(false);
        }
    };
    let verbose = flags.iter().any(|f| f == "--verbose" || f == "-v");
    let mut interp = fs_interp::Interp::with_std_dir(&std);
    let start = std::time::Instant::now();
    if let Err(e) = interp.load_module("onshape/std/geometry.fs") {
        eprintln!("load failed: {e}");
        return Ok(false);
    }
    let modules = interp.modules();
    let load_time = start.elapsed();
    let mut total = 0usize;
    let mut by_kind: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in &modules {
        total += m
            .globals
            .borrow()
            .values()
            .filter(|s| s.lazy.is_some())
            .count();
        for (name, err) in interp.force_all_globals(m) {
            let kind = match &err.kind {
                fs_interp::ErrorKind::Unimplemented(b) => format!("unimplemented {b}"),
                fs_interp::ErrorKind::Thrown(_) => "thrown".to_string(),
                fs_interp::ErrorKind::Precondition(_) => "precondition".to_string(),
                fs_interp::ErrorKind::Type(_) => "type".to_string(),
                fs_interp::ErrorKind::Name(_) => "name".to_string(),
                fs_interp::ErrorKind::Load(_) => "load".to_string(),
                fs_interp::ErrorKind::Runtime(_) => "runtime".to_string(),
            };
            let short = m.path.trim_start_matches("onshape/std/");
            by_kind.entry(kind).or_default().push(if verbose {
                format!("{short}:{name}: {err}")
            } else {
                format!("{short}:{name}")
            });
        }
    }
    let failed: usize = by_kind.values().map(|v| v.len()).sum();
    println!(
        "{} modules loaded in {:.0} ms; {} constants, {} evaluated, {} failed",
        modules.len(),
        load_time.as_secs_f64() * 1000.0,
        total,
        total - failed,
        failed
    );
    for (kind, names) in &by_kind {
        println!("\n[{kind}] ({})", names.len());
        for n in names {
            println!("  {n}");
        }
    }
    Ok(true)
}
