//! The editor window, drawn with egui inside the window the host gives us.
//!
//! # How markdown is drawn
//!
//! The document model hands back one [`Block`] per line, each carrying spans
//! tagged with a style and a visibility flag. Drawing is therefore a
//! translation job rather than a parsing one: build an egui `LayoutJob` from
//! the visible spans, give each one a `TextFormat` matching its style, and let
//! egui lay it out. Hidden markers simply are not appended — which is exactly
//! what makes the view WYSIWYG, and why toggling to raw mode (where every
//! marker is visible) needs no separate renderer.
//!
//! # A note on bold
//!
//! egui ships no bold font family, so bold is drawn the way egui draws its own
//! emphasis: a stronger foreground colour. Italic, strikethrough and underline
//! are real text formatting.

use std::sync::Arc;
use std::time::{Duration, Instant};

use baseview::dpi::{LogicalSize, Size};
use baseview::{WindowHandle, WindowScalePolicy};
use egui::text::LayoutJob;
use egui::{Color32, FontFamily, FontId, Sense, Stroke, TextFormat};
use egui_baseview::{EguiWindow, EguiWindowSettings, ExtraOutputCommands};
use notepad_core::{
    Block, BlockKind, Command, Key, Mods, Span, SpanRole, Style, Theme, ViewMode,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::Shared;

/// A parent window supplied by the host, wrapped so baseview can adopt it.
pub struct ParentWindow(RawWindowHandle);

impl ParentWindow {
    /// Build from the pointer the host passes to `IPlugView::attached`.
    ///
    /// # Safety
    /// `ptr` must be a valid window handle of this platform's native type for
    /// as long as the child window lives.
    pub unsafe fn from_ptr(ptr: *mut std::ffi::c_void) -> Option<ParentWindow> {
        #[cfg(target_os = "windows")]
        {
            use raw_window_handle::Win32WindowHandle;
            let hwnd = std::num::NonZeroIsize::new(ptr as isize)?;
            Some(ParentWindow(RawWindowHandle::Win32(
                Win32WindowHandle::new(hwnd),
            )))
        }
        #[cfg(target_os = "macos")]
        {
            use raw_window_handle::AppKitWindowHandle;
            let view = std::ptr::NonNull::new(ptr)?;
            Some(ParentWindow(RawWindowHandle::AppKit(
                AppKitWindowHandle::new(view),
            )))
        }
        #[cfg(target_os = "linux")]
        {
            use raw_window_handle::XcbWindowHandle;
            let id = std::num::NonZeroU32::new(ptr as usize as u32)?;
            Some(ParentWindow(RawWindowHandle::Xcb(XcbWindowHandle::new(id))))
        }
    }
}

impl HasWindowHandle for ParentWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // Safety: the handle belongs to the host's window, which outlives ours.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(self.0) })
    }
}

/// How often to re-read the OS theme while set to Auto.
///
/// Reading it is a registry/desktop-portal call, far too expensive to do every
/// frame, but the user may flip their system theme while the plugin is open.
const SYSTEM_THEME_POLL: Duration = Duration::from_secs(2);

/// Breathing room around the toolbar row.
const TOOLBAR_MARGIN: egui::Margin = egui::Margin::symmetric(10, 6);

/// Breathing room around the document. Text pressed against the window edge is
/// unpleasant to read and worse to click at, since the first character has no
/// margin to aim at.
const DOCUMENT_MARGIN: egui::Margin = egui::Margin::symmetric(18, 12);

/// State handed to the egui update closure.
struct Gui {
    editor: Shared,
    error: Option<String>,
    /// Last known system setting, refreshed on a timer.
    system_dark: bool,
    checked_at: Instant,
    /// What is currently applied, so visuals are only rebuilt on a change.
    applied_dark: Option<bool>,
    /// Window size as of last frame, used to tell "the window was resized"
    /// apart from "someone changed the stored size".
    last_seen_size: Option<(i32, i32)>,
}

