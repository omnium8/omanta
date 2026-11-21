//! One JSON line in, one JSON line out, over a Unix socket in
//! $XDG_RUNTIME_DIR/omanta. The daemon and the chat each listen on one.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What the daemon does on request.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Speak `text`. `queue`: after what is queued; otherwise stop that first.
    /// `id`: a number that grows with each sentence; `Status::heard` reports
    /// the last one whose audio has started, so the chat can show each
    /// sentence as it is heard.
    Say { text: String, #[serde(default)] queue: bool, #[serde(default)] id: Option<u64> },
    /// Stop talking and drop the queue.
    Stop,
    Status,
    /// Read the selected text aloud; with `toggle`, stop instead when talking.
    SpeakSelection { #[serde(default)] toggle: bool },
    Voices,
    /// For the chat: `talk`, `stop`, or `open`.
    Chat { action: String },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Status {
    /// Talking now, or about to (synthesis in flight, sentences queued).
    pub speaking: bool,
    pub queued: usize,
    /// The voice model is loaded and warm.
    pub ready: bool,
    /// What the voice is doing when it is not ready: "installing", "loading", ...
    pub phase: String,
    /// Why the voice cannot speak, for the user; empty when it can.
    pub problem: String,
    /// The id of the last sentence whose audio has started playing.
    #[serde(default)]
    pub heard: u64,
    /// How far the text that plays now is, 0 to 1.
    #[serde(default)]
    pub progress: f64,
}

/// Sends one request and returns the reply.
pub fn call(socket: &Path, request: &Request, timeout: Duration) -> Result<Value> {
    let mut stream = UnixStream::connect(socket).with_context(|| format!("connect {}", socket.display()))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut line = serde_json::to_vec(request)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    let value: Value = serde_json::from_str(reply.trim()).context("reply is not JSON")?;
    if value.get("ok") == Some(&Value::Bool(false)) {
        anyhow::bail!("{}", value.get("error").and_then(Value::as_str).unwrap_or("failed"));
    }
    Ok(value)
}

/// Binds the socket, replacing a stale one. Fails when another live process
/// answers on it, so only one daemon (or chat) runs.
pub fn bind(socket: &Path) -> Result<UnixListener> {
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    if socket.exists() {
        if UnixStream::connect(socket).is_ok() {
            anyhow::bail!("already running ({})", socket.display());
        }
        std::fs::remove_file(socket)?;
    }
    Ok(UnixListener::bind(socket)?)
}

/// Serves requests on `listener`, one thread per connection.
pub fn serve<F>(listener: UnixListener, handler: F)
where
    F: Fn(Request) -> Value + Send + Sync + 'static,
{
    let handler = std::sync::Arc::new(handler);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let handler = handler.clone();
        std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let reply = match serde_json::from_str::<Request>(line.trim()) {
                Ok(req) => handler(req),
                Err(e) => serde_json::json!({"ok": false, "error": format!("bad request: {e}")}),
            };
            let mut out = serde_json::to_vec(&reply).unwrap_or_default();
            out.push(b'\n');
            let _ = (&stream).write_all(&out);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        let r: Request = serde_json::from_str(r#"{"cmd":"say","text":"Hi."}"#).unwrap();
        assert_eq!(r, Request::Say { text: "Hi.".into(), queue: false, id: None });
        let s = serde_json::to_string(&Request::SpeakSelection { toggle: true }).unwrap();
        assert_eq!(s, r#"{"cmd":"speak_selection","toggle":true}"#);
    }

    #[test]
    fn serves_over_a_socket() {
        let dir = std::env::temp_dir().join(format!("omanta-ipc-{}", std::process::id()));
        let sock = dir.join("t.sock");
        let listener = bind(&sock).unwrap();
        std::thread::spawn(move || {
            serve(listener, |req| match req {
                Request::Status => serde_json::json!({"ok": true, "speaking": false}),
                _ => serde_json::json!({"ok": false, "error": "nope"}),
            })
        });
        let v = call(&sock, &Request::Status, Duration::from_secs(2)).unwrap();
        assert_eq!(v["speaking"], false);
        assert!(call(&sock, &Request::Stop, Duration::from_secs(2)).is_err());
        assert!(bind(&sock).is_err(), "a live socket must not be replaced");
        let _ = std::fs::remove_dir_all(dir);
    }
}
