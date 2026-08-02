//! A command-line VST3 host.
//!
//! Points the minimal host at any VST3 plugin, loads it the way a DAW would,
//! and reports what it found. Useful for checking that a plugin you have just
//! built is actually loadable before dragging it into a DAW.
//!
//! ```text
//! vst3-host <path-to-plugin> [options]
//!
//!   --class <n>     instantiate class n instead of the first audio module
//!   --list          list the factory's classes and stop
//!   --no-editor     do not create the editor view
//!   --type <text>   send the text to the editor as key presses
//!   --state <file>  write the plugin's state to a file
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use notepad_host::{resolve_binary, Module};

struct Args {
    path: PathBuf,
    class: Option<i32>,
    list: bool,
    editor: bool,
    type_text: Option<String>,
    state: Option<PathBuf>,
}

fn parse() -> Option<Args> {
    let mut raw = std::env::args().skip(1);
    let path = PathBuf::from(raw.next()?);
    let mut args = Args {
        path,
        class: None,
        list: false,
        editor: true,
        type_text: None,
        state: None,
    };
    while let Some(flag) = raw.next() {
        match flag.as_str() {
            "--class" => args.class = raw.next().and_then(|v| v.parse().ok()),
            "--list" => args.list = true,
            "--no-editor" => args.editor = false,
            "--type" => args.type_text = raw.next(),
            "--state" => args.state = raw.next().map(PathBuf::from),
            _ => {}
        }
    }
    Some(args)
}

fn main() -> ExitCode {
    let Some(args) = parse() else {
        eprintln!(
            "usage: vst3-host <path-to-plugin.vst3> \
             [--list] [--class N] [--no-editor] [--type TEXT] [--state FILE]"
        );
        return ExitCode::FAILURE;
    };

    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let binary = resolve_binary(&args.path)?;
    println!("binary   {}", binary.display());

    let module = Module::load(&args.path)?;
    println!("vendor   {}", module.vendor());

    let (f1, f2, f3) = module.factory_versions();
    println!(
        "factory  IPluginFactory={f1} IPluginFactory2={f2} IPluginFactory3={f3}"
    );

    let count = module.class_count();
    println!("classes  {count}");
    for i in 0..count {
        if let Some((name, category)) = module.class_info(i) {
            let marker = if Some(i) == module.first_audio_class() {
                "*"
            } else {
                " "
            };
            println!("  {marker}[{i}] {name}  ({category})");
            match module.class_info2(i) {
                Some(info) => {
                    let cid: String = info.cid.iter().map(|b| format!("{b:02X}")).collect();
                    println!(
                        "        subCategories={:?} flags={:#x} cardinality={} cid={cid}",
                        info.sub_categories, info.class_flags, info.cardinality
                    );
                    if let Some(w) = module.class_info_unicode(i) {
                        println!(
                            "        via factory3: name={:?} category={:?} subCategories={:?}",
                            w.name, w.category, w.sub_categories
                        );
                    } else {
                        println!("        via factory3: MISSING");
                    }
                }
                None => println!("        (no IPluginFactory2 — a host cannot tell what type this is)"),
            }
        }
    }
    if args.list {
        return Ok(());
    }

    let index = args.class.or_else(|| module.first_audio_class()).unwrap_or(0);
    println!("\ninstantiating class {index}");

    // Only the processor can be instantiated directly. Asking for a controller
    // class yields a bare E_NOINTERFACE, which is a confusing thing to be told.
    if let Some((_, category)) = module.class_info(index) {
        if category != "Audio Module Class" {
            println!("  note         this is a {category}, not an Audio Module Class.");
            println!("               Only the processor can be created directly; its");
            println!("               controller is created automatically alongside it.");
        }
    }

    let mut plugin = module.create_plugin_at(index)?;

    let shape = if !plugin.has_controller() {
        "processor only (no controller)"
    } else if plugin.has_separate_controller() {
        "processor + separate controller"
    } else {
        "single-component effect"
    };
    println!("  shape        {shape}");
    println!(
        "  audio buses  {} in, {} out",
        plugin.bus_count(true, true),
        plugin.bus_count(true, false)
    );
    println!(
        "  event buses  {} in, {} out",
        plugin.bus_count(false, true),
        plugin.bus_count(false, false)
    );

    let params = plugin.parameter_count();
    println!("  parameters   {params}");
    for i in 0..params.min(5) {
        if let Some(name) = plugin.parameter_name(i) {
            println!("      [{i}] {name}");
        }
    }
    if params > 5 {
        println!("      … {} more", params - 5);
    }

    if args.editor {
        match plugin.open_editor() {
            Ok(()) => {
                let size = plugin.view_size();
                let resizable = plugin.can_resize().unwrap_or(false);
                match size {
                    Ok((w, h)) => println!("  editor       {w}x{h}, resizable: {resizable}"),
                    Err(e) => println!("  editor       created, but getSize failed: {e}"),
                }
            }
            Err(e) => println!("  editor       none ({e})"),
        }
    }

    if let Some(text) = &args.type_text {
        plugin.type_text(text)?;
        println!("  typed        {} characters", text.chars().count());
    }

    let state = plugin.get_state()?;
    println!("  state        {} bytes", state.len());
    if let Ok(text) = std::str::from_utf8(&state) {
        if text.chars().all(|c| !c.is_control() || c == '\n') && !text.is_empty() {
            let preview: String = text.chars().take(200).collect();
            println!("  state text   {preview}");
        }
    }

    if let Some(path) = &args.state {
        std::fs::write(path, &state)?;
        println!("  wrote        {}", path.display());
    }

    Ok(())
}
