//! The AHP terminal channel as a [`PtySource`].
//!
//! `TerminalView` and its whole rendering/selection/keyboard machinery stay
//! untouched — what changes is where the bytes come from: instead of a
//! kernel-side PTY the app owns, the terminal lives in the HOST's registry
//! (`createTerminal`), its output rides `terminal/data` actions on
//! `ahp-terminal:/<id>`, and keystrokes go back as `terminal/input`
//! dispatches. The in-proc leg stays well under the perception threshold, so
//! the emulation mirror costs nothing a user can feel.

use std::io;
use std::sync::Mutex;

use ahp::{Client, SubscriptionEvent};
use ahp_types::actions::{StateAction, TerminalInputAction, TerminalResizedAction};
use ahp_types::state::SnapshotState;
use async_channel::Sender;
use manox_ahp::channels::terminal;
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

/// Ask the host to spawn a terminal in its registry and answer the
/// channel-backed source. The caller wires the source into
/// `manox_terminal::Terminal::spawn` exactly as a PTY source.
pub fn spawn_host_terminal(
    cwd: &std::path::Path,
    cols: u16,
    rows: u16,
) -> Result<ChannelTerminalSource, String> {
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
    let uri = terminal::uri(&terminal_id);
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
    Ok(ChannelTerminalSource::new(client, terminal_id))
}

/// A [`PtySource`] over the AHP terminal channel: reads `terminal/data`
/// actions, writes `terminal/input` dispatches, resizes via
/// `terminal/resized`. The emulation core (and with it the whole view) runs
/// locally against the mirrored stream.
pub struct ChannelTerminalSource {
    client: Client,
    terminal_id: String,
    /// Close signal for the reader task; dropped with the source (the panel
    /// is gone, so the mirror and the host terminal both come down).
    shutdown: Option<Sender<()>>,
}

impl ChannelTerminalSource {
    pub fn new(client: Client, terminal_id: String) -> Self {
        Self {
            client,
            terminal_id,
            shutdown: None,
        }
    }
}

/// One event into the mirror; `Err` means the panel is gone.
async fn forward(tx: &Sender<TerminalEvent>, event: TerminalEvent) -> Result<(), ()> {
    tx.send(event).await.map_err(|_| ())
}

/// The exited settlement: the child code lands on the core's exit state and
/// the mirror stops expecting further output.
async fn settle(event_tx: &Sender<TerminalEvent>, exit_code: i32) -> Result<(), ()> {
    forward(event_tx, TerminalEvent::ChildExit(exit_code)).await?;
    forward(event_tx, TerminalEvent::Exit).await?;
    Ok(())
}

/// Seed the mirror from the subscribe snapshot: output emitted between the
/// host-side spawn and this subscribe never arrives as actions — the visible
/// grid is what AHP's reconstruction contract owes a late subscriber.
/// `Some(code)` means the snapshot already shows an exited process.
async fn seed_from_snapshot(
    snapshot: Option<ahp_types::state::Snapshot>,
    event_tx: &Sender<TerminalEvent>,
) -> Result<Option<i32>, ()> {
    let Some(SnapshotState::Terminal(state)) = snapshot.map(|s| s.state) else {
        return Ok(None);
    };
    for part in state.content {
        // The raw VT stream: an unclassified part contributes its text
        // verbatim, a command part contributes its captured output (the
        // command line was already echoed into the grid), unknown parts
        // carry nothing answerable.
        let bytes = match part {
            ahp_types::state::TerminalContentPart::Unclassified(part) => part.value,
            ahp_types::state::TerminalContentPart::Command(part) => part.output,
            ahp_types::state::TerminalContentPart::Unknown(_) => continue,
        };
        forward(event_tx, TerminalEvent::PtyOutput(bytes.into_bytes())).await?;
    }
    match state.lifecycle {
        ahp_types::state::TerminalLifecycleState::Exited(exited) => {
            Ok(Some(exited.exit_code.unwrap_or(-1) as i32))
        }
        ahp_types::state::TerminalLifecycleState::Running(_) => Ok(None),
    }
}

