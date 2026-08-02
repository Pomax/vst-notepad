//! The VST3 plugin.
//!
//! # Shape of the plugin
//!
//! A single-component effect: one class implementing [`IComponent`],
//! [`IAudioProcessor`] and [`IEditController`]. VST3 permits this, and for a
//! notepad it is the honest shape — the document is edited in the GUI and saved
//! by the processor, and one object means one document with no marshalling.
//!
//! # Being an effect, not an instrument
//!
//! Per the VST3 documentation, the subcategory is what a host reads: `"Fx"` is
//! an effect, `"Instrument"` an instrument, and `"Fx|Instrument"` is what a
//! plugin declares when it wants to be loadable as both. This declares `"Fx"`
//! from every factory version, has an audio input bus and no event bus, and
//! refuses any bus arrangement with no audio input — that last one is how a
//! plugin would otherwise volunteer to be usable as a generator.
//!
//! Audio is passed through untouched.
//!
//! # Testability
//!
//! Keyboard input arrives through `IPlugView::onKeyDown`, which is a normal
//! part of the VST3 ABI. That means the test host drives the plugin through the
//! *real* interface a DAW would use — no test-only backdoor — and it works
//! whether or not a window was ever created.

#![allow(non_snake_case)]

pub mod files;
pub mod fonts;
pub mod gui;
pub mod keys;

use std::cell::RefCell;
use std::ffi::{c_char, c_void, CString};
use std::sync::{Arc, Mutex};

use notepad_core::{Command, Editor, DEFAULT_HEIGHT, DEFAULT_WIDTH, MIN_HEIGHT, MIN_WIDTH};
use vst3::{uid, Class, ComRef, ComWrapper, Steinberg::Vst::*, Steinberg::*};

/// The document, shared between the processor, the controller and the view.
pub type Shared = Arc<Mutex<Editor>>;

const PLUGIN_NAME: &str = "Notepad";
const VENDOR: &str = "vst-notepad";
const VERSION: &str = "0.1.0";
const SDK_VERSION: &str = "VST 3.7.0";
/// This is an effect, not an instrument. It must be reported identically from
/// every factory version, or a host can end up listing the plugin as both.
const SUB_CATEGORY: &str = "Fx";

// ---------------------------------------------------------------------------
// String helpers (VST3 uses fixed-size C and UTF-16 buffers)
// ---------------------------------------------------------------------------

fn copy_cstring(src: &str, dst: &mut [c_char]) {
    let c_string = CString::new(src).unwrap_or_default();
    let bytes = c_string.as_bytes_with_nul();
    for (s, d) in bytes.iter().zip(dst.iter_mut()) {
        *d = *s as c_char;
    }
    if bytes.len() > dst.len() {
        if let Some(last) = dst.last_mut() {
            *last = 0;
        }
    }
}

fn copy_wstring(src: &str, dst: &mut [TChar]) {
    let mut len = 0;
    for (s, d) in src.encode_utf16().zip(dst.iter_mut()) {
        *d = s as TChar;
        len += 1;
    }
    if len < dst.len() {
        dst[len] = 0;
    } else if let Some(last) = dst.last_mut() {
        *last = 0;
    }
}

/// Compare a host-supplied `FIDString` against one of the SDK's constants.
/// Both are NUL-terminated C strings rather than Rust types.
unsafe fn fid_eq(a: FIDString, b: FIDString) -> bool {
    if a.is_null() || b.is_null() {
        return false;
    }
    std::ffi::CStr::from_ptr(a) == std::ffi::CStr::from_ptr(b)
}

// ---------------------------------------------------------------------------
// IBStream helpers
// ---------------------------------------------------------------------------

/// Drain an `IBStream` to a byte vector.
///
/// Hosts are free to return short reads, so this loops until the stream stops
/// producing bytes rather than assuming one read is enough.
unsafe fn read_stream(stream: *mut IBStream) -> Option<Vec<u8>> {
    let stream = ComRef::from_raw(stream)?;
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let mut got: int32 = 0;
        let res = stream.read(
            chunk.as_mut_ptr() as *mut c_void,
            chunk.len() as int32,
            &mut got,
        );
        if res != kResultOk && res != kResultTrue {
            break;
        }
        if got <= 0 {
            break;
        }
        out.extend_from_slice(&chunk[..got as usize]);
    }
    Some(out)
}

