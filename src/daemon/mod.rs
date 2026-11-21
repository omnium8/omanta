//! `omanta daemon`: the user service that keeps the voice warm and speaks.

mod audio;
mod media;
pub mod selection;
pub mod sidecar;
mod speaker;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::json;

use crate::config::Watched;
use crate::ipc::{self, Request, Status};
use crate::paths;
use crate::speech;

pub fn run() -> Result<()> {
    let socket = paths::daemon_socket();
    let listener = ipc::bind(&socket)?;
    let config = Watched::new();
    let port = config.config.voice.sidecar_port;

    let health: sidecar::Shared = Arc::new(Mutex::new(sidecar::Health { phase: "starting".into(), ..Default::default() }));
    let child = Arc::new(Mutex::new(None));
    let speaker = speaker::Speaker::new(audio::Audio::new(config.config.voice.volume));

    // Stop the voice and remove the socket on SIGTERM (systemctl stop) or Ctrl+C.
    {
        let child = child.clone();
        let socket = socket.clone();
        let mut signals = signal_hook::iterator::Signals::new([signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT])?;
        std::thread::spawn(move || {
            if signals.forever().next().is_some() {
                sidecar::kill(&child);
                let _ = std::fs::remove_file(&socket);
                std::process::exit(0);
            }
        });
    }
    {
        let health = health.clone();
        let child = child.clone();
        std::thread::spawn(move || {
            sidecar::supervise(health, child, port, || {
                let c = crate::config::Config::load();
                (c.voice.name, c.voice.language)
            })
        });
    }
    {
        let speaker = speaker.clone();
        let health = health.clone();
        std::thread::spawn(move || speaker.run(health, config));
    }

    eprintln!("omanta: daemon listening on {}", socket.display());
    let health_for_status = health.clone();
    ipc::serve(listener, move |req| match req {
        Request::Say { text, queue, id } => {
            let spoken = speech::speakable(&text);
            let said = if spoken.is_empty() { text.trim().to_string() } else { spoken };
            if said.is_empty() {
                return json!({"ok": false, "error": "text is empty"});
            }
            json!({"ok": true, "queued": speaker.say(&said, queue, id)})
        }
        Request::Stop => {
            speaker.stop();
            json!({"ok": true})
        }
        Request::Status => {
            let h = health_for_status.lock().unwrap();
            let problem = if h.problem.is_empty() { speaker.problem() } else { h.problem.clone() };
            let s = Status {
                speaking: speaker.speaking(),
                queued: speaker.queued(),
                ready: h.ready,
                phase: h.phase.clone(),
                problem,
                heard: speaker.heard(),
                progress: speaker.progress(),
            };
            let mut v = serde_json::to_value(s).unwrap_or_default();
            v["ok"] = json!(true);
            v
        }
        Request::SpeakSelection { toggle } => {
            if toggle && speaker.speaking() {
                speaker.stop();
                return json!({"ok": true, "stopped": true});
            }
            match selection::selected_text() {
                Some(text) => {
                    speaker.stop();
                    // Paragraph by paragraph, so a stop is quick and the first words come soon.
                    for part in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
                        speaker.say(part, true, None);
                    }
                    json!({"ok": true})
                }
                None => json!({"ok": false, "error": "no text is selected"}),
            }
        }
        Request::Voices => json!({"ok": true, "voices": sidecar::voices(port)}),
        Request::Chat { .. } => json!({"ok": false, "error": "this is the daemon, not the chat"}),
    });
    Ok(())
}