impl Gui {
    fn new(editor: Shared) -> Gui {
        Gui {
            editor,
            error: None,
            system_dark: system_is_dark(),
            checked_at: Instant::now(),
            applied_dark: None,
            last_seen_size: None,
        }
    }

    /// Re-read the OS theme if the poll interval has elapsed.
    fn refresh_system_theme(&mut self) {
        if self.checked_at.elapsed() >= SYSTEM_THEME_POLL {
            self.system_dark = system_is_dark();
            self.checked_at = Instant::now();
        }
    }
}

/// What the operating system currently reports.
///
/// `Unspecified` — and any failure to ask — is treated as dark, matching the
/// editor's default appearance rather than flipping to a jarring white.
fn system_is_dark() -> bool {
    !matches!(dark_light::detect(), Ok(dark_light::Mode::Light))
}

fn settings(width: i32, height: i32) -> EguiWindowSettings {
    EguiWindowSettings::new()
        .with_tile("Notepad")
        .with_size(Size::Logical(LogicalSize {
            width: width as f64,
            height: height as f64,
        }))
        .with_scale_policy(WindowScalePolicy::SystemScaleFactor)
}

/// Open the editor window as a child of the host's window.
pub fn open(parent: &ParentWindow, editor: Shared, width: i32, height: i32) -> WindowHandle {
    EguiWindow::open_parented(
        parent,
        settings(width, height),
        Gui::new(editor),
        |_ctx: &egui::Context, _cmds: &mut ExtraOutputCommands, _state: &mut Gui| {},
        |_out: &egui::FullOutput, _vp: &egui::ViewportOutput, _state: &mut Gui| {},
        |ui: &mut egui::Ui, cmds: &mut ExtraOutputCommands, state: &mut Gui| draw(ui, state, cmds),
    )
}

/// Open the editor as a standalone window and block until it closes.
///
/// This runs the *same* drawing and input code the plugin uses, so it is a
/// faithful way to look at the editor without loading it into a DAW.
pub fn open_blocking(editor: Shared, width: i32, height: i32) {
    EguiWindow::open_blocking(
        settings(width, height),
        Gui::new(editor),
        |_ctx: &egui::Context, _cmds: &mut ExtraOutputCommands, _state: &mut Gui| {},
        |_out: &egui::FullOutput, _vp: &egui::ViewportOutput, _state: &mut Gui| {},
        |ui: &mut egui::Ui, cmds: &mut ExtraOutputCommands, state: &mut Gui| draw(ui, state, cmds),
    );
}

// ---------------------------------------------------------------------------
// Frame
// ---------------------------------------------------------------------------

fn draw(ui: &mut egui::Ui, gui: &mut Gui, commands: &mut ExtraOutputCommands) {
    let background = draw_ui(ui, gui);
    // Also hand the background to the renderer, which clears to it before any
    // of our painting happens.
    commands.clear_color(egui::Rgba::from(background));
}

/// Draw one frame and return the background colour the theme calls for.
///
/// Split out from [`draw`] so it can be run against any `Ui` — including a
/// headless one — without an `ExtraOutputCommands` to hand.
fn draw_ui(ui: &mut egui::Ui, gui: &mut Gui) -> Color32 {
    // Theme first: everything below reads colours from the active visuals.
    let background = apply_theme(ui, gui);

    // Paint the background ourselves rather than relying solely on the
    // renderer's clear colour. `run_ui` gives a bare root `Ui` with no panel
    // behind it, so without this the window shows whatever was cleared last.
    // The root `Ui` spans the whole viewport, so its rect is the window.
    ui.painter().rect_filled(ui.max_rect(), 0.0, background);

    sync_window_size(ui, gui);

    // Keyboard next, so the document is current before it is laid out.
    let (events, modifiers) = ui.input(|i| (i.events.clone(), i.modifiers));
    let command = apply_input(&gui.editor, &events, modifiers.ctrl || modifiers.command);
    if let Some(command) = command {
        gui.error = crate::files::perform(&gui.editor, command);
    }

    egui::Frame::default()
        .inner_margin(TOOLBAR_MARGIN)
        .show(ui, |ui| toolbar(ui, gui));
    ui.separator();

    if let Some(error) = &gui.error {
        let colour = ui.visuals().error_fg_color;
        egui::Frame::default()
            .inner_margin(TOOLBAR_MARGIN)
            .show(ui, |ui| ui.colored_label(colour, error));
        ui.separator();
    }

    // The scroll area itself spans the full width so its bar sits against the
    // window edge; the padding goes inside, around the text.
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::default()
                .inner_margin(DOCUMENT_MARGIN)
                .show(ui, |ui| document(ui, gui));
        });

    background
}

