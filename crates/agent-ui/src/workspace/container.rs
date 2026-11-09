//! The host-side SessionContainer — the container model's app half (plan
//! §2.2/§2.3). One sidebar entry stands for one container; two homes share
//! the vocabulary:
//!
//! - **Native** containers are a VIEW over an existing manox thread: the
//!   container id IS the captain session id (D2), the runtime owns the
//!   session, and the fold carries the state — active threads construct on
//!   demand and nothing is stored client-side. The never-spoken boot landing
//!   session remains the hidden-unless-focused empty container it always
//!   was; its attrs are its create intent (the cwd the session channel
//!   carries). No env snapshot exists for a native container — the runtime
//!   keeps no environment history (D11).
//! - **External** containers live as the upgraded `ExternalSessionRecord`
//!   (the in-memory record the spawn path owns): the id mints in the
//!   existing `external:{agent}:{uuid}` namespace (D2), the 0 号 is the CLI
//!   process, and the record carries the creation-time attrs snapshot.
//!
//! The death predicate is one interface ([`Workspace::container_alive`]):
//! native reads the archived bit (the fold-first read `session_retired`
//! already performs), external reads the record's alive bit, flipped by the
//! PTY's child-exit event (D8). A flip raises [`ContainerEvent`] on the
//! workspace; the workspace subscribes to its own emission and runs the
//! teardown ([`Workspace::teardown_container`]) — external containers drop
//! their record (the sidebar row goes with it) and reclaim every resource
//! under their key. The native half needs no subscription leg: a native
//! container's death IS the archive gesture (D7), and the archive paths drop
//! the row.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::*;
use gpui::EventEmitter;

use super::external_sessions::clamp_external_title;

/// Container identity (D2): a native container's id IS its captain session
/// id (one string, shared with the createSession — the native conceptual two
/// steps collapse onto it); an external container's id is the
/// `external:{agent}:{uuid}` row id (the existing namespace, kept clear of
/// manox thread UUIDs). The string face is what the sidebar and the routing
/// speak; this wrapper exists so identity RULES have one home.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerId {
    Native(String),
    External(String),
}

impl ContainerId {
    /// A native container id: the session id, verbatim.
    pub fn native(session_id: impl Into<String>) -> Self {
        Self::Native(session_id.into())
    }

    /// Mint a fresh external container id in the existing row-id namespace.
    pub fn external(agent_id: &str) -> Self {
        Self::External(format!("external:{}:{}", agent_id, uuid::Uuid::new_v4()))
    }

    /// The string face — the sidebar/routing vocabulary both homes speak.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Native(id) | Self::External(id) => id,
        }
    }

    /// Whether `id` names an external container (the namespace prefix; a
    /// bare thread uuid never carries it).
    pub fn is_external_id(id: &str) -> bool {
        id.starts_with("external:")
    }

    /// Classify an id: the `external:` namespace is an external container,
    /// everything else is a native one.
    pub fn parse(id: impl Into<String>) -> Self {
        let id = id.into();
        if Self::is_external_id(&id) {
            Self::External(id)
        } else {
            Self::Native(id)
        }
    }
}

/// The creation-time attribute snapshot (D10): captured once, at container
/// creation, and never re-read — resources spawned later (dock terminals,
/// right-pane CLI tabs, browser tabs) inherit THIS copy, not the live
/// environment. Immutable by construction (`Arc` of a plain map, no mut
/// face): a snapshot cannot drift after the fact.
#[derive(Clone, Debug)]
pub struct ContainerAttrs {
    /// The working directory at creation — the external spawn's root, or the
    /// native create intent's granted directory.
    pub cwd: PathBuf,
    /// The environment at creation (D11: resource inheritance only — it never
    /// enters a session's tool execution environment).
    pub env: Arc<BTreeMap<String, String>>,
}

