//! The CLI spawn faces: the launch agents, the model picker they open on,
//! and the PTY terminals the canvas sheets render.
//!
//! The right pane this module once served (its ToolTab registry and the
//! TabStore-backed families) retired with it: the canvas's right dock hosts
//! these tools as sheets — one [`AgentPicker`] per `agent:` sheet, the
//! spawn's kill handle riding the sheet id's registry entry. What remains
//! here is what the canvas still calls: the spawn paths, the env layering
//! (D11's authority chain) and the picker.

use std::sync::Arc;

use gpui::prelude::*;
use gpui::{App, Context, Entity, SharedString, Window, px};
use steer_ext_agents::cx_session::CxProcess;

/// The FOREGROUND CONTAINER's working directory (D10/D6: resources root at
/// their owning container's attrs cwd — a native container's session-channel
/// grant, an external container's spawn snapshot), falling back to home: a
/// terminal has to spawn SOMEWHERE.
pub(crate) fn thread_cwd_or_home(cx: &App) -> std::path::PathBuf {
    thread_attrs_for_spawn(cx).cwd
}

/// The FOREGROUND CONTAINER's attrs snapshot for a standalone terminal spawn
/// (D10/D6): the cwd the terminal roots at, and the attrs whose env it
/// inherits (D11's approved capability — an external container's
/// creation-time copy; a native container's view re-captures the world at
/// this read, which is the honest capture the runtime's lack of environment
/// history leaves). Falls back to home + no overrides: a terminal has to
/// spawn somewhere.
///
/// This face is for callers OUTSIDE a workspace lease: it reaches the
/// workspace through the process-global handle. A workspace gesture (the
/// launcher submits run inside `Context<Workspace>`) must call
/// [`attrs_for_spawn`] with its own `&self` — re-reading the already-leased
/// entity through the global panics.
pub(crate) fn thread_attrs_for_spawn(cx: &App) -> crate::workspace::container::ContainerAttrs {
    match crate::dispatch::workspace_global() {
        Some(ws) => attrs_for_spawn(ws.read(cx), cx),
        None => home_attrs(),
    }
}

/// The same attrs snapshot taken from a workspace the caller already holds —
/// the lease-safe face for workspace-side gestures (the double-lease panic
/// the global face hits there is what this seam exists to avoid).
pub(crate) fn attrs_for_spawn(
    ws: &crate::Workspace,
    cx: &App,
) -> crate::workspace::container::ContainerAttrs {
    match ws.foreground_container_id(cx) {
        Some(fg) => ws.container_attrs(&fg, cx),
        None => home_attrs(),
    }
}

fn home_attrs() -> crate::workspace::container::ContainerAttrs {
    crate::workspace::container::ContainerAttrs {
        cwd: std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| ".".into()),
        env: Default::default(),
    }
}

/// The standalone spawn's environment layering, as a pure function (the
/// A7-approved capability's testable core): the container's creation-time
/// snapshot rides BETWEEN the process-inherited base and the user's
/// `[terminal].env`. The ghostty spawn applies the returned slice over the
/// inherited world with last-entry-wins, so a container value beats the
/// process base's same-named variable and a settings value beats the
/// container's — the ORDER of the returned pairs is the authority chain.
pub(crate) fn layered_env(
    container: &crate::workspace::container::ContainerAttrs,
    settings: &[(String, String)],
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = container
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    env.extend(settings.iter().cloned());
    env
}

/// The terminal's palette: the chrome surface it sits in, so a light shell
/// reads as part of the conversation panel rather than a dark inset. The
/// component owns no colors of its own.
fn ghostty_theme() -> steer_ghostty::Theme {
    use steer_agent_chrome_ui::theme::{CARD_BG, FG};
    steer_ghostty::Theme {
        fg: rgb(FG),
        bg: rgb(CARD_BG),
    }
}