/// Write every byte to an `IBStream`, tolerating short writes.
unsafe fn write_stream(stream: *mut IBStream, bytes: &[u8]) -> bool {
    let Some(stream) = ComRef::from_raw(stream) else {
        return false;
    };
    let mut written_total = 0usize;
    while written_total < bytes.len() {
        let mut written: int32 = 0;
        let remaining = &bytes[written_total..];
        let res = stream.write(
            remaining.as_ptr() as *mut c_void,
            remaining.len() as int32,
            &mut written,
        );
        if res != kResultOk && res != kResultTrue {
            return false;
        }
        if written <= 0 {
            return false;
        }
        written_total += written as usize;
    }
    true
}

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

pub struct Notepad {
    editor: Shared,
    /// The speaker arrangement the host negotiated, so `getBusArrangement` and
    /// `getBusInfo` report what was actually agreed rather than a fixed guess.
    arrangement: Mutex<SpeakerArrangement>,
}

/// Channels in an arrangement — it is a bitmask, one bit per speaker.
fn channel_count(arrangement: SpeakerArrangement) -> i32 {
    arrangement.count_ones() as i32
}

/// Copy every input channel to its output, silencing outputs that have none.
///
/// Returns the silence mask for the outputs. Written defensively because it
/// runs on the audio thread inside someone else's process: any of these
/// pointers can be null when a bus is inactive, and a slice built from a null
/// pointer is undefined behaviour even when its length is zero.
///
/// # Safety
/// `ins`/`outs` must each either be null or point to at least `in_channels` /
/// `out_channels` channel pointers, each valid for `num_samples` values.
unsafe fn pass_through<T: Copy>(
    ins: *mut *mut T,
    in_channels: usize,
    outs: *mut *mut T,
    out_channels: usize,
    num_samples: usize,
    in_silence: u64,
) -> u64 {
    if outs.is_null() || out_channels == 0 {
        return 0;
    }
    let outs = std::slice::from_raw_parts(outs, out_channels);
    let ins: &[*mut T] = if ins.is_null() || in_channels == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ins, in_channels)
    };

    let mut silence = 0u64;
    for (channel, out) in outs.iter().enumerate() {
        if out.is_null() {
            continue;
        }
        match ins.get(channel) {
            Some(input) if !input.is_null() => {
                // Hosts may process in place, handing back the same buffer.
                if *input != *out {
                    std::ptr::copy_nonoverlapping(*input, *out, num_samples);
                }
                if channel < 64 && (in_silence >> channel) & 1 == 1 {
                    silence |= 1 << channel;
                }
            }
            // No input for this channel: silence it rather than leave whatever
            // the host's buffer happened to contain.
            _ => {
                std::ptr::write_bytes(*out, 0, num_samples);
                if channel < 64 {
                    silence |= 1 << channel;
                }
            }
        }
    }
    silence
}

impl Notepad {
    pub const CID: TUID = uid(0x4E4F5445, 0x50414430, 0x6D64ED17, 0x0A1B2C3D);

    pub fn new() -> Notepad {
        Notepad {
            editor: Arc::new(Mutex::new(Editor::new())),
            arrangement: Mutex::new(SpeakerArr::kStereo),
        }
    }

    fn arrangement(&self) -> SpeakerArrangement {
        self.arrangement
            .lock()
            .map(|a| *a)
            .unwrap_or(SpeakerArr::kStereo)
    }
}

impl Default for Notepad {
    fn default() -> Self {
        Notepad::new()
    }
}

impl Class for Notepad {
    type Interfaces = (
        IComponent,
        IAudioProcessor,
        IEditController,
        IProcessContextRequirements,
    );
}

impl IPluginBaseTrait for Notepad {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Notepad {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        // One class implements both halves, so there is no separate controller
        // class for the host to create.
        kNotImplemented
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }

    unsafe fn getBusCount(&self, media_type: MediaType, dir: BusDirection) -> i32 {
        match media_type as MediaTypes {
            MediaTypes_::kAudio => match dir as BusDirections {
                BusDirections_::kInput | BusDirections_::kOutput => 1,
                _ => 0,
            },
            _ => 0,
        }
    }

