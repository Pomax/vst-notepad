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

/// Height of the toolbar row, so its contents have somewhere to centre in.
const TOOLBAR_HEIGHT: f32 = 26.0;

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
    /// Fonts come from the system and only need installing once.
    fonts_installed: bool,
    /// Scripts already loaded on demand.
    scripts: std::collections::HashSet<crate::fonts::Script>,
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
            fonts_installed: false,
            scripts: std::collections::HashSet::new(),
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
    apply_cursor(ui.ctx());
}

/// Apply the cursor egui asked for, directly.
///
/// egui only *reports* a cursor icon; something has to put it on the window.
/// baseview does that with `addCursorRect:cursor:`, which AppKit only honours
/// from inside `resetCursorRects` — anywhere else the rect is discarded the
/// next time the window rebuilds its cursor rects, which is constantly. The
/// pointer therefore snaps straight back to an arrow, and egui-baseview never
/// re-applies it because it only calls down when the icon *changes*.
///
/// Setting `NSCursor` every frame is what makes the choice stick. It is the
/// last word on the cursor, so it has to follow egui's decision rather than
/// hard-coding one, or the toolbar would get an I-beam too.
#[cfg(target_os = "macos")]
fn apply_cursor(ctx: &egui::Context) {
    use objc2_app_kit::NSCursor;

    let cursor = match ctx.output(|out| out.cursor_icon) {
        egui::CursorIcon::Text => NSCursor::IBeamCursor(),
        _ => NSCursor::arrowCursor(),
    };
    cursor.set();
}

#[cfg(not(target_os = "macos"))]
fn apply_cursor(_ctx: &egui::Context) {}

/// Draw one frame and return the background colour the theme calls for.
///
/// Split out from [`draw`] so it can be run against any `Ui` — including a
/// headless one — without an `ExtraOutputCommands` to hand.
fn draw_ui(ui: &mut egui::Ui, gui: &mut Gui) -> Color32 {
    if !gui.fonts_installed {
        crate::fonts::install_base(ui.ctx());
        gui.fonts_installed = true;
    }

    // Load a font for any script in the document the current fonts cannot draw.
    // A newly added font takes effect next pass, so ask for one.
    let text = gui.editor.lock().map(|e| e.text().to_string()).ok();
    if let Some(text) = text {
        if crate::fonts::ensure_coverage(ui.ctx(), &text, &mut gui.scripts) {
            ui.ctx().request_repaint();
        }
    }

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
        .fill(toolbar_fill(ui.visuals()))
        .inner_margin(TOOLBAR_MARGIN)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            toolbar(ui, gui);
        });
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

/// Background of the toolbar: a step away from the page, in whichever
/// direction the theme leaves room.
fn toolbar_fill(visuals: &egui::Visuals) -> Color32 {
    let panel = visuals.panel_fill;
    let shift = |channel: u8| {
        if visuals.dark_mode {
            channel.saturating_add(18)
        } else {
            channel.saturating_sub(16)
        }
    };
    Color32::from_rgb(shift(panel.r()), shift(panel.g()), shift(panel.b()))
}

