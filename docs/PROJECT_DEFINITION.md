# A VST note-taking app

## Where this stands — resume here

**Open problem: DAWs listed the plugin as an instrument as well as an effect.**
Whether it is still true is unknown — the current build has never been loaded in
a DAW.

The plugin is a single-component effect again; the two-class split was tried and
reverted, since it was machinery a notepad should not need and was never shown
to be the cause. What it declares now, read back out of the built binary through
all three factory versions: one `Audio Module Class`, subcategory `Fx`, one
audio input bus, one audio output bus, no event buses, and it refuses any bus
arrangement with no audio input.

Ruled out as causes: the subcategory string, a missing `IPluginFactory3`,
DAW-side caching, and the enum values behind media type and bus direction.

Next step: install `dist/Notepad.vst3`, rescan a DAW, and report what it says.


High level goal: write a VST3 plugin that models a markdown editor, with as-you-write content conversion.

## Acceptance criteria

Original:

- **A1** — needs to be a universal VST3 plugin that'll load in anything, with both a windows .dll and mac .vst3 build target.
- **A2** — needs to store the user's notes as plugin state setting
- **A3** — needs to allow users to type markdown, with automatic conversion similar to tools like Typora
- **A4** — needs a way to toggle between wysiwyg and raw markdown
- **A5** — the plugin window must be resizable, which should also be stored as plugin state setting
- **A6** — an option to load .md files from disk
- **A7** — save the current file to disk as a standard "Save" operation
- **A8** — ...and as "Save as"

Added during development:

- **A9** — a theme selector: light, dark, or auto (following whatever the system uses). Must be part of plugin state, and must have tests.
- **A10** — the editor needs padding. Text running up to the window border is a bad experience.
- **A11** — audio must pass through the plugin untouched. It is a note-taking effect and has no business altering audio, but it sits in an insert slot: a plugin that fails to write its output buffer silences the track it is on.

## Work criteria

Original:

- **W1** — you will need to write a minimal VST3 host to allow plugin testing
- **W2** — you will need to come up with, and then use, a test runner that lets you load the plugin in the VST host and then perform test operations.
- **W3** — you will define a set of tests that reflect normal text writing that a human user would do using markdown, including but not limited to writing headings, paragraphs, lists including checkkbox lists, styled text, links, etc.
- **W4** — free to pick whichever language is best suited, however only AFTER determining whether this can be done using Rust. If it can, do it in Rust.

Added during development:

- **W5** — a document explaining how to use the VST host, and how to load the plugin — and really *any* VST3 plugin — in it.
- **W6** — the GUI must be verifiable by looking at it, not by asserting around it. Write something that can see the rendered output.

## Status

Verified means checked by a test or by inspecting real output, not by reading the code and concluding it ought to work.

| | Criterion | Status |
|---|---|---|
| A1 | Universal VST3, Windows + macOS targets | **Partly.** Loaded in a real DAW, which found a plugin-type bug now fixed; macOS still unverified |
| A2 | Notes in plugin state | Verified |
| A3 | Typora-style conversion | Verified |
| A4 | WYSIWYG / raw toggle | Verified |
| A5 | Resizable, size in state | **Partly.** State verified; host→window resize unverified |
| A6 | Load .md from disk | **Partly.** Logic verified; dialog untested |
| A7 | Save | **Partly.** Logic verified; dialog untested |
| A8 | Save As | **Partly.** Logic verified; dialog untested |
| A9 | Theme light/dark/auto, in state, tested | Verified |
| A10 | Editor padding | Verified |
| A11 | Audio passes through untouched | Verified, including mono |
| W1 | Minimal VST3 host | Verified |
| W2 | Test runner driving the plugin in the host | Verified |
| W3 | Tests reflecting real markdown writing | Verified |
| W4 | Rust, after establishing feasibility | Verified |
| W5 | Host documentation, any VST3 plugin | Verified |
| W6 | Something that can see the GUI | Verified |

## Registered as an instrument as well as an effect

Reported across twelve DAWs. Two real causes, both now fixed, found by reading
the SDK's own `again` example rather than by inspecting other people's binaries
— which was the wrong instinct and cost several rounds.