/// `Rgba` (f32 channels) → the terminal's 8-bit color.
fn rgb(c: gpui::Rgba) -> steer_ghostty::Rgb {
    let channel = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    steer_ghostty::Rgb::new(channel(c.r), channel(c.g), channel(c.b))
}

/// The full cx launch path: registry-pinned agent + the picked endpoint →
/// `AgentBuilder` (PTY relay) → `CxProcess` → `GhosttyView`. Dropping the
/// view kills the child; the handle is threaded OUT (the CLI sheet's
/// registry record keeps an independent clone) so a close can kill
/// explicitly instead of leaning on drop timing — the contract
/// `steer_ext_agents::cx_session` states.
pub(crate) struct SpawnedAgentTerminal {
    pub view: Entity<steer_ghostty::GhosttyView>,
    /// The child's session handle — the owner's kill clone.
    pub handle: Arc<steer_ext_agents::SessionHandle>,
    /// The REAL external session id (the cx session socket's `<id>.sock`
    /// filename).
    pub cx_session_id: String,
}

pub(crate) fn spawn_agent_terminal(
    agent_id: &str,
    cwd: &std::path::Path,
    provider: &str,
    model: &str,
    wire: Option<String>,
    prompt: Option<&str>,
    cx: &mut App,
) -> Result<SpawnedAgentTerminal, String> {
    let agent = match agent_id {
        "claude" => steer_ext_agents::Agent::Claude,
        "codex" => steer_ext_agents::Agent::Codex,
        "copilot" => steer_ext_agents::Agent::Copilot,
        other => return Err(format!("unknown agent: {other}")),
    };
    let mut builder = steer_ext_agents::AgentBuilder::new()
        .agent(agent)
        .pty(true)
        .provider(provider.to_string())
        .model(model.to_string())
        .cwd(cwd.to_path_buf());
    if let Some(w) = wire {
        builder = builder.wire_api(w);
    }
    if let Some(prompt) = prompt {
        // The launcher's composer text rides the argv as the CLI's initial
        // prompt (AgentBuilder passthrough — no shell, no injection face),
        // after the `--` separator: a draft starting with `-` must not
        // parse as a flag.
        builder = builder.passthrough(["--".to_string(), prompt.to_string()]);
    }
    let handle = Arc::new(builder.spawn().map_err(|e| e.to_string())?);
    // The REAL external session id: the cx session socket's `<id>.sock`
    // filename (the legacy identity link — cx does not expose session_id()).
    let cx_session_id = handle
        .socket_path()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_default();
    let process = CxProcess::new(Arc::clone(&handle));
    let view = steer_ghostty::GhosttyView::spawn_with(Arc::new(process), ghostty_theme(), cx)?;
    Ok(SpawnedAgentTerminal {
        view,
        handle,
        cx_session_id,
    })
}

// ── the agent model picker ────────────────────────────────────────────────

/// One pickable endpoint, straight from the shared cascade projection
/// (`cascade_provider_groups` — the sidebar `+` menu's own rule).
struct PickRow {
    provider: String,
    config_id: String,
    display: String,
    wire: Option<String>,
}

use crate::views::model_cascade::{CascadeEntry, cascade_provider_groups};

impl From<(String, CascadeEntry)> for PickRow {
    fn from((provider, entry): (String, CascadeEntry)) -> Self {
        Self {
            provider,
            config_id: entry.config_id,
            display: entry.display,
            wire: entry.wire,
        }
    }
}

/// One launchable CLI (the picker's first segment: which agent to run).
#[derive(Clone, Copy)]
pub(crate) struct LaunchAgent {
    pub id: &'static str,
    pub display: &'static str,
    /// The brand SVG (the agent glyph's brand mark; the canvas kind keeps
    /// the static terminal glyph for the strip).
    #[allow(dead_code)]
    pub svg: &'static str,
}

