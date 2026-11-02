//! `GhosttyView` — the embeddable GPUI component: owns the shared terminal
//! state and the PTY, translates input, and renders each frame from a
//! libghostty-vt render-state snapshot.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::sync::{Arc, Mutex};

use gpui::{
    App, AppContext as _, BorderStyle, Bounds, Element, ElementId, Entity, FocusHandle, Focusable,
    GlobalElementId, InspectorElementId, InteractiveElement, IntoElement, KeyDownEvent, LayoutId,
    MouseButton, ParentElement, Pixels, Point, Render, ScrollWheelEvent, ShapedLine, SharedString,
    StrikethroughStyle, Styled, TextAlign, TextRun, UnderlineStyle, Window, fill, outline, point,
    px, relative, rgba, size,
};
use unicode_width::UnicodeWidthStr;

use crate::input;
use crate::pty::Pty;
use crate::terminal::{KeyEncoder, RenderState, Rgb, Terminal};

const FONT_FAMILY: &str = "Menlo";
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 1.3;
const INITIAL_COLS: u16 = 80;
const INITIAL_ROWS: u16 = 24;
const THEME_FG: Rgb = Rgb::new(0xd4, 0xd4, 0xd4);
const THEME_BG: Rgb = Rgb::new(0x10, 0x13, 0x18);

/// Events from the terminal world into the view's pump task.
enum TermEvent {
    /// PTY output was folded into the terminal; repaint.
    Wakeup,
    /// OSC title changed; re-read it on the UI thread.
    Title,
    /// Child exited.
    Exited,
}

/// The lock-protected terminal state, shared between the PTY reader thread,
/// the effect callbacks, and the render path.
pub struct TermShared {
    pub inner: Mutex<InnerState>,
    events: async_channel::Sender<TermEvent>,
}

pub struct InnerState {
    pub term: Terminal,
    pub rs: RenderState,
}

impl TermShared {
    fn new(
        cols: u16,
        rows: u16,
        events: async_channel::Sender<TermEvent>,
    ) -> Result<Self, std::ffi::c_int> {
        let term = Terminal::new(cols, rows)?;
        term.set_default_colors(
            Rgb::new(THEME_FG.r, THEME_FG.g, THEME_FG.b),
            Rgb::new(THEME_BG.r, THEME_BG.g, THEME_BG.b),
        );
        let rs = RenderState::new()?;
        Ok(Self {
            inner: Mutex::new(InnerState { term, rs }),
            events,
        })
    }

    fn events_clone(&self) -> async_channel::Sender<TermEvent> {
        self.events.clone()
    }
}

pub struct GhosttyView {
    shared: Arc<TermShared>,
    pty: Arc<Pty>,
    focus: FocusHandle,
    enc: RefCell<KeyEncoder>,
    /// Grid size the terminal+pty are currently resized to; render-driven.
    last_grid: Cell<(u16, u16)>,
    /// Live OSC title, mirrored by the pump on the UI thread.
    live_title: RefCell<Option<String>>,
    exited: Cell<bool>,
}

impl Focusable for GhosttyView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl GhosttyView {
    /// Spawn a `$SHELL` PTY rooted at `cwd` and return the ready view, with
    /// its output pump already running.
    pub fn spawn(cwd: &Path, cx: &mut App) -> Result<Entity<Self>, String> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let (pty_inner, reader) = Pty::spawn_program(&shell, &[], cwd, INITIAL_COLS, INITIAL_ROWS)
            .map_err(|e| e.to_string())?;
        let pty = Arc::new(pty_inner);

        let (events, events_rx) = async_channel::unbounded::<TermEvent>();
        let shared = Arc::new(
            TermShared::new(INITIAL_COLS, INITIAL_ROWS, events)
                .map_err(|e| format!("ghostty init failed: {e}"))?,
        );

