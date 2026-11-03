//! PTY transport: spawn the shell, expose writer/reader/resize/wait/terminate.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// Poll cadence of [`Pty::wait`]. The exit watcher sleeps here between
/// non-blocking reaps so [`Pty::terminate`] can take the child lock promptly;
/// a blocking `wait` held that lock for the child's whole life, which made
/// termination from the UI thread impossible.
const REAP_POLL: Duration = Duration::from_millis(50);

pub struct Pty {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    /// `None` once reaped or terminated: this is the handle every kill path
    /// takes, and dropping it is what lets the process go.
    child: Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>,
    closed: AtomicBool,
}

impl Pty {
    /// Spawn `shell` on a new pty. Returns the handle plus the blocking
    /// reader end (hand it to a thread that feeds the emulator).
    pub fn spawn_program(
        program: &str,
        args: &[&str],
        cwd: &std::path::Path,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<(Self, Box<dyn Read + Send>)> {
        let pair = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(program);
        for a in args {
            cmd.arg(a);
        }
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Steer");
        let child = pair.slave.spawn_command(cmd)?;
        // Drop the parent-side slave handle so the pty EOFs when the child's
        // session dies.
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let reader = pair.master.try_clone_reader()?;
        Ok((
            Self {
                writer: Arc::new(Mutex::new(writer)),
                master: Mutex::new(pair.master),
                child: Mutex::new(Some(child)),
                closed: AtomicBool::new(false),
            },
            reader,
        ))
    }

    pub fn write(&self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let master = self.master.lock().unwrap();
        let _ = master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    /// Terminate the shell. Idempotent, non-blocking, safe from any thread:
    /// closing the child is what unblocks the reader (the pty EOFs) and the
    /// exit watcher. Without this a closed tab leaves the shell — and both of
    /// its watcher threads — running for the life of the process.
    pub fn terminate(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let mut child = self.child.lock().unwrap();
        if let Some(child) = child.as_mut() {
            let _ = child.kill();
        }
    }

    /// Reap the child, polling so the lock is never held across a blocking
    /// wait. Returns the exit code when the child is gone, or `None` when it
    /// was terminated by us.
    pub fn wait(&self) -> Option<u32> {
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return None;
            }
            // `Some(_)` = the child is gone (the inner code is `None` for a
            // reaped-or-failed wait); `None` = still running.
            let reaped: Option<Option<u32>> = {
                let mut guard = self.child.lock().unwrap();
                let child = guard.as_mut()?;
                match child.try_wait() {
                    Ok(Some(status)) => {
                        *guard = None;
                        Some(Some(status.exit_code()))
                    }
                    Ok(None) => None,
                    Err(_) => {
                        *guard = None;
                        Some(None)
                    }
                }
            };
            match reaped {
                Some(code) => return code,
                None => std::thread::sleep(REAP_POLL),
            }
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Belt and braces: a view dropped without `terminate` must not orphan
        // the shell.
        self.terminate();
    }
}
