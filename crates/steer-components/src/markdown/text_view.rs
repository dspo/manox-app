//! The app's single `TextView` assembly point.
//!
//! Every markdown surface in the app — message bodies, reasoning rounds,
//! static chrome, terminal panels — renders through the helpers here, so the
//! theme bridge, the heading ladders, the code-highlight dispatch (`ansi` /
//! `diff` / conflict markers / tree-sitter), the inline diagram blocks
//! (```svg / ```mermaid), and the copy controls stay consistent by construction
//! instead of by per-call-site discipline.
//!
//! This drives the `gpui-base` `TextView` directly (same crates.io train as
//! `gpui-component`, exact-pinned): the component wrapper folds onto the theme
//! but pins its own heading weights, and the app's heading ladder is
//! load-bearing here. The code-block highlighter is installed once as the
//! app-wide `TextViewDefaults` highlighter ([`install_code_highlighter`]),
//! never per document — see that function for why.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, App, DispatchPhase, ElementId, Entity, FontStyle, FontWeight, HighlightStyle, Hsla,
    IntoElement, MouseMoveEvent, Pixels, SharedString, Styled as _, WeakEntity, Window, hsla, rems,
};
use gpui_base::TextSelection;
use gpui_base::TextView;
use gpui_base::text::{CodeBlock, TableData, TextViewMotion, TextViewState};
use gpui_component::Theme;
use gpui_component::highlighter::{HighlightTheme, SyntaxHighlighter};
use ropey::Rope;

use crate::code_surface::is_conflict;
use crate::copy_feedback::{CopyFeedbackHost, copy_button};
use crate::markdown::linkify::{linkify_markdown, pin_reference_definitions};
use crate::markdown::svg_block;

// --- theme bridge ------------------------------------------------------------

/// The app document style shared by every surface: theme colors, tight
/// paragraph gap, per-surface heading ladder. Code blocks keep the old
/// transparent-on-message look (hairline via `border`, no filled panel).
fn base_style(theme: &Theme) -> gpui_base::TextViewStyle {
    gpui_base::TextViewStyle::default()
        .with_foreground(theme.foreground)
        .with_muted_foreground(theme.muted_foreground)
        .with_link(theme.link)
        .with_selection(theme.selection)
        .with_code_background(theme.muted)
        .with_border(theme.border)
        .with_paragraph_gap(rems(0.5))
        .with_inline_code(HighlightStyle {
            color: Some(theme.info),
            background_color: Some(theme.accent.opacity(0.6)),
            ..Default::default()
        })
        .with_dark(theme.is_dark())
}

/// Heading typography rides upstream's relative ladder (0.8.0 heading
/// hierarchy: size, line height and section spacing all scale with the
/// inherited font size, weight uniform semibold, H6 muted) — the app pins no
/// heading overrides of its own, so the ladder tracks the body size each
/// mount sets.
pub fn message_style(theme: &Theme) -> gpui_base::TextViewStyle {
    base_style(theme)
}

/// Heading typography as in [`message_style`]: upstream's ladder, unpinned.
pub fn chrome_style(theme: &Theme) -> gpui_base::TextViewStyle {
    base_style(theme)
}

/// Transparent-surface style for terminal panels: the panel body reads as
/// inline text on the message card, not a filled code panel.
pub fn panel_style(theme: &Theme) -> gpui_base::TextViewStyle {
    message_style(theme).with_code_background(hsla(0., 0., 0., 0.))
}

/// End a text-selection drag as soon as the pointer moves with no button
/// held. Register once per frame from a view that paints whenever selectable
/// markdown is visible.
///
/// Why: the gesture arms on every left `mouse_down` — including the one that
/// opens a popup menu — and ends on `mouse_up`. A menu item's click handler
/// stops propagation, so the window-level selection engine never sees that
/// `mouse_up`; its move handler extends the selection while the gesture is
/// armed without checking whether a button is still down, and the pointer's
/// next travel across the transcript selects whatever it crosses. `end` is the
/// gesture's own normal exit (it keeps a selection that hit text) and is a
/// no-op while idle; a real drag always holds the left button, so it never
/// trips this.
pub fn install_selection_drag_guard(window: &mut Window) {
    window.on_mouse_event(
        |event: &MouseMoveEvent, _: DispatchPhase, window: &mut Window, cx: &mut App| {
            if event.pressed_button.is_none() {
                TextSelection::end(window, cx);
            }
        },
    );
}

