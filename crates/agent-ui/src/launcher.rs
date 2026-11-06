//! The launcher — the new-conversation entry's state machine (design §3):
//! the workspace's pre-container launch pad. [`LauncherState`] holds the
//! composer draft, the target (project / ad-hoc folder / none) and the picks
//! (model / access / effort); the chrome's
//! [`steer_agent_chrome_ui::Launcher`] renders it, and the three submit
//! gestures live on the workspace
//! ([`crate::workspace::launcher_entry`]) — all of them funnel through the
//! ONE container creation entry (`Workspace::create_container`):
//!
//! - **⏎ send**: mint a Manox container, stage the text as
//!   `pending_first_turn`, and flush it through the ordinary submit path at
//!   the attach completion (the attach binds the foreground store
//!   synchronously; the create write already queues ahead of the submit on
//!   the store's FIFO). A failed create refunds the text to the launcher —
//!   never swallowed.
//! - **agent click**: the shared model cascade picks the endpoint, the CLI
//!   spawns as the container's 0 号, the composer text (when non-empty) rides
//!   the argv as the initial prompt.
//! - **terminal click**: no intermediate step, the existing standalone-PTY
//!   path rooted at the target.
//!
//! The New gestures (⌘N, the New button, the group menu's 新建对话) land
//! here WITHOUT minting anything — repeated New no longer depends on
//! draft-refill to avoid stacking empty containers (⌘N on the launcher is
//! idempotent: focus the composer).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, DismissEvent, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, h_flex,
    input::{InputEvent, TextareaState},
};
use steer_agent_chrome_ui::launcher::{Launcher, LauncherAction};
use steer_agent_chrome_ui::theme::{IconAsset, icons};

use crate::views::model_cascade::build_model_menu;

/// One agent cascade pick's launch request (bundled — the call would
/// otherwise arc past clippy's argument ceiling). The `provider`/`model` pair
/// is the cx CONFIG identity (the registry's vocabulary — display names never
/// resolve), the same shape the retired group-menu cascade dispatched.
pub(crate) struct LauncherAgentLaunchPayload {
    pub agent_id: &'static str,
    pub display: &'static str,
    pub svg: &'static str,
    pub provider: String,
    pub model: String,
    pub wire: Option<String>,
}

/// The launcher send's payload, extracted INSIDE the launcher's own update
/// and passed outward to the workspace gesture. The extraction must happen
/// there: the PressEnter subscription fires while the launcher entity is
/// being updated, and a `launcher.read` after the `ws.update` would re-enter
/// the held borrow and panic.
#[derive(Clone, Debug)]
pub(crate) struct LauncherSubmitPayload {
    pub text: String,
    pub target: LauncherTarget,
    pub model: Option<String>,
    pub access: Option<String>,
    pub effort: Option<String>,
}

/// The launcher's target: the container-to-be's belonging. A project target
/// resolves through the registry AT USE TIME (the project may be deleted
/// mid-flight); a folder target is an ad-hoc belonging that never enters the
/// registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LauncherTarget {
    None,
    Project(String),
    Folder(PathBuf),
}

impl LauncherTarget {
    pub(crate) fn project_id(&self) -> Option<&str> {
        match self {
            LauncherTarget::Project(id) => Some(id),
            LauncherTarget::None | LauncherTarget::Folder(_) => None,
        }
    }

    /// The directory the external gestures root at: a project's primary
    /// root, the ad-hoc folder, or `None` for the no-project target (the
    /// seed's existing fallback semantics). Single-cwd by design — the CLI
    /// TUIs take one directory; the full root group rides
    /// [`Self::roots`].
    pub(crate) fn folder(&self) -> Option<PathBuf> {
        match self {
            LauncherTarget::None => None,
            LauncherTarget::Folder(path) => Some(path.clone()),
            LauncherTarget::Project(id) => crate::projects::get_project(id).and_then(|project| {
                project
                    .roots
                    .iter()
                    .find(|root| root.primary)
                    .map(|root| root.path.clone())
            }),
        }
    }