/// Persistent GUI state for headless rendering.
///
/// The state has to live across frames exactly as it does in a real window:
/// rebuilding it each frame would re-apply the theme every time and request a
/// repaint forever.
#[doc(hidden)]
pub struct TestGui(Gui);

impl TestGui {
    pub fn new(editor: Shared, system_dark: bool) -> TestGui {
        let mut gui = Gui::new(editor);
        gui.system_dark = system_dark;
        TestGui(gui)
    }
}

/// Draw one frame into an arbitrary `Ui`.
///
/// Exists so the snapshot tool can render the real GUI headlessly and check
/// the themes pixel by pixel, rather than anyone having to eyeball a window.
#[doc(hidden)]
pub fn draw_frame_for_test(ui: &mut egui::Ui, state: &mut TestGui) {
    draw_ui(ui, &mut state.0);
}

/// Keep the window and the stored size in step, in both directions.
///
/// Two things can change the size and they must not fight:
///
/// - **The window was resized** (the user dragged an edge, or the host resized
///   its frame and called `onSize`, which the window then followed). The window
///   is the truth; record it so it is saved with the project.
/// - **The stored size changed** while the window did not — the host called
///   `IPlugView::onSize`, or a project was loaded with a different size. Then
///   the window has to be told to follow, which `ViewportCommand::InnerSize`
///   does. Without this the plugin's stored size and its actual window silently
///   diverge: the frame resizes and the editor inside it does not.
fn sync_window_size(ui: &mut egui::Ui, gui: &mut Gui) {
    let size = ui.ctx().viewport_rect().size();
    let window = (size.x.round() as i32, size.y.round() as i32);
    if window.0 <= 0 || window.1 <= 0 {
        return;
    }

    let stored = match gui.editor.lock() {
        Ok(e) => (e.width, e.height),
        Err(_) => return,
    };

    if gui.last_seen_size != Some(window) {
        // The window itself changed; adopt it.
        gui.last_seen_size = Some(window);
        if stored != window {
            if let Ok(mut e) = gui.editor.lock() {
                e.set_size(window.0, window.1);
            }
        }
        return;
    }

    // The window is unchanged but the stored size is not: ask it to follow.
    if stored != window {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                stored.0 as f32,
                stored.1 as f32,
            )));
    }
}

/// Resolve the chosen theme against the system and apply it.
///
/// Two things here are easy to get wrong and were:
///
/// 1. **Nothing paints the window background.** `run_ui` hands us a bare root
///    `Ui` on the background layer — there is no panel behind it — so the
///    background is entirely the renderer's clear colour, which defaults to
///    black. Switching to the light theme without this gives dark text on a
///    black background, which looks like the theme did nothing at all.
/// 2. **`set_visuals` lands a frame late.** The root `Ui` was built from the
///    style as it was when this frame started, so the current frame must have
///    its style updated directly or the change is invisible until something
///    else triggers a repaint.
fn apply_theme(ui: &mut egui::Ui, gui: &mut Gui) -> Color32 {
    let theme = gui.editor.lock().map(|e| e.theme).unwrap_or(Theme::Auto);

    // Only the Auto setting needs to know what the system is doing.
    if theme == Theme::Auto {
        gui.refresh_system_theme();
    }

    let dark = theme.is_dark(gui.system_dark);
    if gui.applied_dark != Some(dark) {
        let visuals = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        ui.ctx().set_visuals(visuals.clone());
        ui.style_mut().visuals = visuals;
        gui.applied_dark = Some(dark);
        ui.ctx().request_repaint();
    }

    ui.visuals().panel_fill
}

