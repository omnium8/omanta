//! The user's choices, in ~/.config/omanta/config.toml. Saved atomically.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub chat: Chat,
    pub voice: Voice,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Chat {
    /// An agent id from `agent::catalog()`.
    pub agent: String,
    /// Where the agent works. Empty: the home folder.
    pub workdir: String,
    /// "key": the talk key records and sends. "dictation": your own dictation
    /// tool types into the chat line.
    pub input: String,
    /// "voxtype" or "superwhisper", for the talk key.
    pub stt: String,
    /// The key that starts and stops talking, like "alt+space" or "f2".
    pub talk_key: String,
    /// Send dictated text after this many seconds without new text: 0 (off),
    /// 1 ("instant": as soon as the dictation tool stops typing), or 2 to 5.
    /// Typed text always waits for Enter.
    pub send_after: u32,
    /// Read replies aloud.
    pub speak_replies: bool,
    /// Rows of the recording waveform, 1 to 4. The message line has this
    /// height too, so the screen does not jump when you talk.
    pub wave_rows: u32,
    /// How much of the wallpaper shows through the background: 0 (off: the
    /// terminal's own setting), or 0.1 to 0.9. Needs foot.
    pub transparency: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Voice {
    /// A Kokoro voice, like "af_aoede".
    pub name: String,
    pub speed: f32,
    pub volume: f32,
    pub language: String,
    /// Pause other media (MPRIS players) while speaking.
    pub pause_media: bool,
    /// The port of the Kokoro sidecar on 127.0.0.1.
    pub sidecar_port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Config { chat: Chat::default(), voice: Voice::default() }
    }
}

impl Default for Chat {
    fn default() -> Self {
        Chat {
            agent: "claude".into(),
            workdir: String::new(),
            input: "key".into(),
            stt: "voxtype".into(),
            talk_key: "alt+space".into(),
            send_after: 1,
            speak_replies: true,
            wave_rows: 1,
            transparency: 0.4,
        }
    }
}

impl Default for Voice {
    fn default() -> Self {
        Voice {
            name: "af_aoede".into(),
            speed: 1.1,
            volume: 1.2,
            language: "en".into(),
            pause_media: true,
            sidecar_port: 8765,
        }
    }
}

impl Config {
    pub fn load() -> Config {
        Self::load_from(&paths::config_file())
    }

    pub fn load_from(path: &Path) -> Config {
        let mut c: Config = fs::read_to_string(path).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default();
        c.sanitize();
        c
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&paths::config_file())
    }

    /// Writes beside the target and renames, so a crash never leaves half a file.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, path)
    }

    fn sanitize(&mut self) {
        if !matches!(self.chat.input.as_str(), "key" | "dictation") {
            self.chat.input = "key".into();
        }
        if !matches!(self.chat.stt.as_str(), "superwhisper" | "voxtype") {
            self.chat.stt = "voxtype".into();
        }
        if self.chat.talk_key.trim().is_empty() {
            self.chat.talk_key = "alt+space".into();
        }
        self.chat.send_after = match self.chat.send_after {
            0 | 1 => self.chat.send_after,
            n => n.clamp(2, 5),
        };
        self.chat.wave_rows = self.chat.wave_rows.clamp(1, 4);
        self.chat.transparency = (self.chat.transparency.clamp(0.0, 0.9) * 10.0).round() / 10.0;
        self.voice.speed = self.voice.speed.clamp(0.5, 2.0);
        self.voice.volume = self.voice.volume.clamp(0.0, 2.0);
    }

    pub fn workdir(&self) -> PathBuf {
        let w = PathBuf::from(&self.chat.workdir);
        if !self.chat.workdir.is_empty() && w.is_dir() {
            w
        } else {
            std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
        }
    }
}

/// Re-reads the config when the file changes, for the long-running daemon.
pub struct Watched {
    path: PathBuf,
    stamp: Option<SystemTime>,
    pub config: Config,
}

impl Watched {
    pub fn new() -> Self {
        let path = paths::config_file();
        let stamp = fs::metadata(&path).and_then(|m| m.modified()).ok();
        Watched { config: Config::load_from(&path), path, stamp }
    }

    pub fn refresh(&mut self) -> &Config {
        let stamp = fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if stamp != self.stamp {
            self.stamp = stamp;
            self.config = Config::load_from(&self.path);
        }
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_sanitizes() {
        let dir = std::env::temp_dir().join(format!("omanta-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let mut c = Config::default();
        c.chat.agent = "opencode".into();
        c.chat.stt = "superwhisper".into();
        c.voice.speed = 9.0;
        c.save_to(&path).unwrap();
        let back = Config::load_from(&path);
        assert_eq!(back.chat.agent, "opencode");
        assert_eq!(back.chat.stt, "superwhisper");
        assert_eq!(back.voice.speed, 2.0);

        fs::write(&path, "[chat]\nstt = \"nonsense\"\n").unwrap();
        let partial = Config::load_from(&path);
        assert_eq!(partial.chat.stt, "voxtype");
        assert_eq!(partial.voice.name, "af_aoede");
        let _ = fs::remove_dir_all(dir);
    }
}