        // Terminal → PTY replies (queries, mode reports) go straight back out.
        shared.inner.lock().unwrap().term.set_write_pty({
            let pty = pty.clone();
            Box::new(move |bytes| pty.write(bytes))
        });
        // Title changes are marked in the callback and read back under the
        // lock on the UI thread (no terminal calls inside effect callbacks).
        shared.inner.lock().unwrap().term.set_on_title({
            let events = shared.events_clone();
            Box::new(move || {
                let _ = events.send_blocking(TermEvent::Title);
            })
        });

        // PTY reader thread: fold bytes into the emulator, then wake the UI.
        std::thread::spawn({
            let mut reader = reader;
            let shared = shared.clone();
            let events = shared.events_clone();
            move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            shared.inner.lock().unwrap().term.write(&buf[..n]);
                            if events.send_blocking(TermEvent::Wakeup).is_err() {
                                break;
                            }
                        }
                    }
                }
                let _ = events.send_blocking(TermEvent::Exited);
            }
        });
        // Child exit watcher.
        std::thread::spawn({
            let pty = pty.clone();
            let events = shared.events_clone();
            move || {
                let _ = pty.wait();
                let _ = events.send_blocking(TermEvent::Exited);
            }
        });

        let view = cx.new(|cx| {
            let view = GhosttyView {
                shared: shared.clone(),
                pty,
                focus: cx.focus_handle(),
                enc: RefCell::new(
                    KeyEncoder::new()
                        .map_err(|e| format!("key encoder: {e}"))
                        .expect("key encoder"),
                ),
                last_grid: Cell::new((0, 0)),
                live_title: RefCell::new(None),
                exited: Cell::new(false),
            };
            view.start_pump(events_rx, cx);
            view
        });
        Ok(view)
    }

    fn start_pump(&self, rx: async_channel::Receiver<TermEvent>, cx: &mut gpui::Context<Self>) {
        cx.spawn(async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                if this.update(cx, |view, cx| view.on_event(ev, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_event(&self, ev: TermEvent, cx: &mut gpui::Context<Self>) {
        match ev {
            TermEvent::Wakeup => cx.notify(),
            TermEvent::Title => {
                let title = self.shared.inner.lock().unwrap().term.title();
                *self.live_title.borrow_mut() = title;
                cx.notify();
            }
            TermEvent::Exited => {
                self.exited.set(true);
                cx.notify();
            }
        }
    }

    /// The terminal's OSC 0/2 title, when the program has set one. `None`
    /// until then — embedders show their own fallback label.
    pub fn live_title(&self) -> Option<String> {
        self.live_title.borrow().clone()
    }

    pub fn exited(&self) -> bool {
        self.exited.get()
    }

    /// Render-driven resize: called from the element's prepaint with the grid
    /// size derived from the current bounds. Same-frame, no notify.
    fn prepare_grid(&self, cols: u16, rows: u16, cell_w_px: u32, cell_h_px: u32) {
        let last = self.last_grid.get();
        if last == (cols, rows) {
            return;
        }
        self.last_grid.set((cols, rows));
        {
            let inner = self.shared.inner.lock().unwrap();
            let _ = inner.term.resize(cols, rows, cell_w_px, cell_h_px);
        }
        self.pty.resize(cols, rows);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut gpui::Context<Self>) {
        let ks = &ev.keystroke;
        let mods = &ks.modifiers;

        if mods.platform {
            // cmd+v pastes through the terminal (bracketed-paste aware);
            // every other cmd combination stays with the window.
            if ks.key == "v"
                && let Some(text) = cx.read_from_clipboard().and_then(|c| c.text())
            {
                let _ = self.shared.inner.lock().unwrap().term.paste(&text);
            }
            return;
        }
        if mods.function {
            return;
        }

        let key = input::map_key(&ks.key);
        let gmods = input::ghostty_mods(mods);
        // Text only for plain-ish keys; the encoder derives control sequences
        // from the logical key + mods itself.
        let utf8 = ks.key_char.clone().filter(|t| !t.is_empty());
        let bytes = {
            let inner = self.shared.inner.lock().unwrap();
            let mut enc = self.enc.borrow_mut();
            enc.sync_from_terminal(&inner.term);
            enc.encode_press(key, gmods, utf8.as_deref())
        };
        if !bytes.is_empty() {
            self.pty.write(&bytes);
        }
    }

    fn on_wheel(
        &mut self,
        ev: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let lh = FONT_SIZE * LINE_HEIGHT;
        let dy = ev.delta.pixel_delta(px(lh)).y;
        let lines = (dy / px(lh)).round();
        if lines != 0. {
            let inner = self.shared.inner.lock().unwrap();
            inner.term.scroll_delta(lines as isize);
        }
        cx.notify();
    }
}

impl Render for GhosttyView {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        gpui::div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus, cx);
                }),
            )
            .on_key_down(cx.listener(Self::on_key))
            .on_scroll_wheel(cx.listener(Self::on_wheel))
            .bg(rgba(0x101318ff))
            .child(GhosttyElement { view: cx.entity() })
    }
}

