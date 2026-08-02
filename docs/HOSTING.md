# Using the VST3 host

`notepad-host` is a minimal VST3 host. It was written to test this project's
plugin, but nothing in it is specific to that plugin — it loads **any** VST3
binary, instantiates it the way a DAW would, and lets you drive it.

It comes in two forms:

- **`vst3-host`**, a command-line tool for inspecting a plugin.
- **the library**, for writing scripted interactions in Rust.

---

## Part 1 — the command-line tool

```bash
cargo run -p notepad-host --bin vst3-host -- <path-to-plugin.vst3>
```

Point it at this project's plugin:

```bash
cargo run -p notepad-host --bin vst3-host -- target/release/bundle/Notepad.vst3
```

```text
binary   target/release/bundle/Notepad.vst3\Contents\x86_64-win\Notepad.vst3
vendor   vst-notepad
classes  1
  *[0] Notepad  (Audio Module Class)

instantiating class 0
  shape        single-component effect
  audio buses  1 in, 1 out
  event buses  0 in, 0 out
  parameters   0
  editor       900x620, resizable: true
  state        103 bytes
  state text   {"version":1,"notes":"","mode":"wysiwyg","theme":"auto","width":900,"height":620,"caret":0,"file":null}
```

The `*` marks the class the host will instantiate by default: the first
`Audio Module Class`, which is what a DAW loads.

### Options

| Option | Effect |
|---|---|
| `--list` | List the factory's classes and stop, without instantiating anything |
| `--class <n>` | Instantiate class `n` instead of the first audio module (see note below) |
| `--no-editor` | Skip creating the editor view |
| `--type <text>` | Send `text` to the editor as key presses |
| `--state <file>` | Write the plugin's state blob to a file |

Only an `Audio Module Class` can be instantiated directly. Pointing `--class`
at a `Component Controller Class` fails, because a controller does not
implement `IComponent` — it is created automatically alongside its processor.
The tool says so rather than reporting the bare COM error.

### It works on any VST3, not just this one

Verified against commercial plugins:

```bash
cargo run -p notepad-host --bin vst3-host -- \
  "C:/Program Files/Common Files/VST3/Vital.vst3"
```

```text
vendor   Vital Audio
classes  2
  *[0] Vital  (Audio Module Class)
   [1] Vital  (Component Controller Class)

instantiating class 0
  shape        processor + separate controller
  audio buses  0 in, 1 out
  event buses  1 in, 0 out
  parameters   2983
      [0] Beats Per Minute
      [1] Chorus Filter Cutoff
      …
  editor       1926x1128, resizable: false
  state        178859 bytes
```

That output tells you a lot at a glance: no audio input and one MIDI input, so
it is an instrument; nearly three thousand automatable parameters; a fixed-size
editor; and a large state blob because Vital stores the whole preset in it.

### Where plugins live

| | |
|---|---|
| Windows | `C:\Program Files\Common Files\VST3\` |
| macOS | `/Library/Audio/Plug-Ins/VST3/` and `~/Library/Audio/Plug-Ins/VST3/` |
| Linux | `/usr/lib/vst3/` and `~/.vst3/` |

### Bundles versus bare files

A `.vst3` is either a shared library that happens to have that extension, or a
**bundle directory** with the binary buried inside:

```text
Surge XT Effects.vst3/          <- a directory
  Contents/
    x86_64-win/
      Surge XT Effects.vst3     <- the actual DLL
```

Both are in the wild — Vital ships the first form, Surge the second. Point the
host at the *outer* `.vst3` either way and it finds the binary itself. The
first line of output shows what it resolved to, so you can check.

---

## Part 2 — how a plugin gets loaded

Worth understanding, because a plugin that fails to load usually fails at one
of these steps.

**1. Load the binary and call the module entry point.** `InitDll` on Windows,
`BundleEntry` on macOS, `ModuleEntry` on Linux. Optional, but plugins that use
it will misbehave if it is skipped.

**2. Call `GetPluginFactory`.** The one exported symbol every VST3 must have.
It returns an `IPluginFactory`.

**3. Enumerate classes.** One binary can expose several. The categories that
matter are `Audio Module Class` (the processor — this is the plugin proper) and
`Component Controller Class` (its UI half).

**4. Create the processor** with `createInstance`, asking for `IComponent`, and
call `initialize`.

**5. Get a controller.** Two shapes exist, and a host must handle both:

- **Separate controller** — the usual case. Ask the processor for its
  controller's class ID with `getControllerClassId`, create *that* class asking
  for `IEditController`, and initialize it. The controller starts blank, so the
  host copies the processor's state across with `getState` →
  `setComponentState`, then connects the two through `IConnectionPoint` so they
  can talk. Surge and Vital both work this way.
- **Single-component effect** — one object implements both interfaces, so
  querying the processor for `IEditController` simply succeeds and there is no
  second instance, no state copy and no connection. This project's plugin is
  one of these, because its document lives in the GUI but is saved by the
  processor.

The host tries the single-component case first and falls back to the two-class
path, reporting which it found as `shape`.

**6. Create the editor** with `createView("editor")`, which returns an
`IPlugView`. You can query its size and whether it can be resized without ever
attaching it to a window — which is exactly what the test runner does.

**7. Tear down in reverse:** release the view, disconnect the pair, terminate
the controller, terminate the processor. The host does this in `Drop`, so it
happens even if your code panics.

---

## Part 3 — the library

Add it as a dependency:

```toml
[dependencies]
notepad-host = { path = "crates/notepad-host" }
```

### Loading and inspecting

```rust
use std::path::Path;
use notepad_host::Module;

