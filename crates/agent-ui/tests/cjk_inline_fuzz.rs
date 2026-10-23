//! Fuzz probe for the `invalid text run` abort (`StyledText::with_runs` via
//! `Inline::request_layout`). Feeds seeded pseudo-random CJK/markdown mixes
//! through `TextViewState` (set_text + streamed push_str) and draws each
//! step. A debug-build panic prints the offending text and run.

use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px, size,
};
use steer_components::markdown::linkify_markdown;
use steer_components::markdown::{TextView, TextViewState, stream_fade_motion};

struct Doc {
    state: Entity<TextViewState>,
}

impl Render for Doc {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(TextView::new(&self.state))
    }
}

const WORDS: &[&str] = &[
    "中文", "混排", "English", "加粗", "代码", "link", "文字", "a", "字", "，", "。", "、", "！",
    "？", ":", " ", "\n", "\n\n",
];
const MARKS: &[&str] = &[
    "**", "__", "*", "_", "`", "~~", "[", "]", "(", ")", "!", "<", ">", "$", "$$", "|", "#", "-",
    "\\", "\"", "'", "&", ";", "/",
];
const EXOTIC: &[&str] = &[
    "😀",
    "👨‍👩‍👧",
    "é",
    "e\u{301}",
    "\u{200b}",
    "\u{00a0}",
    "生",
    "𠀀",
    "ｱ",
    "　",
    "ᄀ",
    "\u{fe0f}",
];
const URLS: &[&str] = &[
    "https://example.com/a?b=中文",
    "http://x.test",
    "mailto:a@b.test",
];

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
}

fn gen_doc(rng: &mut Rng) -> String {
    let mut out = String::new();
    let pieces = 4 + rng.below(14);
    for _ in 0..pieces {
        match rng.below(10) {
            0..=4 => out.push_str(rng.pick(WORDS)),
            5..=7 => {
                let m = rng.pick(MARKS);
                out.push_str(m);
                out.push_str(rng.pick(WORDS));
                if rng.below(3) != 0 {
                    out.push_str(m);
                }
            }
            8 => {
                out.push_str(rng.pick(URLS));
            }
            _ => out.push_str(rng.pick(EXOTIC)),
        }
    }
    out
}

fn draw_steps(visual: &mut VisualTestContext, rounds: usize) {
    for _ in 0..rounds {
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
}

#[gpui::test]
async fn fuzz_markdown_set_text(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let mut rng = Rng(0xC5A1);
    for case in 0..400 {
        let doc = gen_doc(&mut rng);
        let linkified = linkify_markdown(&doc);
        let state = cx.update(|cx| {
            cx.new(|cx| TextViewState::markdown(&linkified, cx).motion(stream_fade_motion()))
        });
        let window = cx.open_window(size(px(760.), px(600.)), {
            let state = state.clone();
            move |_, _| Doc { state }
        });
        cx.run_until_parked();
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        draw_steps(&mut visual, 2);
        let _ = case;
    }
}

#[gpui::test]
async fn fuzz_markdown_streamed(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let mut rng = Rng(0xC5A2);
    for _ in 0..120 {
        let doc = gen_doc(&mut rng);
        let linkified = linkify_markdown(&doc);
        let state = cx
            .update(|cx| cx.new(|cx| TextViewState::markdown("", cx).motion(stream_fade_motion())));
        let window = cx.open_window(size(px(760.), px(600.)), {
            let state = state.clone();
            move |_, _| Doc { state }
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        // Stream the document in random byte-aligned chunks, drawing between.
        let mut pos = 0;
        while pos < linkified.len() {
            let mut end = (pos + 1 + rng.below(9)).min(linkified.len());
            while end < linkified.len() && !linkified.is_char_boundary(end) {
                end += 1;
            }
            let delta = linkified[pos..end].to_string();
            state.update(cx, |s, cx| s.push_str(&delta, cx));
            pos = end;
            draw_steps(&mut visual, 1);
        }
        draw_steps(&mut visual, 2);
    }
}
