//! Runs an agent CLI: its stdout lines and its exit come back as messages,
//! and lines go to its stdin.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

pub enum Output {
    Line(String),
    /// The process ended; the end of its stderr explains why.
    Exit(String),
}

pub struct Process {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Process {
    /// Starts `program`; every output goes to `emit`, which runs on reader threads.
    pub fn spawn<F>(program: &Path, args: &[String], cwd: &Path, emit: F) -> std::io::Result<Process>
    where
        F: Fn(Output) + Send + Sync + 'static,
    {
        let mut cmd = Command::new(program);
        cmd.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Started from inside a Claude Code session, the child would think it is
        // nested; this is a separate session.
        cmd.env_remove("CLAUDECODE").env_remove("CLAUDE_CODE_ENTRYPOINT");
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let stdin = child.stdin.take();

        let emit = Arc::new(emit);
        let tail = Arc::new(Mutex::new(String::new()));
        let err_tail = tail.clone();
        let err_thread = thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut reader = stderr;
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut t = err_tail.lock().unwrap();
                t.push_str(&String::from_utf8_lossy(&buf[..n]));
                if t.len() > 4000 {
                    let cut = t.len() - 2000;
                    let cut = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(0);
                    *t = t[cut..].to_string();
                }
            }
        });
        let out_emit = emit.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let line = line.trim().to_string();
                if !line.is_empty() {
                    out_emit(Output::Line(line));
                }
            }
            let _ = err_thread.join();
            let tail = tail.lock().unwrap().clone();
            out_emit(Output::Exit(tail));
        });
        Ok(Process { child, stdin })
    }

    pub fn write_line(&mut self, line: &str) {
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = stdin.write_all(line.as_bytes());
            let _ = stdin.write_all(b"\n");
            let _ = stdin.flush();
        }
    }

    /// Closes stdin, asks it to stop, and kills it if it does not.
    pub fn stop(mut self) {
        self.stdin = None;
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }
        for _ in 0..15 {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn exit_code(&mut self) -> Option<i32> {
        self.child.try_wait().ok().flatten().and_then(|s| s.code())
    }
}