/// Install this crate's code-block highlighter as the application-wide
/// `TextViewDefaults` highlighter.
///
/// Call after `gpui_component::init` and after every
/// `gpui_component::Theme::change` / `Theme::update` — both end in
/// `Theme::sync_base`, which reinstalls Component's own defaults.
///
/// Documents take the highlighter from here and never pass one per view:
/// gpui-base compares the resolved highlighter by `Arc` identity and treats a
/// different one as a reflow that must preserve the selection. An element is
/// rebuilt every frame, so a per-view closure is a new `Arc` every frame; that
/// keeps the selection-preserving reflow armed forever, and the document ends
/// up with no selection at all (a drag paints nothing and copies nothing).
pub fn install_code_highlighter(cx: &mut App) {
    let theme = Theme::global(cx).clone();
    gpui_base::TextViewDefaults::global(cx)
        .with_code_block_highlighter(steer_code_highlighter(&theme))
        .install(cx);
}

// --- document constructors ----------------------------------------------------

/// A persistent markdown document plus the source cache that keeps streaming
/// deltas on upstream's incremental path.
///
/// Upstream `set_text` re-parses from an empty baseline every call — only
/// `push_str` takes the incremental append path. This type remembers the last
/// raw text and its linkified form, so a delta whose linkified source merely
/// appends feeds just the suffix through `push_str` (and lands the more
/// accurate extend-style fade); a mid-stream linkify rewrite (a URL completing)
/// or a wholesale rewrite falls back to `set_text`. An unchanged raw source
/// skips both the rewrite and the parse.
#[derive(Clone, Default)]
pub struct ChatDocument {
    state: Option<Entity<TextViewState>>,
    raw: Option<String>,
    linkified: Option<String>,
}

impl std::fmt::Debug for ChatDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatDocument")
            .field("mounted", &self.state.is_some())
            .field("raw_len", &self.raw.as_ref().map(String::len))
            .finish()
    }
}

impl ChatDocument {
    /// Mount (or adopt) the document for `text`, reusing the existing entity
    /// and incremental path when already present.
    ///
    /// `text` runs through [`pin_reference_definitions`] before linkify, so a
    /// reference definition arriving mid-stream cannot flip already-rendered
    /// text and shrink a leaf under recorded fade ranges (the
    /// `invalid text run` aborts; see the pinning doc for the mechanism). A
    /// line whose shape the pinning changes as it grows — a definition
    /// completing, a fence closing — diverges the transformed source and falls
    /// to `set_text`, whose replace clears the stale fade segments wholesale.
    pub fn sync(&mut self, text: &str, cx: &mut impl gpui::AppContext) {
        if self.raw.as_deref() == Some(text) {
            return;
        }
        let next = linkify_markdown(&pin_reference_definitions(text));
        let state = match &self.state {
            Some(state) => state.clone(),
            None => {
                let state = cx.new(|cx| TextViewState::markdown(&next, cx));
                self.state = Some(state.clone());
                self.raw = Some(text.to_string());
                self.linkified = Some(next);
                return;
            }
        };
        let appended = self
            .linkified
            .as_ref()
            .filter(|prev| next.len() > prev.len() && next.starts_with(prev.as_str()))
            .map(|prev| next[prev.len()..].to_string());
        match appended {
            Some(delta) => state.update(cx, |s, cx| s.push_str(&delta, cx)),
            None => state.update(cx, |s, cx| s.set_text(&next, cx)),
        }
        self.raw = Some(text.to_string());
        self.linkified = Some(next);
    }

    /// The mounted document entity, if [`Self::sync`] has run.
    pub fn state(&self) -> Option<&Entity<TextViewState>> {
        self.state.as_ref()
    }
}

/// A persistent markdown document rendered with message density. The element
/// is rebuilt every frame; the state entity carries content, selection and
/// focus. Chain [`TextView::on_link_click`], [`TextView::code_block_actions`]
/// and [`TextView::table_actions`] at the call site where the copy-feedback
/// host is known.
///
/// `cx` is the app because an ```svg or ```mermaid fence is rasterized at parse
/// time (see [`svg_block`]); the renderer is read here, at the assembly point,
/// and carried by the document.
pub fn message_document(
    state: &Entity<TextViewState>,
    theme: &Theme,
    body: Pixels,
    cx: &App,
) -> TextView {
    let style = message_style(theme);
    svg_block::with_fence_blocks(
        TextView::new(state).style(style.clone()).text_size(body),
        Some(body),
        cx,
    )
}

/// The reasoning tier — the message document in the muted foreground. A round
/// is secondary by color, not by a size of its own: the transcript carries one
/// body size, so a round's body matches the message body it belongs to.
pub fn reasoning_document(
    state: &Entity<TextViewState>,
    theme: &Theme,
    body: Pixels,
    cx: &App,
) -> TextView {
    let style = message_style(theme).with_foreground(theme.muted_foreground);
    svg_block::with_fence_blocks(
        TextView::new(state).style(style.clone()).text_size(body),
        Some(body),
        cx,
    )
}