fn toolbar(ui: &mut egui::Ui, gui: &mut Gui) {
    let (title, mode, theme) = match gui.editor.lock() {
        Ok(e) => (e.display_name(), e.mode, e.theme),
        Err(_) => ("Untitled".to_string(), ViewMode::Wysiwyg, Theme::Auto),
    };

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).strong());
        ui.separator();

        if ui.button("Open…").clicked() {
            gui.error = crate::files::perform(&gui.editor, Command::Open);
        }
        if ui.button("Save").clicked() {
            gui.error = crate::files::perform(&gui.editor, Command::Save);
        }
        if ui.button("Save As…").clicked() {
            gui.error = crate::files::perform(&gui.editor, Command::SaveAs);
        }

        ui.separator();
        let label = match mode {
            ViewMode::Wysiwyg => "Markdown source",
            ViewMode::Raw => "Formatted",
        };
        if ui.button(label).on_hover_text("Ctrl+/").clicked() {
            if let Ok(mut e) = gui.editor.lock() {
                e.toggle_mode();
            }
        }

        // Theme. Auto shows what it currently resolves to, so the button never
        // leaves you guessing which way "auto" went.
        let theme_label = match theme {
            Theme::Light => "Theme: light".to_string(),
            Theme::Dark => "Theme: dark".to_string(),
            Theme::Auto => format!(
                "Theme: auto ({})",
                if gui.system_dark { "dark" } else { "light" }
            ),
        };
        if ui
            .button(theme_label)
            .on_hover_text("Ctrl+T — cycles auto → light → dark")
            .clicked()
        {
            if let Ok(mut e) = gui.editor.lock() {
                e.cycle_theme();
            }
        }
    });
}

fn document(ui: &mut egui::Ui, gui: &mut Gui) {
    let Ok(mut editor) = gui.editor.lock() else {
        return;
    };

    let doc = editor.render();
    let src = editor.text().to_string();
    let caret = editor.caret();
    let raw = editor.mode == ViewMode::Raw;

    ui.spacing_mut().item_spacing.y = 2.0;

    let mut clicked: Option<usize> = None;
    let mut toggled: Option<usize> = None;

    for block in &doc.blocks {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;

            // Blockquote bars and list indentation.
            // Painted rather than drawn as a "▌" glyph: that character is not
            // in egui's default font and rendered as a tofu box.
            for _ in 0..block.quote_depth {
                let height = ui.text_style_height(&egui::TextStyle::Body);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(3.0, height), Sense::hover());
                let colour = ui.visuals().weak_text_color();
                ui.painter().rect_filled(rect, 1.0, colour);
                ui.add_space(6.0);
            }
            ui.add_space(block.kind.indent() as f32 * 14.0);

            // The list glyph replaces the hidden marker. In raw mode the marker
            // text itself is shown instead, so no glyph is drawn.
            if !raw {
                match &block.kind {
                    BlockKind::Bullet { checked: Some(done), .. }
                    | BlockKind::Numbered { checked: Some(done), .. } => {
                        let mut checked = *done;
                        if ui.checkbox(&mut checked, "").changed() {
                            toggled = Some(block.line);
                        }
                        ui.add_space(4.0);
                    }
                    BlockKind::Bullet { .. } => {
                        ui.label("•");
                        ui.add_space(6.0);
                    }
                    BlockKind::Numbered { number, .. } => {
                        ui.label(format!("{number}."));
                        ui.add_space(6.0);
                    }
                    _ => {}
                }
            }

            if let Some(offset) = line_body(ui, block, &src, caret, raw) {
                clicked = Some(offset);
            }
        });
    }

    if let Some(line) = toggled {
        editor.toggle_checkbox(line);
    } else if let Some(offset) = clicked {
        editor.set_caret(offset);
    }
}