let module = Module::load(Path::new("target/release/bundle/Notepad.vst3"))?;

println!("{}", module.vendor());
for i in 0..module.class_count() {
    if let Some((name, category)) = module.class_info(i) {
        println!("[{i}] {name} ({category})");
    }
}

let mut plugin = module.create_plugin()?;   // first audio module class
```

`create_plugin_at(index)` instantiates a specific class instead.

> **Keep the `Module` alive.** It owns the loaded library. Dropping it unloads
> the binary out from under any `Plugin` still using it. Rust's borrow checker
> will not catch this, because the plugin holds COM pointers rather than Rust
> references.

### Driving the editor

```rust
use notepad_core::{Key, Mods};

plugin.open_editor()?;

plugin.type_text("# Shopping\n* milk\neggs")?;      // \n is sent as Return
plugin.send_key(Key::Char('b'), Mods::CTRL)?;        // Ctrl+B
plugin.send_key(Key::Left, Mods::SHIFT)?;            // Shift+Left
```

`type_text` sends one key event per character through
`IPlugView::onKeyDown` — the same call a DAW makes. There is no back door into
the plugin.

### State

```rust
let saved = plugin.get_state()?;     // what a DAW writes into a project file
drop(plugin);

let mut reopened = module.create_plugin()?;
reopened.open_editor()?;
reopened.set_state(&saved)?;         // what a DAW does when reopening
```

This pair is the most useful thing the host offers, because it reproduces a
full session boundary in three lines.

### The window

```rust
let (w, h) = plugin.view_size()?;
plugin.resize(1024, 768)?;                    // as if the user dragged the edge
let accepted = plugin.check_size(100, 50)?;   // what the plugin will allow
let resizable = plugin.can_resize()?;
```

### Everything else

| Method | |
|---|---|
| `has_separate_controller()` | which of the two shapes the plugin uses |
| `parameter_count()` / `parameter_name(i)` | the automatable parameters |
| `bus_count(audio, input)` | e.g. `bus_count(true, false)` for audio outputs |

---

## Part 4 — writing tests against a plugin

`notepad-testrunner` is the worked example. It describes behaviour as a list of
steps in the vocabulary of someone *using* the editor, then executes them
against the real binary:

```rust
scenario(
    "bullet lists continue themselves",
    vec![
        Type("* milk\neggs\nbread"),
        ExpectSource("- milk\n- eggs\n- bread"),
        ExpectRendered("milk\neggs\nbread"),
    ],
),
```

`ExpectSource` reads the document back out of `getState` rather than from any
internal handle, so what it asserts on is exactly what the plugin would write
into a project file. `ReopenProject` saves the state, destroys the instance,
creates a fresh one and restores — a session boundary inside a test.

To test a *different* plugin this way, keep [`scenario.rs`](../crates/notepad-testrunner/src/scenario.rs)
and replace the assertions: the steps that read a document are the only ones
specific to this plugin. Keystroke delivery, resizing and state round-tripping
work against anything.

---

## Limitations

This is a test host, not a DAW. It deliberately does not:

- **Process audio.** `IAudioProcessor::process` is never called, so nothing is
  ever rendered. The host checks that a plugin *loads and behaves*, not that it
  sounds right.
- **Attach a real window.** `createView` is called, but `attached` is not, so
  no plugin GUI is ever drawn. This is why the notepad plugin routes keyboard
  input through `onKeyDown` when no window exists.
- **Implement `IComponentHandler`.** A plugin that tries to report a parameter
  change back to the host gets no handler. Nothing crashes; the notification is
  dropped.
- **Use a connection proxy.** A real DAW inserts one between processor and
  controller to marshal across threads. This host connects them directly, which
  is standard for simple hosts but means everything happens on one thread.
- **Scan directories.** You pass it one plugin path; it does not walk your VST3
  folders looking for plugins.

None of these prevent it from loading and exercising a plugin, but a plugin
that depends on one of them may behave differently here than in a DAW.
