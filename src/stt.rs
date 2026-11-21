//! Push-to-talk through a dictation app that already has its model loaded.
//!
//! Superwhisper: omanta records with pw-record, `superwhisper transcribe FILE`
//! hands the file to the running service (no paste), and `superwhisper read`
//! returns the text.
//!
//! Voxtype: `voxtype record start --file=PATH` makes the running daemon record,
//! and `voxtype record stop` writes the text to PATH instead of typing it.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::paths::{self, find_program};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Recording,
    Transcribing,
}

/// What the Voxtype daemon does now, from its state file. This sees every
/// recording: omanta's talk key, Omarchy's push-to-talk, and any other.
pub fn voxtype_state() -> State {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    parse_voxtype_state(&std::fs::read_to_string(dir.join("voxtype/state")).unwrap_or_default())
}

fn parse_voxtype_state(s: &str) -> State {
    match s.trim() {
        "recording" | "streaming" => State::Recording,
        "transcribing" | "processing" => State::Transcribing,
        _ => State::Idle,
    }
}

pub enum Result {
    Text(String),
    Nothing,
    Problem(String),
}

pub struct Stt {
    engine: String,
    state: State,
    recorder: Option<Child>,
    dir: PathBuf,
}

const TIMEOUT: Duration = Duration::from_secs(60);

fn run(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then_some(out.stdout)
}

impl Stt {
    pub fn new() -> Stt {
        let dir = paths::runtime_dir().join("stt");
        let _ = std::fs::create_dir_all(&dir);
        Stt { engine: "voxtype".into(), state: State::Idle, recorder: None, dir }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Starts recording with `engine` ("superwhisper" or "voxtype").
    /// `overlay`: Voxtype shows its own recording overlay.
    pub fn start(&mut self, engine: &str, overlay: bool) -> std::result::Result<(), String> {
        if self.state != State::Idle {
            return Ok(());
        }
        self.engine = engine.to_string();
        if self.engine == "voxtype" {
            if find_program("voxtype").is_none() {
                return Err("Voxtype is not installed.".into());
            }
            let file = self.dir.join("voxtype.txt");
            let _ = std::fs::remove_file(&file);
            let arg = format!("--file={}", file.display());
            let mut args = vec!["record", "start", arg.as_str()];
            if !overlay {
                args.push("--no-osd");
            }
            if run("voxtype", &args).is_none() {
                return Err("Voxtype did not start recording. Is its service running? systemctl --user start voxtype".into());
            }
        } else {
            if find_program("superwhisper").is_none() {
                return Err("Superwhisper is not installed.".into());
            }
            if find_program("pw-record").is_none() {
                return Err("pw-record is missing. Install pipewire.".into());
            }
            let wav = self.dir.join("talk.wav");
            let _ = std::fs::remove_file(&wav);
            // 16 kHz mono 16-bit, what speech models read.
            let child = Command::new("pw-record")
                .args(["--rate", "16000", "--channels", "1", "--format", "s16"])
                .arg(&wav)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("Could not start the microphone recording: {e}"))?;
            self.recorder = Some(child);
        }
        self.state = State::Recording;
        Ok(())
    }

    /// Stops recording. The returned job does the slow part (transcription)
    /// and is meant to run on another thread.
    pub fn stop(&mut self) -> Option<Box<dyn FnOnce() -> Result + Send>> {
        if self.state != State::Recording {
            return None;
        }
        self.state = State::Transcribing;
        let dir = self.dir.clone();
        if self.engine == "voxtype" {
            return Some(Box::new(move || {
                if run("voxtype", &["record", "stop"]).is_none() {
                    return Result::Problem("Voxtype did not stop recording.".into());
                }
                let file = dir.join("voxtype.txt");
                let start = Instant::now();
                // The daemon writes the file once, when the text is ready; silence writes none.
                while start.elapsed() < TIMEOUT / 2 {
                    if let Ok(text) = std::fs::read_to_string(&file) {
                        if !text.trim().is_empty() {
                            let _ = std::fs::remove_file(&file);
                            return Result::Text(text.trim().to_string());
                        }
                    }
                    sleep(Duration::from_millis(120));
                }
                Result::Nothing
            }));
        }
        let mut recorder = self.recorder.take();
        Some(Box::new(move || {
            if let Some(c) = recorder.as_mut() {
                // SIGINT lets pw-record finish the WAV header before it exits.
                unsafe {
                    libc::kill(c.id() as i32, libc::SIGINT);
                }
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(2) {
                    if matches!(c.try_wait(), Ok(Some(_))) {
                        break;
                    }
                    sleep(Duration::from_millis(20));
                }
                let _ = c.kill();
                let _ = c.wait();
            }
            let wav = dir.join("talk.wav");
            // Under 0.1 s of audio: nothing was said.
            if std::fs::metadata(&wav).map(|m| m.len()).unwrap_or(0) < 3200 {
                return Result::Nothing;
            }
            let Some(out) = run("superwhisper", &["transcribe", &wav.to_string_lossy()]) else {
                return Result::Problem("Superwhisper did not take the recording. Is its service running?".into());
            };
            let Some(session) = serde_json::from_slice::<Value>(&out).ok().and_then(|v| v["session_id"].as_i64()) else {
                return Result::Problem("Superwhisper gave no session for the recording.".into());
            };
            let start = Instant::now();
            while start.elapsed() < TIMEOUT {
                if let Some(last) = run("superwhisper", &["read"]).and_then(|o| serde_json::from_slice::<Value>(&o).ok()) {
                    if last["session_id"].as_i64() == Some(session) {
                        let text = last["text"].as_str().unwrap_or("").trim().to_string();
                        return if text.is_empty() { Result::Nothing } else { Result::Text(text) };
                    }
                }
                sleep(Duration::from_millis(150));
            }
            Result::Problem("Superwhisper did not return any text.".into())
        }))
    }

    /// The transcription job finished.
    pub fn finished(&mut self) {
        self.state = State::Idle;
    }

    pub fn cancel(&mut self) {
        if self.state == State::Recording {
            if self.engine == "voxtype" {
                let _ = run("voxtype", &["record", "cancel"]);
            } else if let Some(mut c) = self.recorder.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
            self.state = State::Idle;
        }
    }
}

impl Drop for Stt {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_voxtype_states() {
        assert_eq!(parse_voxtype_state("recording\n"), State::Recording);
        assert_eq!(parse_voxtype_state("streaming"), State::Recording);
        assert_eq!(parse_voxtype_state("transcribing"), State::Transcribing);
        assert_eq!(parse_voxtype_state("idle"), State::Idle);
        assert_eq!(parse_voxtype_state(""), State::Idle);
    }
}