/// A per-frame static markdown document. Upstream keys the state off `id`, so
/// selection persists across frames without the caller owning an entity.
pub fn static_document(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    theme: &Theme,
    cx: &App,
) -> TextView {
    svg_block::with_fence_blocks(
        TextView::markdown(id, linkify_markdown(text.into().as_ref())).style(chrome_style(theme)),
        None,
        cx,
    )
}

/// A code-block highlighter that can be handed to a document every frame.
///
/// Identity is load-bearing: gpui-base reuses a code block's highlights only
/// while the highlighter is the same `Arc`, and a different one re-arms the
/// selection-preserving reflow path — see [`install_code_highlighter`]. Cache
/// one per distinct run set instead of rebuilding it per render.
pub type CodeHighlighter =
    Arc<dyn Fn(&CodeBlock) -> Vec<(Range<usize>, HighlightStyle)> + Send + Sync>;

/// The terminal-panel body's highlight runs: a muted-italic base over the whole
/// text, with the caller's runs layered on top so unpainted ranges read as
/// secondary text.
pub fn panel_styled_runs(
    text: &str,
    runs: Vec<(Range<usize>, HighlightStyle)>,
    theme: &Theme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut styled = Vec::with_capacity(runs.len() + 1);
    styled.push((
        0..text.len(),
        HighlightStyle {
            color: Some(theme.muted_foreground),
            font_style: Some(FontStyle::Italic),
            ..Default::default()
        },
    ));
    styled.extend(runs);
    styled
}

/// The highlighter for one panel body's styled runs.
pub fn panel_highlighter(styled: Vec<(Range<usize>, HighlightStyle)>) -> CodeHighlighter {
    Arc::new(move |block: &CodeBlock| clamp_runs(layer_runs(styled.clone()), &block.code()))
}

/// A per-frame document rendering raw text verbatim inside one `ansi` fence —
/// the terminal-panel body host. `text` is the composed source and
/// `highlighter` carries its runs ([`panel_styled_runs`] +
/// [`panel_highlighter`], cached by the panel so the `Arc` survives frames).
/// The motion argument carries the streaming fade policy (default motion when
/// not streaming).
pub fn panel_document(
    id: impl Into<ElementId>,
    text: &str,
    highlighter: CodeHighlighter,
    theme: &Theme,
    motion: TextViewMotion,
) -> TextView {
    let fence = "`".repeat(fence_len(text));
    let mut source = String::with_capacity(text.len() + fence.len() * 2 + 8);
    source.push_str(&fence);
    source.push_str("ansi\n");
    source.push_str(text);
    if !text.ends_with('\n') {
        source.push('\n');
    }
    source.push_str(&fence);

    TextView::markdown(id, source)
        .style(panel_style(theme))
        .motion(motion)
        .shared_code_block_highlighter(highlighter)
}

/// The streaming fade policy measured upstream from claude.ai (~6-word chunks,
/// ~280 ms fade, 10 ms stagger): new text lights up word by word; a large
/// backfill fades as one block.
pub fn stream_fade_motion() -> TextViewMotion {
    TextViewMotion::default()
        .with_stream_fade(std::time::Duration::from_millis(280))
        .with_stream_fade_stagger(std::time::Duration::from_millis(10))
}

/// Fold overlapping highlight runs into disjoint, ascending ranges. The code
/// block highlight channel feeds runs straight into the text run builder,
/// which requires exactly that shape — no merging happens upstream. Later
/// ranges override only the fields they set, so the base run's italic slant
/// survives under an ANSI color run that pins no slant of its own (the same
/// field-wise merge the old renderer applied).
///
/// Single sweep: candidates stably sorted by start (ties keep feed order, so
/// the base run still folds first), an in-order active list carries every
/// range covering the current position, and each segment between boundaries
/// folds the active list front to back. Later layers win per field because
/// they fold after earlier ones.
fn layer_runs(
    mut runs: Vec<(Range<usize>, HighlightStyle)>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    runs.retain(|(range, _)| !range.is_empty());
    if runs.is_empty() {
        return runs;
    }
    runs.sort_by_key(|(range, _)| range.start);
    let max_end = runs.iter().map(|(r, _)| r.end).max().unwrap_or(0);

    // `active` keeps covering ranges in insertion (= feed) order; an entry
    // leaves once the sweep passes its end.
    let mut active: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut out: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut position = runs[0].0.start;
    // Cursor into the sorted candidates: every run is admitted exactly once,
    // when the sweep first reaches its start, so the sweep stays linear.
    let mut next = 0usize;
    while position < max_end {
        active.retain(|(range, _)| range.end > position);
        while next < runs.len() && runs[next].0.start <= position {
            active.push(runs[next].clone());
            next += 1;
        }
        let mut end = max_end;
        if let Some((range, _)) = runs.get(next) {
            end = end.min(range.start);
        }
        for (range, _) in &active {
            end = end.min(range.end);
        }
        let mut style = HighlightStyle::default();
        for (_, layer) in &active {
            style = style.highlight(*layer);
        }
        out.push((position..end, style));
        position = end;
    }
    out
}