    /// Every directory the container-to-be gets tool authorization for: a
    /// project's roots with the primary FIRST (the host binds the first
    /// `working_directories` entry as the session's cwd), the single ad-hoc
    /// folder, none for the no-project target. The registry's stored order
    /// is insertion order — the primary badge is an attribute, so the
    /// primary is hoisted here.
    pub(crate) fn roots(&self) -> Vec<PathBuf> {
        match self {
            LauncherTarget::None => Vec::new(),
            LauncherTarget::Folder(path) => vec![path.clone()],
            LauncherTarget::Project(id) => {
                let mut ordered: Vec<PathBuf> = Vec::new();
                let mut rest: Vec<PathBuf> = Vec::new();
                if let Some(project) = crate::projects::get_project(id) {
                    for root in &project.roots {
                        if root.primary {
                            ordered.push(root.path.clone());
                        } else {
                            rest.push(root.path.clone());
                        }
                    }
                }
                ordered.extend(rest);
                ordered
            }
        }
    }

    /// The chip's display: the project's display name, the folder's
    /// basename, or the localized none line.
    fn label(&self) -> String {
        match self {
            LauncherTarget::None => steer_i18n::t("launcher-project-none").to_string(),
            LauncherTarget::Folder(path) => basename_or_path(path),
            LauncherTarget::Project(id) => crate::projects::get_project(id)
                .map(|project| crate::projects::display_name(&project))
                .unwrap_or_else(|| steer_i18n::t("launcher-project-none").to_string()),
        }
    }
}

/// The external CLI agents the action row offers, in row order (the same trio
/// the retired group-menu cascade offered; their product names are never
/// localized).
const EXTERNAL_AGENTS: [(&str, &str, &str); 3] = [
    ("claude", "Claude Code", "icons/claude.svg"),
    ("codex", "Codex", "icons/codex.svg"),
    ("copilot", "GitHub Copilot", "icons/githubcopilot.svg"),
];