impl ContainerAttrs {
    /// Snapshot `cwd` together with the CURRENT process environment. The
    /// capture happens at the call — for the spawn path that call sits inside
    /// [`Workspace::create_container`], so the snapshot IS the creation
    /// moment. Non-UTF-8 variables are skipped (a snapshot never panics on
    /// hostile environment content).
    pub fn snapshot(cwd: PathBuf) -> Self {
        let env = std::env::vars_os()
            .filter_map(|(k, v)| match (k.into_string(), v.into_string()) {
                (Ok(k), Ok(v)) => Some((k, v)),
                _ => None,
            })
            .collect();
        Self {
            cwd,
            env: Arc::new(env),
        }
    }
}

/// The unified creation entry's payload (§2.2's `create_container(kind,
/// attrs)`): a native seed IS a manox container by definition; an external
/// seed's 0 号 names itself in its row-id's agent segment
/// (`external:{agent}:{uuid}` — the vocabulary `ContainerId` carries).
pub enum ContainerSeed<'a> {
    /// A manox conversation container: the native conceptual two steps
    /// (container record + session) fold into ONE `createSession` with the
    /// id shared. `attrs` derive here — the cwd from the belonging (or the
    /// window's workspace root), the env from process state the session
    /// never consumes (D11). The launcher send is the only remaining native
    /// mint (the New gestures land the launcher instead — the draft-refill
    /// pre-minting retired with it).
    Manox(NativeContainerSeed),
    /// An external CLI container: the record lands here, the process spawned
    /// before (记录 + spawn 两步 — see the launcher's action row and the
    /// project menu's terminal/VS Code rows).
    External(ExternalContainerSeed<'a>),
}

/// The native create intent — everything the createSession and the object row
/// carry.
pub struct NativeContainerSeed {
    /// The conversation's granted roots, primary first: the launcher target's
    /// project roots (the full group — `working_directories` is the create
    /// intent's tool-authorization carrier and the host grants every entry),
    /// or the one ad-hoc folder. Empty = the seed's fallback semantics (the
    /// window's workspace root as the single root).
    pub roots: Vec<PathBuf>,
    /// The project entity the container joins. The grouping fact itself is
    /// written at the ATTACH TAIL (the seed only carries it there): a session
    /// that exists — the host answered the create — can never orphan its
    /// membership key, while a refused create never reaches the tail and
    /// never writes one.
    pub project_id: Option<String>,
    /// The picked model, wire-shaped (`provider/id`).
    pub model: Option<String>,
    /// The picked access mode (the kebab wire values).
    pub access: Option<String>,
    /// The picked reasoning effort.
    pub effort: Option<String>,
}

/// The external record seed — the already-spawned PTY's identity and row
/// face (the launch payload, plus the CLI agent's kill handle). The
/// container's project grouping does not ride the seed: membership is
/// written by the launch site (the launcher's gestures and the project
/// menu's terminal row key on the returned container id).
pub struct ExternalContainerSeed<'a> {
    /// The row-id namespace segment (`external:{agent}:{uuid}`).
    pub agent_id: &'a str,
    /// The REAL external session id (the cx session naming
    /// `~/.manox/sessions/<id>.sock`); empty for plain terminals and when the
    /// IPC bind failed.
    pub cx_session_id: String,
    /// The launch label (the agent's display name, or the localized terminal
    /// name).
    pub label: String,
    pub svg: &'static str,
    /// The directory the terminal was spawned in: the D10 attrs snapshot's
    /// cwd until the program claims one via OSC 7.
    pub cwd: PathBuf,
    /// The spawned terminal (its view occupies the main column while the
    /// session is foreground; dropping it kills the process).
    pub view: gpui::Entity<steer_ghostty::GhosttyView>,
    /// The CLI agent's session handle — the owner's independent clone for
    /// the explicit kill on close (the contract `cx_session` states). `None`
    /// for plain terminals: their PTY dies with the view drop.
    pub handle: Option<Arc<steer_ext_agents::SessionHandle>>,
}