/// Fence length for `text`: one longer than the longest backtick run inside,
/// floored at three, so no content line can close the fence early (GFM rule).
fn fence_len(text: &str) -> usize {
    let mut longest = 0usize;
    let mut run = 0usize;
    for b in text.bytes() {
        if b == b'`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    (longest + 1).max(3)
}

// --- copy controls -------------------------------------------------------------

/// The hover copy control for tables, copying the table's GFM serialization.
pub fn table_copy_actions<T: CopyFeedbackHost>(
    owner: WeakEntity<T>,
) -> impl Fn(&TableData, &mut Window, &mut App) -> AnyElement + Send + Sync + 'static {
    move |table: &TableData, _window: &mut Window, cx: &mut App| {
        copy_control(&owner, table.markdown.clone(), cx).into_any_element()
    }
}

fn copy_control<T: CopyFeedbackHost>(
    owner: &WeakEntity<T>,
    text: String,
    cx: &mut App,
) -> impl IntoElement {
    let id: ElementId = "copy".into();
    let active = owner
        .upgrade()
        .is_some_and(|entity| entity.update(cx, |host, _| host.copied().is_active(&id)));
    copy_button(id, text, active, owner.clone())
}

// --- code highlight dispatch -----------------------------------------------------

/// The app's code-block highlighter, handed to documents as the `TextViewDefaults`
/// highlighter. Code fences in prose render through the editor block
/// ([`crate::markdown::code_block`]) which calls [`dispatch_runs`] directly;
/// this closure serves the surfaces that still ride the upstream code block —
/// terminal panels, the svg block's code face — and exists because those
/// documents take their highlighter from the defaults.
///
/// Identity is load-bearing: gpui-base reuses a code block's highlights only
/// while the highlighter is the same `Arc` — see [`install_code_highlighter`]
/// for why it is installed once, never per document.
pub fn steer_code_highlighter(
    theme: &Theme,
) -> impl Fn(&CodeBlock) -> Vec<(Range<usize>, HighlightStyle)> + Send + Sync + 'static {
    let palette = DispatchPalette::new(theme);
    move |block: &CodeBlock| {
        let lang_holder = block.lang();
        dispatch_runs(&block.code(), lang_holder.as_deref(), &palette)
    }
}

/// The palette one dispatch call reads from the theme: the highlight theme
/// plus the five flat colors the line-based dispatches tint with. Plain data,
/// `Send + Sync` — the `TextViewDefaults` closure holds it across threads,
/// which [`Theme`] itself cannot (its config rides an `Rc`).
#[derive(Clone)]
pub(crate) struct DispatchPalette {
    highlight_theme: Arc<HighlightTheme>,
    foreground: Hsla,
    muted: Hsla,
    secondary: Hsla,
    add_fg: Hsla,
    del_fg: Hsla,
}

impl DispatchPalette {
    pub(crate) fn new(theme: &Theme) -> Self {
        Self {
            highlight_theme: theme.highlight_theme.clone(),
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            secondary: theme.secondary,
            add_fg: theme.success,
            del_fg: theme.danger,
        }
    }
}

/// The app's code-highlight dispatch over one block's raw text and language
/// token. `ansi` fences found in prose parse their escape sequences (without
/// them the content is plain and unhighlighted); fences carrying conflict
/// markers classify as conflicts regardless of language; `diff` fences color
/// per line; everything else highlights through tree-sitter, memoized by the
/// thread-local cache below — the message list re-renders every dirty frame,
/// so this cache is what keeps a long conversation from re-highlighting every
/// completed block on each streaming delta.
pub(crate) fn dispatch_runs(
    code: &str,
    lang: Option<&str>,
    palette: &DispatchPalette,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let lang = lang.unwrap_or("text");
    let runs = if lang.eq_ignore_ascii_case("ansi") {
        ansi_runs(code)
    } else if is_conflict(code) {
        conflict_runs(
            code,
            palette.foreground,
            palette.muted,
            palette.secondary,
            palette.add_fg,
            palette.del_fg,
        )
    } else if lang.eq_ignore_ascii_case("diff") {
        diff_runs(
            code,
            palette.foreground,
            palette.muted,
            palette.secondary,
            palette.add_fg,
            palette.del_fg,
        )
    } else {
        // An svg fence is markup, and markup is what the registry carries
        // for it: `svg` itself is unregistered, which would color nothing
        // and warn on every distinct fence.
        let lang = if lang.eq_ignore_ascii_case("svg") {
            "html"
        } else {
            lang
        };
        tree_sitter_runs(code, lang, &palette.highlight_theme)
    };
    normalize_runs(clamp_runs(runs, code))
}

