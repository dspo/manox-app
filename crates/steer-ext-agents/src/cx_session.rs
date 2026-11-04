//! [`TerminalProcess`] over a `cx::SessionHandle`. This bridge lives in
//! ext-agents (next to the `SessionHandle` it wraps); the terminal view
//! itself is `steer_ghostty::GhosttyView`.
//!
//! An external agent CLI (claude/codex/copilot) runs under a PTY that cx
//! owns; this module bridges cx's blocking `SessionHandle::read` / `wait`
//! into the ghostty view's [`ProcessOutput`], so the rendering pipeline is
//! identical for a local shell and a cx-hosted agent.

use std::io;
use std::sync::Arc;
use std::thread;

use steer_term_process::{ProcessOutput, TerminalProcess};

use crate::SessionHandle;

/// A [`TerminalProcess`] over a shared `cx::SessionHandle`. The handle is
/// held behind `Arc` so the external-session owner (the workspace's
/// `ExternalSession`) keeps an independent clone for explicit `kill` on
/// close, while the terminal render path drives IO through this process.
pub struct CxProcess {
    handle: Arc<SessionHandle>,
}

impl CxProcess {
    /// Wrap a shared handle. The caller keeps its own `Arc` clone for the
    /// external-session close path; this process only borrows the share.
    pub fn new(handle: Arc<SessionHandle>) -> Self {
        Self { handle }
    }
}

impl TerminalProcess for CxProcess {
    fn attach(self: Arc<Self>, out: ProcessOutput) {
        // Reader: blocking `handle.read` → `output`. EOF (0 bytes) means the
        // agent closed its stdout / died; `Interrupted` is retried, other
        // errors break. A bare `std::thread` (not `spawn_blocking`) so the
        // tokio pool is never blocked by an unbounded PTY read.
        let reader_handle = Arc::clone(&self.handle);
        let reader_out = out.clone();
        thread::Builder::new()
            .name("steer-cx-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader_handle.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => reader_out.output(&buf[..n]),
                        Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            })
            .expect("spawn cx reader thread");

        // Waiter: blocking `handle.wait` reaps the child once, then reports
        // the exit. `wait` is one-shot (takes the child out of its Mutex),
        // so exactly one waiter thread exists per session — the process is
        // constructed once per session and `attach` is called once.
        let waiter_handle = Arc::clone(&self.handle);
        thread::Builder::new()
            .name("steer-cx-wait".into())
            .spawn(move || {
                let code = waiter_handle
                    .wait()
                    .ok()
                    .map(|result| result.exit_code as u32);
                out.exited(code);
            })
            .expect("spawn cx waiter thread");
    }

    fn write(&self, bytes: &[u8]) {
        // `write_bytes` — raw, no trailing newline. Keystroke-grade TUI
        // driving (arrow keys, Ctrl+C, paste fragments) rather than line
        // injection; `cx::SessionHandle::write` is the line path.
        let _ = self.handle.write_bytes(bytes);
    }

    fn resize(&self, cols: u16, rows: u16) {
        let _ = self.handle.resize(cols, rows);
    }

    fn kill(&self) {
        let _ = self.handle.kill();
    }
}