pub(crate) const LAUNCH_AGENTS: [LaunchAgent; 3] = [
    LaunchAgent {
        id: "claude",
        display: "Claude Code",
        svg: "icons/claude.svg",
    },
    LaunchAgent {
        id: "codex",
        display: "Codex",
        svg: "icons/codex.svg",
    },
    LaunchAgent {
        id: "copilot",
        display: "GitHub Copilot",
        svg: "icons/githubcopilot.svg",
    },
];

/// The model-picker body an agent sheet opens on: the multiplexer's wire
/// model rows this agent may use, grouped provider→model (the sidebar
/// cascade's rule — a missing `agents` column means non-cx registration and
/// stays visible). Picking one spawns the agent under that endpoint and
/// this entity renders the terminal from then on; a spawn failure stays on
/// the picker with the error surfaced. The picker holds the spawn's payload
/// (view + kill handle) — it IS the CLI sheet's record. The store owns the
/// entity for the sheet id's whole life, so a tab switch never rebuilds a
/// running conversation.
pub(crate) struct AgentPicker {
    /// The SHEET instance this picker rides — the kill handle's registry
    /// key (the sheet's close and the container teardown both kill through
    /// it).
    pub(crate) sheet_id: String,
    mux: Entity<crate::multiplexer::SessionMultiplexer>,
    /// The CLI this picker launches; `None` renders the agent segment
    /// first (the canvas's one agent kind carries every CLI, so the pick
    /// is the sheet's own first gesture).
    pub(crate) agent: Option<LaunchAgent>,
    launched: Option<SpawnedAgentTerminal>,
    error: Option<String>,
}

impl AgentPicker {
    /// A picker for `sheet`: it opens on the agent segment (the canvas's one
    /// agent kind carries every CLI).
    pub(crate) fn for_sheet(
        sheet: &str,
        mux: Entity<crate::multiplexer::SessionMultiplexer>,
    ) -> Self {
        Self {
            sheet_id: sheet.to_string(),
            mux,
            agent: None,
            launched: None,
            error: None,
        }
    }

    /// The picked CLI's display name, once the first segment is chosen.
    pub(crate) fn picked_display(&self) -> Option<&'static str> {
        self.agent.map(|agent| agent.display)
    }

    fn groups(&self, cx: &App) -> Vec<(String, Vec<PickRow>)> {
        let models = self.mux.read(cx).agents(cx);
        cascade_provider_groups(&models)
            .into_iter()
            .map(|(provider, entries)| {
                (
                    provider.clone(),
                    entries
                        .into_iter()
                        .map(|e| PickRow::from((provider.clone(), e)))
                        .collect(),
                )
            })
            .collect()
    }
}