/// Sort by start and clip overlaps so the list is always in the one shape
/// the run builder accepts (ascending, disjoint): upstream filters bounds
/// only, and a tree-sitter capture set that ever overlapped would reach
/// `StyledText::with_runs` unmerged — fade_highlights combines only while a
/// fade frame is active.
fn normalize_runs(
    mut runs: Vec<(Range<usize>, HighlightStyle)>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    runs.sort_by_key(|(range, _)| (range.start, range.end));
    let mut out: Vec<(Range<usize>, HighlightStyle)> = Vec::with_capacity(runs.len());
    for (mut range, style) in runs {
        if let Some((last, _)) = out.last() {
            if range.end <= last.end {
                continue;
            }
            range.start = range.start.max(last.end);
        }
        if range.start < range.end {
            out.push((range, style));
        }
    }
    out
}

/// Snap every highlight range onto the block's char boundaries, inside its
/// bounds. The run builder (`StyledText::with_runs`) aborts the process on a
/// range that ends mid-character — a real-device class of crash under
/// streaming CJK tool output — so the highlighter layer owns the invariant
/// the same way the runtime already drops out-of-bounds ranges: a boundary
/// that slipped snaps inward, and what collapses to empty leaves.
pub(crate) fn clamp_runs(
    runs: Vec<(Range<usize>, HighlightStyle)>,
    text: &str,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let len = text.len();
    runs.into_iter()
        .filter_map(|(mut range, style)| {
            range.start = range.start.min(len);
            range.end = range.end.min(len);
            while !text.is_char_boundary(range.start) {
                range.start += 1;
            }
            while range.end < len && !text.is_char_boundary(range.end) {
                range.end += 1;
            }
            (range.start < range.end).then_some((range, style))
        })
        .collect()
}

/// Per-line runs for a unified-diff fence: added/removed lines carry the
/// accent foreground and its faded wash, metadata lines read muted, context
/// keeps the foreground.
fn diff_runs(
    code: &str,
    foreground: Hsla,
    muted: Hsla,
    secondary: Hsla,
    add_fg: Hsla,
    del_fg: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut runs = Vec::new();
    for (offset, line) in line_ranges(code) {
        let (bg, fg) = if line.starts_with("diff ")
            || line.starts_with("index ")
            || line.starts_with("@@")
            || line.starts_with("+++ ")
            || line.starts_with("--- ")
            || line.starts_with("\\ ")
        {
            (Some(secondary), muted)
        } else if line.starts_with('+') {
            (Some(wash(add_fg)), add_fg)
        } else if line.starts_with('-') {
            (Some(wash(del_fg)), del_fg)
        } else {
            (None, foreground)
        };
        runs.push((
            offset..offset + line.len(),
            HighlightStyle {
                color: Some(fg),
                background_color: bg,
                ..Default::default()
            },
        ));
    }
    runs
}

/// Per-line runs for a merge-conflict fence: the four marker lines become bold
/// section headers tinted by their side; content lines carry their section's
/// wash.
fn conflict_runs(
    code: &str,
    foreground: Hsla,
    muted: Hsla,
    secondary: Hsla,
    add_fg: Hsla,
    del_fg: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    use ConflictSection::*;

    let mut runs = Vec::new();
    let mut state = Context;
    for (offset, line) in line_ranges(code) {
        let (color, bg, bold) = if line.starts_with("<<<<<<<") {
            state = Ours;
            (add_fg, Some(add_fg.opacity(0.22)), true)
        } else if line.starts_with("|||||||") {
            state = Base;
            (muted, Some(muted.opacity(0.18)), true)
        } else if line == "=======" {
            state = Theirs;
            (muted, Some(secondary), true)
        } else if line.starts_with(">>>>>>>") {
            state = Context;
            (del_fg, Some(del_fg.opacity(0.22)), true)
        } else {
            match state {
                Context => (foreground, None, false),
                Ours => (foreground, Some(wash(add_fg)), false),
                Base => (foreground, Some(secondary), false),
                Theirs => (foreground, Some(wash(del_fg)), false),
            }
        };
        runs.push((
            offset..offset + line.len(),
            HighlightStyle {
                color: Some(color),
                background_color: bg,
                font_weight: bold.then_some(FontWeight::BOLD),
                ..Default::default()
            },
        ));
    }
    runs
}

/// Which side of a conflict blob the current content lines belong to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConflictSection {
    Context,
    Ours,
    Base,
    Theirs,
}

/// `code` split into `(byte offset, line)` pairs, excluding line terminators —
/// highlight ranges cover the line content only.
fn line_ranges(code: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0usize;
    code.split_inclusive('\n').map(move |chunk| {
        let line = chunk.strip_suffix('\n').unwrap_or(chunk);
        let start = offset;
        offset += chunk.len();
        (start, line)
    })
}