fn toolbar(ui: &mut egui::Ui, gui: &mut Gui) {
    // Only a note backed by a file can be out of step with one; notes that live
    // solely in plugin state are always saved with the project.
    let (mode, theme, unsaved) = match gui.editor.lock() {
        Ok(e) => (e.mode, e.theme, e.file.is_some() && e.is_dirty()),
        Err(_) => (ViewMode::Wysiwyg, Theme::Auto, false),
    };

    // A button should look like a button whether or not the pointer is over it,
    // so the outline is on in every state rather than appearing on hover. The
    // face is kept at the page colour so it stands out from the darker bar.
    let dark = ui.visuals().dark_mode;
    let outline = Stroke::new(
        1.0,
        if dark {
            Color32::from_gray(105)
        } else {
            Color32::from_gray(150)
        },
    );
    let face = ui.visuals().panel_fill;
    let widgets = &mut ui.visuals_mut().widgets;
    for state in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active] {
        state.bg_stroke = outline;
    }
    widgets.inactive.weak_bg_fill = face;

    ui.horizontal(|ui| {
        ui.set_min_height(TOOLBAR_HEIGHT);

        if ui.button("Open…").clicked() {
            gui.error = crate::files::perform(&gui.editor, Command::Open);
        }
        let save = if unsaved {
            egui::RichText::new("Save *").strong()
        } else {
            egui::RichText::new("Save")
        };
        if ui.button(save).clicked() {
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

    ui.spacing_mut().item_spacing.y = 0.0;

    let mut clicked: Option<usize> = None;
    let mut toggled: Option<usize> = None;

    // Registered before any line, so every line and control sits on top of it
    // and keeps its own clicks. This only catches what falls between them: the
    // gaps separating lines, and the empty space below the last one.
    let focus = ui.id().with("document-background");
    let background = ui
        .interact(ui.available_rect_before_wrap(), focus, Sense::click())
        .on_hover_cursor(egui::CursorIcon::Text);
    hold_keyboard_focus(ui, focus, &background);

    let mut lines: Vec<LineHit> = Vec::new();
    let em = egui::TextStyle::Body.resolve(ui.style()).size;
    let mut previous: Option<&Block> = None;

    for block in &doc.blocks {
        if let Some(previous) = previous {
            ui.add_space(block_gap(previous, block, em));
        }
        previous = Some(block);

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

            let hit = line_body(ui, block, &src, caret, raw);
            if let Some(offset) = hit.clicked {
                clicked = Some(offset);
            }
            lines.push(hit);
        });
    }

    // Nothing was hit directly, so fall back to whichever line is closest.
    if clicked.is_none() && background.clicked() {
        if let Some(pos) = background.interact_pointer_pos() {
            clicked = nearest_offset(&lines, pos);
        }
    }

    if let Some(line) = toggled {
        editor.toggle_checkbox(line);
    } else if let Some(offset) = clicked {
        editor.set_caret(offset);
    }
}

/// Keep egui's keyboard focus on the document.
///
/// This is what stops keystrokes reaching the DAW, and it is not obvious.
/// baseview hands a key to the window handler and, if the handler reports it
/// unused, passes it to `super.keyDown:` — straight up the responder chain into
/// the host, which runs its own shortcut. egui-baseview reports a key as used
/// only when `egui_wants_keyboard_input()` is true, and that is true only when
/// some egui widget holds focus.
///
/// Nothing here is a `TextEdit`: the document is painted directly and keys are
/// read from the raw event stream, so without this no widget is ever focused,
/// every keystroke is reported unused, and typing in the notepad also drives
/// the DAW. Holding focus on the document is what makes the editor look like a
/// focused text field to that check.
///
/// The lock filter is the second half. A focused widget normally gives Tab and
/// the arrow keys back to egui for focus navigation, which would send Tab to
/// the toolbar instead of indenting a list item. Claiming them here keeps them
/// with the document, exactly as `TextEdit` does.
fn hold_keyboard_focus(ui: &mut egui::Ui, id: egui::Id, background: &egui::Response) {
    if background.clicked() || ui.memory(|memory| memory.focused().is_none()) {
        ui.memory_mut(|memory| memory.request_focus(id));
    }
    ui.memory_mut(|memory| {
        memory.set_focus_lock_filter(
            id,
            egui::EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            },
        )
    });
}

/// Vertical space to leave between two consecutive lines.
///
/// One line is not one block: a wrapped paragraph, the lines of a fenced code
/// block and the items of a list are each several lines belonging together, and
/// only get separated where one element ends and the next begins.
fn block_gap(previous: &Block, current: &Block, em: f32) -> f32 {
    use BlockKind::*;

    // A blank line is already a line's worth of space.
    if matches!(previous.kind, Blank) || matches!(current.kind, Blank) {
        return 0.0;
    }

    // Continuation lines of one blockquote.
    if previous.quote_depth > 0 && previous.quote_depth == current.quote_depth {
        return 0.0;
    }

    match (&previous.kind, &current.kind) {
        // Lines of one paragraph, and lines inside one fence.
        (Paragraph, Paragraph) => 0.0,
        (Code, Code) | (Fence { .. }, Code) | (Code, Fence { .. }) => 0.0,

        // Items of one list sit closer together than separate elements.
        (Bullet { .. } | Numbered { .. }, Bullet { .. } | Numbered { .. }) => em * 0.3,

        _ => em,
    }
}

/// One drawn line, kept so a click that missed every line can still be mapped
/// onto the nearest one.
struct LineHit {
    rect: egui::Rect,
    galley: Arc<egui::Galley>,
    /// Source byte offset for each char position in the galley.
    map: Vec<usize>,
    /// Set when this line itself was clicked.
    clicked: Option<usize>,
}

