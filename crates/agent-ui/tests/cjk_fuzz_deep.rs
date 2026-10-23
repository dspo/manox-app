//! Deep fuzz for the `invalid text run` abort (`StyledText::with_runs` via
//! `Inline::request_layout`, crash reports 2026-10-19 .. 2026-10-22). Extends
//! `cjk_inline_probe` / `cjk_inline_fuzz` along the axes their coverage missed:
//!
//! - the production assembly (`ChatDocument::sync` → `message_document`), so
//!   the markdown extensions (math / `svg` / `mermaid`) and the app code
//!   highlighter are live — inline math and images turn paragraphs into inline
//!   flows, which the earlier probes never exercised;
//! - animation frames pumped explicitly (`simulate_next_frame`), so streaming
//!   fade segments survive across updates the way they do on screen;
//! - narrow and mid-stream-resized columns, byte-level chunking, and
//!   mid-stream wholesale replacements (the Retry shape).
//!
//! # Status: reproduces
//!
//! `fuzz_deep_push_bytes` crashes at case 0 (seed `0xDEE0_0001`); the other
//! streaming batteries crash within their first ~25 cases. Minimized, the
//! crash needs only a reference whose definition arrives while streaming —
//! `minimal_repro` (set `CJK_FUZZ_REPRO=1`) fails with:
//!
//! ```text
//! invalid text run. Text: '续', run: TextRun { len: 1, .. alpha ~0.0 }
//! ```
//!
//! Root cause: a reference with no definition in the document renders
//! literally — a footnote reference as `[^1]` (4 bytes), a link reference as
//! `[label]` — and once the definition line streams in, the same paragraph
//! re-parses to the short form (`[1]`, the bare label). The stream-fade
//! tracker only revisits blocks whose source span crosses the append point,
//! so the fade segments it recorded against the longer rendering stay live
//! and render against the shorter one — a segment boundary lands inside a
//! CJK character, `text_runs` emits a 1-byte run, and `with_runs` aborts.
//! Without the streaming fade the same documents stream cleanly
//! (`trace_minimal_stream`, `CJK_FUZZ_TRACE=1`, shows the flip).
//!
//! The batteries are env-gated (`CJK_FUZZ_RUN=1`) because they find this
//! crash and would hold the suite red until the upstream fix lands. Every
//! case prints its full input to stderr before drawing, so the last
//! `=== CASE` block before a panic is the culprit. A debug-build panic
//! prints the offending text and run payload.

use std::time::Duration;

use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Pixels, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px, size,
};
use steer_components::markdown::{ChatDocument, message_document, stream_fade_motion};

// --- harness ------------------------------------------------------------------

/// Roots the window at the shell so the driver can update it across cases.
struct Mount(Entity<Shell>);

impl Render for Mount {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}

struct Shell {
    doc: ChatDocument,
    streaming: bool,
    width: Pixels,
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = self.width;
        let Some(state) = self.doc.state().cloned() else {
            return div().w(width);
        };
        let theme = gpui_component::Theme::global(cx);
        let view = message_document(&state, theme, px(13.), cx).motion(if self.streaming {
            stream_fade_motion()
        } else {
            Default::default()
        });
        div().w(width).child(view)
    }
}