/// The death-predicate flip (§2.3): the container's 0 号 reached its kind's
/// death — a manox session archived (D7), an external process exited (D8).
/// Emitted on the [`Workspace`]; the workspace's own subscription runs the
/// external teardown on it, and the archive path raises it for the native
/// ids it retires (the explicit call there is the reclaim, not a duplicate).
#[derive(Debug)]
pub enum ContainerEvent {
    /// The container id whose `alive` read flipped to false.
    Died(String),
}

impl EventEmitter<ContainerEvent> for Workspace {}

impl Workspace {
    /// The ONE container creation entry (§2.2 `create_container(kind,
    /// attrs)`). Native folds its conceptual two steps (container record +
    /// session) into one `createSession` — the object row and the attach all
    /// live here; the launcher's ⏎ send is the thin wrapper over this entry.
    /// External lands the record (the spawn already happened). Returns the
    /// container id — the routing string both homes share.
    pub fn create_container(&mut self, seed: ContainerSeed, cx: &mut Context<Self>) -> String {
        match seed {
            ContainerSeed::Manox(intent) => self.create_native_container(intent, cx),
            ContainerSeed::External(seed) => self.create_external_container(seed, cx),
        }
    }

    /// The native half: a manox conversation container. The id is
    /// client-minted; the attach lands asynchronously. The launcher send
    /// stages its text before calling — this half's failure leg refunds it.
    fn create_native_container(
        &mut self,
        intent: NativeContainerSeed,
        cx: &mut Context<Self>,
    ) -> String {
        let NativeContainerSeed {
            roots,
            project_id,
            model,
            access,
            effort: effort_str,
        } = intent;
        let ws = cx.weak_entity();
        // v3: the id is client-minted; the picked config rides the create
        // intent, and the attach lands once the host answered.
        let sid = uuid::Uuid::new_v4().to_string();
        let mut config = serde_json::Map::new();
        if let Some(model) = &model {
            config.insert("model".into(), serde_json::json!(model));
        }
        if let Some(access) = &access {
            config.insert("accessMode".into(), serde_json::json!(access));
        }
        if let Some(effort) = &effort_str {
            config.insert("reasoningEffort".into(), serde_json::json!(effort));
        }
        // `working_directories` is the create intent's only directory carrier
        // AND the tool-authorization grant: a project launch carries its FULL
        // root group (primary first — the host binds the first entry as the
        // session's cwd, and the landing chip and the sidebar both read it
        // back), an ad-hoc folder carries itself, and an empty list falls
        // back to the window's workspace root as the single root.
        let primary = roots.first().cloned();
        let mut granted = roots;
        if granted.is_empty() {
            granted.push(self.cwd.clone());
        }
        let cwds: Vec<String> = granted
            .iter()
            .map(|root| format!("file://{}", root.to_string_lossy()))
            .collect();
        // The object row: created at once, carrying the conversation's
        // belonging (the primary root) and picks.
        let (provider, model_id) = super::attach::split_wire_model(model.as_deref());
        super::attach::land_conversation_row(
            &sid,
            primary.as_deref(),
            provider,
            model_id,
            access.as_deref(),
            effort_str.as_deref(),
        );
        let reply = self.with_foreground_store(cx, |store, _| {
            store.create_session(&sid, cwds, Some(config))
        });
        let sid2 = sid.clone();
        cx.spawn(async move |_, cx| {
            // A create that did not ride (store unbound — no create can even
            // go out) or was refused lands a visible notification AND refunds
            // the staged first turn to the launcher: a user-initiated send
            // must never die quietly, and the text must not be swallowed
            // (design §6's failure leg). A fresh client-minted uuid cannot
            // collide with a live session, so there is no reopen arm.
            let refusal = match reply {
                Some(reply) => match reply.recv().await {
                    Ok(Ok(_)) => None,
                    Ok(Err(err)) => Some(err.message),
                    Err(closed) => Some(closed.to_string()),
                },
                None => Some(i18n::t("new-thread-no-session").to_string()),
            };
            if let Some(err) = refusal {
                tracing::warn!(session_id = %sid2, error = %err, "createSession failed");
                let _ = ws.update_in(cx, |this, window, cx| {
                    window.push_notification(
                        Notification::error(i18n::t_str(
                            "new-thread-create-failed",
                            &[("err", &err)],
                        )),
                        cx,
                    );
                    this.refund_launcher_first_turn(window, cx);
                });
                return;
            }
            if let Some(dir) = primary.as_deref() {
                let _ = ws.update(cx, |_ws, cx| {
                    Workspace::register_project_in_store(dir, cx);
                });
            }
            match ws.update_in(cx, |this, window, cx| {
                this.attach_created_session(&sid2, project_id.as_deref(), window, cx)
            }) {
                Ok(()) => tracing::info!(session_id = %sid2, "new-thread attach landed"),
                Err(err) => {
                    tracing::warn!(session_id = %sid2, error = %err, "new-thread attach FAILED")
                }
            }
            // The session channel needs no client-side fill here: the host
            // seeds it from the create intent and the object row, and the
            // journal (once the first turn materializes it) wins over both.
        })
        .detach();
        sid
    }

