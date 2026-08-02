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
                 test [--release] [--snapshots] [--keep-target]\n  \
                 \n  \
                 Both tasks delete target/ when they succeed; it survives a\n  \
                 failure so there is something to debug with.\n  \
                 --snapshots also runs the headless pixel tests, which need the\n  \
                 wgpu stack: correct, but gigabytes of build output.\n  \
                 --keep-target leaves target/ in place for a fast next run."
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
    // xtask lives at <root>/src/xtask.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
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

    if is_macos(&triple) {
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

    // 3b. Describe the bundle in moduleinfo.json.
    match write_module_info(&bundle_root, &contents) {
        Ok(path) => println!("info:   {}", path.display()),
        Err(e) => println!("note:   could not write moduleinfo.json: {e}"),
    }

    // 4. Copy the plugin binary to dist/, so the build result can be picked up
    //    without digging through target/.
    let dist = write_dist(&root, &lib, &bundle_root, &triple)?;
    println!("dist:   {}", dist.display());

    // 5. The build succeeded and dist/ has the result, so the intermediates are
    //    only of interest when something went wrong.
    remove_target(&root);

    Ok(bundle_root)
}

/// Delete `dist/`.
fn remove_dist(root: &Path) {
    let dist = root.join("dist");
    if !dist.exists() {
        return;
    }
    match fs::remove_dir_all(&dist) {
        Ok(()) => println!("removed: dist/"),
        Err(e) => println!("note:   could not remove {}: {e}", dist.display()),
    }
}

/// Delete `target/` after a successful build.
///
/// Everything worth keeping has already been copied into `dist/`; what remains
/// exists only to diagnose a build that went wrong.
///
/// The one thing that can survive is the build tool itself: on Windows a
/// running executable cannot be deleted, and `cargo dist` runs xtask out of
/// `target/`. Whatever is left is reported.
fn remove_target(root: &Path) {
    let target = root.join("target");
    if !target.exists() {
        return;
    }
    let before = directory_size(&target);
    let running = std::env::current_exe().ok();

    if fs::remove_dir_all(&target).is_ok() {
        println!("removed: target/ ({})", human_size(before));
        return;
    }

    // Only the running executable is in the way. Clear everything else now and
    // hand the remainder to a process that outlives this one.
    purge(&target, running.as_deref());
    if schedule_removal(&target) {
        println!("removed: target/ ({})", human_size(before));
    } else {
        println!(
            "removed: {} from target/, {} left (the build tool running this)",
            human_size(before.saturating_sub(directory_size(&target))),
            human_size(directory_size(&target))
        );
    }
}

