//! The gpui-backed `CapabilityClient` provider — the process-wide seam the
//! kernel reaches the frontend through (`manox_agent::capability::provider()`).
//!
//! Two capability families ride here:
//! - **Clipboard** (OSC 52 copy/paste, the host clipboard tools) — available on
//!   every platform. gpui `App` is main-thread only and `AsyncApp` is not
//!   `Send`, so kernel-side requests hop to the foreground through a channel.
//! - **Browser ops** (`web_explore_*`) — only on platforms that build the
//!   integrated browser. The embedded webview is compiled out on
//!   Linux/FreeBSD (wry parents Linux child webviews through Xlib only, and a
//!   native Wayland window cannot host one), so `browser_op` there is a
//!   clean refusal; the runtime keeps `web_explore_*` fail-closed.

use std::sync::Arc;

use futures::future::BoxFuture;
use manox_agent::capability::CapabilityClient;
use manox_agent::thread_engine::{BrowserOp, BrowserReply};

/// A browser op parked on the main-thread service loop, paired with its reply
/// oneshot. Only constructed on platforms that build the integrated browser.
#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
struct BrowserCapabilityMsg {
    op: BrowserOp,
    reply: futures::channel::oneshot::Sender<Result<BrowserReply, String>>,
}

/// A clipboard request parked on its own channel: gpui `App` is main-thread
/// only, so the kernel side hops to the foreground through this channel
/// rather than holding an `AsyncApp` (which is not `Send`).
enum ClipboardMsg {
    Write {
        text: String,
    },
    Read {
        reply: futures::channel::oneshot::Sender<Result<Option<String>, String>>,
    },
}

/// The gpui capability provider. Browser ops ride the browser channel (absent
/// where the integrated browser is compiled out); clipboard ops always ride
/// the clipboard channel.
pub struct GpuiCapability {
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    tx: async_channel::Sender<BrowserCapabilityMsg>,
    clipboard_tx: async_channel::Sender<ClipboardMsg>,
}

impl GpuiCapability {
    /// Create the provider and spawn its main-thread service loops on `cx`.
    fn start(cx: &gpui::AsyncApp) -> Arc<Self> {
        // One task per browser op so a suspending op (e.g. `YieldToUser`,
        // parked until the user hands the tab back) does not serialize later
        // ops — parity with the prior per-request `cx.spawn`.
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let (tx, rx) = async_channel::unbounded::<BrowserCapabilityMsg>();
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            while let Ok(msg) = rx.recv().await {
                cx.spawn(async move |cx: &mut gpui::AsyncApp| {
                    let result = crate::browser_host::execute_browser_op(msg.op, cx).await;
                    let _ = msg.reply.send(result);
                })
                .detach();
            }
        })
        .detach();
        let (clipboard_tx, clipboard_rx) = async_channel::unbounded::<ClipboardMsg>();
        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            while let Ok(msg) = clipboard_rx.recv().await {
                match msg {
                    ClipboardMsg::Write { text } => {
                        cx.update(|cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text))
                        });
                    }
                    ClipboardMsg::Read { reply } => {
                        let text =
                            cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
                        let _ = reply.send(Ok(text));
                    }
                }
            }
        })
        .detach();
        Arc::new(Self {
            #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
            tx,
            clipboard_tx,
        })
    }
}

/// One-shot App-startup wiring: register the capability provider so the kernel
/// reaches the frontend through `capability::provider()`.
pub fn install(cx: &mut gpui::AsyncApp) {
    manox_agent::capability::set_provider(GpuiCapability::start(cx));
}

impl CapabilityClient for GpuiCapability {
    /// OSC 52 copy: parked on the clipboard channel and run on the
    /// foreground; fire-and-forget, so a closed channel is the only failure.
    fn clipboard_write(&self, text: String) -> Result<(), String> {
        self.clipboard_tx
            .try_send(ClipboardMsg::Write { text })
            .map_err(|_| "clipboard capability closed".to_string())
    }

    /// OSC 52 paste: text entries only; an image-only clipboard reads as
    /// empty rather than failing. Awaits the foreground round-trip — never
    /// blocks, so it is safe to call from a runtime worker.
    fn clipboard_read(&self) -> BoxFuture<'static, Result<Option<String>, String>> {
        let tx = self.clipboard_tx.clone();
        Box::pin(async move {
            let (reply, rx) = futures::channel::oneshot::channel();
            if tx
                .try_send(ClipboardMsg::Read { reply })
                .map_err(|_| "clipboard capability closed".to_string())
                .is_err()
            {
                return Err("clipboard capability closed".to_string());
            }
            rx.await
                .unwrap_or_else(|_| Err("clipboard capability dropped".to_string()))
        })
    }

    fn browser_op(&self, op: BrowserOp) -> BoxFuture<'static, Result<BrowserReply, String>> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let _ = op;
            Box::pin(std::future::ready(Err(
                "integrated browser is not available on this platform".to_string(),
            )))
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        {
            let tx = self.tx.clone();
            Box::pin(async move {
                let (reply, rx) = futures::channel::oneshot::channel();
                if tx.send(BrowserCapabilityMsg { op, reply }).await.is_err() {
                    return Err("browser capability channel closed".to_string());
                }
                rx.await
                    .unwrap_or_else(|_| Err("browser capability dropped".to_string()))
            })
        }
    }
}
