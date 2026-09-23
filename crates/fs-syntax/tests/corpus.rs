//! Parses every `.fs` file in the Onshape standard library mirror and checks
//! that printing and re-parsing yields the same tree (via printer idempotence).
//!
//! The corpus is looked up at `$FS_STD_CORPUS`, falling back to
//! `../feature_script_std` next to this repository (which is where
//! <https://github.com/yepher/feature_script_std> lands if you clone it
//! beside `feature-script-parser`). Missing corpus = test skipped.

use fs_syntax::{parse_module, print_module};
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

fn corpus_files() -> Vec<PathBuf> {
    let Some(dir) = corpus_dir() else {
        eprintln!("corpus not found; set FS_STD_CORPUS to run this test");
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read corpus dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "fs"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no .fs files in {}", dir.display());
    files
}

#[test]
fn every_std_library_file_parses_and_round_trips() {
    let files = corpus_files();
    let mut failures = Vec::new();
    for file in &files {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(file).unwrap();
        let module = match parse_module(&src) {
            Ok(m) => m,
            Err(e) => {
                failures.push(format!("{name}: {}", e.render(&name, &src)));
                continue;
            }
        };
        let printed = print_module(&module);
        let reparsed = match parse_module(&printed) {
            Ok(m) => m,
            Err(e) => {
                failures.push(format!(
                    "{name}: re-parse of printed output failed: {}",
                    e.render(&name, &printed)
                ));
                continue;
            }
        };
        // The printer is canonical, so equal trees print identically and a
        // structural difference shows up as a textual one (spans excluded).
        let reprinted = print_module(&reparsed);
        if printed != reprinted {
            let diff = printed
                .lines()
                .zip(reprinted.lines())
                .enumerate()
                .find(|(_, (x, y))| x != y);
            failures.push(format!(
                "{name}: round-trip mismatch at {:?}",
                diff.map(|(i, (x, y))| (i, x.trim(), y.trim()))
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} files failed:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
    eprintln!("{} files parsed and round-tripped", files.len());
}
