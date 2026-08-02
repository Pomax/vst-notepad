# vst-notepad

A VST3 markdown notepad. It loads on any track in any DAW, keeps your notes in
the project file, and converts markdown as you type it — the way Typora does.

Written in Rust. (The plan asked whether Rust could do this before considering
anything else: it can. The [`vst3`](https://crates.io/crates/vst3) crate ships
complete VST3 bindings with no C++ SDK dependency, and supports both
*implementing* COM interfaces — the plugin — and *calling* them — the test host.)

## Layout

| Crate | What it is |
|---|---|
| `crates/notepad-core` | The editor: markdown parsing, as-you-type conversion, undo, plugin state, file I/O. No UI, no plugin dependencies. |
| `crates/notepad-plugin` | The VST3 plugin, plus the egui GUI. |
| `crates/notepad-host` | A minimal VST3 host — enough of one to load a plugin, open its view, send keys and read state back. |
| `crates/notepad-testrunner` | Runs scripted editing scenarios against the real plugin binary through that host. |
| `xtask` | Assembles the `.vst3` bundle. |

## Building

```bash
cargo dist
```

That leaves the installable plugin — one file, nothing else — in `dist/`:

```text
dist/
  Notepad.vst3        # the plugin binary, ready to copy into your VST3 folder
```

`dist/` is wiped on every build, so it only ever holds the current binary.

A release build cleans up after itself: it deletes the debug tree (superseded)
and the release intermediates — `deps/`, `build/`, `incremental/`,
`.fingerprint/` — keeping only the finished artefacts. That takes the build
directory from roughly 500 MB to under 6 MB. The trade is that the next release
build is a full one rather than incremental.
(`cargo dist` is an alias for `cargo run -p xtask -- bundle --release`; plain
`cargo build` cannot do this, because cargo has no post-build hook that can see
the finished cdylib. `cargo dist-debug` builds the debug profile instead.)

On Windows and Linux that single file is all you need — a VST3 may be a bare
shared library with a `.vst3` extension, which is how Vital ships. **macOS is
the exception:** there a plugin must be a bundle directory, so use the bundle
below rather than the file in `dist/`.

The same command also assembles the full bundle under `target/<profile>/bundle`,
which is the form macOS requires:

```text
Notepad.vst3/
  Contents/
    x86_64-win/Notepad.vst3     # Windows
    MacOS/Notepad               # macOS
    Info.plist                  # macOS
    PkgInfo                     # macOS
```

Install by copying into your VST3 folder — on Windows, `dist/Notepad.vst3`; on
macOS, the bundle directory:

- Windows: `C:\Program Files\Common Files\VST3\`
- macOS: `~/Library/Audio/Plug-Ins/VST3/`

Cross-compiling for macOS is supported by the same task:

```bash
cargo run -p xtask -- bundle --release --target aarch64-apple-darwin
```

## Testing

```bash
cargo run -p xtask -- test
```

That builds the plugin, then runs the unit tests and the scenario suite in that
order. The order is the point: the scenario runner loads the plugin binary at
runtime rather than linking it, so cargo has no idea the two are related and
will happily run the suite against a stale `.dll`. (The runner refuses to run
if it spots one, because that actually happened here — a deliberately broken
`process` sailed through a green suite.)

The two halves can still be run alone:

```bash
cargo test --workspace
cargo build -p notepad-plugin && cargo run -p notepad-testrunner
```

The scenario runner loads the built plugin binary and drives it through the
same interfaces a DAW uses — `GetPluginFactory`, `createInstance`,
`createView`, `IPlugView::onKeyDown`, `IComponent::get/setState`. There is no
test-only backdoor: a scenario types `* milk`, Enter, `eggs` one keystroke at a
time and then reads the document back out of the state blob the plugin would
write into a project file.

To look at the editor without a DAW:

```bash
cargo run -p notepad-plugin --example preview
```

To render the GUI headlessly to PNGs — no window, no human needed:

```bash
cargo run -p notepad-plugin --features snapshots --example snapshot
```

The `snapshots` feature is opt-in because the headless renderer pulls in the
whole wgpu/naga stack: several gigabytes of build output for a plugin that
ships as 5 MB. `cargo run -p xtask -- test --snapshots` runs the pixel tests
along with everything else.

That writes `target/snapshots/{light,dark}.png` through a real rasteriser and
reports the background and text brightness of each. The same machinery backs
[`tests/theme_rendering.rs`](crates/notepad-plugin/tests/theme_rendering.rs),
which asserts on actual pixels: that light really is light, that the text
contrasts with it, and that the two themes do not look alike. Those tests exist
because the light theme once passed every non-visual check while the window
stayed black — nothing painted the background, so egui's dark-on-light text was
drawn onto a black clear colour. Only pixels catch that.

The headless renderer proves the *drawing code* is right. To prove the *real
window* is — the path through baseview and OpenGL, where the background is the
renderer's clear colour rather than anything egui draws — screenshot it
(Windows only):

```bash
powershell -File tools/capture-window.ps1 -Theme light -Out target/window-light.png
```

It launches the preview, finds the editor window by title, grabs its pixels off
the screen and closes it. Finding the window by title matters: the process also
owns a console window, and `MainWindowHandle` names whichever appeared first.

The host is also a standalone tool that loads **any** VST3 plugin, not just
this one:

```bash
cargo run -p notepad-host --bin vst3-host -- target/release/bundle/Notepad.vst3
```

See [docs/HOSTING.md](docs/HOSTING.md) for how to use it and how a VST3 plugin
is loaded.

## Keys

| | |
|---|---|
| `Ctrl+B` / `Ctrl+I` / `Ctrl+D` | bold / italic / strikethrough (wraps the selection) |
| `Ctrl+E` | inline code |
| `Ctrl+K` | turn the selection into a link, caret left in the `()` |
| `Ctrl+/` | toggle WYSIWYG ↔ raw markdown |
| `Ctrl+T` | cycle the theme: auto → light → dark |
| `Ctrl+Z` / `Ctrl+Shift+Z` | undo / redo |
| `Ctrl+O` / `Ctrl+S` / `Ctrl+Shift+S` | Open / Save / Save As |
| `Tab` / `Shift+Tab` | indent / outdent a list item |

Typing converts as you go: `* ` becomes a `- ` bullet, `-[] ` becomes a
`- [ ] ` task box, Enter continues lists and blockquotes, Enter on an empty
list item ends the list, numbered lists renumber themselves, and an opening
code fence closes itself.

## Design notes

**Markdown source is the source of truth.** The parser keeps marker spans
rather than stripping them, tagging each visible or hidden. Hiding them yields
WYSIWYG; revealing the ones on the caret's line gives Typora's behaviour where
punctuation appears on the line you are editing. Raw mode just makes every
marker visible. One code path serves the GUI and the tests, and the text you
save is exactly the text you typed.

**The plugin is a single-component effect** — one COM object implementing both
`IComponent` and `IEditController`. The document is edited in the GUI but
persisted by the processor, and splitting them into two objects would mean
marshalling the whole note text through parameters or `IMessage` on every
keystroke.

**State** is JSON: notes, view mode, theme, window size, caret, and file path.
Anything that fails to parse as JSON is kept as raw note text rather than
discarded, so a malformed blob loses formatting but never the user's words.
Unknown fields are ignored and missing ones default, so a project saved before
a setting existed still opens.

**Theme** is `light`, `dark` or `auto`. `auto` is stored *as* `auto` rather
than as whatever it resolved to, so a project moved between a light machine and
a dark one follows each. Resolving it is the GUI's job: `notepad-core` has no
OS dependency and instead exposes `Theme::is_dark(system_dark)`, which keeps the
decision table unit-testable without an operating system in the loop. The
system setting is polled every two seconds rather than every frame, since
reading it hits the registry on Windows and a desktop portal on Linux.

## Known limitations

- **The macOS build is scripted but unverified.** `xtask` emits the correct
  bundle layout and the code has no Windows-only paths, but it has only been
  built and tested on Windows.
- **Bold is drawn as a stronger colour, not a bold typeface.** egui ships no
  bold font family; this is the same approach egui uses for its own emphasis.
  Embedding a bold font would fix it properly.
- **No mouse text selection.** Clicking places the caret; selection is
  keyboard-only (`Shift`+motion, `Ctrl+A`).
- **Input ownership.** When the host has attached a window, the GUI receives
  key events natively and `onKeyDown` returns `kResultFalse` — handling both
  would type every character twice. Without a window, `onKeyDown` is the only
  input path, which is how the test host drives the plugin.