    unsafe fn getBusInfo(
        &self,
        media_type: MediaType,
        dir: BusDirection,
        index: i32,
        bus: *mut BusInfo,
    ) -> tresult {
        if media_type as MediaTypes != MediaTypes_::kAudio || index != 0 {
            return kInvalidArgument;
        }
        let is_input = dir as BusDirections == BusDirections_::kInput;
        let bus = &mut *bus;
        bus.mediaType = MediaTypes_::kAudio as MediaType;
        bus.direction = dir;
        bus.channelCount = channel_count(self.arrangement());
        copy_wstring(if is_input { "Input" } else { "Output" }, &mut bus.name);
        bus.busType = BusTypes_::kMain as BusType;
        bus.flags = BusInfo_::BusFlags_::kDefaultActive as uint32;
        kResultOk
    }

    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }

    unsafe fn activateBus(
        &self,
        _media_type: MediaType,
        _dir: BusDirection,
        _index: i32,
        _state: TBool,
    ) -> tresult {
        kResultOk
    }

    unsafe fn setActive(&self, _state: TBool) -> tresult {
        kResultOk
    }

    /// Restore the notes the DAW saved with the project.
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let Some(bytes) = read_stream(state) else {
            return kInvalidArgument;
        };
        let Ok(mut editor) = self.editor.lock() else {
            return kInternalError;
        };
        editor.load_state_bytes(&bytes);
        kResultOk
    }

    /// Hand the notes to the DAW to store in the project.
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let Ok(editor) = self.editor.lock() else {
            return kInternalError;
        };
        let bytes = editor.state_bytes();
        drop(editor);
        if write_stream(state, &bytes) {
            kResultOk
        } else {
            kResultFalse
        }
    }
}

impl IAudioProcessorTrait for Notepad {
    /// Accept exactly one audio input and one audio output, of equal width.
    ///
    /// **Requiring the input is what makes this an effect.** A host establishes
    /// whether a plugin can work as a generator by asking it to run with no
    /// audio input at all; answering yes is an invitation to file it with the
    /// instruments. This plugin copies input to output and is useless without
    /// an input, so it says no.
    ///
    /// Input and output widths must match for the same reason: a mismatch would
    /// leave some output channels with no source.
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        if num_ins != 1 || num_outs != 1 || inputs.is_null() || outputs.is_null() {
            return kResultFalse;
        }
        let wanted_in = *inputs;
        let wanted_out = *outputs;
        if wanted_in != wanted_out || wanted_in == 0 {
            return kResultFalse;
        }
        if let Ok(mut current) = self.arrangement.lock() {
            *current = wanted_in;
        }
        kResultTrue
    }

    unsafe fn getBusArrangement(
        &self,
        _dir: BusDirection,
        index: i32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        // Report what was negotiated, not a fixed guess.
        *arr = self.arrangement();
        kResultOk
    }

    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        match size as SymbolicSampleSizes {
            SymbolicSampleSizes_::kSample32 | SymbolicSampleSizes_::kSample64 => kResultOk,
            _ => kInvalidArgument,
        }
    }

    unsafe fn getLatencySamples(&self) -> u32 {
        0
    }

    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }

    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }

    /// Pass audio through untouched.
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        if data.is_null() {
            return kResultOk;
        }
        let data = &*data;
        if data.numSamples <= 0 {
            return kResultOk;
        }
        let num_samples = data.numSamples as usize;

        // No output bus means there is nothing to fill.
        if data.numOutputs < 1 || data.outputs.is_null() {
            return kResultOk;
        }
        let output = &mut *data.outputs;
        let out_channels = output.numChannels.max(0) as usize;

        // The input bus can legitimately be absent or deactivated. That is not
        // a reason to skip the output: leaving it unwritten hands the host back
        // whatever was in the buffer, which is noise on the track.
        let (in_channels, in_bus) = if data.numInputs >= 1 && !data.inputs.is_null() {
            let input = &*data.inputs;
            (input.numChannels.max(0) as usize, Some(input))
        } else {
            (0, None)
        };
        let in_silence = in_bus.map(|b| b.silenceFlags).unwrap_or(0);

        let sixty_four =
            data.symbolicSampleSize as SymbolicSampleSizes == SymbolicSampleSizes_::kSample64;
        let silence = if sixty_four {
            let ins = in_bus
                .map(|b| b.__field0.channelBuffers64)
                .unwrap_or(std::ptr::null_mut());
            let outs = output.__field0.channelBuffers64;
            pass_through(ins, in_channels, outs, out_channels, num_samples, in_silence)
        } else {
            let ins = in_bus
                .map(|b| b.__field0.channelBuffers32)
                .unwrap_or(std::ptr::null_mut());
            let outs = output.__field0.channelBuffers32;
            pass_through(ins, in_channels, outs, out_channels, num_samples, in_silence)
        };

        // Tell the host which output channels ended up silent, so it can skip
        // work downstream rather than assuming the worst.
        output.silenceFlags = silence;
        kResultOk
    }

    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