/// Draw one line's text, its caret, and report a click position.
fn line_body(
    ui: &mut egui::Ui,
    block: &Block,
    src: &str,
    caret: usize,
    raw: bool,
) -> Option<usize> {
    let base = base_format(block);
    let palette = Palette::from(ui.visuals());

    let mut job = LayoutJob::default();
    // Byte offset in the source for each byte offset in the job, so a click can
    // be mapped back onto the document.
    let mut map: Vec<usize> = Vec::new();

    // The raw marker is drawn when the caret is on this line (Typora-style) or
    // when the whole document is in raw mode.
    if block.marker_visible && !block.marker.is_empty() {
        let text = &src[block.marker.clone()];
        push(
            &mut job,
            &mut map,
            text,
            block.marker.start,
            marker_format(&base, &palette),
        );
    }

    for span in &block.spans {
        if !span.visible {
            continue;
        }
        push(
            &mut job,
            &mut map,
            span.text(src),
            span.range.start,
            span_format(span, &base, &palette),
        );
    }

    // An empty line still needs height and a click target.
    if job.text.is_empty() {
        push(&mut job, &mut map, " ", block.range.start, base.clone());
    }
    map.push(block.range.end);

    let galley = ui.painter().layout_job(job);
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::click());
    ui.painter()
        .galley(rect.min, Arc::clone(&galley), ui.visuals().text_color());

    // Caret: measured by laying out the text preceding it in the same fonts,
    // which keeps it aligned even across mixed heading/code formatting.
    if caret >= block.range.start && caret <= block.range.end {
        // `map` is char-indexed, so the caret's char position is the first
        // entry at or past it; the text before that, measured in the same font,
        // gives the caret's x.
        let char_index = map
            .iter()
            .position(|&offset| offset >= caret)
            .unwrap_or(map.len().saturating_sub(1));
        let prefix: String = galley.text().chars().take(char_index).collect();
        let x = ui
            .painter()
            .layout_no_wrap(prefix, base.font_id.clone(), Color32::TRANSPARENT)
            .size()
            .x;
        let top = rect.min + egui::vec2(x, 0.0);
        ui.painter().line_segment(
            [top, top + egui::vec2(0.0, galley.size().y)],
            Stroke::new(1.5, ui.visuals().strong_text_color()),
        );
    }

    let _ = raw;

    if response.clicked() {
        let pos = response.interact_pointer_pos()?;
        let cursor = galley.cursor_from_pos(pos - rect.min);
        let index = cursor.index.0.min(map.len().saturating_sub(1));
        return map.get(index).copied();
    }
    None
}

/// Append `text` to the job, recording the source offset of every byte.
fn push(job: &mut LayoutJob, map: &mut Vec<usize>, text: &str, source_start: usize, fmt: TextFormat) {
    for (i, _) in text.char_indices() {
        map.push(source_start + i);
    }
    job.append(text, 0.0, fmt);
}

// ---------------------------------------------------------------------------
// Styling
// ---------------------------------------------------------------------------

fn base_format(block: &Block) -> TextFormat {
    let size = match block.kind {
        BlockKind::Heading(1) => 28.0,
        BlockKind::Heading(2) => 23.0,
        BlockKind::Heading(3) => 20.0,
        BlockKind::Heading(4) => 18.0,
        BlockKind::Heading(5) => 16.5,
        BlockKind::Heading(6) => 15.5,
        _ => 15.0,
    };
    let family = match block.kind {
        BlockKind::Code | BlockKind::Fence { .. } => FontFamily::Monospace,
        _ => FontFamily::Proportional,
    };
    TextFormat {
        font_id: FontId::new(size, family),
        color: Color32::PLACEHOLDER,
        ..Default::default()
    }
}

/// Colours taken from the active theme.
///
/// Everything the document renderer draws comes from here rather than from
/// literals, so light mode is not a washed-out copy of the dark palette.
#[derive(Clone, Copy)]
struct Palette {
    strong: Color32,
    dim: Color32,
    link: Color32,
    code_background: Color32,
}