    /// The external half: the record lands through the container vocabulary —
    /// identity minted in the `external:{agent}:{uuid}` namespace, the attrs
    /// snapshot captured at THIS call (the creation moment), the PTY's title
    /// stream followed and its child-exit wired to the death predicate.
    fn create_external_container(
        &mut self,
        seed: ExternalContainerSeed,
        cx: &mut Context<Self>,
    ) -> String {
        let ExternalContainerSeed {
            agent_id,
            cx_session_id,
            label,
            svg,
            cwd,
            view,
            handle,
        } = seed;
        let id = ContainerId::external(agent_id);
        // The terminal may carry a title already — the program claims it
        // through the PTY as soon as it boots — and that claim beats the
        // launch label for the initial row.
        let default_label = label;
        let label = view
            .read(cx)
            .live_title()
            .map(|t| clamp_external_title(&t))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| default_label.clone());
        // The D10 snapshot: the 0 号's spawn cwd (what the terminal was
        // rooted at — the launch cwd, frozen before the program can claim
        // anything else via OSC 7) and the environment as of this call — the
        // record's later life never re-reads either.
        let attrs = ContainerAttrs::snapshot(cwd);
        // Title and death ride the view's gpui event face: the pump emits
        // `TitleChanged` on OSC title (`None` = the program released it →
        // back to the launch label) and `Exited` when the process dies (D8)
        // — the exit flips the record's alive bit and raises the container
        // event. The explicit handle kill (teardown_container) is the
        // owner's close path; the view drop's own kill is the backstop.
        let id_for_sub = id.as_str().to_string();
        let title_sub = cx.subscribe(
            &view,
            move |this, _, ev: &steer_ghostty::GhosttyEvent, cx| match ev {
                steer_ghostty::GhosttyEvent::TitleChanged(title) => {
                    this.apply_terminal_title(&id_for_sub, title.clone(), cx);
                }
                steer_ghostty::GhosttyEvent::Exited => {
                    this.retire_external_container(&id_for_sub, cx);
                }
            },
        );
        self.externals
            .push(super::external_sessions::ExternalSessionRecord {
                id: id.as_str().to_string(),
                cx_session_id,
                default_label,
                label,
                svg,
                created_at: chrono::Utc::now().timestamp(),
                view,
                attrs,
                handle,
                alive: true,
                _title_sub: title_sub,
            });
        // The identity link, read from the record: a row id traces back to
        // the cx session's socket on disk.
        let record = self.externals.last().expect("just pushed");
        tracing::info!(
            row_id = %record.id,
            cx_session_id = %record.cx_session_id,
            agent = agent_id,
            "external session registered"
        );
        id.as_str().to_string()
    }

    /// The external container's death (D8): the 0 号 process exited — flip
    /// the record's alive bit and raise the event. The event drives the
    /// teardown (the workspace subscribes to its own emission): the record
    /// and its sidebar row drop, and every resource under the container's
    /// key is reclaimed. A second exit event after the flip is a no-op.
    fn retire_external_container(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(record) = self.externals.iter_mut().find(|s| s.id == id) else {
            return;
        };
        if !record.alive {
            return;
        }
        record.alive = false;
        cx.emit(ContainerEvent::Died(id.to_string()));
    }

    /// The ONE container teardown (§2.3: predicate flip ⇒ same teardown):
    /// kill the 0 号's process (external homes — the owner's independent
    /// handle clone, the cx_session contract), kill every CLI tab the
    /// container owns (the registry's kill handles), reclaim its browser
    /// tabs (webview drop + host-route reclaim), and drop its registry
    /// entries (the stashed dock view and right-pane session — the PTYs die
    /// with the views). The shell half — the LIVE dock view and pane session,
    /// when this container owns them — reclaims outside the workspace lease
    /// (`chrome_assembly::reclaim_container_panes`): closing live tabs
    /// re-enters the shell, which a leased workspace update must not.
    ///
    /// Triggers: `ContainerEvent::Died` (the workspace's own subscription —
    /// an external container's process exit drops its record and sidebar row
    /// here) and the explicit close (`close_external_session`, D8: closing a
    /// container kills its process). Idempotent: a container with nothing
    /// left under its key tears down to no-ops.
    pub(crate) fn teardown_container(&mut self, container_id: &str, cx: &mut Context<Self>) {
        let was_foreground = self.active_external.as_deref() == Some(container_id);
        // The external record: the owner's kill fires FIRST (the dropped
        // view's own tear-down then lands on an already-dead child — no
        // blocking reap on the main thread), then the record goes, sidebar
        // row included.
        let handle = self
            .externals
            .iter()
            .find(|s| s.id == container_id)
            .and_then(|s| s.handle.clone());
        if let Some(handle) = handle {
            let _ = handle.kill();
        }
        self.externals.retain(|s| s.id != container_id);
        // The CLI tabs' children (right-pane terminal/CLI tabs, dock included
        // via their spawn-time owner stamp).
        self.kill_handles_of_container(container_id);
        // The browser tabs: webview drop + host-route reclaim, owned tabs
        // only — other containers' tabs survive. (Compiled out with the
        // integrated browser on Linux/FreeBSD — no tab ever registers.)
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        self.close_container_browser_tabs(container_id, cx);
        // The stashed dock view + right-pane session.
        self.reclaim_stash(container_id);
        // The durable pane snapshot: a restore source only for a LIVE
        // container's return — the dead container's registry key must not
        // outlive it.
        crate::chrome_assembly::remove_right_pane(container_id);
        // The membership prune rides the ONE teardown: an EXTERNAL
        // container's death detaches it from its project (the record is gone
        // for good — the process model has no revival to keep the fact
        // for). Native ids keep their membership: the archive gesture is
        // reversible (unarchive revives the whole tree into its group), so
        // the grouping fact must survive the archive.
        if ContainerId::is_external_id(container_id) {
            crate::projects::remove_membership(container_id);
        }
        // The live dock/pane half: outside the lease (see above).
        let dead = container_id.to_string();
        cx.defer(move |cx| crate::chrome_assembly::reclaim_container_panes(&dead, cx));
        if was_foreground {
            self.active_external = None;
            self.view_mode = super::ViewMode::Workspace;
            // The highlight follows this mode flip on its own: it is derived
            // from the foreground container at render time.
        }
        cx.notify();
    }

    /// The ONE death predicate (§2.3). Native: the session is not retired —
    /// the same two voices the draft refill consults: the fold-first
    /// archived read (`session_retired`: the session channel's echo, the
    /// list snapshot's status bit, the host store's row) and the object
    /// row's own archived bit (the choke-point write every archive passes
    /// through; a never-materialized container has no fold voice at all, so
    /// its row bit is the only retirement fact). External: the record exists
    /// and its PTY child has not exited. A container id with no record
    /// behind it at all reads by its namespace — an unknown native id is
    /// unarchived (alive), an unknown or closed external id is gone (dead).
    /// The landing session reads alive until it is archived, which is
    /// exactly the "hidden unless focused" status quo.
    pub fn container_alive(&self, container_id: &str, cx: &App) -> bool {
        if ContainerId::is_external_id(container_id) {
            self.externals
                .iter()
                .any(|s| s.id == container_id && s.alive)
        } else {
            !self.session_retired(cx, container_id)
                && !manox_agent::object_store::ObjectStore::try_global()
                    .and_then(|objects| objects.conversation(container_id))
                    .is_some_and(|row| row.archived)
        }
    }

    /// The container's attributes (D10). External: the creation-time
    /// snapshot on the record, verbatim. Native: a view — the cwd is the
    /// session channel's (protocol state carries the create intent's grant,
    /// falling back to the window's workspace root for a session the fold
    /// has not met), and the env half is captured at THIS read: the runtime
    /// keeps no environment history, so no honest creation-time capture
    /// exists for a native container (D11) — a later resource spawn reads
    /// the world as the view last saw it.
    pub fn container_attrs(&self, container_id: &str, cx: &App) -> ContainerAttrs {
        if ContainerId::is_external_id(container_id)
            && let Some(record) = self.externals.iter().find(|s| s.id == container_id)
        {
            return record.attrs.clone();
        }
        let cwd = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .and_then(|(store, _)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, container_id)
                    .cwd()
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| self.cwd.clone());
        ContainerAttrs::snapshot(cwd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── identity rules (D2) ──

    /// The external namespace: namespaced `external:{agent}:{uuid}`, unique,
    /// and never colliding with a bare thread UUID.
    #[test]
    fn external_ids_are_namespaced_and_unique() {
        let id = ContainerId::external("claude");
        assert!(id.as_str().starts_with("external:claude:"));
        assert_ne!(
            ContainerId::external("claude").as_str(),
            id.as_str(),
            "ids are unique"
        );
        assert!(!id.as_str().starts_with("0197"), "never a bare uuid shape");
        assert!(ContainerId::is_external_id(id.as_str()));
        assert_eq!(id, ContainerId::parse(id.as_str()));
    }

    /// A native container id is the session id verbatim — the createSession
    /// shares it, nothing is prefixed.
    #[test]
    fn native_ids_are_the_session_id_verbatim() {
        let id = ContainerId::native("0197abcd-session");
        assert_eq!(id.as_str(), "0197abcd-session");
        assert!(!ContainerId::is_external_id(id.as_str()));
        assert_eq!(
            ContainerId::parse("0197abcd-session"),
            ContainerId::Native("0197abcd-session".into())
        );
    }

    // ── attrs capture (D10 snapshot semantics) ──

    /// A snapshot carries the cwd it was given and the CURRENT environment;
    /// clones share ONE capture (a later construction never re-reads the
    /// world into an existing snapshot), while two creations capture
    /// independent worlds.
    #[test]
    fn attrs_snapshot_captures_the_creation_moment() {
        let attrs = ContainerAttrs::snapshot(PathBuf::from("/tmp/proj"));
        assert_eq!(attrs.cwd, PathBuf::from("/tmp/proj"));
        assert!(
            attrs.env.contains_key("PATH"),
            "the snapshot carries the process environment"
        );
        let clone = attrs.clone();
        assert!(
            Arc::ptr_eq(&attrs.env, &clone.env),
            "clones share the one capture"
        );
        let later = ContainerAttrs::snapshot(PathBuf::from("/tmp/other"));
        assert!(!Arc::ptr_eq(&attrs.env, &later.env));
        assert_eq!(later.cwd, PathBuf::from("/tmp/other"));
    }
}
