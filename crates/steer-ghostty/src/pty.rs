//! PTY transport: spawn the shell, expose writer/reader/resize/wait.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

pub struct Pty {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
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
        cmd.env("TERM_PROGRAM", "gterm");
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
                child: Mutex::new(child),
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

    pub fn wait(&self) -> Option<u32> {
        self.child
            .lock()
            .unwrap()
            .wait()
            .ok()
            .map(|s| s.exit_code())
    }
}