impl Palette {
    fn from(visuals: &egui::Visuals) -> Palette {
        Palette {
            strong: visuals.strong_text_color(),
            dim: visuals.weak_text_color(),
            link: visuals.hyperlink_color,
            // A faint wash of the panel's opposite, readable in either theme.
            code_background: if visuals.dark_mode {
                Color32::from_gray(60)
            } else {
                Color32::from_gray(225)
            },
        }
    }
}

/// Markdown punctuation, shown dimmed when revealed.
fn marker_format(base: &TextFormat, palette: &Palette) -> TextFormat {
    TextFormat {
        color: palette.dim,
        ..base.clone()
    }
}

fn span_format(span: &Span, base: &TextFormat, palette: &Palette) -> TextFormat {
    let mut fmt = base.clone();

    if span.style.contains(Style::CODE) {
        fmt.font_id = FontId::new(fmt.font_id.size - 1.0, FontFamily::Monospace);
        fmt.background = palette.code_background;
    }
    if span.style.contains(Style::ITALIC) {
        fmt.italics = true;
    }
    if span.style.contains(Style::STRIKE) {
        fmt.strikethrough = Stroke::new(1.0, palette.dim);
    }
    // egui has no bold family, so bold is a stronger colour — the same trick
    // egui's own `RichText::strong` uses.
    if span.style.contains(Style::BOLD) {
        fmt.color = palette.strong;
    }
    if span.is_marker() {
        fmt.color = palette.dim;
    }
    match &span.role {
        SpanRole::Link { .. } | SpanRole::Image { .. } => {
            fmt.color = palette.link;
            fmt.underline = Stroke::new(1.0, palette.link);
        }
        _ => {}
    }
    fmt
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// Feed egui's key and text events into the editor.
///
/// Returns a host command if one was requested (Ctrl+O / Ctrl+S / Ctrl+Shift+S),
/// so the caller can run it after releasing the document lock.
fn apply_input(editor: &Shared, events: &[egui::Event], command_held: bool) -> Option<Command> {
    let mut command = None;
    let Ok(mut ed) = editor.lock() else {
        return None;
    };

    for event in events {
        match event {
            // Printable input.
            //
            // `Event::Text` carries no modifiers, and on some platforms a
            // shortcut emits both a text event and a key event. Skipping text
            // while a command modifier is held stops Ctrl+B inserting a "b"
            // as well as toggling bold.
            egui::Event::Text(text) if !command_held => {
                for c in text.chars() {
                    if !c.is_control() {
                        ed.handle_key(Key::Char(c), Mods::NONE);
                    }
                }
            }
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let mods = Mods {
                    ctrl: modifiers.ctrl || modifiers.command,
                    shift: modifiers.shift,
                    alt: modifiers.alt,
                };
                if let Some(k) = translate_key(*key, mods.ctrl) {
                    let result = ed.handle_key(k, mods);
                    if let Some(c) = result.command {
                        command = Some(c);
                    }
                }
            }
            _ => {}
        }
    }
    command
}

fn translate_key(key: egui::Key, ctrl: bool) -> Option<Key> {
    use egui::Key as E;
    let k = match key {
        E::Enter => Key::Enter,
        E::Backspace => Key::Backspace,
        E::Delete => Key::Delete,
        E::Tab => Key::Tab,
        E::ArrowLeft => Key::Left,
        E::ArrowRight => Key::Right,
        E::ArrowUp => Key::Up,
        E::ArrowDown => Key::Down,
        E::Home => Key::Home,
        E::End => Key::End,
        E::PageUp => Key::PageUp,
        E::PageDown => Key::PageDown,
        E::Escape => Key::Escape,
        // Letters and `/` only matter while Ctrl is held; without it they
        // arrive as text events and are inserted there.
        other if ctrl => {
            let name = other.name();
            if other == E::Slash {
                Key::Char('/')
            } else if name.len() == 1 {
                let c = name.chars().next()?;
                if c.is_ascii_alphabetic() {
                    Key::Char(c.to_ascii_lowercase())
                } else {
                    return None;
                }
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(k)
}