impl IProcessContextRequirementsTrait for Notepad {
    unsafe fn getProcessContextRequirements(&self) -> u32 {
        0
    }
}

impl IEditControllerTrait for Notepad {
    unsafe fn setComponentState(&self, _state: *mut IBStream) -> tresult {
        // Same object as the component: the state is already applied.
        kResultOk
    }

    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        // UI-only state; the document lives in the component state.
        kResultOk
    }

    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn getParameterCount(&self) -> i32 {
        0
    }

    unsafe fn getParameterInfo(&self, _index: i32, _info: *mut ParameterInfo) -> tresult {
        kInvalidArgument
    }

    unsafe fn getParamStringByValue(&self, _id: u32, _v: f64, _s: *mut String128) -> tresult {
        kInvalidArgument
    }

    unsafe fn getParamValueByString(&self, _id: u32, _s: *mut TChar, _v: *mut f64) -> tresult {
        kInvalidArgument
    }

    unsafe fn normalizedParamToPlain(&self, _id: u32, v: f64) -> f64 {
        v
    }

    unsafe fn plainParamToNormalized(&self, _id: u32, v: f64) -> f64 {
        v
    }

    unsafe fn getParamNormalized(&self, _id: u32) -> f64 {
        0.0
    }

    unsafe fn setParamNormalized(&self, _id: u32, _v: f64) -> tresult {
        kInvalidArgument
    }

    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
        kResultOk
    }

    unsafe fn createView(&self, name: *const c_char) -> *mut IPlugView {
        if !name.is_null() && !fid_eq(name, ViewType::kEditor) {
            return std::ptr::null_mut();
        }
        ComWrapper::new(NotepadView::new(self.editor.clone()))
            .to_com_ptr::<IPlugView>()
            .map(|p| p.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }
}

// ---------------------------------------------------------------------------
// The editor view
// ---------------------------------------------------------------------------

/// The plugin's editor window.
///
/// Keyboard input is delivered by the host through `onKeyDown`; the view
/// forwards it to the shared [`Editor`]. Resizing is negotiated through
/// `checkSizeConstraint`/`onSize`, and the accepted size is written straight
/// into the editor so it is captured by the next `getState`.
pub struct NotepadView {
    editor: Shared,
    /// Last file-operation error, kept so the GUI can surface it.
    last_error: Mutex<Option<String>>,
    /// The child window, once the host has attached us to one.
    ///
    /// `RefCell` rather than `Mutex` because baseview's handle is `!Send` and
    /// VST3 guarantees `IPlugView` calls arrive on the UI thread.
    window: RefCell<Option<baseview::WindowHandle>>,
}

impl NotepadView {
    pub fn new(editor: Shared) -> NotepadView {
        NotepadView {
            editor,
            last_error: Mutex::new(None),
            window: RefCell::new(None),
        }
    }

    /// True once a GUI window exists, in which case it owns keyboard input.
    fn has_window(&self) -> bool {
        self.window
            .try_borrow()
            .map(|w| w.is_some())
            .unwrap_or(false)
    }

    /// The most recent file-operation error, if any.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|e| e.clone())
    }

    fn set_error(&self, message: String) {
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some(message);
        }
    }

    fn clear_error(&self) {
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = None;
        }
    }

    /// Carry out an action the editor cannot do itself.
    ///
    /// Delegates to [`crate::files`] so the toolbar buttons and the keyboard
    /// shortcuts take exactly the same path.
    fn perform(&self, command: Command) {
        self.clear_error();
        if let Some(message) = files::perform(&self.editor, command) {
            self.set_error(message);
        }
    }
}

impl Class for NotepadView {
    type Interfaces = (IPlugView,);
}

