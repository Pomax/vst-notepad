//! Loads the built VST3 plugin into the minimal host and runs every scenario
//! against it.
//!
//! Usage: `notepad-testrunner [path-to-plugin-binary]`. With no argument it
//! looks for the plugin next to this executable, which is where cargo puts it.

mod scenario;
mod scenarios;

use std::path::PathBuf;
use std::process::ExitCode;

use notepad_host::{plugin_file_name, Module};

fn locate_plugin() -> Option<PathBuf> {
    if let Some(arg) = std::env::args().nth(1) {
        return Some(PathBuf::from(arg));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join(plugin_file_name());
    if candidate.exists() {
        return Some(candidate);
    }
    // When run through `cargo test`, binaries live in `deps/`.
    let candidate = dir.parent()?.join(plugin_file_name());
    candidate.exists().then_some(candidate)
}

/// Newest modification time under a directory tree.
fn newest_mtime(dir: &std::path::Path) -> Option<std::time::SystemTime> {
    let mut newest = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let time = if path.is_dir() {
            newest_mtime(&path)
        } else {
            path.metadata().ok()?.modified().ok()
        };
        if let Some(t) = time {
            newest = Some(newest.map_or(t, |n: std::time::SystemTime| n.max(t)));
        }
    }
    newest
}

/// Refuse to test a plugin binary older than the code it was built from.
///
/// `notepad-testrunner` deliberately does not depend on `notepad-plugin` — it
/// loads the binary at runtime, the way a DAW does. The cost of that is that
/// cargo has no idea the two are related and will happily run this against a
/// stale `.dll`, reporting passes for code that is no longer there. This check
/// exists because that actually happened.
fn warn_if_stale(plugin: &std::path::Path) -> bool {
    let Ok(built) = plugin.metadata().and_then(|m| m.modified()) else {
        return false;
    };
    // This crate lives at <root>/src/crates/notepad-testrunner.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .map(|p| p.to_path_buf());
    let Some(root) = root else { return false };

    let newest = [
        "src/crates/notepad-plugin/src",
        "src/crates/notepad-core/src",
    ]
        .iter()
        .filter_map(|d| newest_mtime(&root.join(d)))
        .max();

    match newest {
        Some(source) if source > built => {
            eprintln!(
                "stale plugin binary: {} is older than the sources it was built from.\n\
                 Run `cargo build -p notepad-plugin` (or `cargo run -p xtask -- test`) first —\n\
                 otherwise these scenarios test code that no longer exists.\n",
                plugin.display()
            );
            true
        }
        _ => false,
    }
}

fn main() -> ExitCode {
    let Some(path) = locate_plugin() else {
        eprintln!(
            "could not find {} — run `cargo build` first",
            plugin_file_name()
        );
        return ExitCode::FAILURE;
    };

    if warn_if_stale(&path) {
        return ExitCode::FAILURE;
    }

    let module = match Module::load(&path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    println!("plugin:  {}", path.display());
    println!("vendor:  {}", module.vendor());
    for i in 0..module.class_count() {
        if let Some((name, category)) = module.class_info(i) {
            println!("class:   {name} ({category})");
        }
    }
    println!();

    let scenarios = scenarios::all();
    let mut failures = Vec::new();

    for s in &scenarios {
        match scenario::run(&module, s) {
            Ok(()) => println!("  ok    {}", s.name),
            Err(f) => {
                println!("  FAIL  {}", s.name);
                failures.push((s.name, f));
            }
        }
    }

    println!();
    if failures.is_empty() {
        println!("{} scenarios, all passed", scenarios.len());
        ExitCode::SUCCESS
    } else {
        for (name, f) in &failures {
            println!("--- {name}");
            println!("{f}");
            println!();
        }
        println!("{} of {} scenarios failed", failures.len(), scenarios.len());
        ExitCode::FAILURE
    }
}
