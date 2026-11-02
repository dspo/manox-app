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
//! workspace. Nothing consumes the event yet — the teardown cascade
//! (resource reclaim → sub-session archive → row drop) lands with the
//! lifecycle PRs.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::*;
use gpui::EventEmitter;
use manox_terminal::event::TerminalEvent;

use super::external_sessions::clamp_external_title;

/// Which conversation owns the container — the type vocabulary the 0 号
/// resource decides (§2.2). The kind IS the capability set: only
/// [`ContainerKind::Manox`] may own level-1 sub-sessions; the external kinds
/// own their 0 号 process plus resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    /// A manox session (the 0 号 is a Captain).
    Manox,
    Claude,
    Codex,
    Copilot,
}

impl ContainerKind {
    /// The kind the external row-id namespace names (`external:{agent}:{uuid}`'s
    /// agent segment — the id the project menu mints). `None` for a plain
    /// terminal (`agent == "terminal"`): its 0 号 is a bare shell, not a CLI
    /// agent, and the kind vocabulary has no shell entry — the container
    /// still exists, it just carries no agent kind.
    pub fn from_external_agent(agent_id: &str) -> Option<Self> {
        match agent_id {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "copilot" => Some(Self::Copilot),
            _ => None,
        }
    }

    /// Whether the kind's 0 号 is an app-spawned process (D8: process exit is
    /// death, no archive gesture) rather than a manox session (D7: archive is
    /// death).
    pub fn is_external(self) -> bool {
        !matches!(self, Self::Manox)
    }
}

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
/// attrs)`): the kind rides the seed — a native seed IS a
/// [`ContainerKind::Manox`] container by definition, an external seed's kind
/// is its 0 号's agent namespace.
pub enum ContainerSeed<'a> {
    /// A manox conversation container: the native conceptual two steps
    /// (container record + session) fold into ONE `createSession` with the
    /// id shared. `attrs` derive here — the cwd from the belonging (or the
    /// window's workspace root), the env from process state the session
    /// never consumes (D11).
    Manox(NativeContainerSeed),
    /// An external CLI container: the record lands here, the process spawned
    /// before (记录 + spawn 两步 — see `project_menu`'s cascade).
    External(ExternalContainerSeed<'a>),
}

/// The native create intent — everything downstream of the draft-refill
/// decision (the inherited picks ride the wire create intent).
pub struct NativeContainerSeed {
    /// The conversation's belonging: the launch project (the sidebar row
    /// handed it in), or the foreground conversation's inherited one.
    pub project: Option<PathBuf>,
    /// The inherited model, wire-shaped (`provider/id`).
    pub model: Option<String>,
    /// The inherited access mode.
    pub access: Option<String>,
    /// The inherited reasoning effort.
    pub effort: Option<String>,
}

/// The external record seed — the already-spawned PTY's identity and row
/// face (the project menu's launch payload, unchanged).
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
    /// The project directory the session was launched in — the sidebar group
    /// it merges into; `None` → the loose Chats bucket.
    pub project: Option<PathBuf>,
    /// The spawned terminal (its view occupies the main column while the
    /// session is foreground; dropping it tears the process tree down).
    pub view: gpui::Entity<terminal_ui::TerminalView>,
}

/// The death-predicate flip (§2.3): the container's 0 号 reached its kind's
/// death — a manox session archived (D7), an external process exited (D8).
/// Emitted on the [`Workspace`]; the teardown cascade lands with the
/// lifecycle PRs.
#[derive(Debug)]
pub enum ContainerEvent {
    /// The container id whose `alive` read flipped to false.
    Died(String),
}

impl EventEmitter<ContainerEvent> for Workspace {}

impl Workspace {
    /// The ONE container creation entry (§2.2 `create_container(kind,
    /// attrs)`). Native folds its conceptual two steps (container record +
    /// session) into one `createSession` — the draft-refill-or-mint decision,
    /// the object row and the attach all live here, verbatim from the former
    /// `start_new_thread` body; external lands the record (the spawn already
    /// happened). Returns the container id — the routing string both homes
    /// share.
    pub fn create_container(&mut self, seed: ContainerSeed, cx: &mut Context<Self>) -> String {
        match seed {
            ContainerSeed::Manox(intent) => self.create_native_container(intent, cx),
            ContainerSeed::External(seed) => self.create_external_container(seed, cx),
        }
    }

