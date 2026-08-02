//! The scenario engine.
//!
//! A scenario is a list of [`Step`]s executed against a plugin instance that
//! was loaded from a real binary and is driven through the real VST3
//! interfaces. Steps are deliberately written in the vocabulary of a person
//! using the editor — "type this", "press Enter", "resize the window",
//! "reopen the project" — so the tests read as descriptions of user behaviour
//! rather than of internal calls.

use notepad_core::{Editor, Key, Mods, PluginState};
use notepad_host::{HostError, Module, Plugin};

#[derive(Clone, Debug)]
pub enum Step {
    /// Type text. `\n` is sent as the Return key.
    Type(&'static str),
    /// Press a single key with modifiers.
    Press(Key, Mods),
    /// The markdown source must be exactly this.
    ExpectSource(&'static str),
    /// The document as a reader sees it, markers stripped.
    ExpectRendered(&'static str),
    /// The caret must sit at this byte offset.
    ExpectCaret(usize),
    /// The view must report this size.
    ExpectSize(i32, i32),
    /// Drag the window to a new size.
    Resize(i32, i32),
    /// Propose a size and check what the plugin clamps it to.
    ExpectClampedSize {
        proposed: (i32, i32),
        accepted: (i32, i32),
    },
    /// The persisted view mode, "wysiwyg" or "raw".
    ExpectMode(&'static str),
    /// The persisted theme: "light", "dark" or "auto".
    ExpectTheme(&'static str),
    /// The persisted file path.
    ExpectFile(Option<&'static str>),
    /// Save the project, close the plugin, reopen it and restore the state —
    /// exactly what a DAW does across a session.
    ReopenProject,
    /// The plugin must present itself as an effect, consistently, everywhere a
    /// host might look.
    ///
    /// A DAW decides "effect or instrument" from factory metadata. If the
    /// versions disagree — or the newest one a host wants is simply missing —
    /// the plugin can end up listed as both and refuse to load properly as
    /// either.
    ExpectEffectNotInstrument,
    /// Push raw bytes in through `IComponent::setState`, as a host would when
    /// restoring a project — including a corrupt or hand-edited one.
    LoadRawState(&'static [u8]),
    /// With no input bus, the output must come back silent rather than
    /// carrying whatever was already in the host's buffer.
    ExpectSilenceWithNoInput,
    /// Negotiate a mono track and require the plugin to report mono back.
    ///
    /// Accepting an arrangement and then advertising a different one is a way
    /// to be subtly broken in a host: the plugin says yes to mono and then
    /// claims two channels.
    ExpectMonoIsHonoured,
    /// Push a block of audio through and require it to come out untouched.
    ///
    /// A notepad has no business altering audio, but it is sitting in an insert
    /// slot: if `process` fails to write the output buffer, the track it is on
    /// goes silent. This is the assertion that the plugin is inaudible rather
    /// than merely quiet.
    ExpectAudioPassThrough,
}

pub struct Scenario {
    pub name: &'static str,
    pub steps: Vec<Step>,
}

pub fn scenario(name: &'static str, steps: Vec<Step>) -> Scenario {
    Scenario { name, steps }
}

/// Why a scenario stopped.
#[derive(Debug)]
pub struct Failure {
    pub step_index: usize,
    pub step: String,
    pub message: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "step {} ({}): {}", self.step_index + 1, self.step, self.message)
    }
}

/// Read the plugin's state back and rebuild the document it describes.
///
/// This goes through `IComponent::getState`, so it asserts on what the plugin
/// would actually write into a project file, not on some internal handle.
fn snapshot(plugin: &Plugin) -> Result<(PluginState, Editor), HostError> {
    let bytes = plugin.get_state()?;
    let state = PluginState::from_bytes(&bytes);
    let mut editor = Editor::new();
    editor.load_state(&state);
    Ok((state, editor))
}

fn diff(label: &str, want: &str, got: &str) -> String {
    format!("{label}\n  expected: {want:?}\n  actual:   {got:?}")
}

pub fn run(module: &Module, scenario: &Scenario) -> Result<(), Failure> {
    let fail = |i: usize, step: &Step, message: String| Failure {
        step_index: i,
        step: format!("{step:?}"),
        message,
    };

    let mut plugin = module.create_plugin().map_err(|e| Failure {
        step_index: 0,
        step: "<create plugin>".into(),
        message: e.to_string(),
    })?;
    plugin.open_editor().map_err(|e| Failure {
        step_index: 0,
        step: "<open editor>".into(),
        message: e.to_string(),
    })?;

    for (i, step) in scenario.steps.iter().enumerate() {
        let err = |m: String| fail(i, step, m);
        match step {
            Step::Type(text) => {
                plugin.type_text(text).map_err(|e| err(e.to_string()))?;
            }
            Step::Press(key, mods) => {
                plugin.send_key(*key, *mods).map_err(|e| err(e.to_string()))?;
            }
            Step::ExpectSource(want) => {
                let (_, editor) = snapshot(&plugin).map_err(|e| err(e.to_string()))?;
                if editor.text() != *want {
                    return Err(err(diff("markdown source", want, editor.text())));
                }
            }
            Step::ExpectRendered(want) => {
                let (_, editor) = snapshot(&plugin).map_err(|e| err(e.to_string()))?;
                let got = editor.rendered_text();
                if got != *want {
                    return Err(err(diff("rendered text", want, &got)));
                }
            }
            Step::ExpectCaret(want) => {
                let (state, _) = snapshot(&plugin).map_err(|e| err(e.to_string()))?;
                if state.caret != *want {
                    return Err(err(format!(
                        "caret\n  expected: {want}\n  actual:   {}",
                        state.caret
                    )));
                }
            }
            Step::ExpectSize(w, h) => {
                let got = plugin.view_size().map_err(|e| err(e.to_string()))?;
                if got != (*w, *h) {
                    return Err(err(format!(
                        "view size\n  expected: {w}x{h}\n  actual:   {}x{}",
                        got.0, got.1
                    )));
                }
            }
            Step::Resize(w, h) => {
                plugin.resize(*w, *h).map_err(|e| err(e.to_string()))?;
            }
            Step::ExpectClampedSize { proposed, accepted } => {
                let got = plugin
                    .check_size(proposed.0, proposed.1)
                    .map_err(|e| err(e.to_string()))?;
                if got != *accepted {
                    return Err(err(format!(
                        "clamped size\n  expected: {}x{}\n  actual:   {}x{}",
                        accepted.0, accepted.1, got.0, got.1
                    )));
                }
            }
            Step::ExpectMode(want) => {
                let (state, _) = snapshot(&plugin).map_err(|e| err(e.to_string()))?;
                if state.mode != *want {
                    return Err(err(diff("view mode", want, &state.mode)));
                }
            }
            Step::ExpectTheme(want) => {
                let (state, _) = snapshot(&plugin).map_err(|e| err(e.to_string()))?;
                if state.theme != *want {
                    return Err(err(diff("theme", want, &state.theme)));
                }
            }
            Step::ExpectFile(want) => {
                let (state, _) = snapshot(&plugin).map_err(|e| err(e.to_string()))?;
                let got = state.file.clone();
                if got.as_deref() != *want {
                    return Err(err(format!(
                        "file path\n  expected: {want:?}\n  actual:   {got:?}"
                    )));
                }
            }
            Step::ExpectEffectNotInstrument => {
                let (f1, f2, f3) = module.factory_versions();
                if !(f1 && f2 && f3) {
                    return Err(err(format!(
                        "a host may not find the type at all\n  \
                         IPluginFactory={f1} IPluginFactory2={f2} IPluginFactory3={f3}\n  \
                         all three are needed; the v1 info has no subCategories field"
                    )));
                }

                let from2 = module.class_info2(0);
                let from3 = module.class_info_unicode(0);
                let (Some(from2), Some(from3)) = (from2, from3) else {
                    return Err(err("a class info query returned nothing".into()));
                };

                if from2.sub_categories != from3.sub_categories {
                    return Err(err(format!(
                        "the factory versions disagree about the plugin type\n  \
                         getClassInfo2:      {:?}\n  getClassInfoUnicode: {:?}",
                        from2.sub_categories, from3.sub_categories
                    )));
                }

                for (source, info) in [("getClassInfo2", &from2), ("getClassInfoUnicode", &from3)] {
                    if !info.sub_categories.split('|').any(|c| c == "Fx") {
                        return Err(err(format!(
                            "{source} does not declare this an effect\n  \
                             expected \"Fx\" among {:?}",
                            info.sub_categories
                        )));
                    }
                    if info.sub_categories.contains("Instrument") {
                        return Err(err(format!(
                            "{source} declares this an instrument: {:?}",
                            info.sub_categories
                        )));
                    }
                    if info.category != "Audio Module Class" {
                        return Err(err(format!(
                            "{source} category\n  expected: \"Audio Module Class\"\n  \
                             actual:   {:?}",
                            info.category
                        )));
                    }
                }

                // The behavioural half of "I am an effect": a host asks whether
                // the plugin can run with no audio input, and a generator says
                // yes. An effect must refuse, or it gets filed under both.
                let no_input = plugin
                    .negotiate(None, Some(Plugin::STEREO))
                    .map_err(|e| err(e.to_string()))?;
                if no_input {
                    return Err(err(
                        "the plugin accepted a configuration with no audio input, \
                         which is how a host decides something is a generator"
                            .into(),
                    ));
                }
                // ...and it must still accept the ordinary effect layout.
                let normal = plugin
                    .negotiate(Some(Plugin::STEREO), Some(Plugin::STEREO))
                    .map_err(|e| err(e.to_string()))?;
                if !normal {
                    return Err(err(
                        "the plugin refused a plain stereo-in/stereo-out effect layout".into(),
                    ));
                }

                // An effect that advertises MIDI inputs invites a host to file
                // it with the instruments.
                let events_in = plugin.bus_count(false, true);
                if events_in != 0 {
                    return Err(err(format!(
                        "an effect should have no event input buses, found {events_in}"
                    )));
                }
                // ...and it must have an audio input, or it looks like a generator.
                let audio_in = plugin.bus_count(true, true);
                if audio_in < 1 {
                    return Err(err(format!(
                        "an effect needs an audio input bus, found {audio_in}"
                    )));
                }
            }
            Step::LoadRawState(bytes) => {
                plugin.set_state(bytes).map_err(|e| err(e.to_string()))?;
            }
            Step::ExpectSilenceWithNoInput => {
                // A recognisable non-zero value: if the plugin never writes the
                // output, this is exactly what comes back.
                const GARBAGE: f32 = 0.75;
                // A frame count that is not the default, so a host helper that
                // quietly ignores it shows up here.
                const FRAMES: usize = 97;
                let output = plugin
                    .process_audio_without_input(2, FRAMES, GARBAGE)
                    .map_err(|e| err(e.to_string()))?;

                for (channel, samples) in output.iter().enumerate() {
                    if samples.len() != FRAMES {
                        return Err(err(format!(
                            "channel {channel} length\n  expected: {FRAMES}\n  actual:   {}",
                            samples.len()
                        )));
                    }
                }

                for (channel, samples) in output.iter().enumerate() {
                    if let Some(bad) = samples.iter().find(|s| **s != 0.0) {
                        let note = if *bad == GARBAGE {
                            " — the plugin left the host's buffer untouched"
                        } else {
                            ""
                        };
                        return Err(err(format!(
                            "channel {channel} should be silent with no input{note}\n  \
                             expected: 0.0\n  actual:   {bad}"
                        )));
                    }
                }
            }
            Step::ExpectMonoIsHonoured => {
                let accepted = plugin
                    .set_bus_arrangement(Plugin::MONO)
                    .map_err(|e| err(e.to_string()))?;
                if !accepted {
                    return Err(err("the plugin refused a mono arrangement".into()));
                }
                for input in [true, false] {
                    let side = if input { "input" } else { "output" };
                    let arrangement = plugin
                        .bus_arrangement(input)
                        .map_err(|e| err(e.to_string()))?;
                    if arrangement != Plugin::MONO {
                        return Err(err(format!(
                            "{side} arrangement after agreeing to mono\n  \
                             expected: {:#x}\n  actual:   {arrangement:#x}",
                            Plugin::MONO
                        )));
                    }
                    let channels = plugin.bus_channels(input).map_err(|e| err(e.to_string()))?;
                    if channels != 1 {
                        return Err(err(format!(
                            "{side} channel count after agreeing to mono\n  \
                             expected: 1\n  actual:   {channels}"
                        )));
                    }
                }
                // Put it back so later steps see the usual stereo plugin.
                plugin
                    .set_bus_arrangement(Plugin::STEREO)
                    .map_err(|e| err(e.to_string()))?;
            }
            Step::ExpectAudioPassThrough => {
                // Two distinguishable ramps, so a channel swap or a dropped
                // channel fails as loudly as silence does.
                let input: Vec<Vec<f32>> = vec![
                    (0..64).map(|i| i as f32 / 64.0).collect(),
                    (0..64).map(|i| -(i as f32) / 64.0).collect(),
                ];
                let output = plugin
                    .process_audio(&input)
                    .map_err(|e| err(e.to_string()))?;

                if output.len() != input.len() {
                    return Err(err(format!(
                        "channel count\n  expected: {}\n  actual:   {}",
                        input.len(),
                        output.len()
                    )));
                }
                for (channel, (want, got)) in input.iter().zip(output.iter()).enumerate() {
                    if want != got {
                        let silent = got.iter().all(|s| *s == 0.0);
                        let note = if silent {
                            " (output is silent — the plugin did not write its output buffer)"
                        } else {
                            ""
                        };
                        return Err(err(format!(
                            "audio on channel {channel} was altered{note}\n  \
                             expected: {:?}…\n  actual:   {:?}…",
                            &want[..4.min(want.len())],
                            &got[..4.min(got.len())]
                        )));
                    }
                }
            }
            Step::ReopenProject => {
                let saved = plugin.get_state().map_err(|e| err(e.to_string()))?;
                drop(plugin);
                let mut fresh = module.create_plugin().map_err(|e| err(e.to_string()))?;
                fresh.open_editor().map_err(|e| err(e.to_string()))?;
                fresh.set_state(&saved).map_err(|e| err(e.to_string()))?;
                plugin = fresh;
            }
        }
    }

    Ok(())
}
