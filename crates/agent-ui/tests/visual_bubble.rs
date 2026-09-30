//! Visual acceptance for the conversation info bubble (harness=false so it
//! runs on the main thread; MacPlatform requires it for offscreen
//! rendering; the Metal readback is macOS-only, everything else prints a
//! skip line).
//!
//! Run with `BUBBLE_SHOT=/tmp/bubble.png cargo test -p agent-ui --features
//! test-support --test visual_bubble`; `BUBBLE_STATE=open` renders the
//! bubble expanded (default `closed`). Seeds the AhpStore book directly
//! (`detached`), so like every harness that talks to the AHP fold this
//! binary only builds under `--features test-support`.
//!
//! Real faces under test: `ContextRail::render_bubble` — the six segments,
//! the fold/`+N` rows, the divider rule and the width clamp. Mocked: the
//! composer row's stand-ins, the seed data and the pill mount glue
//! (mirrored: same tail SVG asset + offsets, but the height cap here is an
//! independent estimate — production measures via `BubbleClearance`, which
//! a Workspace-less harness cannot drive end to end).
//! `Workspace::render_context_usage_ring` is a `Workspace` method the
//! harness cannot construct, so the pill mount glue is mirrored here; the
//! pixel output still comes from the real render pipeline. Per AGENTS.md
//! the sketches under design/ are NOT acceptance faces — this harness is,
//! Only the >=90% warning-line assertion below is falsifiable: the pixel
//! predicates could not separate the tail from the bubble's own
//! border+shadow, and the alignment reference resolved to a text edge
//! rather than the pill's geometry — debug_bounds-class assertions are
//! the follow-up.

