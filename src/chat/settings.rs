//! The settings screen: a list of rows. ←/→ change a value, enter edits a text
//! or runs an action, esc closes. Each change is saved at once.

use crossterm::event::{KeyCode, KeyEvent};

use super::input::{KeySpec, LineInput};
use crate::agent::AgentInfo;
use crate::config::Config;

pub enum Action {
    Close,
    Saved { agent_changed: bool },
    AllowAll(bool),
    Login,
    StartVoice,
    TestVoice,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Agent,
    Workdir,
    Input,
    Stt,
    TalkKey,
    SendAfter,
    WaveRows,
    Transparency,
    SpeakReplies,
    Voice,
    Speed,
    Volume,
    PauseMedia,
    AllowAll,
    Login,
    StartVoice,
    TestVoice,
}

pub const ROWS: [Row; 17] = [
    Row::Agent,
    Row::Workdir,
    Row::Input,
    Row::Stt,
    Row::TalkKey,
    Row::SendAfter,
    Row::WaveRows,
    Row::Transparency,
    Row::SpeakReplies,
    Row::Voice,
    Row::Speed,
    Row::Volume,
    Row::PauseMedia,
    Row::AllowAll,
    Row::Login,
    Row::StartVoice,
    Row::TestVoice,
];

/// Kokoro voices, when the daemon has not listed its own yet.
const VOICES: [&str; 12] = [
    "af_aoede", "af_heart", "af_bella", "af_nicole", "af_sky", "am_adam", "am_michael", "am_puck", "bf_emma",
    "bf_isabella", "bm_george", "bm_lewis",
];

pub struct SettingsView {
    pub selected: usize,
    /// The text being edited, for the working folder and the talk key.
    pub editing: Option<LineInput>,
    pub voices: Vec<String>,
    pub allow_all: bool,
    pub error: String,
}

impl SettingsView {
    pub fn new(_config: &Config) -> SettingsView {
        SettingsView {
            selected: 0,
            editing: None,
            voices: VOICES.iter().map(|s| s.to_string()).collect(),
            allow_all: false,
            error: String::new(),
        }
    }

    pub fn set_voices(&mut self, voices: Vec<String>) {
        if !voices.is_empty() {
            self.voices = voices;
        }
    }

    pub fn paste(&mut self, s: &str) {
        if let Some(e) = self.editing.as_mut() {
            e.insert_str(s.trim());
        }
    }

    pub fn row(&self) -> Row {
        ROWS[self.selected]
    }

    /// The label, value and hint of each row, for drawing.
    pub fn describe(&self, row: Row, c: &Config, catalog: &[AgentInfo], allow_all: bool) -> (&'static str, String, &'static str) {
        let on = |b: bool| if b { "on" } else { "off" }.to_string();
        match row {
            Row::Agent => {
                let a = catalog.iter().find(|a| a.id == c.chat.agent);
                let name = a.map(|a| a.name).unwrap_or("?");
                let v = if a.is_some_and(|a| a.installed) { name.to_string() } else { format!("{name} (not installed)") };
                ("Agent", v, "The agent must be logged in: use \"Log in\" below once.")
            }
            Row::Workdir => {
                let v = if c.chat.workdir.is_empty() { "~ (home)".into() } else { c.chat.workdir.clone() };
                ("Working folder", v, "Where the agent reads and changes files. Takes effect with the next conversation.")
            }
            Row::Input => {
                let v = if c.chat.input == "key" { "talk key".into() } else { "your own dictation".into() };
                ("Speech input", v, "Talk key: omanta records and sends. Your own dictation: it types into the message line.")
            }
            Row::Stt => {
                let v = if c.chat.stt == "voxtype" { "Voxtype".into() } else { "Superwhisper".into() };
                ("Speech to text", v, "Its service must run with the model loaded. Its own shortcut stays as it is.")
            }
            Row::TalkKey => ("Talk key", c.chat.talk_key.clone(), "Works while omanta has focus. For any window: `omanta talk` in a Hyprland bind."),
            Row::SendAfter => {
                let v = match c.chat.send_after {
                    0 => "off (press enter)".into(),
                    1 => "instant".into(),
                    n => format!("{n} s"),
                };
                ("Send dictation", v, "Instant: as soon as you release push-to-talk. Text you type waits for enter.")
            }
            Row::WaveRows => {
                let n = c.chat.wave_rows;
                ("Waveform height", format!("{n} row{}", if n == 1 { "" } else { "s" }), "The message line has this height too.")
            }
            Row::Transparency => {
                let t = c.chat.transparency;
                let v = if t <= 0.0 { "off".to_string() } else { format!("{t:.1}") };
                ("Background transparency", v, "How much wallpaper shows through. Needs foot, Omarchy's terminal.")
            }
            Row::SpeakReplies => ("Read replies aloud", on(c.chat.speak_replies), ""),
            Row::Voice => ("Voice", c.voice.name.clone(), "a = American, b = British; f = female, m = male."),
            Row::Speed => ("Speed", format!("{:.2}", c.voice.speed), ""),
            Row::Volume => ("Volume", format!("{:.1}", c.voice.volume), ""),
            Row::PauseMedia => ("Pause other media", on(c.voice.pause_media), "Pause music and videos while omanta speaks."),
            Row::AllowAll => ("Auto mode", on(allow_all), "Allow every action without asking, for this run. tab turns it on and off."),
            Row::Login => ("Log in", "open the agent in a terminal".into(), ""),
            Row::StartVoice => ("Voice service", "start".into(), "Starts the voice now and at every login."),
            Row::TestVoice => ("Test the voice", "speak a sample".into(), ""),
        }
    }