/// The accent hue faded into a wash for line backgrounds.
fn wash(color: Hsla) -> Hsla {
    color.opacity(0.15)
}

/// ANSI-parsed runs over an `ansi` fence found in prose. Run ranges address
/// the escape-stripped text, so a fence with escapes highlights the wrong
/// bytes and falls back to nothing.
fn ansi_runs(code: &str) -> Vec<(Range<usize>, HighlightStyle)> {
    let (plain, runs) = crate::markdown::terminal_panel::parse_ansi(code);
    if plain.len() != code.len() {
        return Vec::new();
    }
    runs
}

// Per-(language, content, highlight theme) tree-sitter highlight cache. The
// message list re-renders every dirty frame; upstream's own highlight cache is
// keyed by highlighter identity and this dispatcher's closure is rebuilt per
// frame, so this cache is what keeps a long conversation from re-highlighting
// every completed block on each streaming delta. Lives on the render thread —
// `SyntaxHighlighter` owns a thread-local `Parser` and is not `Send`.
//
// The cache is bounded because a streaming block writes one entry per
// intermediate shape and none of them expire on their own: past [`CODE_HL_
// CACHE_MAX`] entries the table clears wholesale (completed blocks re-highlight
// once), and a single block above [`CODE_HL_CACHE_MAX_BYTES`] highlights but
// never caches, so one giant fence cannot evict the table for everyone.
type HighlightCache = HashMap<(u64, usize), Vec<(Range<usize>, HighlightStyle)>>;

thread_local! {
    static CODE_HL_CACHE: RefCell<HighlightCache> = RefCell::new(HashMap::new());
}

const CODE_HL_CACHE_MAX: usize = 1024;
const CODE_HL_CACHE_MAX_BYTES: usize = 64 * 1024;

