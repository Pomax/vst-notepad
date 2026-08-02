//! Build tasks.
//!
//! A VST3 plugin is not a bare shared library — it is a *bundle*: a directory
//! named `Something.vst3` with a prescribed layout that differs per platform.
//! Hosts scan for that directory, so a raw `.dll` sitting in `target/debug`
//! will not be found by any DAW. This task builds the library and assembles
//! the bundle around it.
//!
//! ```text
//! Notepad.vst3/
//!   Contents/
//!     x86_64-win/Notepad.vst3        (Windows: the DLL, renamed)
//!     MacOS/Notepad                  (macOS: the dylib, no extension)
//!     Info.plist                     (macOS only)
//!     PkgInfo                        (macOS only)
//! ```
//!
//! Usage:
//! ```text
//! cargo run -p xtask -- bundle [--release] [--target <triple>]
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const PLUGIN_CRATE: &str = "notepad-plugin";
const BUNDLE_NAME: &str = "Notepad";
const BUNDLE_ID: &str = "com.vst-notepad.notepad";
const VERSION: &str = "0.1.0";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("bundle");

    match command {
        "bundle" => match bundle(&args[1..]) {
            Ok(path) => {
                println!("bundle: {}", path.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        "test" => match test(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        "help" | "--help" | "-h" => {
            println!(
                "tasks:\n  \
                 bundle [--release] [--target <triple>]   assemble the VST3 bundle\n  \
                 test [--release] [--snapshots]           build the plugin, then run every test\n  \
                 \n  \
                 --snapshots also runs the headless pixel tests, which need the\n  \
                 wgpu stack: correct, but gigabytes of build output."
            );
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown task: {other}");
            ExitCode::FAILURE
        }
    }
}

struct Options {
    release: bool,
    target: Option<String>,
}

fn parse(args: &[String]) -> Options {
    let mut opts = Options {
        release: false,
        target: None,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--release" => opts.release = true,
            "--target" => {
                opts.target = args.get(i + 1).cloned();
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    opts
}

fn workspace_root() -> PathBuf {
    // xtask lives at <root>/xtask, so the manifest dir's parent is the root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf()
}

fn bundle(args: &[String]) -> Result<PathBuf, String> {
    let opts = parse(args);
    let root = workspace_root();

    // 1. Build the shared library.
    let mut cmd = Command::new(env!("CARGO"));
    cmd.current_dir(&root).arg("build").arg("-p").arg(PLUGIN_CRATE);
    if opts.release {
        cmd.arg("--release");
    }
    if let Some(target) = &opts.target {
        cmd.arg("--target").arg(target);
    }
    let status = cmd.status().map_err(|e| format!("running cargo: {e}"))?;
    if !status.success() {
        return Err("building the plugin failed".into());
    }

    // 2. Find what it produced.
    let profile_dir = if opts.release { "release" } else { "debug" };
    let mut out_dir = root.join("target");
    if let Some(target) = &opts.target {
        out_dir = out_dir.join(target);
    }
    let out_dir = out_dir.join(profile_dir);

    let triple = opts.target.clone().unwrap_or_else(host_triple);
    let lib = out_dir.join(library_name(&triple));
    if !lib.exists() {
        return Err(format!("expected {} to exist", lib.display()));
    }

    // 3. Assemble the bundle.
    let bundle_root = out_dir.join("bundle").join(format!("{BUNDLE_NAME}.vst3"));
    if bundle_root.exists() {
        fs::remove_dir_all(&bundle_root).map_err(|e| format!("clearing old bundle: {e}"))?;
    }
    let contents = bundle_root.join("Contents");

    if triple.contains("apple") || triple.contains("darwin") {
        let macos = contents.join("MacOS");
        fs::create_dir_all(&macos).map_err(|e| format!("creating {}: {e}", macos.display()))?;
        copy(&lib, &macos.join(BUNDLE_NAME))?;
        write(&contents.join("Info.plist"), &info_plist())?;
        // 'BNDL????' is what the VST3 SDK writes for plugin bundles.
        write(&contents.join("PkgInfo"), "BNDL????")?;
    } else {
        let arch_dir = contents.join(platform_dir(&triple));
        fs::create_dir_all(&arch_dir)
            .map_err(|e| format!("creating {}: {e}", arch_dir.display()))?;
        // On Windows and Linux the binary inside the bundle keeps the .vst3
        // extension rather than .dll/.so.
        copy(&lib, &arch_dir.join(format!("{BUNDLE_NAME}.vst3")))?;
    }

    // 4. Drop the bare plugin binary into dist/.
    let dist = write_dist(&root, &lib, &triple)?;
    println!("dist:   {}", dist.display());

    // 5. A release build supersedes the debug one, and the debug tree is by far
    //    the larger of the two. Nothing needs it once release exists.
    if opts.release {
        prune_debug(&root, &opts);
        prune_intermediates(&out_dir);
    }

    Ok(bundle_root)
}

/// Strip a release directory back to the finished artefacts.
///
/// `deps/`, `build/`, `incremental/` and `.fingerprint/` exist only to make the
/// *next* compile faster; they are worth hundreds of megabytes and nothing
/// needs them once the binary is built and copied out. The trade is that the
/// next release build is a full one.
fn prune_intermediates(release_dir: &Path) {
    let before = directory_size(release_dir);
    let Ok(entries) = fs::read_dir(release_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Keep the assembled bundle: on macOS it is the only loadable form.
        if path.file_name().is_some_and(|n| n == "bundle") {
            continue;
        }
        let _ = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
    }
    let freed = before.saturating_sub(directory_size(release_dir));
    if freed > 0 {
        println!("pruned: {} of release intermediates", human_size(freed));
    }
}

/// Delete the debug build output after a successful release build.
///
/// Failures are reported, not fatal: on Windows the directory cannot be removed
/// while anything still has a binary in it mapped.
fn prune_debug(root: &Path, opts: &Options) {
    let mut debug = root.join("target");
    if let Some(target) = &opts.target {
        debug = debug.join(target);
    }
    let debug = debug.join("debug");
    if !debug.exists() {
        return;
    }
    let before = directory_size(&debug);
    if fs::remove_dir_all(&debug).is_err() {
        // Remove what can be removed. Anything still mapped — most often the
        // running xtask binary itself — is simply left behind.
        if let Ok(entries) = fs::read_dir(&debug) {
            for entry in entries.flatten() {
                let path = entry.path();
                let _ = if path.is_dir() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
            }
        }
    }
    let after = directory_size(&debug);
    let freed = before.saturating_sub(after);
    if freed > 0 {
        println!("pruned: {} of debug build output", human_size(freed));
    }
    if after > 0 {
        println!(
            "note:   {} left in {} (still in use)",
            human_size(after),
            debug.display()
        );
    }
}

fn human_size(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else {
        format!("{} MB", bytes / MB)
    }
}

fn directory_size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => directory_size(&entry.path()),
            Ok(meta) => meta.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Put the plugin binary, and nothing else, in `<root>/dist`.
///
/// On Windows and Linux a VST3 may be a bare shared library with a `.vst3`
/// extension — Vital ships exactly that way — so this single file is directly
/// installable into the system VST3 folder. The directory is wiped first, so it
/// only ever holds the current build and never accumulates stale binaries.
///
/// macOS is the exception: there a plugin *must* be a bundle directory, so the
/// file here is a building block rather than something a DAW can load. Use the
/// bundle for macOS.
fn write_dist(root: &Path, lib: &Path, triple: &str) -> Result<PathBuf, String> {
    let dist = root.join("dist");
    fs::create_dir_all(&dist).map_err(|e| format!("creating {}: {e}", dist.display()))?;

    let target = dist.join(format!("{BUNDLE_NAME}.vst3"));

    // Remove everything except the file we are about to write. Deleting the
    // directory wholesale fails on Windows whenever anything still has the
    // plugin mapped, which a host that loaded it a moment ago will.
    if let Ok(entries) = fs::read_dir(&dist) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path == target {
                continue;
            }
            let _ = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
        }
    }

    copy(lib, &target).map_err(|e| {
        format!("{e}\n  (if this says the file is in use, something still has the plugin loaded)")
    })?;

    if triple.contains("apple") || triple.contains("darwin") {
        println!(
            "note:   macOS requires the bundle form; the file in dist/ is not \
             loadable on its own"
        );
    }
    Ok(target)
}

/// Build the plugin, then run the unit tests and the scenario suite.
///
/// The scenario runner loads the plugin binary at runtime rather than linking
/// it, so cargo does not know to rebuild it first. Running the two in the right
/// order is the whole point of this task.
fn test(args: &[String]) -> Result<(), String> {
    let opts = parse(args);
    let root = workspace_root();

    let run = |what: &str, extra: &[&str]| -> Result<(), String> {
        let mut cmd = Command::new(env!("CARGO"));
        cmd.current_dir(&root);
        cmd.args(extra);
        if opts.release {
            cmd.arg("--release");
        }
        let status = cmd.status().map_err(|e| format!("running cargo: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{what} failed"))
        }
    };

    run("building the plugin", &["build", "-p", "notepad-plugin"])?;
    run("unit tests", &["test", "--workspace"])?;
    run("scenarios", &["run", "-q", "-p", "notepad-testrunner"])?;

    // The pixel tests need the wgpu stack, which is gigabytes of build output,
    // so they are opt-in rather than part of every run.
    if args.iter().any(|a| a == "--snapshots") {
        run(
            "rendering tests",
            &[
                "test",
                "-p",
                "notepad-plugin",
                "--features",
                "snapshots",
                "--test",
                "theme_rendering",
            ],
        )?;
    }
    Ok(())
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("copying {} -> {}: {e}", from.display(), to.display()))
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents).map_err(|e| format!("writing {}: {e}", path.display()))
}