fn pump(visual: &mut VisualTestContext, frames: usize) {
    for _ in 0..frames {
        visual.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
}

// --- seeded rng -----------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, pool: &[&'a str]) -> &'a str {
        pool[self.below(pool.len())]
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

// --- content generation ----------------------------------------------------------

const WORDS: &[&str] = &[
    "中文", "混排", "流式", "渲染", "高亮", "区间", "测试", "文字", "加粗", "代码",
];
const PUNCT: &[&str] = &["，", "。", "、", "！", "？", "：", "……", "（）", "「」"];
const EXOTIC: &[&str] = &[
    "😀",
    "👨‍👩‍👧",
    "é",
    "e\u{301}",
    "\u{200b}",
    "\u{00a0}",
    "ｱ",
    "　",
    "𠀀",
    "\u{fe0f}",
];
const URLS: &[&str] = &[
    "https://example.com/a?b=中文路径",
    "https://example.com/very/long/path/that/keeps/growing/while/streaming",
    "http://x.test",
];

/// One inline-level fragment of a paragraph body.
fn inline_fragment(rng: &mut Rng) -> String {
    fn word(rng: &mut Rng) -> String {
        let mut s = String::new();
        for _ in 0..(1 + rng.below(3)) {
            s.push_str(rng.pick(WORDS));
        }
        s
    }
    match rng.below(22) {
        // Plain CJK, optionally with punctuation or exotics glued on.
        0..=5 => {
            let mut s = word(rng);
            if rng.chance(40) {
                s.push_str(rng.pick(PUNCT));
            }
            if rng.chance(20) {
                s.push_str(rng.pick(EXOTIC));
            }
            s
        }
        // Emphasis tight against CJK, no spaces — the shapes the crash
        // reports carry.
        6 => format!("{}**{}**{}", word(rng), word(rng), word(rng)),
        7 => format!("{}*{}*{}", word(rng), word(rng), word(rng)),
        8 => format!("{}`{} `{}`{}", word(rng), word(rng), word(rng), ""),
        9 => format!("{}~~{}~~{}", word(rng), word(rng), word(rng)),
        10 => format!(
            "{}[{}](https://example.com/链){}",
            word(rng),
            word(rng),
            word(rng)
        ),
        11 => format!("{}<u>{}</u>{}", word(rng), word(rng), word(rng)),
        12 => format!("{}<mark>{}</mark>{}", word(rng), word(rng), word(rng)),
        13 => format!("{}<s>{}</s>{}", word(rng), word(rng), word(rng)),
        // Inline math claimed by the app plugin -> inline flow object.
        14 => format!("{}$x^2 + {}$继续", word(rng), word(rng)),
        15 => format!("{}$\\frac{{a}}{{b}}${}", word(rng), word(rng)),
        // One-line display pair — a block when the paragraph holds nothing
        // else, an inline run otherwise.
        16 => format!("$$E = m{}^2$$", word(rng)),
        // Inline image next to CJK — the other inline-flow activator.
        17 => format!(
            "![{}替代文字](https://example.com/图.png){}",
            word(rng),
            word(rng)
        ),
        18 => format!("{}![图]({})继续", word(rng), rng.pick(URLS)),
        // Bare URL: drives ChatDocument's mid-stream linkify rewrite.
        19 => format!("看{} {}", word(rng), rng.pick(URLS)),
        // Entities and escapes.
        20 => format!("{}&#x4e2d;\\*转义\\*{}", word(rng), word(rng)),
        _ => format!("{}👨‍👩‍👧‍👦{}", word(rng), rng.pick(PUNCT)),
    }
}

fn paragraph(rng: &mut Rng) -> String {
    let pieces = 2 + rng.below(6);
    (0..pieces)
        .map(|_| inline_fragment(rng))
        .collect::<Vec<_>>()
        .join("")
}

const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"80\" height=\"40\"><rect width=\"80\" height=\"40\" fill=\"#3b82f6\"/></svg>";
const MERMAID: &str =
    "flowchart LR\n    A[输入] --> B{判定}\n    B -->|是| C[完成]\n    B -->|否| D[重试]";

/// One block-level construct.
fn block(rng: &mut Rng) -> String {
    match rng.below(20) {
        0..=5 => paragraph(rng),
        6 => format!("## {}标题\n\n{}", rng.pick(WORDS), paragraph(rng)),
        7 => format!("{}\n====\n\n{}", rng.pick(WORDS), paragraph(rng)),
        8 => format!(
            "```ansi\n处理中... 中文\n[OK] 第一步完成：{}\n\x1b[32m绿色中文\x1b[0m\n```",
            rng.pick(WORDS)
        ),
        9 => format!(
            "```rust\n// 注释：{}\nfn main() {{ let s = \"{}字符串\"; }}\n```",
            rng.pick(WORDS),
            rng.pick(WORDS)
        ),
        10 => format!("```svg\n{SVG}\n```"),
        11 => format!("```mermaid\n{MERMAID}\n```"),
        12 => {
            let a = rng.pick(WORDS);
            let b = rng.pick(WORDS);
            format!(
                "| 列甲 | 列乙 | 备注 |\n|---|---|---|\n| {a} | **{b}** | `{a}码` |\n| $x$数学 | [链接](https://e.com/{b}) | {a}{b} |"
            )
        }
        13 => format!(
            "> 引用{}：**{}**\n> - 项一{}\n> - [ ] 待办{}\n\n{}",
            rng.pick(PUNCT),
            rng.pick(WORDS),
            rng.pick(WORDS),
            rng.pick(WORDS),
            paragraph(rng)
        ),
        14 => format!(
            "- [ ] 待办**一**{}\n- [x] 已办`{}`{}\n  - 嵌套{}\n\n{}",
            rng.pick(WORDS),
            rng.pick(WORDS),
            rng.pick(WORDS),
            rng.pick(WORDS),
            paragraph(rng)
        ),
        15 => format!(
            "<div>\n中文块级元素{}\n</div>\n\n{}",
            rng.pick(WORDS),
            paragraph(rng)
        ),
        16 => format!(
            "引用式：[{}标签][ref] 与 [简洁] 继续{}。\n\n[ref]: https://example.com/ref\n[简洁]: https://example.org",
            rng.pick(WORDS),
            rng.pick(WORDS)
        ),
        17 => format!(
            "脚注：{}正文[^1]继续{}。\n\n[^1]: 脚注内容{}。",
            rng.pick(WORDS),
            rng.pick(WORDS),
            rng.pick(WORDS)
        ),
        18 => format!("{}\n\n---\n\n{}", paragraph(rng), paragraph(rng)),
        // An unclosed fence: byte chunking streams the closer in later.
        _ => format!("```rust\nfn 中文() {{\n    let {} = 1;\n", rng.pick(WORDS)),
    }
}

fn gen_doc(rng: &mut Rng) -> String {
    let blocks = 2 + rng.below(4);
    (0..blocks)
        .map(|_| block(rng))
        .collect::<Vec<_>>()
        .join("\n\n")
}

// --- streaming drivers --------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Drive {
    /// `sync` per byte chunk of the carried size (rides `push_str`).
    PushBytes(usize),
    /// `sync` per whole prefix of the carried step (bigger deltas, and the
    /// bare-URL rewrite that turns an append into a full `set_text`).
    SyncPrefix(usize),
    /// Stream to roughly half, hand `sync` a rewritten document (the Retry
    /// shape), keep streaming the rewrite to the end.
    ReplaceMid,
    /// The whole document in one `sync` — the remount / history-open shape.
    Whole,
}