    /// The native half: a manox conversation container. The id is
    /// client-minted (or the draft's own); the attach lands asynchronously.
    fn create_native_container(
        &mut self,
        intent: NativeContainerSeed,
        cx: &mut Context<Self>,
    ) -> String {
        let NativeContainerSeed {
            project,
            model,
            access,
            effort: effort_str,
        } = intent;
        let ws = cx.weak_entity();
        let dir_for_store = project.clone();
        // The draft refill: a never-materialized conversation under the
        // same project (or project-less) is that project's unfinished
        // draft — return to it instead of minting a sibling. Its row
        // already carries the model/access picks the user left there.
        // The FOREGROUND session and archived rows are excluded: the
        // landing conversation (created project-less at boot, never
        // spoken) is itself the latest project-less draft, and a just
        // archived draft would be resurrected — refilling either would
        // make "new session" a no-op on the very session it should leave.
        let foreground = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(_, sid)| sid.clone());
        let objects = manox_agent::object_store::ObjectStore::try_global();
        let refill = objects.as_ref().and_then(|store| {
            store.latest_draft(
                project
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .as_deref(),
            )
        });
        let refill = refill.filter(|row| {
            !row.archived
                && foreground.as_deref() != Some(row.id.as_str())
                && !self.session_retired(cx, &row.id)
        });
        // v3: the id is client-minted; a refill carries the draft's own
        // picks (its row wins over inheritance) and a fresh conversation the
        // inherited config, and the attach lands once the host answered.
        let sid = refill
            .as_ref()
            .map(|row| row.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
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
        if let Some(row) = &refill {
            if let Some(model_id) = row.model_id.as_deref() {
                let model = match row.model_provider.as_deref() {
                    Some(provider) => format!("{provider}/{model_id}"),
                    None => model_id.to_string(),
                };
                config.insert("model".into(), serde_json::json!(model));
            }
            if let Some(access) = &row.access_mode {
                config.insert("accessMode".into(), serde_json::json!(access));
            }
            if let Some(effort) = &row.reasoning_effort {
                config.insert("reasoningEffort".into(), serde_json::json!(effort));
            }
        }
        // `working_directories` is the create intent's only directory
        // carrier (the host binds its first entry as the session's cwd): a
        // project launch binds the project — the landing chip and the
        // sidebar group both read it back — a loose one the fallback cwd.
        // An empty vector leaves the session directory-less ("choose
        // project" on a thread the user launched FROM a project).
        let cwd = project
            .as_ref()
            .unwrap_or(&self.cwd)
            .to_string_lossy()
            .to_string();
        let cwds = vec![format!("file://{cwd}")];
        // The object row: created at once, carrying the conversation's
        // belonging and picks. A refilled draft reuses its row (the picks
        // it holds win over inheritance); a fresh conversation starts a
        // new row.
        let (provider, model_id, access, effort) = match &refill {
            Some(row) => (
                row.model_provider.as_deref(),
                row.model_id.as_deref(),
                row.access_mode.as_deref(),
                row.reasoning_effort.as_deref(),
            ),
            None => {
                let (provider, model_id) = super::attach::split_wire_model(model.as_deref());
                (provider, model_id, access.as_deref(), effort_str.as_deref())
            }
        };
        super::attach::land_conversation_row(
            &sid,
            project.as_deref(),
            provider,
            model_id,
            access,
            effort,
        );
        let reply = self.with_foreground_store(cx, |store, _| {
            store.create_session(&sid, cwds, Some(config))
        });
        let sid2 = sid.clone();
        cx.spawn(async move |_, cx| {
            // A create refused with the protocol's SESSION_ALREADY_EXISTS
            // means the refill named a session the host still carries: the
            // draft is LIVE (the archive-time bit and the host-retired check
            // keep retired rows out of the refill). The host owns that live
            // session and this create does not touch it — attaching to it is
            // the refill's intent, the same way the host's claim table reuses
            // a blank id. Every other failure (and a store unbound, where no
            // create can even ride) lands a visible notification: a
            // user-initiated new session must never die quietly.
            let refusal = match reply {
                Some(reply) => match reply.recv().await {
                    Ok(Ok(_)) => None,
                    Ok(Err(err)) if err.is_already_exists() => {
                        tracing::info!(
                            session_id = %sid2,
                            error = %err,
                            "createSession met a live draft — opening it instead"
                        );
                        None
                    }
                    Ok(Err(err)) => Some(err.message),
                    Err(closed) => Some(closed.to_string()),
                },
                None => Some(i18n::t("new-thread-no-session").to_string()),
            };
            if let Some(err) = refusal {
                tracing::warn!(session_id = %sid2, error = %err, "createSession failed");
                let _ = ws.update_in(cx, |_this, window, cx| {
                    window.push_notification(
                        Notification::error(i18n::t_str(
                            "new-thread-create-failed",
                            &[("err", &err)],
                        )),
                        cx,
                    );
                });
                return;
            }
            if let Some(dir) = &dir_for_store {
                let _ = ws.update(cx, |_ws, cx| {
                    Workspace::register_project_in_store(dir, cx);
                });
            }
            match ws.update_in(cx, |this, window, cx| {
                this.attach_created_session(&sid2, window, cx);
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
            project,
            view,
        } = seed;
        let kind = ContainerKind::from_external_agent(agent_id);
        let id = ContainerId::external(agent_id);
        // The terminal may carry a title already — the program claims it
        // through the PTY as soon as it boots — and that claim beats the
        // launch label for the initial row.
        let default_label = label;
        let label = view
            .read(cx)
            .terminal()
            .read(cx)
            .title()
            .map(|t| clamp_external_title(&t))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| default_label.clone());
        // The D10 snapshot: the 0 号's spawn cwd (what the terminal was
        // rooted at) and the environment as of this call — the record's later
        // life never re-reads either.
        let attrs = ContainerAttrs::snapshot(view.read(cx).terminal().read(cx).cwd());
        // Title events ride the PROXY's gpui event face, never the kernel
        // handle's broadcast: the proxy pump assumes it is the handle's sole
        // subscriber (its `Arc::try_unwrap` drops every event otherwise).
        // The same face carries the death signal (D8): the child's exit
        // flips the record's alive bit and raises the container event.
        let proxy = view.read(cx).terminal().clone();
        let id_for_sub = id.as_str().to_string();
        let title_sub = cx.subscribe(&proxy, move |this, _, ev: &TerminalEvent, cx| match ev {
            TerminalEvent::Title(title) => {
                this.apply_terminal_title(&id_for_sub, title.clone(), cx);
            }
            TerminalEvent::ChildExit(_) | TerminalEvent::Exit => {
                this.retire_external_container(&id_for_sub, cx);
            }
            _ => {}
        });
        self.externals
            .push(super::external_sessions::ExternalSessionRecord {
                id: id.as_str().to_string(),
                cx_session_id,
                default_label,
                label,
                svg,
                project,
                created_at: chrono::Utc::now().timestamp(),
                view,
                kind,
                attrs,
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
    /// the record's alive bit and raise the event. The record itself stays
    /// (the sidebar row remains until the lifecycle PR wires the teardown);
    /// a second exit event after the flip is a no-op.
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

    /// The project association, LIVE-read (D10): native consults the
    /// conversation's object row per call (the belonging is registry state —
    /// re-binding mid-life is legal and shows up on the next read); external
    /// reads the record's launch-time project (pure app memory, §2.2). The
    /// sidebar's removed-projects overlay applies on top of this at
    /// projection time, exactly as before.
    pub fn container_project(&self, container_id: &str) -> Option<PathBuf> {
        if ContainerId::is_external_id(container_id) {
            self.externals
                .iter()
                .find(|s| s.id == container_id)?
                .project
                .clone()
        } else {
            manox_agent::object_store::ObjectStore::try_global()
                .and_then(|objects| objects.project_of(container_id))
                .map(PathBuf::from)
        }
    }

    /// The container's kind, read by id: native is always
    /// [`ContainerKind::Manox`] (the 0 号 is a Captain — the view constructs
    /// on demand); external resolves through the record (a plain terminal
    /// carries no kind). An id with no record behind it has no kind.
    pub fn container_kind(&self, container_id: &str) -> Option<ContainerKind> {
        if ContainerId::is_external_id(container_id) {
            self.externals.iter().find(|s| s.id == container_id)?.kind
        } else {
            Some(ContainerKind::Manox)
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

    // ── the kind is the 0 号's namespace (§2.2) ──

    /// The three CLI agents name their kinds; a plain terminal names none
    /// (its 0 号 is a bare shell) — the container exists but carries no
    /// agent kind. Manox is never derivable from the external namespace.
    #[test]
    fn external_agents_name_their_kind_and_terminals_name_none() {
        assert_eq!(
            ContainerKind::from_external_agent("claude"),
            Some(ContainerKind::Claude)
        );
        assert_eq!(
            ContainerKind::from_external_agent("codex"),
            Some(ContainerKind::Codex)
        );
        assert_eq!(
            ContainerKind::from_external_agent("copilot"),
            Some(ContainerKind::Copilot)
        );
        assert_eq!(
            ContainerKind::from_external_agent("terminal"),
            None,
            "a plain terminal's 0 号 is a bare shell — no kind"
        );
        assert!(!ContainerKind::Manox.is_external());
        assert!(ContainerKind::Claude.is_external());
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