    /// Handles a key; returns what the chat must do.
    pub fn key(&mut self, k: &KeyEvent, c: &mut Config, catalog: &[AgentInfo]) -> Option<Action> {
        if let Some(edit) = self.editing.as_mut() {
            match k.code {
                KeyCode::Esc => self.editing = None,
                KeyCode::Enter => {
                    let text = edit.text().trim().to_string();
                    self.editing = None;
                    match self.row() {
                        Row::Workdir => {
                            let expanded = expand_home(&text);
                            if !text.is_empty() && !std::path::Path::new(&expanded).is_dir() {
                                self.error = format!("{text} is not a folder.");
                                return None;
                            }
                            c.chat.workdir = expanded;
                        }
                        Row::TalkKey => {
                            if KeySpec::parse(&text).is_none() {
                                self.error = format!("\"{text}\" is not a key. Examples: alt+space, ctrl+space, f2.");
                                return None;
                            }
                            c.chat.talk_key = text.to_lowercase();
                        }
                        _ => {}
                    }
                    self.error.clear();
                    return Some(Action::Saved { agent_changed: false });
                }
                _ => {
                    edit.key(k);
                }
            }
            return None;
        }
        self.error.clear();
        let step: i32 = match k.code {
            KeyCode::Esc | KeyCode::Char('q') => return Some(Action::Close),
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = (self.selected + ROWS.len() - 1) % ROWS.len();
                return None;
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.selected = (self.selected + 1) % ROWS.len();
                return None;
            }
            KeyCode::Left | KeyCode::Char('h') => -1,
            KeyCode::Right | KeyCode::Char('l') => 1,
            KeyCode::Enter | KeyCode::Char(' ') => 0,
            _ => return None,
        };
        let cycle = |list: &[String], current: &str, step: i32| -> String {
            let i = list.iter().position(|x| x == current).unwrap_or(0) as i32;
            let n = list.len() as i32;
            list[((i + if step == 0 { 1 } else { step }) % n + n) as usize % n as usize].clone()
        };
        match self.row() {
            Row::Agent => {
                let ids: Vec<String> = catalog.iter().filter(|a| a.installed || a.id == c.chat.agent).map(|a| a.id.to_string()).collect();
                if ids.is_empty() {
                    return None;
                }
                let next = cycle(&ids, &c.chat.agent, step);
                let changed = next != c.chat.agent;
                c.chat.agent = next;
                return Some(Action::Saved { agent_changed: changed });
            }
            Row::Workdir | Row::TalkKey if step == 0 => {
                let mut e = LineInput::default();
                e.set(if self.row() == Row::Workdir { &c.chat.workdir } else { &c.chat.talk_key });
                self.editing = Some(e);
                return None;
            }
            Row::Workdir | Row::TalkKey => return None,
            Row::Input => c.chat.input = if c.chat.input == "key" { "dictation".into() } else { "key".into() },
            Row::Stt => c.chat.stt = if c.chat.stt == "voxtype" { "superwhisper".into() } else { "voxtype".into() },
            Row::SendAfter => {
                const STEPS: [u32; 6] = [0, 1, 2, 3, 4, 5];
                let i = STEPS.iter().position(|v| *v == c.chat.send_after).unwrap_or(1) as i32;
                let d = if step == 0 { 1 } else { step };
                c.chat.send_after = STEPS[(i + d).rem_euclid(STEPS.len() as i32) as usize];
            }
            Row::WaveRows => {
                let d = if step == 0 { 1 } else { step };
                c.chat.wave_rows = ((c.chat.wave_rows as i32 - 1 + d).rem_euclid(4) + 1) as u32;
            }
            Row::Transparency => {
                let d = if step == 0 { 1 } else { step };
                let tenth = ((c.chat.transparency * 10.0).round() as i32 + d).rem_euclid(10);
                c.chat.transparency = tenth as f32 / 10.0;
            }
            Row::SpeakReplies => c.chat.speak_replies = !c.chat.speak_replies,
            Row::Voice => c.voice.name = cycle(&self.voices, &c.voice.name, step),
            Row::Speed => {
                let d = if step == 0 { 1 } else { step };
                c.voice.speed = ((c.voice.speed + 0.05 * d as f32) * 100.0).round() / 100.0;
                c.voice.speed = c.voice.speed.clamp(0.5, 2.0);
            }
            Row::Volume => {
                let d = if step == 0 { 1 } else { step };
                c.voice.volume = ((c.voice.volume + 0.1 * d as f32) * 10.0).round() / 10.0;
                c.voice.volume = c.voice.volume.clamp(0.0, 2.0);
            }
            Row::PauseMedia => c.voice.pause_media = !c.voice.pause_media,
            Row::AllowAll => {
                self.allow_all = !self.allow_all;
                return Some(Action::AllowAll(self.allow_all));
            }
            Row::Login => return (step == 0).then_some(Action::Login),
            Row::StartVoice => return (step == 0).then_some(Action::StartVoice),
            Row::TestVoice => return (step == 0).then_some(Action::TestVoice),
        }
        Some(Action::Saved { agent_changed: false })
    }
}

fn expand_home(p: &str) -> String {
    match p.strip_prefix("~") {
        Some(rest) => format!("{}{rest}", std::env::var("HOME").unwrap_or_default()),
        None => p.to_string(),
    }
}