/// Delete `dir` from a process that outlives this one.
///
/// Windows refuses to unlink a running executable, and `cargo dist` runs this
/// tool out of `target/`, so the last of it has to go after this process ends.
fn schedule_removal(dir: &Path) -> bool {
    // Forward slashes so the path works in a POSIX shell on every platform.
    let path = dir.display().to_string().replace('\\', "/");
    Command::new("sh")
        .arg("-c")
        .arg(format!("sleep 2; rm -rf '{path}'"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// Delete everything under `dir` except the path to `keep`, if given.
fn purge(dir: &Path, keep: Option<&Path>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let needed = keep.is_some_and(|k| k.starts_with(&path));
        if needed {
            if path.is_dir() {
                purge(&path, keep);
            }
            continue;
        }
        let _ = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
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

/// Write `Contents/Resources/moduleinfo.json`, part of the bundle layout since
/// VST 3.7.5.
///
/// It declares the plugin's classes and their subcategories so a host can see
/// what kind of plugin this is — effect or instrument — without loading the
/// binary at all. The contents are read back out of the binary that was just
/// built, rather than written from constants here, so the description cannot
/// disagree with the thing it describes.
fn write_module_info(bundle_root: &Path, contents: &Path) -> Result<PathBuf, String> {
    let module = notepad_host::Module::load(bundle_root).map_err(|e| e.to_string())?;
    let factory = module.factory_info();

    let mut classes = Vec::new();
    for index in 0..module.class_count() {
        let Some(info) = module.class_info2(index) else {
            continue;
        };
        let subs: Vec<String> = info
            .sub_categories
            .split('|')
            .filter(|s| !s.is_empty())
            .map(|s| format!("\"{s}\""))
            .collect();
        classes.push(format!(
            "    {{
      \"CID\": \"{cid}\",
      \"Category\": \"{category}\",
      \"Name\": \"{name}\",
      \"Vendor\": \"{vendor}\",
      \"Version\": \"{version}\",
      \"SDKVersion\": \"{sdk}\",
      \"Sub Categories\": [{subs}],
      \"Class Flags\": {flags},
      \"Cardinality\": {cardinality}
    }}",
            cid = info.cid_string(),
            category = info.category,
            name = info.name,
            vendor = info.vendor,
            version = info.version,
            sdk = info.sdk_version,
            subs = subs.join(", "),
            flags = info.class_flags,
            cardinality = info.cardinality,
        ));
    }

    // `kUnicode` is bit 4 of the factory flags.
    let unicode = factory.flags & (1 << 4) != 0;
    let json = format!(
        "{{
  \"Name\": \"{name}\",
  \"Version\": \"{version}\",
  \"Factory Info\": {{
    \"Vendor\": \"{vendor}\",
    \"URL\": \"{url}\",
    \"E-Mail\": \"{email}\",
    \"Flags\": {{
      \"Unicode\": {unicode},
      \"Classes Discardable\": false,
      \"Component Non Discardable\": false
    }}
  }},
  \"Classes\": [
{classes}
  ],
  \"Compatibility\": []
}}
",
        name = BUNDLE_NAME,
        version = VERSION,
        vendor = factory.vendor,
        url = factory.url,
        email = factory.email,
        classes = classes.join(",\n"),
    );

    let resources = contents.join("Resources");
    fs::create_dir_all(&resources).map_err(|e| format!("creating {}: {e}", resources.display()))?;
    let path = resources.join("moduleinfo.json");
    write(&path, &json)?;
    Ok(path)
}


/// Replace `<root>/dist` with the build result.
///
/// The directory is deleted and recreated, so it holds this build and nothing
/// else. On Windows and Linux that is the plugin binary; on macOS it is the
/// bundle, which is the only loadable form there.
fn write_dist(
    root: &Path,
    lib: &Path,
    bundle_root: &Path,
    triple: &str,
) -> Result<PathBuf, String> {
    let dist = root.join("dist");
    if dist.exists() {
        fs::remove_dir_all(&dist).map_err(|e| {
            format!(
                "removing {}: {e}\n  (if this says the file is in use, something still \
                 has the plugin loaded)",
                dist.display()
            )
        })?;
    }
    fs::create_dir(&dist).map_err(|e| format!("creating {}: {e}", dist.display()))?;

    let target = dist.join(format!("{BUNDLE_NAME}.vst3"));
    if is_macos(triple) {
        copy_tree(bundle_root, &target)?;
    } else {
        copy(lib, &target)?;
    }
    Ok(target)
}

/// Recursively copy a directory.
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("creating {}: {e}", to.display()))?;
    let entries = fs::read_dir(from).map_err(|e| format!("reading {}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let source = entry.path();
        let destination = to.join(entry.file_name());
        if source.is_dir() {
            copy_tree(&source, &destination)?;
        } else {
            copy(&source, &destination)?;
        }
    }
    Ok(())
}

fn is_macos(triple: &str) -> bool {
    triple.contains("apple") || triple.contains("darwin")
}

/// Build the plugin, then run the unit tests and the scenario suite.
///
/// The scenario runner loads the plugin binary at runtime rather than linking
/// it, so cargo does not know to rebuild it first. Running the two in the right
/// order is the whole point of this task.
fn test(args: &[String]) -> Result<(), String> {
    let opts = parse(args);
    let root = workspace_root();

    // A test run compiles the plugin but is not a build of it, so anything in
    // dist/ is now describing older code. Remove it rather than leave something
    // stale that looks current.
    remove_dist(&root);

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

    // Everything passed, so the build output is only of interest when something
    // fails. `--keep-target` leaves it for a fast next run.
    if !args.iter().any(|a| a == "--keep-target") {
        remove_target(&root);
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