/// Convenience: spawn at `cwd`, falling back to home when it does not exist
/// (the shell cannot start in a deleted directory).
pub fn spawn_ghostty_view(cwd: &Path, cx: &mut App) -> Result<Entity<GhosttyView>, String> {
    let cwd = if cwd.is_dir() {
        cwd.to_path_buf()
    } else {
        std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| Path::new(".").to_path_buf())
    };
    GhosttyView::spawn(&cwd, cx)
}

// ---------------------------------------------------------------------------
// GhosttyElement — paint-only element: snapshot → shape in prepaint, emit in paint
// ---------------------------------------------------------------------------

struct GhosttyElement {
    view: Entity<GhosttyView>,
}

struct CursorPlan {
    bounds: Bounds<Pixels>,
    style: i32,
    color: gpui::Rgba,
    /// The character under a block cursor, repainted inverted on top.
    cell_text: Option<String>,
}

struct PrepaintState {
    bounds: Bounds<Pixels>,
    line_h: Pixels,
    runs: Vec<(Point<Pixels>, ShapedLine)>,
    bg_quads: Vec<(Bounds<Pixels>, gpui::Rgba)>,
    cursor: Option<CursorPlan>,
    cursor_text: Option<(Point<Pixels>, ShapedLine)>,
}

fn gpui_color(c: Rgb) -> gpui::Rgba {
    rgba(((c.r as u32) << 24) | ((c.g as u32) << 16) | ((c.b as u32) << 8) | 0xff)
}

fn faint(c: gpui::Rgba) -> gpui::Rgba {
    c.alpha(0.55)
}

fn font(style: crate::terminal::Cell) -> gpui::Font {
    gpui::Font {
        family: FONT_FAMILY.into(),
        features: Default::default(),
        weight: if style.bold {
            gpui::FontWeight::BOLD
        } else {
            gpui::FontWeight::NORMAL
        },
        style: if style.italic {
            gpui::FontStyle::Italic
        } else {
            gpui::FontStyle::Normal
        },
        fallbacks: None,
    }
}