fn char_boundary_forward(text: &str, mut end: usize) -> usize {
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    end
}

/// Streams `raw` into the shell's document per `drive`, drawing after every
/// chunk. Prints the case input first, so the last printed case before a
/// panic is the culprit. With `resize`, the column width changes mid-stream.
/// One fuzz case's knobs.
struct Case {
    raw: String,
    drive: Drive,
    width: Pixels,
    resize: bool,
}

fn run_case(
    case: usize,
    input: &Case,
    shell: &Entity<Shell>,
    cx: &mut TestAppContext,
    visual: &mut VisualTestContext,
) {
    let Case {
        raw,
        drive,
        width,
        resize,
    } = input;
    eprintln!(
        "=== CASE {case} drive={drive:?} width={width:?} resize={resize} bytes={}\nDOC<<\n{raw}\n>>DOC",
        raw.len()
    );
    shell.update(cx, |s, cx| {
        s.doc = ChatDocument::default();
        s.streaming = true;
        s.width = *width;
        cx.notify();
    });
    pump(visual, 1);

    const RESIZE_WIDTHS: &[f32] = &[760., 300., 520., 180., 640.];
    let mut n = 0;
    let mut step = |shell: &Entity<Shell>,
                    feed: &str,
                    cx: &mut TestAppContext,
                    visual: &mut VisualTestContext| {
        shell.update(cx, |s, cx| {
            if *resize {
                s.width = px(RESIZE_WIDTHS[n % RESIZE_WIDTHS.len()]);
            }
            s.doc.sync(feed, cx);
            cx.notify();
        });
        n += 1;
        pump(visual, if n % 5 == 0 { 2 } else { 1 });
    };

    match drive {
        Drive::Whole => {
            step(shell, raw, cx, visual);
            pump(visual, 3);
        }
        Drive::PushBytes(chunk) | Drive::SyncPrefix(chunk) => {
            let stride = (*chunk).max(2);
            let mut pos = 0;
            while pos < raw.len() {
                let end = char_boundary_forward(raw, (pos + stride).min(raw.len()));
                step(shell, &raw[..end], cx, visual);
                pos = end;
            }
        }
        Drive::ReplaceMid => {
            let mid = char_boundary_forward(raw, raw.len() / 2);
            let mut pos = 0;
            while pos < mid {
                let end = char_boundary_forward(raw, (pos + 7).min(mid));
                step(shell, &raw[..end], cx, visual);
                pos = end;
            }
            // The rewrite: keep the prefix, splice a marker in, stream on.
            let rewritten = format!("{}**改写段**{}", &raw[..mid], &raw[mid..]);
            let mut pos = mid + "**改写段**".len();
            while pos < rewritten.len() {
                let end = char_boundary_forward(&rewritten, (pos + 7).min(rewritten.len()));
                step(shell, &rewritten[..end], cx, visual);
                pos = end;
            }
        }
    }

    // Finalize: production drops the streaming motion, which clears fades.
    shell.update(cx, |s, cx| {
        s.streaming = false;
        cx.notify();
    });
    pump(visual, 2);
}