/// File name cargo gives the cdylib for a target.
fn library_name(triple: &str) -> String {
    if triple.contains("windows") {
        "notepad_plugin.dll".into()
    } else if triple.contains("apple") || triple.contains("darwin") {
        "libnotepad_plugin.dylib".into()
    } else {
        "libnotepad_plugin.so".into()
    }
}

/// The architecture directory name the VST3 spec expects inside `Contents`.
fn platform_dir(triple: &str) -> String {
    let arch = if triple.starts_with("x86_64") {
        "x86_64"
    } else if triple.starts_with("aarch64") {
        "arm64"
    } else if triple.starts_with("i686") || triple.starts_with("i586") {
        "x86"
    } else {
        "unknown"
    };
    if triple.contains("windows") {
        format!("{arch}-win")
    } else {
        format!("{arch}-linux")
    }
}

fn host_triple() -> String {
    // Good enough for the host build; explicit --target covers everything else.
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };
    let os = if cfg!(target_os = "windows") {
        "pc-windows-msvc"
    } else if cfg!(target_os = "macos") {
        "apple-darwin"
    } else {
        "unknown-linux-gnu"
    };
    format!("{arch}-{os}")
}

fn info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>{BUNDLE_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>{BUNDLE_ID}</string>
    <key>CFBundleName</key>
    <string>{BUNDLE_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>{BUNDLE_NAME}</string>
    <key>CFBundlePackageType</key>
    <string>BNDL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>CFBundleVersion</key>
    <string>{VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>{VERSION}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.13</string>
</dict>
</plist>
"#
    )
}