impl IPlugViewTrait for NotepadView {
    unsafe fn isPlatformTypeSupported(&self, r#type: FIDString) -> tresult {
        if r#type.is_null() {
            return kInvalidArgument;
        }
        let native = if cfg!(target_os = "windows") {
            kPlatformTypeHWND
        } else if cfg!(target_os = "macos") {
            kPlatformTypeNSView
        } else {
            kPlatformTypeX11EmbedWindowID
        };
        if fid_eq(r#type, native) {
            kResultTrue
        } else {
            kResultFalse
        }
    }

    /// The host has given us a window to live in; open the editor GUI inside it.
    unsafe fn attached(&self, parent: *mut c_void, _type: FIDString) -> tresult {
        if parent.is_null() {
            return kInvalidArgument;
        }
        let Some(parent) = gui::ParentWindow::from_ptr(parent) else {
            return kInvalidArgument;
        };
        let (width, height) = match self.editor.lock() {
            Ok(e) => (e.width, e.height),
            Err(_) => (DEFAULT_WIDTH, DEFAULT_HEIGHT),
        };
        let handle = gui::open(&parent, self.editor.clone(), width, height);
        match self.window.try_borrow_mut() {
            Ok(mut slot) => {
                // A host that attaches twice without removing in between would
                // otherwise strand the previous window, still open and still
                // holding the editor.
                if let Some(previous) = slot.replace(handle) {
                    previous.close();
                }
                kResultOk
            }
            Err(_) => kInternalError,
        }
    }

    unsafe fn removed(&self) -> tresult {
        if let Ok(mut slot) = self.window.try_borrow_mut() {
            if let Some(handle) = slot.take() {
                handle.close();
            }
        }
        kResultOk
    }

    unsafe fn onWheel(&self, _distance: f32) -> tresult {
        kResultOk
    }

    /// Keyboard input from the host.
    ///
    /// Once a GUI window exists it receives key events natively and owns input;
    /// handling them here as well would insert every character twice. Without a
    /// window — which is how the test host drives the plugin, and how a host
    /// that never opens the editor behaves — this is the only input path.
    unsafe fn onKeyDown(&self, key: char16, keyCode: int16, modifiers: int16) -> tresult {
        if self.has_window() {
            return kResultFalse;
        }
        let Some(k) = keys::decode_key(key as u16, keyCode) else {
            return kResultFalse;
        };
        let mods = keys::decode_mods(modifiers);
        let result = {
            let Ok(mut editor) = self.editor.lock() else {
                return kInternalError;
            };
            editor.handle_key(k, mods)
        };
        // Open/Save/Save As need the host and a file dialog, so they run once
        // the document lock has been dropped.
        if let Some(command) = result.command {
            self.perform(command);
        }
        if result.handled {
            kResultTrue
        } else {
            kResultFalse
        }
    }

    unsafe fn onKeyUp(&self, _key: char16, _keyCode: int16, _modifiers: int16) -> tresult {
        kResultFalse
    }

    unsafe fn getSize(&self, size: *mut ViewRect) -> tresult {
        let Ok(editor) = self.editor.lock() else {
            return kInternalError;
        };
        let rect = &mut *size;
        rect.left = 0;
        rect.top = 0;
        rect.right = editor.width;
        rect.bottom = editor.height;
        kResultOk
    }

    /// The host has resized us; remember it so it is saved with the project.
    unsafe fn onSize(&self, new_size: *mut ViewRect) -> tresult {
        let rect = &*new_size;
        let Ok(mut editor) = self.editor.lock() else {
            return kInternalError;
        };
        editor.set_size(rect.right - rect.left, rect.bottom - rect.top);
        kResultOk
    }

    unsafe fn onFocus(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn setFrame(&self, _frame: *mut IPlugFrame) -> tresult {
        kResultOk
    }

    unsafe fn canResize(&self) -> tresult {
        kResultTrue
    }

    /// Clamp a proposed size to the minimum the editor is usable at.
    unsafe fn checkSizeConstraint(&self, rect: *mut ViewRect) -> tresult {
        let rect = &mut *rect;
        let width = (rect.right - rect.left).max(MIN_WIDTH);
        let height = (rect.bottom - rect.top).max(MIN_HEIGHT);
        rect.right = rect.left + width;
        rect.bottom = rect.top + height;
        kResultTrue
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

struct Factory;

impl Class for Factory {
    // All three factory versions. A host reaches for the newest it knows, and
    // one that wants `IPluginFactory3` and does not find it can fall back to
    // the v1 `getClassInfo` — which has no `subCategories` field at all, so it
    // cannot tell an effect from an instrument and may list the plugin as
    // both. Every shipping plugin implements all three.
    type Interfaces = (IPluginFactory, IPluginFactory2, IPluginFactory3);
}

/// Copy into a UTF-16 buffer of `char16`, NUL-terminated.
fn copy_wstring16(src: &str, dst: &mut [char16]) {
    let mut len = 0;
    for (s, d) in src.encode_utf16().zip(dst.iter_mut()) {
        *d = s;
        len += 1;
    }
    if len < dst.len() {
        dst[len] = 0;
    } else if let Some(last) = dst.last_mut() {
        *last = 0;
    }
}

/// The two classes this plugin exports.
///
/// Mirrors the SDK's `again` example: an audio-effect class carrying the `Fx`
/// subcategory, and a component-controller class with an empty one. Every
/// factory version reports these identically, so no host can reach a different
/// conclusion about what kind of plugin this is.
#[derive(Clone, Copy)]
enum ClassKind {
    Processor,
}

impl ClassKind {
    fn at(index: i32) -> Option<ClassKind> {
        match index {
            0 => Some(ClassKind::Processor),
            _ => None,
        }
    }

    fn cid(self) -> TUID {
        match self {
            ClassKind::Processor => Notepad::CID,
        }
    }

    fn category(self) -> &'static str {
        match self {
            ClassKind::Processor => "Audio Module Class",
        }
    }

    fn name(self) -> &'static str {
        match self {
            ClassKind::Processor => PLUGIN_NAME,
        }
    }

    fn sub_category(self) -> &'static str {
        match self {
            ClassKind::Processor => SUB_CATEGORY,
        }
    }
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let info = &mut *info;
        copy_cstring(VENDOR, &mut info.vendor);
        copy_cstring("https://example.invalid/vst-notepad", &mut info.url);
        copy_cstring("noreply@example.invalid", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as int32;
        kResultOk
    }

    unsafe fn countClasses(&self) -> i32 {
        1
    }

    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        let Some(class) = ClassKind::at(index) else {
            return kInvalidArgument;
        };
        let info = &mut *info;
        info.cid = class.cid();
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring(class.category(), &mut info.category);
        copy_cstring(class.name(), &mut info.name);
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        if cid.is_null() || iid.is_null() || obj.is_null() {
            return kInvalidArgument;
        }
        if *(cid as *const TUID) != Notepad::CID {
            return kInvalidArgument;
        }
        let Some(instance) = ComWrapper::new(Notepad::new()).to_com_ptr::<FUnknown>() else {
            return kInternalError;
        };
        let ptr = instance.as_ptr();
        ((*(*ptr).vtbl).queryInterface)(ptr, iid as *mut TUID, obj)
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
        let Some(class) = ClassKind::at(index) else {
            return kInvalidArgument;
        };
        let info = &mut *info;
        info.cid = class.cid();
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring(class.category(), &mut info.category);
        copy_cstring(class.name(), &mut info.name);
        info.classFlags = 0;
        copy_cstring(class.sub_category(), &mut info.subCategories);
        copy_cstring(VENDOR, &mut info.vendor);
        copy_cstring(VERSION, &mut info.version);
        copy_cstring(SDK_VERSION, &mut info.sdkVersion);
        kResultOk
    }
}