1. **The plugin volunteered to be a generator.** `setBusArrangements` rejected
   only `num_ins > 1`, so a host asking "can you run with *no* audio input?" —
   which is exactly how a host establishes that something can be a generator —
   got a yes. It now requires exactly one input and one output of equal width.
2. **Splitting into two classes was tried and reverted.** The SDK's `again`
   example declares an audio-effect class and a separate component-controller
   class, so the plugin was rebuilt that way — processor and controller sharing
   one `Editor` over `IConnectionPoint`. It was never shown to fix anything,
   and it is a lot of COM machinery for a notepad, so it was taken back out.
   The plugin is a single-component effect again.

Only the first of these is still in the code. Whether it resolves the report is
unknown until the current build is loaded in a DAW.

### Dead ends, recorded so they are not retried

- **The subcategory string was never wrong.** It read back as `"Fx"` from every
  factory version throughout.
- **A missing `IPluginFactory3` was a real gap and was fixed**, but it was not
  the cause: the misclassification survived it.
- **DAW-side caching was not the cause either.** A clean rescan with the
  entries deleted still produced the wrong classification.
- **Comparing against installed third-party binaries was the wrong method.** It
  cost several rounds and answered nothing that the SDK source did not answer
  directly. Read the SDK first.

One leftover from that period: FL's stale entry was renamed rather than
deleted, at `Installed/Generators/VST3/Notepad.{nfo,fst}.disabled-by-claude`.
Drop the suffix to restore it.

## Found by loading it in a DAW

**The plugin was registered as both an effect and an instrument**, and loading
it as an effect made the DAW complain. Reported by the user; none of the 37
scenarios or 73 tests caught it, because every one of them used this project's
own host, which was happy with what the plugin provided.

Cause: the factory only implemented `IPluginFactory` and `IPluginFactory2`.
Every shipping plugin implements `IPluginFactory3` as well — confirmed by
comparing against Surge XT Effects and Vital, both of which expose all three. A
host that asks for the newest factory and does not find it can fall back to the
v1 `getClassInfo`, whose `PClassInfo` struct **has no `subCategories` field at
all**, leaving nothing to distinguish an effect from an instrument.

`IPluginFactory3::getClassInfoUnicode` is now implemented, the subcategory is a
single shared constant used by all three versions, and a scenario asserts that
all three factories exist and agree that this is an `Fx` and not an
`Instrument`. That scenario was confirmed to fail with factory 3 removed.

## Found by the third audit

- **`process_audio_without_input` ignored its `frames` argument.** The helper
  took a frame count and then used a hardcoded 64. The existing test passed
  only because it happened to ask for 64. The scenario now asks for 97 and
  checks the returned length.
- **No randomised testing existed at all.** Every test was a case someone
  thought of, which only finds bugs someone imagined. There are now five
  property tests covering random key sequences, random direct API calls
  (`set_caret`, `select`, `insert_str`, `toggle_checkbox` with arbitrary and
  mid-character offsets), random state bytes, and random documents. They assert
  that the caret and anchor stay on char boundaries and inside the buffer, that
  spans tile each line exactly, and that layout never panics. The core survived
  all of it — which is a result, not a formality.

## Found by the second audit

Six more, all in code that no test reached:

- **With no input bus, the output was left as garbage.** `process` returned
  early whenever `numInputs < 1` — which a host produces by deactivating the
  input bus — without writing the output buffer at all. Whatever the host had
  in that memory went to the track as noise. Outputs with no matching input are
  now silenced. Confirmed to fail against the old code.
- **Slices were built from possibly-null pointers.** `slice::from_raw_parts` is
  undefined behaviour on a null pointer even with a length of zero, and channel
  buffers are null when a bus is inactive. All of `process` is now null-checked.
- **`silenceFlags` was never set.** The plugin now reports which output channels
  ended up silent, and propagates the input's flags for those it copies.
- **Attaching twice stranded a window.** `IPlugView::attached` overwrote the
  stored handle without closing the previous one, leaving an orphaned window
  still holding the editor.
- **A shortcut could type its own letter.** `Event::Text` carries no modifiers,
  and some platforms emit both a text and a key event for Ctrl+B. Text is now
  ignored while a command modifier is held.