const WIDTHS: &[f32] = &[760., 480., 320., 220.];

fn run_fuzz(seed: u64, cases: usize, whole: bool, cx: &mut TestAppContext) {
    if std::env::var("CJK_FUZZ_RUN").is_err() {
        return;
    }
    cx.update(gpui_component::init);
    let shell = cx.update(|cx| {
        cx.new(|_| Shell {
            doc: ChatDocument::default(),
            streaming: true,
            width: px(760.),
        })
    });
    let window = cx.open_window(size(px(760.), px(900.)), {
        let shell = shell.clone();
        move |_, _| Mount(shell)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    pump(&mut visual, 1);

    let mut rng = Rng(seed);
    for case in 0..cases {
        let raw = gen_doc(&mut rng);
        let drive = if whole {
            Drive::Whole
        } else {
            match rng.below(10) {
                0..=5 => Drive::PushBytes(2 + rng.below(18)),
                6..=8 => Drive::SyncPrefix(6 + rng.below(30)),
                _ => Drive::ReplaceMid,
            }
        };
        let width = px(WIDTHS[rng.below(WIDTHS.len())]);
        run_case(
            case,
            &Case {
                raw,
                drive,
                width,
                resize: false,
            },
            &shell,
            cx,
            &mut visual,
        );
    }
}

/// The minimal reproducing case, run plainly (no caught panics): fails with
/// the authentic `invalid text run` panic. Inactive unless CJK_FUZZ_REPRO is
/// set, so the suite stays green; `CJK_FUZZ_DOC` / `CJK_FUZZ_STEP` override
/// the input for hand trials.
///
/// The same streams, ungated, are the pinning regression below: with
/// `pin_reference_definitions` active in `ChatDocument::sync` the flip never
/// happens and both complete without aborting.
#[gpui::test]
async fn minimal_repro(cx: &mut TestAppContext) {
    if std::env::var("CJK_FUZZ_REPRO").is_err() {
        return;
    }
    let raw = match std::env::var("CJK_FUZZ_DOC") {
        Ok(doc) => doc,
        Err(_) if std::env::var_os("CJK_FUZZ_DOC").is_none() => "文[^1]续\n\n[^1]: 注".to_string(),
        // A non-UTF-8 value would otherwise silently run the default document
        // and read as a crash that never came from the input under test.
        Err(_) => panic!("CJK_FUZZ_DOC is not valid UTF-8"),
    };
    let step: usize = std::env::var("CJK_FUZZ_STEP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);
    cx.update(gpui_component::init);
    let shell = cx.update(|cx| {
        cx.new(|_| Shell {
            doc: ChatDocument::default(),
            streaming: true,
            width: px(220.),
        })
    });
    let window = cx.open_window(size(px(760.), px(900.)), {
        let shell = shell.clone();
        move |_, _| Mount(shell)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    pump(&mut visual, 1);

    let mut pos = 0;
    let mut n = 0;
    while pos < raw.len() {
        let end = char_boundary_forward(&raw, (pos + step.max(2)).min(raw.len()));
        let feed = raw[..end].to_string();
        shell.update(cx, |s, cx| s.doc.sync(&feed, cx));
        pos = end;
        n += 1;
        pump(&mut visual, 1);
    }
    shell.update(cx, |s, cx| {
        s.streaming = false;
        cx.notify();
    });
    pump(&mut visual, 2);
    panic!(
        "no crash reproduced for {n}-chunk stream of {} bytes",
        raw.len()
    );
}

/// The pinning regression: the two streams that aborted before
/// `pin_reference_definitions` (footnote flip, link-reference flip) stream
/// through the production path with fades on and must draw clean.
#[gpui::test]
async fn pinned_streams_survive_the_flip_constructs(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    for raw in [
        "文[^1]续\n\n[^1]: 注",
        "看[简洁]这里的说明。\n\n[简洁]: https://example.com/docs",
        "> 引用里的[^2]脚注\n>\n> [^2]: 引用内定义",
        "- 列表[参考]项\n- [参考]: https://example.com/ref",
    ] {
        let shell = cx.update(|cx| {
            cx.new(|_| Shell {
                doc: ChatDocument::default(),
                streaming: true,
                width: px(220.),
            })
        });
        let window = cx.open_window(size(px(760.), px(900.)), {
            let shell = shell.clone();
            move |_, _| Mount(shell)
        });
        cx.run_until_parked();
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        pump(&mut visual, 1);

        let mut pos = 0;
        while pos < raw.len() {
            let end = char_boundary_forward(raw, (pos + 6).min(raw.len()));
            let feed = raw[..end].to_string();
            shell.update(cx, |s, cx| s.doc.sync(&feed, cx));
            pos = end;
            pump(&mut visual, 2);
        }
        shell.update(cx, |s, cx| {
            s.streaming = false;
            cx.notify();
        });
        pump(&mut visual, 3);
    }
}

/// Streams the minimal document and prints the rendered text after every
/// chunk, with and without the streaming fade. Without the fade the stream
/// completes, showing the footnote reference's rendered-form flip (a
/// literal `[^1]` becoming `[1]`) that the fade tracker's stale segments miss.
#[gpui::test]
async fn trace_minimal_stream(cx: &mut TestAppContext) {
    if std::env::var("CJK_FUZZ_TRACE").is_err() {
        return;
    }
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let raw = r##"```mermaid
flowchart LR
    A[输入] --> B{判定}
    B -->|是| C[完成]
    B -->|否| D[重试]
```

脚注：区间正文[^1]继续区间。

[^1]: 脚注"##;
    eprintln!("===== WITH FADES (the crashing shape) =====");
    trace_stream(cx, raw, 48, 220.0, true);
    eprintln!("===== WITHOUT FADES (completes; watch the footnote line) =====");
    trace_stream(cx, raw, 48, 220.0, false);
    std::panic::set_hook(previous_hook);
}

/// Streams the minimal document with `step`-byte chunks and prints the
/// rendered text after every chunk, catching draw panics so the trace
/// completes. With `motion` off the stream runs to the end, which is what
/// exposes the footnote reference's rendered-form flip across parses.
fn trace_stream(cx: &mut TestAppContext, raw: &str, step: usize, width: f32, motion: bool) {
    cx.update(gpui_component::init);
    let shell = cx.update(|cx| {
        cx.new(|_| Shell {
            doc: ChatDocument::default(),
            streaming: motion,
            width: px(width),
        })
    });
    let window = cx.open_window(size(px(760.), px(900.)), {
        let shell = shell.clone();
        move |_, _| Mount(shell)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    pump(&mut visual, 1);

    let mut pos = 0;
    let mut n = 0;
    while pos < raw.len() {
        let end = char_boundary_forward(raw, (pos + step.max(2)).min(raw.len()));
        let feed = raw[..end].to_string();
        shell.update(cx, |s, cx| s.doc.sync(&feed, cx));
        pos = end;
        n += 1;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pump(&mut visual, 1);
        }));
        if result.is_err() {
            eprintln!("--- chunk {n} (fed {feed:?}) draw PANICKED");
            break;
        }
        // Read after pumping: this is the landed parse of the chunk just fed.
        let rendered = cx.update(|cx| {
            shell
                .read(cx)
                .doc
                .state()
                .map(|state| state.read(cx).rendered_text().as_str().to_string())
        });
        eprintln!(
            "--- after chunk {n}: rendered {} bytes: {:?}",
            rendered.as_ref().map(String::len).unwrap_or(0),
            rendered
        );
    }
    let _ = window.update(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
}

// --- the fuzz batteries --------------------------------------------------------------

#[gpui::test]
async fn fuzz_deep_push_bytes(cx: &mut TestAppContext) {
    run_fuzz(0xDEE0_0001, 550, false, cx);
}

#[gpui::test]
async fn fuzz_deep_sync_prefix(cx: &mut TestAppContext) {
    run_fuzz(0xDEE0_0002, 500, false, cx);
}

#[gpui::test]
async fn fuzz_deep_replace_mid(cx: &mut TestAppContext) {
    run_fuzz(0xDEE0_0003, 350, false, cx);
}

/// Every case resizes the column while streaming — flow re-wrapping under
/// live fade segments.
#[gpui::test]
async fn fuzz_deep_narrow_resize(cx: &mut TestAppContext) {
    if std::env::var("CJK_FUZZ_RUN").is_err() {
        return;
    }
    cx.update(gpui_component::init);
    let shell = cx.update(|cx| {
        cx.new(|_| Shell {
            doc: ChatDocument::default(),
            streaming: true,
            width: px(760.),
        })
    });
    let window = cx.open_window(size(px(760.), px(900.)), {
        let shell = shell.clone();
        move |_, _| Mount(shell)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    pump(&mut visual, 1);

    let mut rng = Rng(0xDEE0_0004);
    for case in 0..400 {
        let raw = gen_doc(&mut rng);
        let drive = match rng.below(10) {
            0..=6 => Drive::PushBytes(2 + rng.below(18)),
            7..=8 => Drive::SyncPrefix(6 + rng.below(30)),
            _ => Drive::ReplaceMid,
        };
        let width = px(WIDTHS[rng.below(WIDTHS.len())]);
        run_case(
            case,
            &Case {
                raw,
                drive,
                width,
                resize: true,
            },
            &shell,
            cx,
            &mut visual,
        );
    }
}

/// Fresh mount of a whole document every case: new `TextViewState` entities
/// against a window whose element state still holds the previous cases'.
#[gpui::test]
async fn fuzz_deep_whole_mount(cx: &mut TestAppContext) {
    run_fuzz(0xDEE0_0005, 400, true, cx);
}

/// Duration sanity only: the production motion must stay what the app ships.
#[test]
fn stream_fade_motion_matches_production_timing() {
    let motion = stream_fade_motion();
    assert_eq!(motion.stream_fade(), Duration::from_millis(280));
    assert_eq!(motion.stream_fade_stagger(), Duration::from_millis(10));
}

// --- minimizer (env-gated; inactive unless CJK_FUZZ_MIN is set) -----------------
//
// Streams a candidate document through the same harness and reports whether
// the draw aborted. Trials run under catch_unwind with a fresh window each,
// so a delta-debug loop can try many candidates in one process. The final
// result must be re-verified by a plain run.

const CASE_0_DOC: &str = r##"加粗渲染测试$\frac{a}{b}$高亮高亮看代码混排 http://x.test代码👨‍👩‍👧‍👦「」加粗代码

```mermaid
flowchart LR
    A[输入] --> B{判定}
    B -->|是| C[完成]
    B -->|否| D[重试]
```

脚注：区间正文[^1]继续区间。

[^1]: 脚注内容混排。

> 引用……：**区间**
> - 项一渲染
> - [ ] 待办代码

渲染文字<s>中文加粗加粗</s>测试混排代码代码。😀"##;

/// Streams `raw` in `step`-byte chunks at `width`; returns the chunk index
/// that was being streamed when the draw aborted, or `None`. The window is
/// closed afterwards so a long trial loop does not accumulate windows.
fn crashing_chunk(
    cx: &mut TestAppContext,
    raw: &str,
    step: usize,
    width: f32,
    motion: bool,
) -> Option<(usize, String)> {
    cx.update(gpui_component::init);
    let shell = cx.update(|cx| {
        cx.new(|_| Shell {
            doc: ChatDocument::default(),
            streaming: motion,
            width: px(width),
        })
    });
    let window = cx.open_window(size(px(760.), px(900.)), {
        let shell = shell.clone();
        move |_, _| Mount(shell)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    pump(&mut visual, 1);

    let mut pos = 0;
    let mut n = 0;
    let mut crashed = None;
    let payload_of = |err: Box<dyn std::any::Any + Send>| {
        err.downcast_ref::<String>()
            .cloned()
            .or_else(|| err.downcast_ref::<&'static str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "<non-string panic>".to_string())
    };
    while pos < raw.len() {
        let end = char_boundary_forward(raw, (pos + step.max(2)).min(raw.len()));
        let feed = raw[..end].to_string();
        shell.update(cx, |s, cx| s.doc.sync(&feed, cx));
        pos = end;
        n += 1;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pump(&mut visual, 1);
        }));
        if let Err(err) = result {
            eprintln!("TRIAL PANIC at chunk {n}: {}", payload_of(err));
            crashed = Some((n, feed));
            break;
        }
    }
    if crashed.is_none() && motion {
        shell.update(cx, |s, cx| {
            s.streaming = false;
            cx.notify();
        });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pump(&mut visual, 2);
        }));
        if let Err(err) = result {
            eprintln!("TRIAL PANIC at finalize: {}", payload_of(err));
            crashed = Some((n, raw.to_string()));
        }
    }
    let _ = window.update(cx, |_, window, _| window.remove_window());
    cx.run_until_parked();
    crashed
}

/// Greedy delta-debug: drop whole blocks, then byte segments, then widen the
/// chunk step, keeping every reduction that still crashes.
#[gpui::test]
async fn minimize_repro(cx: &mut TestAppContext) {
    if std::env::var("CJK_FUZZ_MIN").is_err() {
        return;
    }
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut raw = CASE_0_DOC.to_string();
    let mut step = 16;
    let width = 220.0;

    let (chunk, feed) = crashing_chunk(cx, &raw, step, width, true).expect("the seed case crashes");
    eprintln!(
        "SEED ({} bytes) crashes at chunk {chunk} of {step} bytes: {feed:?}",
        raw.len()
    );

    // Phase 1: drop one `\n\n`-separated block at a time (fence-aware split;
    // separators are exactly one blank line, so a trim + `"\n\n"` join
    // round-trips the document byte for byte).
    let split_blocks = |doc: &str| -> Vec<String> {
        let mut blocks = Vec::new();
        let mut current = String::new();
        let mut in_fence = false;
        for line in doc.split_inclusive('\n') {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
            }
            if !in_fence && line.trim_end_matches('\n').is_empty() && !current.is_empty() {
                blocks.push(
                    std::mem::take(&mut current)
                        .trim_end_matches('\n')
                        .to_string(),
                );
            } else {
                current.push_str(line);
            }
        }
        if !current.is_empty() {
            blocks.push(current.trim_end_matches('\n').to_string());
        }
        blocks
    };
    loop {
        let blocks = split_blocks(&raw);
        if blocks.len() <= 1 {
            break;
        }
        let mut reduced = false;
        'outer: for drop in 0..blocks.len() {
            let candidate = blocks
                .iter()
                .enumerate()
                .filter(|(ix, _)| *ix != drop)
                .map(|(_, b)| b.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            if candidate.len() >= raw.len() {
                continue;
            }
            if crashing_chunk(cx, &candidate, step, width, true).is_some() {
                raw = candidate;
                eprintln!("dropped block {drop}; {} bytes remain", raw.len());
                reduced = true;
                break 'outer;
            }
        }
        if !reduced {
            break;
        }
    }

    // Phase 2: delete byte segments. Try removing each slice of roughly a
    // quarter of the document, shrinking the window as the document shrinks.
    loop {
        let window_len = (raw.len() / 4).max(1);
        let mut reduced = false;
        'outer: for start in (0..raw.len()).step_by(window_len / 2 + 1) {
            let end = (start + window_len).min(raw.len());
            if !raw.is_char_boundary(start) || !raw.is_char_boundary(end) {
                continue;
            }
            let mut candidate = String::new();
            candidate.push_str(&raw[..start]);
            candidate.push_str(&raw[end..]);
            if candidate.is_empty() {
                continue;
            }
            if crashing_chunk(cx, &candidate, step, width, true).is_some() {
                raw = candidate;
                eprintln!("cut {start}..{end}; {} bytes remain", raw.len());
                reduced = true;
                break 'outer;
            }
        }
        if !reduced {
            break;
        }
    }

    // Phase 3: widen the chunk step — the fewest, largest chunks that still
    // crash pin the essential streaming shape.
    for candidate_step in [64, 48, 32, 24, 20, 18] {
        if candidate_step <= step {
            continue;
        }
        if crashing_chunk(cx, &raw, candidate_step, width, true).is_some() {
            step = candidate_step;
            eprintln!("chunk step widened to {step}");
        }
    }

    // Phase 4: the default width.
    if crashing_chunk(cx, &raw, step, 760.0, true).is_some() {
        eprintln!("crashes at the default width too");
    }

    // Phase 4b: attribution — does the crash need the streaming fade?
    let without_fades = crashing_chunk(cx, &raw, step, width, false).is_some();
    eprintln!("crashes without stream fades: {without_fades}");

    // Phase 4c: attribution — does the crash need each remaining block?
    let blocks = split_blocks(&raw);
    for (drop, name) in blocks.iter().enumerate() {
        let candidate = blocks
            .iter()
            .enumerate()
            .filter(|(ix, _)| *ix != drop)
            .map(|(_, b)| b.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let verdict = if candidate.is_empty() {
            "skipped"
        } else if crashing_chunk(cx, &candidate, step, width, true).is_some() {
            "still crashes"
        } else {
            "REQUIRED"
        };
        eprintln!("block {drop} {name:?}: {verdict}");
    }

    // Phase 4d: trace the streaming parse — rendered text after every chunk.
    // The crash's fade space is the rendered text, so a flip between chunks
    // shows up as a rendered-length jump while the source only appends.
    trace_stream(cx, &raw, step, width, true);

    eprintln!(
        "=== MINIMAL ({step}-byte chunks, width {width}) ===\n{raw}\n=== END ({}, case crashes) ===",
        raw.len()
    );
    // Re-verify in-process as a sanity check.
    assert!(
        crashing_chunk(cx, &raw, step, width, true).is_some(),
        "the minimized case no longer crashes"
    );
    std::panic::set_hook(previous_hook);
}