impl IPluginFactory3Trait for Factory {
    unsafe fn getClassInfoUnicode(&self, index: i32, info: *mut PClassInfoW) -> tresult {
        let Some(class) = ClassKind::at(index) else {
            return kInvalidArgument;
        };
        if info.is_null() {
            return kInvalidArgument;
        }
        let info = &mut *info;
        info.cid = class.cid();
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_cstring(class.category(), &mut info.category);
        copy_wstring16(class.name(), &mut info.name);
        info.classFlags = 0;
        copy_cstring(class.sub_category(), &mut info.subCategories);
        copy_wstring16(VENDOR, &mut info.vendor);
        copy_wstring16(VERSION, &mut info.version);
        copy_wstring16(SDK_VERSION, &mut info.sdkVersion);
        kResultOk
    }

    unsafe fn setHostContext(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
}

// ---------------------------------------------------------------------------
// Module entry points
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn InitDll() -> bool {
    true
}

#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn ExitDll() -> bool {
    true
}

#[cfg(target_os = "macos")]
#[no_mangle]
extern "system" fn BundleEntry(_bundle_ref: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "macos")]
#[no_mangle]
extern "system" fn BundleExit() -> bool {
    true
}

#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleEntry(_handle: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleExit() -> bool {
    true
}

/// The one symbol every VST3 host looks for.
#[no_mangle]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    ComWrapper::new(Factory)
        .to_com_ptr::<IPluginFactory>()
        .map(|p| p.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