impl IntoElement for GhosttyElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for GhosttyElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some("ghostty-grid".into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = gpui::Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        let layout_id = window.request_layout(style, std::iter::empty(), cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let font_size = px(FONT_SIZE);
        let line_h = px(FONT_SIZE * LINE_HEIGHT);

        // Measure the cell advance from one monospace glyph.
        let probe = TextRun {
            len: 1,
            font: gpui::Font {
                family: FONT_FAMILY.into(),
                ..Default::default()
            },
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped_m = window.text_system().shape_line(
            SharedString::from("m"),
            font_size,
            std::slice::from_ref(&probe),
            None,
        );
        let cell_w = shaped_m.width().max(px(1.));

        let cols = ((bounds.size.width / cell_w).floor() as u16).max(1);
        let rows = ((bounds.size.height / line_h).floor() as u16).max(1);

        self.view.update(cx, |v, _| {
            v.prepare_grid(
                cols,
                rows,
                (f32::from(cell_w) as u32).max(1),
                (f32::from(line_h) as u32).max(1),
            );
        });

        // Snapshot the grid under the lock, then release before shaping.
        let frame = self.view.update(cx, |v, _| {
            let mut inner = v.shared.inner.lock().unwrap();
            let InnerState { term, rs } = &mut *inner;
            rs.snapshot(term).ok()
        });

        let mut runs: Vec<(Point<Pixels>, ShapedLine)> = Vec::new();
        let mut bg_quads: Vec<(Bounds<Pixels>, gpui::Rgba)> = Vec::new();
        let mut cursor: Option<CursorPlan> = None;
        let mut cursor_text: Option<(Point<Pixels>, ShapedLine)> = None;

        let Some(frame) = frame else {
            return PrepaintState {
                bounds,
                line_h,
                runs,
                bg_quads,
                cursor,
                cursor_text,
            };
        };

        let default_bg = gpui_color(frame.colors.background);
        let default_fg = gpui_color(frame.colors.foreground);

        for (y, row) in frame.cells.iter().enumerate() {
            let row_y = bounds.origin.y + (y as f32) * line_h;

            for (x, cell) in row.iter().enumerate() {
                let pos = point(bounds.origin.x + (x as f32) * cell_w, row_y);

                // Cell backgrounds that differ from the default get a quad;
                // inverse cells render the foreground as their background.
                let raw_fg = gpui_color(cell.fg);
                let cell_bg = if cell.inverse {
                    raw_fg
                } else {
                    gpui_color(cell.bg)
                };
                if cell_bg != default_bg {
                    bg_quads.push((Bounds::new(pos, size(cell_w, line_h)), cell_bg));
                }
            }

            // Group same-style runs (split at wide chars, whose advance is
            // doubled instead of forced to the narrow cell width; and at
            // inversion boundaries — grouping by raw colors would swallow
            // inverse cells into a plain run and paint them invisible).
            let mut x = 0usize;
            while x < row.len() {
                let cell = &row[x];
                if cell.text.is_empty() || cell.invisible {
                    x += 1;
                    continue;
                }
                let wide = UnicodeWidthStr::width(cell.text.as_str()) > 1;

                let (mut fg, mut bg) = (gpui_color(cell.fg), gpui_color(cell.bg));
                if cell.inverse {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if cell.faint {
                    fg = faint(fg);
                }
                let sig = (
                    fg,
                    cell.bold,
                    cell.italic,
                    cell.underline,
                    cell.strikethrough,
                    wide,
                    cell.invisible,
                );

                let mut text = String::new();
                let mut end = x;
                while end < row.len() {
                    let c = &row[end];
                    if c.text.is_empty() || c.invisible {
                        break;
                    }
                    let c_wide = UnicodeWidthStr::width(c.text.as_str()) > 1;
                    let mut c_fg = if c.inverse {
                        gpui_color(c.bg)
                    } else {
                        gpui_color(c.fg)
                    };
                    if c.faint {
                        c_fg = faint(c_fg);
                    }
                    let c_sig = (
                        c_fg,
                        c.bold,
                        c.italic,
                        c.underline,
                        c.strikethrough,
                        c_wide,
                        c.invisible,
                    );
                    if c_sig != sig {
                        break;
                    }
                    text.push_str(&c.text);
                    end += 1;
                }

                let cell_font = font(cell.clone());
                let text_run = TextRun {
                    len: text.len(),
                    font: cell_font,
                    color: fg.into(),
                    background_color: None,
                    underline: (cell.underline != crate::sys::GHOSTTY_SGR_UNDERLINE_NONE)
                        .then_some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(fg.into()),
                            wavy: false,
                        }),
                    strikethrough: cell.strikethrough.then_some(StrikethroughStyle {
                        thickness: px(1.),
                        color: Some(fg.into()),
                    }),
                };
                let advance = if wide { cell_w * 2. } else { cell_w };
                let shaped = window.text_system().shape_line(
                    SharedString::from(text.as_str()),
                    px(FONT_SIZE),
                    std::slice::from_ref(&text_run),
                    Some(advance),
                );
                runs.push((point(bounds.origin.x + (x as f32) * cell_w, row_y), shaped));
                x = end;
            }
        }

        // Cursor: a block inverts its cell; other styles are quads only.
        if let Some(cur) = &frame.cursor
            && cur.visible
            && (cur.x as usize) < cols as usize
            && (cur.y as usize) < rows as usize
        {
            let pos = point(
                bounds.origin.x + (cur.x as f32) * cell_w,
                bounds.origin.y + (cur.y as f32) * line_h,
            );
            let cell_bounds = Bounds::new(
                pos,
                size(if cur.wide_tail { cell_w * 2. } else { cell_w }, line_h),
            );
            let color = frame.colors.cursor.map(gpui_color).unwrap_or(default_fg);

            match cur.style {
                crate::sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK => {
                    let cell = frame
                        .cells
                        .get(cur.y as usize)
                        .and_then(|r| r.get(cur.x as usize));
                    let inverted = cell.map(|c| {
                        if c.invisible {
                            None
                        } else {
                            Some(c.text.clone())
                        }
                    });
                    cursor = Some(CursorPlan {
                        bounds: cell_bounds,
                        style: cur.style,
                        color,
                        cell_text: inverted.flatten().filter(|t| !t.is_empty()),
                    });
                    if let (Some(text), Some(c)) = (&cursor.as_ref().unwrap().cell_text, cell) {
                        let text_run = TextRun {
                            len: text.len(),
                            font: font(c.clone()),
                            color: default_bg.into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let shaped = window.text_system().shape_line(
                            SharedString::from(text.as_str()),
                            px(FONT_SIZE),
                            std::slice::from_ref(&text_run),
                            Some(cell_w),
                        );
                        cursor_text = Some((pos, shaped));
                    }
                }
                crate::sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => {
                    cursor = Some(CursorPlan {
                        bounds: Bounds::new(
                            point(pos.x, pos.y + line_h - px(2.)),
                            size(cell_w, px(2.)),
                        ),
                        style: cur.style,
                        color,
                        cell_text: None,
                    });
                }
                crate::sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => {
                    cursor = Some(CursorPlan {
                        bounds: Bounds::new(pos, size(px(2.), line_h)),
                        style: cur.style,
                        color,
                        cell_text: None,
                    });
                }
                _ => {
                    // Hollow block (focus lost / IME).
                    cursor = Some(CursorPlan {
                        bounds: cell_bounds,
                        style: cur.style,
                        color,
                        cell_text: None,
                    });
                }
            }
        }

        PrepaintState {
            bounds,
            line_h,
            runs,
            bg_quads,
            cursor,
            cursor_text,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Default background.
        window.paint_quad(fill(prepaint.bounds, rgba(0x101318ff)));

        for (bounds, color) in &prepaint.bg_quads {
            window.paint_quad(fill(*bounds, *color));
        }

        for (pos, shaped) in &prepaint.runs {
            let _ = shaped.paint(*pos, prepaint.line_h, TextAlign::Left, None, window, cx);
        }

        if let Some(cur) = &prepaint.cursor {
            if cur.style == crate::sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK {
                window.paint_quad(fill(cur.bounds, cur.color));
            } else if cur.style == crate::sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW
            {
                window.paint_quad(outline(cur.bounds, cur.color, BorderStyle::default()));
            } else {
                window.paint_quad(fill(cur.bounds, cur.color));
            }
        }
        if let Some((pos, shaped)) = &prepaint.cursor_text {
            let _ = shaped.paint(*pos, prepaint.line_h, TextAlign::Left, None, window, cx);
        }
    }
}
