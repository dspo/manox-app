//! The AHP terminal channel as a [`PtySource`] (#886's companion).
//!
//! `TerminalView` and its whole rendering/selection/keyboard machinery stay
//! untouched — what changes is where the bytes come from: instead of a
//! kernel-side PTY the app owns, the terminal lives in the HOST's registry
//! (`createTerminal`), its output rides `terminal/data` actions on
//! `ahp-terminal:/<id>`, and keystrokes go back as `terminal/input`
//! dispatches. The in-proc leg measured well under the perception threshold
//! (dspo/manox#886's latency gate), so the emulation mirror costs nothing a
//! user can feel.

use std::io;
use std::sync::Mutex;

use ahp::{Client, SubscriptionEvent};
use ahp_types::actions::{StateAction, TerminalInputAction, TerminalResizedAction};
use async_channel::Sender;
use manox_terminal::event::TerminalEvent;
use manox_terminal::pty_source::PtySource;

/// The process's connected client + the foreground session id — the bridge's
/// two facts. One host per process (the gateway singleton), one focused
/// conversation; the store publishes both on connect and focus changes.
#[derive(Default)]
struct Shared {
    client: Option<Client>,
    focused_session: Option<String>,
}

fn shared() -> &'static Mutex<Shared> {
    static SHARED: std::sync::OnceLock<Mutex<Shared>> = std::sync::OnceLock::new();
    SHARED.get_or_init(|| Mutex::new(Shared::default()))
}

/// Publish the connected client (the store calls this on handshake; `None`
/// on transport loss).
pub fn set_client(client: Option<Client>) {
    if let Ok(mut guard) = shared().lock() {
        guard.client = client;
    }
}

/// Publish the foreground session id (the terminal a dock spawn claims —
/// the host derives ownership and the default cwd from it).
pub fn set_focused_session(session_id: Option<String>) {
    if let Ok(mut guard) = shared().lock() {
        guard.focused_session = session_id;
    }
}

/// `ahp-terminal:/<id>`.
fn terminal_uri(id: &str) -> String {
    format!("ahp-terminal:/{id}")
}

/// Ask the host to spawn a terminal in its registry and answer the minted
/// channel-backed view pair: `(terminal id, source)`. The caller wires the
/// source into `manox_terminal::Terminal::spawn` exactly as a PTY source.
pub fn spawn_host_terminal(
    cwd: &std::path::Path,
    cols: u16,
    rows: u16,
) -> Result<(String, ChannelTerminalSource), String> {
    let (client, session_id) = {
        let guard = shared().lock().map_err(|e| e.to_string())?;
        let client = guard.client.clone().ok_or_else(|| {
            "the AHP host is not connected — the dock terminal needs the session link".to_string()
        })?;
        let session = guard
            .focused_session
            .clone()
            .unwrap_or_else(|| "s-dock".to_string());
        (client, session)
    };
    let terminal_id = format!("t-{}", uuid::Uuid::new_v4().simple());
    let uri = terminal_uri(&terminal_id);
    let params = ahp_types::commands::CreateTerminalParams {
        channel: uri.clone(),
        meta: None,
        claim: ahp_types::state::TerminalClaim::Session(ahp_types::state::TerminalSessionClaim {
            session: crate::ahp_store::session_uri(&session_id),
            chat: crate::ahp_store::session_uri(&session_id),
            turn_id: None,
            tool_call_id: None,
        }),
        name: Some("dock".to_string()),
        cwd: Some(format!("file://{}", cwd.display())),
        cols: Some(cols as i64),
        rows: Some(rows as i64),
    };
    // Blocking ask on the runtime handle: the spawn path must not surface a
    // terminal whose host-side creation has not landed (the subscribe in
    // `start` would race the registry insert).
    let create_client = client.clone();
    manox_agent::runtime::handle().block_on(async move {
        create_client
            .request::<_, serde_json::Value>("createTerminal", params)
            .await
            .map_err(|err| err.to_string())?;
        Ok::<(), String>(())
    })?;
    Ok((
        terminal_id.clone(),
        ChannelTerminalSource::new(client, terminal_id),
    ))
}

/// A [`PtySource`] over the AHP terminal channel: reads `terminal/data`
/// actions, writes `terminal/input` dispatches, resizes via
/// `terminal/resized`. The emulation core (and with it the whole view) runs
/// locally against the mirrored stream.
pub struct ChannelTerminalSource {
    client: Client,
    terminal_id: String,
}

impl ChannelTerminalSource {
    pub fn new(client: Client, terminal_id: String) -> Self {
        Self {
            client,
            terminal_id,
        }
    }
}

impl PtySource for ChannelTerminalSource {
    fn start(&mut self, event_tx: Sender<TerminalEvent>) {
        let client = self.client.clone();
        let uri = terminal_uri(&self.terminal_id);
        // The reader leg: subscribe, then pump this channel's data actions
        // into the emulation core as raw bytes. The stream ends with the
        // transport; an `Exit` then settles the local mirror.
        manox_agent::runtime::handle().spawn(async move {
            if let Err(err) = client
                .request::<_, serde_json::Value>("subscribe", serde_json::json!({ "channel": uri }))
                .await
            {
                tracing::warn!(channel = %uri, %err, "terminal bridge: subscribe failed");
                let _ = event_tx.send(TerminalEvent::Exit).await;
                return;
            }
            let mut events = client.events();
            while let Some(event) = events.recv().await {
                if event.channel != uri {
                    continue;
                }
                if let SubscriptionEvent::Action(envelope) = event.event {
                    match envelope.action {
                        StateAction::TerminalData(data) => {
                            let _ = event_tx
                                .send(TerminalEvent::PtyOutput(data.data.into_bytes()))
                                .await;
                        }
                        // The exited lifecycle settles the local mirror: the
                        // host's retained grid stays answerable for history,
                        // but a live panel's process is gone.
                        StateAction::TerminalCwdChanged(_) => {}
                        _ => {}
                    }
                }
            }
            tracing::info!(channel = %uri, "terminal bridge: transport closed");
            let _ = event_tx.send(TerminalEvent::Exit).await;
        });
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        // Enqueue-only (the trait's invariant): the dispatch rides the
        // runtime's task queue, never the caller's thread.
        let data = String::from_utf8_lossy(bytes).into_owned();
        let client = self.client.clone();
        let uri = terminal_uri(&self.terminal_id);
        manox_agent::runtime::handle().spawn(async move {
            if let Err(err) = client
                .dispatch(
                    uri.clone(),
                    StateAction::TerminalInput(TerminalInputAction { data }),
                )
                .await
            {
                tracing::warn!(channel = %uri, %err, "terminal bridge: input dropped");
            }
        });
        Ok(())
    }

    fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let client = self.client.clone();
        let uri = terminal_uri(&self.terminal_id);
        manox_agent::runtime::handle().spawn(async move {
            if let Err(err) = client
                .dispatch(
                    uri.clone(),
                    StateAction::TerminalResized(TerminalResizedAction {
                        cols: cols as i64,
                        rows: rows as i64,
                    }),
                )
                .await
            {
                tracing::warn!(channel = %uri, %err, "terminal bridge: resize dropped");
            }
        });
        Ok(())
    }
}
