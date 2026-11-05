//! The launcher's workspace-side gestures (the entry + the three submit
//! hands): this module is a member of the workspace family (`super::*` —
//! `ViewMode`, the composer's submit face and the workspace's private fields
//! resolve unchanged). [`crate::launcher::LauncherState`] is the page's own
//! state machine; everything that MINTS a container or flips the workspace's
//! mode lives here.

use super::*;
use crate::launcher::{LauncherAgentLaunchPayload, LauncherState};
use crate::workspace::external_sessions::ExternalSessionLaunch;

impl crate::Workspace {
    /// Enter the launcher — the New gestures' landing (⌘N, the New button,
    /// the group menu's 新建对话 with a preset). Nothing is minted here. The
    /// entry yields the main column the way every other mode flip does (an
    /// on-screen external session parks, the sub-agent page drops, the
    /// settings nav unmounts); already on the launcher, it is idempotent —
    /// the composer takes focus. `preset` preselects the target (the group
    /// menu's project); a fresh entry without one follows the memory (last
    /// used project > none).
    pub fn enter_launcher(
        &mut self,
        preset: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let already = matches!(self.view_mode, ViewMode::Launcher);
        if !already {
            self.leave_external_session();
            self.leave_subagent_stream(cx);
            if matches!(self.view_mode, ViewMode::Settings) {
                crate::chrome_assembly::leave_settings_layout(cx);
            }
            self.view_mode = ViewMode::Launcher;
        }
        let launcher = match self.launcher.clone() {
            Some(launcher) => launcher,
            None => {
                let ws = cx.weak_entity();
                let launcher = cx.new(|cx| LauncherState::new(ws, window, cx));
                self.launcher = Some(launcher.clone());
                launcher
            }
        };
        launcher.update(cx, |state, cx| {
            match preset.as_deref() {
                Some(dir) => state.preset_folder(dir, cx),
                // A preset entry keeps the current target; a bare fresh entry
                // re-validates the memory.
                None if !already => state.apply_default_target(cx),
                None => {}
            }
            state.focus(window, cx);
        });
        cx.notify();
    }