/// The live reader: `Ok(code)` when the stream is over (the exited action's
/// code, or `-1` when it simply ended); `Err` when the panel is gone.
async fn pump_subscription(
    mut subscription: ahp::SessionSubscription,
    event_tx: &Sender<TerminalEvent>,
    shutdown_rx: async_channel::Receiver<()>,
) -> Result<i32, ()> {
    loop {
        let event = tokio::select! {
            event = subscription.recv() => event,
            // The source dropped: the panel is gone, nothing left to mirror.
            _ = shutdown_rx.recv() => return Err(()),
        };
        let Some(SubscriptionEvent::Action(envelope)) = event else {
            return Ok(-1);
        };
        match envelope.action {
            StateAction::TerminalData(data) => {
                forward(event_tx, TerminalEvent::PtyOutput(data.data.into_bytes())).await?;
            }
            StateAction::TerminalExited(exited) => {
                return Ok(exited.exit_code.unwrap_or(-1) as i32);
            }
            _ => {}
        }
    }
}

impl PtySource for ChannelTerminalSource {
    fn start(&mut self, event_tx: Sender<TerminalEvent>) {
        let client = self.client.clone();
        let uri = terminal::uri(&self.terminal_id);
        let (shutdown_tx, shutdown_rx) = async_channel::bounded::<()>(1);
        self.shutdown = Some(shutdown_tx);
        // The reader leg: the per-channel subscription (not the all-events
        // firehose — a lagging broadcast consumer silently skips frames, and
        // a terminal stream must not re-order or drop), seeded from the
        // subscribe snapshot, ending with the transport or the source.
        manox_agent::runtime::handle().spawn(async move {
            let (result, subscription) = match client.subscribe(uri.clone()).await {
                Ok(pair) => pair,
                Err(err) => {
                    tracing::warn!(channel = %uri, %err, "terminal bridge: subscribe failed");
                    let _ = settle(&event_tx, -1).await;
                    return;
                }
            };
            match seed_from_snapshot(result.snapshot, &event_tx).await {
                Err(()) => return,
                // The host-side process is already gone (a re-attach to a
                // retained grid): settle and stop mirroring.
                Ok(Some(code)) => {
                    let _ = settle(&event_tx, code).await;
                    return;
                }
                Ok(None) => {}
            }
            // The stream over (transport closed or the exited action) settles
            // the mirror; a gone panel leaves teardown to the source's drop.
            if let Ok(exit_code) = pump_subscription(subscription, &event_tx, shutdown_rx).await {
                tracing::info!(channel = %uri, "terminal bridge: stream ended");
                let _ = settle(&event_tx, exit_code).await;
            }
        });
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        // Enqueue-only (the trait's invariant): the dispatch rides the
        // runtime's task queue, never the caller's thread.
        let data = String::from_utf8_lossy(bytes).into_owned();
        let client = self.client.clone();
        let uri = terminal::uri(&self.terminal_id);
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
        let uri = terminal::uri(&self.terminal_id);
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

impl Drop for ChannelTerminalSource {
    fn drop(&mut self) {
        // The panel is gone: release the host terminal so the shell dies
        // with its view (the PTY-handle drop this replaces killed the
        // child), and let the reader task see the close signal.
        self.shutdown = None;
        let Some(handle) = manox_agent::runtime::try_handle() else {
            return;
        };
        let client = self.client.clone();
        let uri = terminal::uri(&self.terminal_id);
        handle.spawn(async move {
            // The typed unsubscribe also tears down the client's local fan-out
            // for the URI; a raw request would strand the per-channel sender.
            if let Err(err) = client.unsubscribe(uri.clone()).await {
                tracing::warn!(channel = %uri, %err, "terminal bridge: unsubscribe failed");
            }
            if let Err(err) = client
                .request::<_, serde_json::Value>(
                    "disposeTerminal",
                    serde_json::json!({ "channel": uri }),
                )
                .await
            {
                tracing::warn!(channel = %uri, %err, "terminal bridge: dispose failed");
            }
        });
    }
}