#[cfg(any(not(target_os = "macos"), not(feature = "test-support")))]
fn main() {
    eprintln!("visual_bubble: skipped (needs macOS Metal readback + test-support)");
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
fn main() {
    macos::main();
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
mod macos {
    use std::sync::Arc;

    use agent_ui::ahp_store::AhpStore;
    use agent_ui::assets::ExtrasAssetSource;
    use agent_ui::views::context_rail::{BUBBLE_MAX_W, BUBBLE_MIN_W, ContextRail, PlanFileEntry};
    use ahp_types::state::{ChatState, SessionState};
    use gpui::{
        App, AppContext as _, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement,
        PathBuilder, PathStyle, Pixels, Point, Render, StrokeOptions, Styled, VisualTestAppContext,
        Window, canvas, point, px, rgb, size,
    };
    use gpui_component::{
        ActiveTheme as _, Icon, Root, Sizable as _, ThemeStyled as _, h_flex, v_flex,
    };
    use lyon::tessellation::LineCap;

    // ── production geometry constants (mirrored from composer_render.rs) ──

    const RING_SIZE: f32 = 14.0;
    const RING_RADIUS: f32 = 5.5;
    const RING_STROKE: f32 = 2.0;

    pub fn main() {
        let Ok(path) = std::env::var("BUBBLE_SHOT") else {
            eprintln!("visual_bubble: BUBBLE_SHOT not set, skipping");
            return;
        };
        let open = std::env::var("BUBBLE_STATE").as_deref() == Ok("open");

        manox_agent::init();
        let platform = gpui_platform::current_platform(false);
        let mut cx = VisualTestAppContext::with_asset_source(platform, Arc::new(ExtrasAssetSource));
        cx.update(|cx| {
            gpui_component::init(cx);
            manox_i18n::init();
            manox_agent_chrome_ui::register_fonts(cx);
        });

        let handle = cx
            .open_offscreen_window(size(px(1280.), px(820.)), |window, cx| {
                let store = cx.new(|_| AhpStore::detached());
                // Seed the usage tree from REAL registered models so the
                // wire-api tint resolves through the same registry the
                // bubble reads; two distinct apis when available.
                let registry = manox_agent::provider_glue::global();
                let mut registered = Vec::new();
                for _ in 0..40 {
                    registered = registry.models();
                    if !registered.is_empty() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                let mut chosen: Vec<manox_harness::types::Model> = Vec::new();
                for m in registered {
                    if !chosen.iter().any(|c| c.api == m.api) {
                        chosen.push(m);
                    }
                    if chosen.len() == 2 {
                        break;
                    }
                }
                let (info_json, fg_identity) = if chosen.is_empty() {
                    (
                        serde_json::json!({
                            "models": [
                                {"provider": "百炼", "model": "百炼/glm-5.2",
                                 "input": 112_000, "output": 16_400,
                                 "cacheRead": 88_200, "cacheWrite": 0},
                            ],
                        }),
                        "百炼/glm-5.2".to_string(),
                    )
                } else {
                    let rows: Vec<serde_json::Value> = chosen
                        .iter()
                        .enumerate()
                        .map(|(i, m)| {
                            serde_json::json!({
                                "provider": m.provider,
                                "model": format!("{}/{}", m.provider, m.id),
                                "input": 512_000 - i as u64 * 300_000,
                                "output": 64_000,
                                "cacheRead": 300_000,
                                "cacheWrite": 40_000,
                            })
                        })
                        .collect();
                    let fg = &chosen[0];
                    (
                        serde_json::json!({ "models": rows }),
                        format!("{}/{}", fg.provider, fg.id),
                    )
                };
                store.update(cx, |h, _| {
                    let book = &mut h.book;
                    // Per-model usage, folded exactly as a metrics-channel
                    // delta lands (the map is keyed by the metrics channel).
                    book.metrics
                        .entry(format!(
                            "{}visual-session",
                            manox_ahp::ext::channels::METRICS
                        ))
                        .or_default()
                        .apply("conversation", &info_json);
                    // The foreground model identity rides the session config;
                    // the working directory rides the newest working-directories
                    // grant (the config echo is absent by design here).
                    let session: SessionState = serde_json::from_value(serde_json::json!({
                        "provider": "pi",
                        "title": "visual",
                        "status": 0,
                        "lifecycle": "ready",
                        "activeClients": [],
                        "chats": [],
                        "workingDirectories": [
                            "file:///Users/chenzhongrun/projects/dspo/manox-app-bubble"
                        ],
                        "config": {
                            "schema": { "type": "object", "properties": {} },
                            "values": { "model": fg_identity },
                        },
                    }))
                    .expect("session seed");
                    book.sessions.insert("visual-session".to_string(), session);
                    // One completed turn carrying the last-request usage: the
                    // in-flight turn is absent, so `last_usage` reads this.
                    let near_full = chosen[0].context_window as u64 * 92 / 100;
                    let chat: ChatState = serde_json::from_value(serde_json::json!({
                        "resource": "ahp-chat:/visual-session",
                        "title": "visual",
                        "status": 0,
                        "modifiedAt": "2026-09-30T00:00:00Z",
                        "turns": [{
                            "id": "t1",
                            "message": { "text": "hi", "origin": { "kind": "user" } },
                            "responseParts": [],
                            "usage": {
                                "inputTokens": near_full,
                                "cacheReadTokens": 61_000,
                            },
                            "state": "complete",
                        }],
                    }))
                    .expect("chat seed");
                    book.chats.insert("visual-session".to_string(), chat);
                });

                let rail =
                    cx.new(|cx| ContextRail::new(Some((store, "visual-session".to_string())), cx));
                rail.update(cx, |rail, cx| {
                    // Two live subagents + three finished (fold into +N).
                    use manox_agent::ToolCallStatus as S;
                    rail.apply_subagent_progress(
                        "s1",
                        "explore",
                        Some("定位 sidebar 三行渲染"),
                        S::Running,
                        None,
                        cx,
                    );
                    rail.apply_subagent_progress(
                        "s2",
                        "verify",
                        Some("回归四态与跑马灯"),
                        S::Running,
                        None,
                        cx,
                    );
                    rail.apply_subagent_progress(
                        "s3",
                        "explore",
                        Some("梳理 render.rs 挂载点"),
                        S::Success,
                        None,
                        cx,
                    );
                    rail.apply_subagent_progress(
                        "s4",
                        "audit",
                        Some("盘点挂载点消费者"),
                        S::Error,
                        None,
                        cx,
                    );
                    rail.apply_subagent_progress(
                        "s5",
                        "verify",
                        Some("复核自适应宽度边界"),
                        S::Success,
                        None,
                        cx,
                    );
                    rail.plan = Some(manox_agent::PlanSnapshot {
                        explanation: None,
                        steps: vec![
                            step(
                                "拆掉 context rail 卡片与纵向空间",
                                manox_agent::PlanStepStatus::Completed,
                            ),
                            step(
                                "圆圈点击开关 + 锚定气泡",
                                manox_agent::PlanStepStatus::InProgress,
                            ),
                            step(
                                "Plan 标题点击开右栏 markdown 预览",
                                manox_agent::PlanStepStatus::Pending,
                            ),
                            step("补 UI-MAP 与截图回归", manox_agent::PlanStepStatus::Pending),
                        ],
                    });
                    rail.git_branch_display = Some(agent_ui::git_status::GitBranchDisplay {
                        branch: Some("feat/conversation-info-bubble".into()),
                        detached_sha: None,
                        is_worktree: true,
                    });
                    rail.bubble_open = open;
                });

                // Seven written plans (cap 5 → `+2`).
                let plan_files: Vec<PlanFileEntry> = [
                    "conversation-info-bubble",
                    "chrome-callout-refactor",
                    "x",
                    "y",
                    "z",
                    "older-plan",
                    "oldest-plan",
                ]
                .iter()
                .map(|slug| PlanFileEntry {
                    title: format!("{slug}-plan"),
                })
                .collect();

                let sheet = cx.new(|_| BubbleSheet { rail, plan_files });
                cx.new(|cx| Root::new(sheet, window, cx))
            })
            .expect("offscreen window");

        for _ in 0..5 {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        let shot = cx.capture_screenshot(handle.into()).expect("capture");
        shot.save(&path).expect("save png");
        // The one falsifiable assertion: the seeded foreground usage is
        // >=90%, so the warning-colored `├ Context …` line MUST be inside
        // the visible bubble — this fails when the model section scrolls
        // out of the height cap. (Right-alignment/tail geometry is
        // verified by eye on the regenerated captures: the pixel
        // predicates could not separate the tail from the bubble's own
        // border+shadow, and the alignment reference resolved to a text
        // edge — debug_bounds-class assertions are the follow-up.)
        if open {
            let img = &shot;
            // Warning-colored Context line: the seeded foreground usage is
            // >=90%, so an orange-ish text cluster must exist in the bubble.
            let mut warning_pixels = 0;
            for y in (img.height() / 3)..img.height() {
                for x in (img.width() / 2)..img.width() {
                    let p = img.get_pixel(x, y).0;
                    if p[0] > 180 && p[1] > 130 && p[2] < 100 && p[0] - p[2] > 100 {
                        warning_pixels += 1;
                    }
                }
            }
            assert!(
                warning_pixels >= 20,
                "expected the >=90% warning-colored Context line, found {warning_pixels} px"
            );
        }
        println!(
            "visual_bubble: wrote {path} ({}x{}) state={}",
            shot.width(),
            shot.height(),
            if open { "open" } else { "closed" }
        );
    }

    fn step(title: &str, status: manox_agent::PlanStepStatus) -> manox_agent::PlanStep {
        manox_agent::PlanStep {
            step: title.to_string(),
            status,
        }
    }

    struct BubbleSheet {
        rail: gpui::Entity<ContextRail>,
        plan_files: Vec<PlanFileEntry>,
    }

    impl Render for BubbleSheet {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = cx.theme().clone();
            let rail = self.rail.clone();
            let plan_files = self.plan_files.clone();

            v_flex()
                .size_full()
                .bg(rgb(0xFAFAFD))
                .child(
                    gpui::div().flex_1().child(
                        gpui::div()
                            .ml(px(80.))
                            .mt(px(40.))
                            .text_sm()
                            .child("…会话消息…"),
                    ),
                )
                .child(
                    // Composer footer stand-in: model chip + the pill + send.
                    h_flex()
                        .w_full()
                        .flex_none()
                        .px_4()
                        .py_2()
                        .justify_between()
                        .items_center()
                        .child(
                            gpui::div()
                                .text_sm()
                                .text_color(theme.foreground)
                                .child("Opus 4.5"),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    gpui::div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("百炼 · glm-5.2 · high"),
                                )
                                .child(PopoverHost {
                                    rail: rail.clone(),
                                    plan_files: plan_files.clone(),
                                })
                                .child(
                                    gpui::div()
                                        .size(px(24.))
                                        .rounded_full()
                                        .bg(theme.accent)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_color(rgb(0xFFFFFF))
                                        .child("↑"),
                                ),
                        ),
                )
        }
    }

    /// The pill + bubble mount, mirrored from `render_context_usage_ring`:
    /// tail hangs from the bubble slot, offset to land on the ring centre, the
    /// plain relative/absolute mount (the 38px transparent apron keeps the
    /// surface clear of the pill; the tail bridges that band), and the
    /// real `render_bubble` content under the popover chrome.
    #[derive(IntoElement)]
    struct PopoverHost {
        rail: gpui::Entity<ContextRail>,
        plan_files: Vec<PlanFileEntry>,
    }

    impl gpui::RenderOnce for PopoverHost {
        fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
            let theme = cx.theme().clone();
            let open = self.rail.read(cx).bubble_open;
            let track = theme.border;
            let fill = theme.muted_foreground;
            let pill = h_flex()
                .id("context-usage-pill")
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded(theme.radius)
                .cursor_pointer()
                .hover(|s| s.bg(theme.accent.opacity(0.08)))
                .child(
                    gpui::div()
                        .relative()
                        .flex_none()
                        .child(occupancy_ring(track, fill, 0.37)),
                )
                .child(gpui::div().text_xs().text_color(fill).child("37%"));

            if !open {
                return pill.into_any_element();
            }
            let viewport = _window.viewport_size();
            let avail_w = f32::from(viewport.width) - 32.0;
            let max_w = px(BUBBLE_MAX_W.min(avail_w.max(BUBBLE_MIN_W)));
            let max_h = px((f32::from(viewport.height) - 180.0).max(160.0));
            let rail = self.rail.clone();
            let bubble = v_flex()
                .absolute()
                .right(px(0.))
                .bottom(px(0.))
                .id("conversation-info-bubble")
                .occlude()
                .popover_style(cx)
                .p_3()
                .child(ContextRail::render_bubble(
                    &rail,
                    &self.plan_files,
                    max_w,
                    max_h,
                    cx,
                ));

            // The bubble slot: a zero-height full-width row pulled up over the
            // pill by a fixed apron, so the surface's bottom edge lands a
            // fixed distance above the pill top and its right edge aligns
            // with the pill's — plain relative/absolute, no deferred pass.
            gpui::div()
                .flex()
                .flex_col()
                .items_end()
                .flex_none()
                .relative()
                .child(pill)
                .child(
                    gpui::div()
                        .relative()
                        .w_full()
                        .h(px(0.))
                        .mt(px(-38.))
                        .child(bubble)
                        .children(open.then(|| {
                            gpui::div()
                                .absolute()
                                .left(px(9.))
                                .bottom(px(-6.))
                                .size(px(12.))
                                .child(
                                    Icon::default()
                                        .path("icons/context-bubble-tail.svg")
                                        .with_size(gpui_component::Size::Size(px(12.)))
                                        .text_color(theme.border),
                                )
                        })),
                )
                .into_any_element()
        }
    }

    // ── ring + tail drawing (mirrored from composer_render.rs) ────────────

    fn occupancy_ring(track: Hsla, fill: Hsla, pct: f64) -> impl IntoElement {
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let center = bounds.center();
                let radius = px(RING_RADIUS);
                if let Ok(track_path) = circle_path(center, radius) {
                    window.paint_path(track_path, track);
                }
                let clamped = pct.clamp(0.0, 1.0);
                let fill_path = if clamped >= 1.0 {
                    circle_path(center, radius).ok()
                } else {
                    arc_end(RING_RADIUS, clamped)
                        .and_then(|(dx, dy, large)| arc_path(center, radius, px(dx), px(dy), large))
                };
                if let Some(fill_path) = fill_path {
                    window.paint_path(fill_path, fill);
                }
            },
        )
        .size(px(RING_SIZE))
    }

    fn stroke_builder() -> PathBuilder {
        PathBuilder::default().with_style(PathStyle::Stroke(
            StrokeOptions::default()
                .with_line_width(RING_STROKE)
                .with_line_cap(LineCap::Round),
        ))
    }

    fn circle_path(
        center: Point<Pixels>,
        radius: Pixels,
    ) -> Result<gpui::Path<Pixels>, anyhow::Error> {
        let mut b = stroke_builder();
        b.move_to(point(center.x + radius, center.y));
        b.arc_to(
            point(radius, radius),
            px(0.),
            true,
            true,
            point(center.x - radius, center.y),
        );
        b.arc_to(
            point(radius, radius),
            px(0.),
            true,
            true,
            point(center.x + radius, center.y),
        );
        b.build()
    }

    fn arc_path(
        center: Point<Pixels>,
        radius: Pixels,
        dx: Pixels,
        dy: Pixels,
        large_arc: bool,
    ) -> Option<gpui::Path<Pixels>> {
        let mut b = stroke_builder();
        b.move_to(point(center.x, center.y - radius));
        b.arc_to(
            point(radius, radius),
            px(0.),
            large_arc,
            true,
            point(center.x + dx, center.y + dy),
        );
        b.build().ok()
    }

    fn arc_end(radius: f32, pct: f64) -> Option<(f32, f32, bool)> {
        let alpha = pct.clamp(0.0, 1.0) * std::f64::consts::TAU;
        if alpha <= 0.0 || alpha >= std::f64::consts::TAU {
            return None;
        }
        let phi = -std::f64::consts::FRAC_PI_2 + alpha;
        Some((
            (radius as f64 * phi.cos()) as f32,
            (radius as f64 * phi.sin()) as f32,
            alpha > std::f64::consts::PI,
        ))
    }
}