- **Window size had no upper bound.** `set_size` clamped only the minimum, so a
  corrupt project carrying `i32::MAX` would have been handed to the host as a
  real window size. Now clamped to `MAX_WIDTH`/`MAX_HEIGHT`.

Checked and found already correct: caret offsets from state are clamped to a
char boundary (a caret inside a multi-byte character cannot panic), non-JSON
state is kept as note text, and empty state opens an empty document. All three
now have scenarios rather than resting on a reading of the code.

The audit also caught the README claiming 52 unit tests and 23 scenarios when
there were 68 and 37. The counts are gone rather than corrected — they only rot.

## Found by the first audit

Two real defects, neither of which any existing test would have caught:

- **The window did not follow the host's resize.** `IPlugView::onSize` recorded
  the new size in the editor but never resized the child window, so in a DAW the
  frame would have resized while the editor inside it stayed put. The GUI now
  reconciles the two directions — window resized versus stored size changed —
  via `ViewportCommand::InnerSize`. A5 was previously being reported as met on
  the strength of a test that only checked the stored number.
- **Bus arrangements were reported dishonestly.** The plugin accepted a mono
  arrangement and then advertised stereo from both `getBusArrangement` and
  `getBusInfo`. It now stores what was negotiated and reports it, refuses
  mismatched input/output widths, and silences any output channel with no
  matching input. Covered by a scenario that was confirmed to fail against the
  old behaviour.

## Fonts

Nothing is bundled. The plugin starts with the system UI and monospace faces and
loads a font for any other script the moment text needs it, from the machine it
is running on. This removed 1.4 MB and widened coverage at the same time:
Devanagari, Thai and CJK render now and never did before.

Measured, on Windows:

| approach | size |
|---|---:|
| egui's embedded font set | 5,793,792 |
| `egui-system-fonts` (pulls in `fontdb`) | 5,915,648 |
| OS fonts, loaded on demand | **4,454,400** |

The middle row is the trap: a font database costs more code than the fonts it
replaces, and it picks by locale rather than by what was typed.

## Known gaps

Things that are not done, or are done but unproven. None of these are hidden
behind a passing test.

1. **macOS is unverified.** `xtask` emits the correct bundle layout and the code
   has no Windows-only paths, but it has never been compiled or run on a Mac.
2. **Barely exercised in a real DAW.** It has now been loaded in one, which
   immediately found a bug three audits had missed. Every automated test still
   drives the plugin through this project's own host, which deliberately does
   not attach a window, implement `IComponentHandler`, or use a connection
   proxy — so it is forgiving in exactly the ways a DAW is not.
3. **The host→window resize path is unverified.** `onSize` now updates the
   stored size and the GUI follows it via `ViewportCommand::InnerSize`, but
   proving it needs a real host window to drag.
4. **`IPlugFrame::resizeView` is not called.** If the editor ever wants to
   resize *itself* — restoring a project whose stored size differs while the
   window is open — a well-behaved plugin asks the host first. We keep the
   frame pointer but never call it, so in that case our window would resize
   without the host's frame following.
5. **File dialogs are untested end to end.** `open_path`/`save`/`save_as` have
   unit tests, but nothing drives the native dialog, so the wiring between the
   keyboard shortcut and the dialog is only verified by hand.
6. **The GUI keyboard path is untested.** Scenarios drive `onKeyDown`, which is
   the only input path when no window exists. With a window, egui handles keys
   natively and that translation layer has no automated coverage.
7. **No HiDPI negotiation.** `IPlugViewContentScaleSupport` is not implemented,
   so a host on a scaled display cannot tell the plugin its scale factor.
8. **Bold is a colour, not a typeface.** egui ships no bold family. Fixing it
   properly means embedding a font.
9. **No mouse selection.** Clicking places the caret; selection is keyboard-only.
10. **The host is never told the notes changed.** VST3 has no "state is dirty"
    signal — the usual trick is a hidden parameter bumped on every edit. Without
    one, a DAW still saves the notes (it always asks for state when the project
    is saved) but may not consider the project modified, so it might not prompt
    to save on close. Adding a dummy parameter has its own cost: it shows up in
    automation lanes.