/// Tree-sitter highlight runs for a bare code fragment (no fence) — the
/// terminal panel's file-shaped bodies share the markdown stack's dispatch
/// and thread-local cache.
pub(crate) fn tree_sitter_runs(
    value: &str,
    lang: &str,
    theme: &Arc<HighlightTheme>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let theme_ptr = Arc::as_ptr(theme) as usize;
    let mut hasher = DefaultHasher::new();
    lang.hash(&mut hasher);
    value.hash(&mut hasher);
    let key = (hasher.finish(), theme_ptr);
    if let Some(hit) = CODE_HL_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let rope = Rope::from_str(value);
    let mut hl = SyntaxHighlighter::new(&lang.to_ascii_lowercase());
    let _ = hl.update(None, &rope, None);
    let result = hl.styles(&(0..value.len()), theme.as_ref()).clone();
    if value.len() <= CODE_HL_CACHE_MAX_BYTES {
        CODE_HL_CACHE.with(|c| {
            let mut cache = c.borrow_mut();
            if cache.len() >= CODE_HL_CACHE_MAX {
                cache.clear();
            }
            cache.insert(key, result.clone());
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_len_grows_past_embedded_backtick_runs() {
        assert_eq!(fence_len("plain"), 3);
        assert_eq!(fence_len("has ``` inside"), 4);
        assert_eq!(fence_len("````\n"), 5);
    }

    /// The panel's ranges are byte offsets into the text it fenced; the
    /// highlighter receives the mdast-parsed block value. Any byte
    /// divergence between the two shifts every range — and once CJK
    /// follows, a boundary lands mid-character and `with_runs` aborts the
    /// app. The text carries the hazard shape: 2-space indents, ASCII
    /// columns, CJK mid-line.
    #[test]
    fn panel_ranges_match_the_parsed_code_block_byte_for_byte() {
        let text = concat!(
            "== synth/project-alpha :: fix/resize-behavior\n",
            "  PR!101 [merged] ->main 2026-01-02 fix(ui): 窗口缩放行为显式声明，缩放语义不再依赖默认值\n",
            "  PR!102 [merged] ->main 2026-01-03 docs: correct the config path to ~/.config/synth\n",
        );
        // The exact `panel_document` source construction.
        let fence = "`".repeat(fence_len(text));
        let mut source = String::new();
        source.push_str(&fence);
        source.push_str("ansi\n");
        source.push_str(text);
        if !text.ends_with('\n') {
            source.push('\n');
        }
        source.push_str(&fence);

        let options = markdown::ParseOptions::default();
        let root = markdown::to_mdast(&source, &options).expect("parses");
        let code = collect_first_code(&root).expect("a fenced code block");
        // The parser contract the panel's ranges ride on: the block value is
        // the fenced body minus exactly one trailing newline. Any further
        // divergence shifts every range — and once CJK follows, a boundary
        // lands mid-character and aborts the app.
        let expected = text.strip_suffix('\n').unwrap_or(text);
        assert_eq!(code.value, expected);
    }

    fn collect_first_code(node: &markdown::mdast::Node) -> Option<&markdown::mdast::Code> {
        use markdown::mdast::Node;
        match node {
            Node::Code(code) => Some(code),
            _ => node
                .children()
                .and_then(|children| children.iter().find_map(collect_first_code)),
        }
    }

    /// The end-to-end pure pipeline over a raw tool listing (a columned
    /// table with CJK cells): highlight remap (File kind) →
    /// `panel_document` fence → mdast block value → `layer_runs` → the same
    /// invariant `text_runs`+`with_runs` enforce: every range boundary must
    /// be a char boundary inside the block.
    #[test]
    fn a_tool_listing_produces_char_safe_panel_ranges() {
        let raw = include_str!("../../tests/fixtures/tool-table-output.txt");
        let parts: Vec<&str> = raw.strip_suffix('\n').unwrap_or(raw).split('\n').collect();
        // The settled panel's default visible window.
        let v = parts.len().min(400);

        // File kind: gutter + tree-sitter highlight remap — the layer with
        // the only non-trivial offset math in the pipeline.
        let mut s = String::new();
        let mut runs: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        let width = parts.len().to_string().len();
        let mut content_starts = Vec::with_capacity(v);
        for (i, line) in parts.iter().take(v).enumerate() {
            let line_start = s.len();
            let gutter = format!("{:>width$}  ", i + 1);
            let g_len = gutter.len();
            s.push_str(&gutter);
            runs.push((
                line_start..line_start + g_len,
                HighlightStyle {
                    color: Some(gpui::Hsla::default()),
                    ..Default::default()
                },
            ));
            content_starts.push(s.len());
            s.push_str(line);
            s.push('\n');
        }
        crate::markdown::terminal_panel::highlight_content(
            &mut runs,
            &parts[..v],
            &content_starts,
            "bash",
            &gpui_component::highlighter::HighlightTheme::default_light(),
        );

        // The panel_document fence over the composed text.
        let fence = "`".repeat(fence_len(&s));
        let mut source = String::new();
        source.push_str(&fence);
        source.push_str("ansi\n");
        source.push_str(&s);
        if !s.ends_with('\n') {
            source.push('\n');
        }
        source.push_str(&fence);
        let options = markdown::ParseOptions::default();
        let root = markdown::to_mdast(&source, &options).expect("parses");
        let code = collect_first_code(&root).expect("a fenced code block");
        let block = &code.value;

        let layered = layer_runs(runs);
        let mut bad = 0;
        for (range, _) in &layered {
            for (label, edge) in [("start", range.start), ("end", range.end)] {
                if edge > block.len() || !block.is_char_boundary(edge) {
                    if bad == 0 {
                        eprintln!(
                            "first bad range {range:?} ({label} at {edge}, block {} bytes)",
                            block.len()
                        );
                    }
                    bad += 1;
                }
            }
        }
        assert_eq!(
            bad, 0,
            "{bad} range edges are not char boundaries of the block"
        );
    }

    /// A CJK-heavy listing fenced exactly like `panel_document` does must
    /// come back from mdast verbatim: whatever byte transformation the
    /// parser applied to the block shows up as a divergence here, with the
    /// first differing window printed on char boundaries.
    #[test]
    fn a_fenced_cjk_listing_survives_the_fence_verbatim() {
        let text = include_str!("../../tests/fixtures/fenced-cjk-listing.txt");
        let fence = "`".repeat(fence_len(text));
        let mut source = String::new();
        source.push_str(&fence);
        source.push_str("ansi\n");
        source.push_str(text);
        if !text.ends_with('\n') {
            source.push('\n');
        }
        source.push_str(&fence);
        let options = markdown::ParseOptions::default();
        let root = markdown::to_mdast(&source, &options).expect("parses");
        let code = collect_first_code(&root).expect("a fenced code block");
        let expected = text.strip_suffix('\n').unwrap_or(text).to_string();
        if code.value != expected {
            let a = expected.as_bytes();
            let b = code.value.as_bytes();
            let diverge = a
                .iter()
                .zip(b.iter())
                .position(|(x, y)| x != y)
                .unwrap_or(a.len().min(b.len()));
            fn floor_boundary(s: &str, mut i: usize) -> usize {
                while i > 0 && !s.is_char_boundary(i) {
                    i -= 1;
                }
                i
            }
            fn ceil_boundary(s: &str, mut i: usize) -> usize {
                while i < s.len() && !s.is_char_boundary(i) {
                    i += 1;
                }
                i
            }
            let lo = floor_boundary(expected.as_str(), diverge.saturating_sub(24));
            let hi_a = ceil_boundary(expected.as_str(), (diverge + 24).min(expected.len()));
            let hi_b = ceil_boundary(&code.value, (diverge + 24).min(code.value.len()));
            panic!(
                "mdast transformed the block: {} vs {} bytes, first divergence at {}:\n  expected: {:?}\n  parsed:   {:?}",
                a.len(),
                b.len(),
                diverge,
                &expected[lo..hi_a],
                &code.value[lo..hi_b],
            );
        }
    }

    #[test]
    fn line_ranges_exclude_terminators() {
        let ranges: Vec<(usize, &str)> = line_ranges("ab\ncd\n").collect();
        assert_eq!(ranges, vec![(0, "ab"), (3, "cd")]);
    }

    #[test]
    fn layer_runs_keeps_feed_order_for_overlapping_starts() {
        // The panel feeds a base run first, then specific runs that share its
        // start: the specific layer must win color on its range while the base
        // slant survives underneath, and the tail keeps the base alone.
        let base = HighlightStyle {
            font_style: Some(FontStyle::Italic),
            ..Default::default()
        };
        let accent = HighlightStyle {
            color: Some(Hsla {
                h: 0.,
                s: 1.,
                l: 0.5,
                a: 1.,
            }),
            ..Default::default()
        };
        let layered = layer_runs(vec![(0..10, base), (0..4, accent)]);
        assert_eq!(layered.len(), 2);
        assert_eq!(layered[0].0, 0..4);
        assert_eq!(layered[0].1.color, accent.color);
        assert_eq!(layered[0].1.font_style, Some(FontStyle::Italic));
        assert_eq!(layered[1].0, 4..10);
        assert_eq!(layered[1].1.color, None);
        assert_eq!(layered[1].1.font_style, Some(FontStyle::Italic));
    }

    #[test]
    fn layer_runs_later_layer_wins_per_field() {
        let first = HighlightStyle {
            color: Some(Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 1.,
            }),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let second = HighlightStyle {
            color: Some(Hsla {
                h: 0.5,
                s: 0.,
                l: 0.5,
                a: 1.,
            }),
            ..Default::default()
        };
        let layered = layer_runs(vec![(0..6, first), (2..4, second)]);
        assert_eq!(layered.len(), 3);
        assert_eq!(layered[0].0, 0..2);
        assert_eq!(layered[0].1.color, first.color);
        assert_eq!(layered[1].0, 2..4);
        assert_eq!(layered[1].1.color, second.color, "later layer wins color");
        assert_eq!(
            layered[1].1.font_weight,
            Some(FontWeight::BOLD),
            "earlier field survives where the later layer sets nothing"
        );
        assert_eq!(layered[2].0, 4..6);
    }

    #[gpui::test]
    fn an_svg_fence_highlights_as_markup(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let runs = cx.update(|cx| {
            let highlight = steer_code_highlighter(gpui_component::Theme::global(cx));
            highlight(&CodeBlock::from_code(
                "<svg><circle cx=\"4\" cy=\"4\" r=\"3\"/></svg>",
                Some("svg"),
            ))
        });
        assert!(
            !runs.is_empty(),
            "an svg fence takes the markup grammar instead of falling through unhighlighted"
        );
    }

    /// The rendered plain text of a synced [`ChatDocument`].
    fn chat_document_text(doc: &ChatDocument, cx: &mut gpui::TestAppContext) -> String {
        cx.update(|cx: &mut App| {
            doc.state()
                .expect("synced document")
                .read(cx)
                .rendered_text()
                .as_str()
                .trim_end_matches('\n')
                .to_string()
        })
    }

    #[gpui::test]
    fn chat_document_appends_extend_and_rewrites_replace(cx: &mut gpui::TestAppContext) {
        let mut doc = ChatDocument::default();
        doc.sync("alpha beta", cx);
        cx.run_until_parked();
        assert_eq!(chat_document_text(&doc, cx), "alpha beta");

        // An appended raw delta keeps the prefix and lands the suffix.
        doc.sync("alpha beta gamma", cx);
        cx.run_until_parked();
        assert_eq!(chat_document_text(&doc, cx), "alpha beta gamma");

        // A wholesale rewrite replaces the document.
        doc.sync("other text", cx);
        cx.run_until_parked();
        assert_eq!(chat_document_text(&doc, cx), "other text");
    }

    #[gpui::test]
    fn chat_document_mid_stream_linkify_rewrite_lands_the_full_source(
        cx: &mut gpui::TestAppContext,
    ) {
        let mut doc = ChatDocument::default();
        doc.sync("see https://examp", cx);
        cx.run_until_parked();
        assert_eq!(chat_document_text(&doc, cx), "see https://examp");

        // Completing the URL rewrites an earlier span, so the new linkified
        // source no longer extends the previous one: the sync must fall back
        // to the full-replace path and still land the complete source.
        doc.sync("see https://example.com done", cx);
        cx.run_until_parked();
        assert_eq!(chat_document_text(&doc, cx), "see https://example.com done");
    }
}