    /// ⏎ on the launcher composer: mint the Manox container (the target's
    /// folder rides the create intent; the picks land in its config), stage
    /// the text, and let the attach completion flush it. The mode stays
    /// Launcher until the attach lands — the attach tail flips the column and
    /// [`Self::flush_pending_first_turn`] sends.
    pub(crate) fn submit_launcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(launcher) = self.launcher.clone() else {
            return;
        };
        // Single flight: the staged text already names the container being
        // minted, and the flush is keyed to its attach. A second submit
        // during the flight would race it (the input cleared on the first
        // submit, so only a genuinely concurrent send can get here).
        if self.pending_first_turn.is_some() {
            tracing::warn!("launcher send ignored: a first turn is already in flight");
            return;
        }
        let (target, model, access, effort) = launcher.read(cx).snapshot();
        let text = launcher.update(cx, |state, cx| state.take_text(window, cx));
        if text.trim().is_empty() {
            return;
        }
        let project_path = target.folder();
        let project_id = target.project_id().map(str::to_string);
        let sid = self.create_container(
            crate::workspace::container::ContainerSeed::Manox(
                crate::workspace::container::NativeContainerSeed {
                    project: project_path,
                    model,
                    access,
                    effort,
                },
            ),
            cx,
        );
        if let Some(project_id) = &project_id {
            // The grouping fact lands with the mint (design §2.1's write
            // timing), and the target becomes the remembered default.
            crate::projects::set_membership(&sid, project_id);
            crate::projects::set_last_launcher_project(Some(project_id));
        }
        self.pending_first_turn = Some(text);
        cx.notify();
    }

    /// The launcher send's flush point (design §3.2): called from the attach
    /// completion, where the foreground store is bound to the fresh session.
    /// The send leaves ONE executor turn after `attach_thread`'s
    /// `open_or_create` spawn — that body issues the session/chat subscribes
    /// this submit must trail on the wire (issued earlier = queued ahead
    /// during a replay window = transport-ordered otherwise), and a submit
    /// that outruns them is refused by the host for a chat it has not opened
    /// yet — the journal never materializes and the message is gone. The
    /// text rides the ordinary submit path — optimistic bubble and the
    /// sidebar's seed row included.
    pub(crate) fn flush_pending_first_turn(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.pending_first_turn.take() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |this, cx| {
                this.send_user_turn(text, Vec::new(), SubmitBehavior::Send, cx);
            });
        })
        .detach();
    }

    /// The failed create's refund leg (design §6): the staged text returns to
    /// the launcher — re-opened, text restored, never swallowed.
    pub(crate) fn refund_launcher_first_turn(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.pending_first_turn.take() else {
            return;
        };
        self.enter_launcher(None, window, cx);
        if let Some(launcher) = &self.launcher {
            launcher.update(cx, |state, cx| state.restore_text(text, window, cx));
        }
        cx.notify();
    }

    /// One agent cascade pick: spawn the CLI as the container-to-be's 0 号,
    /// rooted at the target (the primary root or ad-hoc folder; the
    /// foreground-container fallback the old menu kept). A non-empty composer
    /// text rides the argv as the CLI's initial prompt; the membership lands
    /// with the mint.
    pub(crate) fn launch_agent_from_launcher(
        &mut self,
        launch: LauncherAgentLaunchPayload,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(launcher) = self.launcher.clone() else {
            return;
        };
        let (target, _, _, _) = launcher.read(cx).snapshot();
        let prompt = launcher.update(cx, |state, cx| state.take_text_if_present(window, cx));
        let project_path = target.folder();
        let project_id = target.project_id().map(str::to_string);
        let cwd = project_path
            .clone()
            .unwrap_or_else(|| crate::tool_tabs::thread_cwd_or_home(cx));
        match crate::tool_tabs::spawn_agent_terminal(
            launch.agent_id,
            &cwd,
            &launch.provider,
            &launch.model,
            launch.wire,
            prompt.as_deref(),
            cx,
        ) {
            Ok(spawned) => {
                let id = self.spawn_external_session(
                    ExternalSessionLaunch {
                        agent_id: launch.agent_id,
                        cx_session_id: spawned.cx_session_id,
                        label: launch.display.to_string(),
                        svg: launch.svg,
                        project: project_path,
                        cwd,
                        handle: Some(spawned.handle),
                    },
                    spawned.view,
                    window,
                    cx,
                );
                if let Some(project_id) = project_id {
                    crate::projects::set_membership(&id, &project_id);
                }
            }
            Err(e) => spawn_failed_notification(launch.display, &e, window, cx),
        }
    }

    /// The terminal pill: no intermediate step — the existing standalone-PTY
    /// path (the retired group menu's 新建终端), rooted at the target. The
    /// composer text is ignored.
    pub(crate) fn launch_terminal_from_launcher(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(launcher) = self.launcher.clone() else {
            return;
        };
        let (target, _, _, _) = launcher.read(cx).snapshot();
        let project_path = target.folder();
        let project_id = target.project_id().map(str::to_string);
        let cwd = project_path
            .clone()
            .unwrap_or_else(|| crate::tool_tabs::thread_cwd_or_home(cx));
        // The shell PTY IS the new container's 0 号 — no container env to
        // inherit; its own attrs snapshot is captured at the registration.
        let no_attrs = crate::workspace::container::ContainerAttrs {
            cwd: cwd.clone(),
            env: Default::default(),
        };
        match crate::tool_tabs::spawn_standalone_terminal(&cwd, &no_attrs, cx) {
            Ok(view) => {
                let id = self.spawn_external_session(
                    ExternalSessionLaunch {
                        agent_id: "terminal",
                        cx_session_id: String::new(),
                        label: steer_i18n::t("chrome-tab-ghostty").to_string(),
                        svg: "icons/terminal.svg",
                        project: project_path,
                        cwd,
                        handle: None,
                    },
                    view,
                    window,
                    cx,
                );
                if let Some(project_id) = project_id {
                    crate::projects::set_membership(&id, &project_id);
                }
            }
            Err(e) => {
                spawn_failed_notification(&steer_i18n::t("chrome-tab-ghostty"), &e, window, cx)
            }
        }
    }
}

/// The failure notice names the program that failed to start (the project
/// menu's own shape — a spawn failure must not open anything silently).
fn spawn_failed_notification(prog: &str, error: &str, window: &mut Window, cx: &mut App) {
    use gpui_component::WindowExt as _;
    window.push_notification(
        gpui_component::notification::Notification::error(steer_i18n::t_str(
            "chrome-spawn-failed",
            &[("prog", prog), ("err", error)],
        )),
        cx,
    );
}