impl LineHit {
    /// Source offset for a point, in screen coordinates.
    ///
    /// The point does not have to be inside the line: `cursor_from_pos` clamps
    /// to the nearest position, which is what puts the caret at the end of the
    /// line when the click was past the last character.
    fn offset_at(&self, pos: egui::Pos2) -> Option<usize> {
        let cursor = self.galley.cursor_from_pos(pos - self.rect.min);
        let index = cursor.index.0.min(self.map.len().saturating_sub(1));
        self.map.get(index).copied()
    }
}

/// Draw one line's text, its caret, and report a click position.
fn line_body(
    ui: &mut egui::Ui,
    block: &Block,
    src: &str,
    caret: usize,
    raw: bool,
) -> LineHit {
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

    // The click target runs to the edge of the window rather than stopping at
    // the last character. Clicking to the right of a line is how everyone ends
    // a line in a text editor, and a rect the exact width of the glyphs makes
    // that a no-op. The text is still painted at the left edge of the rect.
    let width = ui.available_width().max(galley.size().x);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, galley.size().y), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::Text);
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

    let mut hit = LineHit {
        rect,
        galley,
        map,
        clicked: None,
    };
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            hit.clicked = hit.offset_at(pos);
        }
    }
    hit
}

/// Map a click that landed on no line at all onto the nearest one.
///
/// Lines are separated by real gaps, and the document ends well above the
/// bottom of the window, so a good part of the editor is not covered by any
/// line's rect. Clicking there should still move the caret — below the last
/// line means the end of the document, exactly as it does anywhere else.
fn nearest_offset(lines: &[LineHit], pos: egui::Pos2) -> Option<usize> {
    lines
        .iter()
        .min_by(|a, b| {
            a.rect
                .distance_sq_to_pos(pos)
                .total_cmp(&b.rect.distance_sq_to_pos(pos))
        })?
        .offset_at(pos)
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


#[cfg(test)]
mod tests {
    use super::*;
    use notepad_core::Editor;
    use std::sync::Mutex;

    /// The real GUI, driven headlessly.
    ///
    /// A plain `egui::Context` is enough — these assert on state, not pixels,
    /// so none of the rasteriser the snapshot tests need is involved.
    struct Headless {
        ctx: egui::Context,
        state: TestGui,
    }

    impl Headless {
        fn new() -> Headless {
            let editor: Shared = Arc::new(Mutex::new(Editor::with_text("# Notes\n\nsome text")));
            Headless {
                ctx: egui::Context::default(),
                state: TestGui::new(editor, false),
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(700.0, 400.0),
                )),
                events,
                ..Default::default()
            };
            let Headless { ctx, state } = self;
            let _ = ctx.run_ui(input, |ui| draw_frame_for_test(ui, state));
        }

        fn settle(&mut self) {
            for _ in 0..3 {
                self.frame(Vec::new());
            }
        }

        fn focused(&self) -> Option<egui::Id> {
            self.ctx.memory(|memory| memory.focused())
        }
    }

    fn press(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn the_document_holds_keyboard_focus_so_keys_never_reach_the_host() {
        let mut gui = Headless::new();
        gui.settle();
        // egui-baseview reports a key as used only when this is true, and
        // baseview hands anything reported unused to `super.keyDown:`, which
        // walks the responder chain into the DAW. This one boolean is the
        // whole difference between typing in the notepad and playing the set.
        assert!(
            gui.ctx.egui_wants_keyboard_input(),
            "nothing holds egui focus, so every keystroke would be passed to the host"
        );
    }

    #[test]
    fn tab_and_arrows_stay_with_the_document_instead_of_moving_focus() {
        let mut gui = Headless::new();
        gui.settle();
        let before = gui.focused().expect("the document should hold focus");

        // Without the lock filter egui treats these as focus navigation, so Tab
        // would move focus to the toolbar rather than indenting a list item —
        // and once focus left, keystrokes would start reaching the host again.
        for key in [
            egui::Key::Tab,
            egui::Key::ArrowLeft,
            egui::Key::ArrowRight,
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
        ] {
            gui.frame(vec![press(key)]);
            assert_eq!(
                gui.focused(),
                Some(before),
                "{key:?} moved focus away from the document"
            );
            assert!(gui.ctx.egui_wants_keyboard_input(), "{key:?} lost focus");
        }
    }
}