impl gpui::Render for AgentPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        use gpui::{InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, div};
        use gpui_component::{ActiveTheme as _, h_flex, v_flex};
        let theme = cx.theme().clone();

        // Launched: the terminal IS the sheet body from here on.
        if let Some(spawned) = &self.launched {
            return v_flex()
                .size_full()
                .p(px(4.))
                .child(spawned.view.clone())
                .into_any_element();
        }

        let mut list = v_flex().w_full().flex_1().min_h_0().gap_1();
        let Some(agent) = self.agent else {
            // First segment: which CLI. The kind is one; the CLIs are three.
            let heading = steer_i18n::t("chrome-agent-pick-cli");
            for launch in LAUNCH_AGENTS {
                let next = launch;
                list = list.child(
                    div()
                        .id(SharedString::from(format!("pick-agent-{}", launch.id)))
                        .on_click(cx.listener(
                            move |this: &mut AgentPicker,
                                  _,
                                  _,
                                  cx: &mut gpui::Context<AgentPicker>| {
                                this.agent = Some(next);
                                cx.notify();
                            },
                        ))
                        .py(px(6.))
                        .px(px(8.))
                        .rounded(px(4.))
                        .hover(|s| s.bg(theme.list_hover))
                        .child(h_flex().items_center().child(launch.display)),
                );
            }
            return v_flex()
                .id("agent-picker")
                .debug_selector(|| "agent-picker".into())
                .size_full()
                .p(px(8.))
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(heading),
                )
                .child(list)
                .into_any_element();
        };
        let heading = steer_i18n::t_str("chrome-agent-pick-model", &[("agent", agent.display)]);
        let groups = self.groups(cx);
        if let Some(err) = self.error.clone() {
            list = list.child(
                div()
                    .py_2()
                    .text_color(theme.danger)
                    .child(steer_i18n::t_str(
                        "chrome-spawn-failed",
                        &[("prog", agent.display), ("err", &err)],
                    )),
            );
        }
        if groups.is_empty() {
            list = list.child(
                div()
                    .py_4()
                    .text_color(theme.muted_foreground)
                    .child(steer_i18n::t("external-wizard-no-model")),
            );
        }
        for (provider, rows) in groups {
            let mut group = v_flex().gap_1();
            group = group.child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme.muted_foreground)
                    .child(provider.clone()),
            );
            for row in rows {
                let agent_id = agent.id;
                let wire = row.wire.clone();
                let display_name = row.display.clone();
                let elem_id =
                    SharedString::from(format!("pick-{}-{}", row.provider, row.config_id));
                let on_pick = cx.listener(
                    move |this: &mut AgentPicker,
                          _: &gpui::ClickEvent,
                          _w: &mut Window,
                          cx: &mut Context<AgentPicker>| {
                        let cwd = thread_cwd_or_home(cx);
                        match spawn_agent_terminal(
                            agent_id,
                            &cwd,
                            &row.provider,
                            &row.config_id,
                            wire.clone(),
                            None,
                            cx,
                        ) {
                            Ok(spawned) => {
                                // The kill handle rides the registry under
                                // (foreground container, this tab): the
                                // tab's close and the container teardown
                                // both kill through it.
                                if let Some(ws) = crate::dispatch::workspace_global() {
                                    let container =
                                        ws.read(cx).foreground_container_id(cx).unwrap_or_default();
                                    let tab_id = this.sheet_id.clone();
                                    let handle = spawned.handle.clone();
                                    ws.update(cx, |ws, _| {
                                        ws.register_kill_handle(&container, &tab_id, handle);
                                    });
                                }
                                this.launched = Some(spawned);
                            }
                            Err(e) => this.error = Some(e),
                        }
                        let _ = display_name;
                        cx.notify();
                    },
                );
                group = group.child(
                    div()
                        .id(elem_id)
                        .on_click(move |e, w, cx| on_pick(e, w, cx))
                        .py(px(4.))
                        .px(px(8.))
                        .rounded(px(4.))
                        .hover(|s| s.bg(theme.list_hover))
                        .child(h_flex().items_center().child(row.display.clone())),
                );
            }
            list = list.child(group);
        }
        v_flex()
            .id("agent-picker")
            .debug_selector(|| "agent-picker".into())
            .size_full()
            .p(px(8.))
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(heading),
            )
            .child(list)
            .into_any_element()
    }
}

/// A standalone PTY spawn (the project-menu path): the terminal is an
/// app-side surface no agent turn touches, and the shell lands where the
/// caller picked — the cascade's folder for the project menu. The PTY
/// inherits the owning container's env snapshot (D11) layered over the
/// process environment, with the user's `[terminal].env` (settings)
/// applying last and winning — the same authority `layered_env` documents.
pub(crate) fn spawn_standalone_terminal(
    cwd: &std::path::Path,
    attrs: &crate::workspace::container::ContainerAttrs,
    cx: &mut App,
) -> Result<Entity<steer_ghostty::GhosttyView>, String> {
    let env_layer = layered_env(attrs, &steer_ghostty::settings::load().env);
    steer_ghostty::GhosttyView::spawn_shell(cwd, &env_layer, ghostty_theme(), cx)
}