/// The launcher page state — one per workspace (created with it; the draft
/// and picks live for the process lifetime, per the non-goal "no draft
/// persistence across restarts").
pub(crate) struct LauncherState {
    ws: WeakEntity<crate::Workspace>,
    input: Entity<TextareaState>,
    target: LauncherTarget,
    /// The picked model, wire-shaped (`provider/id`); `None` shows — and
    /// seeds nothing but — the registry default.
    model: Option<String>,
    /// The access tier the switch shows (`true` = full access); always seeds
    /// the create intent (the switch shows a definite tier).
    access_full: bool,
    /// The picked reasoning effort ("high" / "max"); `None` = the host
    /// default.
    effort: Option<String>,
    project_menu_open: bool,
    project_menu: Option<Entity<PopupMenu>>,
    project_menu_sub: Option<Subscription>,
    model_menu_open: bool,
    model_menu: Option<Entity<PopupMenu>>,
    model_menu_sub: Option<Subscription>,
    /// The open agent cascade: the agent id whose pill anchors it, the menu
    /// entity and its dismiss subscription.
    agent_menu: Option<(&'static str, Entity<PopupMenu>, Subscription)>,
    /// Held for its keep-alive: dropping a Subscription cancels the observer,
    /// and the Enter submission must live as long as the page does.
    _input_sub: Subscription,
}

impl LauncherState {
    pub(crate) fn new(
        ws: WeakEntity<crate::Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 12)
                .submit_on_enter(true)
                .placeholder(steer_i18n::t("workspace-input-placeholder"))
        });
        // ⏎ submits: the plain Enter rides the workspace's launcher send; ⌘↩
        // (secondary) and shift-enter (newline) do nothing here — the light
        // composer has no steer/queue lanes (no running turn to route to).
        let input_sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter {
                secondary: false,
                shift: false,
            } = ev
            {
                // The payload extracts INSIDE this update — and only when a
                // send may actually go out: an in-flight send or a blank
                // draft leaves the composer untouched, so a second ⏎ during
                // the flight keeps its text instead of clearing it into a
                // lost send.
                let Some(payload) = this.submit_payload(window, cx) else {
                    return;
                };
                let _ = this.ws.update(cx, |ws, cx| ws.submit_launcher(payload, cx));
            }
        });
        // The default target follows the memory (last used > none) — the
        // preset entries override it.
        let target = crate::projects::last_launcher_project()
            .map(LauncherTarget::Project)
            .unwrap_or(LauncherTarget::None);
        Self {
            ws,
            input,
            target,
            model: None,
            access_full: false,
            effort: None,
            project_menu_open: false,
            project_menu: None,
            project_menu_sub: None,
            model_menu_open: false,
            model_menu: None,
            model_menu_sub: None,
            agent_menu: None,
            _input_sub: input_sub,
        }
    }

    /// Preset the target from a directory (the group menu's 新建对话): a
    /// registry project owning the directory wins (canonical compare, the
    /// dialog's own rule), the folder becomes an ad-hoc target otherwise.
    pub(crate) fn preset_folder(&mut self, dir: &Path, cx: &mut Context<Self>) {
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        self.target = crate::projects::list_projects()
            .into_iter()
            .find(|project| project.roots.iter().any(|root| root.path == dir))
            .map(|project| LauncherTarget::Project(project.id))
            .unwrap_or_else(|| LauncherTarget::Folder(dir));
        cx.notify();
    }

    /// A fresh (non-launcher) entry without a preset: the memory decides —
    /// last used project when it still exists, none otherwise. The entity's
    /// constructor already applied the memory; re-validate here so a project
    /// deleted since the last visit cannot linger as the target.
    pub(crate) fn apply_default_target(&mut self, cx: &mut Context<Self>) {
        if let LauncherTarget::Project(id) = &self.target
            && crate::projects::get_project(id).is_none()
        {
            self.target = LauncherTarget::None;
            cx.notify();
        }
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |state, cx| state.focus(window, cx));
    }

    /// The composer's current text (the refund regressions' read face).
    #[cfg(feature = "test-support")]
    pub(crate) fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// Consume the composer text only when it carries something — the send
    /// gestures read it only when it carries a turn (the agent gesture uses
    /// it as the CLI's initial prompt and leaves an empty field alone).
    pub(crate) fn take_text_if_present(&self, window: &mut Window, cx: &mut App) -> Option<String> {
        let text = self.input.read(cx).value().to_string();
        (!text.trim().is_empty()).then(|| {
            self.input.update(cx, |state, cx| {
                state.set_value("", window, cx);
            });
            text
        })
    }

    /// Refill the composer — the failure legs' refund write. Fills ONLY an
    /// empty composer: a submit that landed while the refund was in flight
    /// is the fresher intent, and the refund must not clobber it.
    pub(crate) fn restore_text(&self, text: String, window: &mut Window, cx: &mut App) {
        if !self.input.read(cx).value().trim().is_empty() {
            return;
        }
        self.input.update(cx, |state, cx| {
            state.set_value(text, window, cx);
        });
    }

    /// Replace the composer's text outright — the diagnostic typing face
    /// (the refund path's [`Self::restore_text`] only fills an empty
    /// composer). Test-support.
    #[cfg(feature = "test-support")]
    pub(crate) fn set_text(&self, text: String, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |state, cx| {
            state.set_value(text, window, cx);
        });
    }

    /// Adopt the project the create dialog last minted (「新建项目…」 from the
    /// chip menu): one consumer takes the record, the chip selects it, and it
    /// becomes the remembered default. Runs in the launcher's render pass —
    /// the dialog closes outside any event this state subscribes to.
    pub(crate) fn adopt_created_project(&mut self, cx: &mut Context<Self>) {
        let Some(id) = crate::projects::consume_last_created_project() else {
            return;
        };
        if crate::projects::get_project(&id).is_some() {
            self.target = LauncherTarget::Project(id.clone());
            crate::projects::set_last_launcher_project(Some(&id));
            cx.notify();
        }
    }

    /// The picks override (test-support diagnostics drive the create-intent
    /// coverage through it).
    #[cfg(feature = "test-support")]
    pub(crate) fn set_picks(
        &mut self,
        model: Option<String>,
        access_full: bool,
        effort: Option<String>,
    ) {
        self.model = model;
        self.access_full = access_full;
        self.effort = effort;
    }

    /// The picks snapshot the native seed consumes.
    pub(crate) fn snapshot(
        &self,
    ) -> (
        LauncherTarget,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        (
            self.target.clone(),
            self.model.clone(),
            Some(access_wire(self.access_full).to_string()),
            self.effort.clone(),
        )
    }

    /// The send's payload, consumed out of this state — `None` when no send
    /// may go out: a first turn already in flight (the single-flight peek
    /// happens BEFORE the composer is consumed — the text stays put instead
    /// of being cleared into a lost second send) or a blank draft. See
    /// [`LauncherSubmitPayload`] for why the extraction lives on this side
    /// of the `ws.update`.
    pub(crate) fn submit_payload(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<LauncherSubmitPayload> {
        if self.first_turn_in_flight(cx) {
            tracing::warn!("launcher send ignored: a first turn is already in flight");
            return None;
        }
        let (target, model, access, effort) = self.snapshot();
        let text = self.take_text_if_present(window, cx)?;
        Some(LauncherSubmitPayload {
            text,
            target,
            model,
            access,
            effort,
        })
    }

    /// Whether a first turn is staged on the workspace but its container not
    /// yet attached (the ⏎-to-attach window the single-flight peek guards).
    fn first_turn_in_flight(&self, cx: &App) -> bool {
        self.ws
            .upgrade()
            .is_some_and(|ws| ws.read(cx).first_turn_in_flight())
    }

    // ── menus ─────────────────────────────────────────────────────────

    fn close_project_menu(&mut self, cx: &mut Context<Self>) {
        if !self.project_menu_open && self.project_menu.is_none() {
            return;
        }
        self.project_menu_open = false;
        self.project_menu = None;
        self.project_menu_sub = None;
        cx.notify();
    }

    /// The project chip's menu: the registry (display name + primary basename
    /// sub-line, the current target checked) → open-other-folder (rfd
    /// single-select, ad-hoc target) → new-project dialog → none.
    fn open_project_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_menu_open {
            self.close_project_menu(cx);
            return;
        }
        let this = cx.entity().downgrade();
        let current = self.target.project_id().map(str::to_string);
        let menu = PopupMenu::build(window, cx, |menu, _window, cx| {
            let mut menu = menu;
            for project in crate::projects::list_projects() {
                let id = project.id.clone();
                let selected = current.as_deref() == Some(project.id.as_str());
                let name = crate::projects::display_name(&project);
                let primary = project
                    .roots
                    .iter()
                    .find(|root| root.primary)
                    .map(|root| basename_or_path(&root.path));
                let themed = cx.theme().clone();
                let primary = primary.clone();
                menu = menu.item(
                    PopupMenuItem::element(move |_w, _cx| {
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                gpui::div()
                                    .text_sm()
                                    .text_color(themed.foreground)
                                    .child(name.clone()),
                            )
                            .children(primary.iter().map(|sub| {
                                gpui::div()
                                    .text_xs()
                                    .text_color(themed.muted_foreground)
                                    .child(sub.clone())
                            }))
                            .when(selected, |el| {
                                el.child(
                                    Icon::new(IconName::Check).small().text_color(themed.accent),
                                )
                            })
                    })
                    .on_click({
                        let this = this.clone();
                        let id = id.clone();
                        move |_, _, cx| {
                            let _ = this.update(cx, |state, cx| {
                                state.target = LauncherTarget::Project(id.clone());
                                crate::projects::set_last_launcher_project(Some(&id));
                                state.close_project_menu(cx);
                            });
                        }
                    }),
                );
            }
            let this_folder = this.clone();
            let this_none = this.clone();
            menu = menu
                .separator()
                .item(
                    PopupMenuItem::new(steer_i18n::t("launcher-open-folder"))
                        .icon(Icon::default().path("icons/folder-open.svg"))
                        .on_click(move |_, _, cx| {
                            let _ = this_folder.update(cx, LauncherState::pick_folder);
                        }),
                )
                .item(
                    PopupMenuItem::new(steer_i18n::t("project-new-menu"))
                        .icon(Icon::default().path("icons/plus.svg"))
                        .on_click(move |_, window, cx| {
                            // The launcher stays mounted behind the dialog;
                            // the closing render adopts the fresh project.
                            crate::chrome_assembly::open_project_dialog(None, window, cx);
                        }),
                )
                .item(
                    PopupMenuItem::new(steer_i18n::t("launcher-project-none"))
                        .icon(Icon::default().path("icons/circle.svg"))
                        .on_click(move |_, _, cx| {
                            let _ = this_none.update(cx, |state, cx| {
                                state.target = LauncherTarget::None;
                                state.close_project_menu(cx);
                            });
                        }),
                );
            menu
        });
        let sub = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            if this.project_menu_open {
                this.close_project_menu(cx);
            }
        });
        self.project_menu_open = true;
        self.project_menu = Some(menu);
        self.project_menu_sub = Some(sub);
        cx.notify();
    }

    /// 打开其他文件夹…: the native single-select folder picker, as an ASYNC
    /// dialog. The panel is a modal loop of its own: spinning it inside the
    /// click's event dispatch panics the loop, so the panel runs through
    /// rfd's ModalFuture — dispatched to the main thread after this dispatch
    /// settles — and the result lands back on the entity. A cancelled picker
    /// is a no-op.
    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        self.close_project_menu(cx);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let Some(handle) = rfd::AsyncFileDialog::new().pick_folder().await else {
                return;
            };
            let dir = handle.path().to_path_buf();
            let _ = this.update_in(cx, |state, _window, cx| {
                state.target = LauncherTarget::Folder(dir);
                cx.notify();
            });
        })
        .detach();
    }

    fn close_model_menu(&mut self, cx: &mut Context<Self>) {
        if !self.model_menu_open && self.model_menu.is_none() {
            return;
        }
        self.model_menu_open = false;
        self.model_menu = None;
        self.model_menu_sub = None;
        cx.notify();
    }

    /// The model pick's menu: the shared cascade (provider display-name
    /// submenus, wire-tag rows — the composer selector's own builder) plus the
    /// reasoning-effort knob, the composer dropdown's own pairing.
    fn open_model_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model_menu_open {
            self.close_model_menu(cx);
            return;
        }
        let this = cx.entity().downgrade();
        let current_effort = self.effort.as_deref().and_then(parse_effort);
        let models = crate::model_catalog::rows();
        let menu = PopupMenu::build(window, cx, |menu, window, cx| {
            let mut menu = build_model_menu(
                menu,
                models,
                {
                    let this = this.clone();
                    move |row, _window, cx| {
                        let _ = this.update(cx, |state, cx| {
                            state.model = Some(format!("{}/{}", row.provider, row.id));
                            state.close_model_menu(cx);
                        });
                    }
                },
                window,
                cx,
            );
            menu = menu.separator();
            menu = menu.label(steer_i18n::t("workspace-reasoning-effort"));
            for effort in manox_agent::language_model::ReasoningEffort::ALL {
                let this = this.clone();
                let themed = cx.theme().clone();
                let label = match effort {
                    manox_agent::language_model::ReasoningEffort::High => {
                        steer_i18n::t("workspace-reasoning-high")
                    }
                    manox_agent::language_model::ReasoningEffort::Max => {
                        steer_i18n::t("workspace-reasoning-max")
                    }
                };
                let selected = Some(effort) == current_effort;
                menu = menu.item(
                    PopupMenuItem::element(move |_w, _cx| {
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                gpui::div()
                                    .text_sm()
                                    .text_color(themed.foreground)
                                    .child(label.clone()),
                            )
                            .when(selected, |el| {
                                el.child(
                                    Icon::new(IconName::Check).small().text_color(themed.accent),
                                )
                            })
                    })
                    .on_click(move |_, _, cx| {
                        let _ = this.update(cx, |state, cx| {
                            state.effort = Some(effort_wire(effort).to_string());
                            state.close_model_menu(cx);
                        });
                    }),
                );
            }
            menu
        });
        let sub = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            if this.model_menu_open {
                this.close_model_menu(cx);
            }
        });
        self.model_menu_open = true;
        self.model_menu = Some(menu);
        self.model_menu_sub = Some(sub);
        cx.notify();
    }

    fn close_agent_menu(&mut self, cx: &mut Context<Self>) {
        if self.agent_menu.is_none() {
            return;
        }
        self.agent_menu = None;
        cx.notify();
    }

    /// One agent pill's model cascade: the shared builder over the registry
    /// rows this agent may run (the registration-time `effective_agents`
    /// column, the retired group-menu cascade's own filter). A pick spawns
    /// the CLI as the container's 0 号.
    fn open_agent_menu(
        &mut self,
        agent_id: &'static str,
        display: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = display;
        if self
            .agent_menu
            .as_ref()
            .is_some_and(|(open, _, _)| *open == agent_id)
        {
            self.close_agent_menu(cx);
            return;
        }
        let this = cx.entity().downgrade();
        let ws = self.ws.clone();
        let agent_models: Vec<_> = crate::model_catalog::rows()
            .into_iter()
            .filter(|row| row.agents.iter().any(|a| a == agent_id))
            .collect();
        let launch_row = EXTERNAL_AGENTS
            .into_iter()
            .find(|(id, _, _)| *id == agent_id);
        let menu = PopupMenu::build(window, cx, |menu, window, cx| {
            build_model_menu(
                menu,
                agent_models,
                move |row, window, cx| {
                    // The pick leaves this state first (the menu's dismiss
                    // subscription is fine with an already-closed flag), then
                    // the launch runs on the workspace.
                    let _ = this.update(cx, |state, cx| state.close_agent_menu(cx));
                    let Some((agent_id, display, svg)) = launch_row else {
                        return;
                    };
                    let _ = ws.update(cx, |ws, cx| {
                        ws.launch_agent_from_launcher(
                            LauncherAgentLaunchPayload {
                                agent_id,
                                display,
                                svg,
                                provider: row.cx_name.clone(),
                                model: row.config_id.clone(),
                                wire: crate::views::model_cascade::launch_wire_key(&row.api),
                            },
                            window,
                            cx,
                        )
                    });
                },
                window,
                cx,
            )
        });
        let sub = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.close_agent_menu(cx)
        });
        self.agent_menu = Some((agent_id, menu, sub));
        cx.notify();
    }

    /// The chrome `Launcher` props for this frame; `this` is this state's own
    /// handle (the render path is the only Context scope that builds it).
    fn element(&self, this: WeakEntity<Self>) -> Launcher {
        Launcher {
            project_label: self.target.label(),
            project_menu_open: self.project_menu_open,
            project_menu: self.project_menu.clone(),
            input: self.input.clone(),
            model_label: self.model_label(),
            model_menu_open: self.model_menu_open,
            model_menu: self.model_menu.clone(),
            access_full: self.access_full,
            on_project_chip: {
                let this = this.clone();
                Rc::new(move |window, cx| {
                    let _ = this.update(cx, |state, cx| state.open_project_menu(window, cx));
                })
            },
            on_model_pick: {
                let this = this.clone();
                Rc::new(move |window, cx| {
                    let _ = this.update(cx, |state, cx| state.open_model_menu(window, cx));
                })
            },
            on_access_pick: {
                let this = this.clone();
                Rc::new(move |full, _window, cx| {
                    let _ = this.update(cx, |state, cx| {
                        state.access_full = full;
                        cx.notify();
                    });
                })
            },
            actions: self.actions(this),
        }
    }

    /// The direct-open action row: the terminal (always available — the PTY
    /// needs no binary) and the three CLI agents, grayed when the binary is
    /// not on the machine (`resolve_binary`'s PATH + known-fallback scan).
    fn actions(&self, this: WeakEntity<Self>) -> Vec<LauncherAction> {
        let ws = self.ws.clone();
        let open_agent = self.agent_menu.as_ref().map(|(id, _, _)| *id);
        let mut row = vec![LauncherAction {
            id: "terminal",
            icon: icons::TERMINAL,
            label: steer_i18n::t("launcher-action-terminal").into(),
            enabled: true,
            unavailable: None,
            cascade: None,
            on_click: {
                let ws = ws.clone();
                Rc::new(move |_, window, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.launch_terminal_from_launcher(window, cx));
                })
            },
        }];
        for (agent_id, display, svg) in EXTERNAL_AGENTS {
            let available = steer_ext_agents::resolve_binary(agent_id).is_ok();
            let cascade = (open_agent == Some(agent_id))
                .then(|| self.agent_menu.as_ref().map(|(_, menu, _)| menu.clone()))
                .flatten();
            row.push(LauncherAction {
                id: agent_id,
                icon: IconAsset(svg),
                label: display.into(),
                enabled: available,
                unavailable: (!available).then(|| {
                    SharedString::from(steer_i18n::t_str(
                        "launcher-action-unavailable",
                        &[("prog", display)],
                    ))
                }),
                cascade,
                on_click: {
                    let this = this.clone();
                    Rc::new(move |_, window, cx| {
                        let _ = this.update(cx, |state, cx| {
                            state.open_agent_menu(agent_id, display, window, cx)
                        });
                    })
                },
            });
        }
        row
    }

    /// The model pick's display: the composer chip's `provider · model ·
    /// effort` face over the picked — or registry-default — identity.
    fn model_label(&self) -> String {
        let identity = self.model.clone().or_else(|| {
            manox_agent::provider_glue::default_model().map(|m| format!("{}/{}", m.provider, m.id))
        });
        let (provider_display, model_display) = identity
            .as_deref()
            .and_then(|id| {
                let (provider, model_id) = id.split_once('/')?;
                crate::model_catalog::resolve(provider, model_id)
                    .map(|row| (row.provider_display, row.name))
            })
            .unwrap_or_else(|| (String::new(), identity.clone().unwrap_or_default()));
        if model_display.is_empty() {
            return steer_i18n::t("workspace-no-model").to_string();
        }
        let effort = self
            .effort
            .as_deref()
            .and_then(parse_effort)
            .unwrap_or_default()
            .wire_value()
            .to_string();
        if provider_display.is_empty() {
            format!("{model_display} · {effort}")
        } else {
            format!("{provider_display} · {model_display} · {effort}")
        }
    }
}

impl Render for LauncherState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.element(cx.entity().downgrade())
    }
}

fn basename_or_path(path: &Path) -> String {
    match path.file_name() {
        Some(base) => base.to_string_lossy().into_owned(),
        None => path.to_string_lossy().into_owned(),
    }
}

/// The kebab wire form the create intent's `accessMode` config speaks.
fn access_wire(full: bool) -> &'static str {
    if full { "full-access" } else { "sandbox" }
}

/// The canonical effort string from the enum (the create intent's
/// `reasoningEffort` config speaks the wire values).
fn effort_wire(effort: manox_agent::language_model::ReasoningEffort) -> &'static str {
    match effort {
        manox_agent::language_model::ReasoningEffort::High => "high",
        manox_agent::language_model::ReasoningEffort::Max => "max",
    }
}

/// The canonical effort string → the display enum (the chips module's rule).
fn parse_effort(raw: &str) -> Option<manox_agent::language_model::ReasoningEffort> {
    match raw {
        "high" => Some(manox_agent::language_model::ReasoningEffort::High),
        "max" => Some(manox_agent::language_model::ReasoningEffort::Max),
        _ => None,
    }
}
