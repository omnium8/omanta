//! The chat TUI: one spoken conversation with one agent.

mod input;
mod levels;
mod settings;
mod ui;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::{execute, terminal};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use serde_json::json;

use crate::agent::process::{Output, Process};
use crate::agent::{self, Act, AgentInfo, Event, Link};
use crate::config::Config;
use crate::ipc::{self, Request, Status};
use crate::paths;
use crate::speech::{self, Answer, Splitter};
use crate::stt::{self, Stt};
use crate::theme::Theme;
use input::{Dictation, KeySpec, LineInput};
use settings::SettingsView;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    User,
    Agent,
    Step,
    Note,
}

pub struct Message {
    pub kind: Kind,
    pub text: String,
    /// For a reply read aloud: how much of `text` is heard so far, so the
    /// text appears with the voice. None shows all of it.
    pub shown: Option<usize>,
}

pub struct Ask {
    id: String,
    pub act: Act,
    pub detail: String,
    key: String,
}

pub struct Question {
    id: String,
    pub text: String,
    pub choices: Vec<String>,
}

/// What the voice service says about itself.
#[derive(Default, Clone)]
pub struct Voice {
    pub reachable: bool,
    pub status: Status,
}

enum Msg {
    Key(KeyEvent),
    Paste(String),
    Redraw,
    Agent(u64, Output),
    Heard(stt::Result),
    Voice(Voice),
    External(String),
    Tick,
    /// Voxtype started or stopped a recording or a transcription.
    Voxtype(stt::State),
    /// Draw the next animation frame.
    Frame,
    /// The terminal window got (true) or lost (false) the keyboard focus.
    Focus(bool),
}

enum VoiceCmd {
    /// A sentence, and its id when the chat waits to show it until it is heard.
    Say(String, Option<u64>),
    Stop,
}

pub enum Overlay {
    None,
    Help,
    Settings(SettingsView),
}

pub struct App {
    pub config: Config,
    pub catalog: Vec<AgentInfo>,
    pub agent: AgentInfo,
    link: Option<Box<dyn Link>>,
    process: Option<Process>,
    generation: u64,
    session: Option<String>,
    sessions: HashMap<String, String>,
    pub messages: Vec<Message>,
    reply_open: bool,
    splitter: Splitter,
    /// Pieces of the open reply, spoken together when it ends.
    unsaid: Vec<(String, usize)>,
    always: HashSet<String>,
    pub allow_all: bool,
    pub ask: Option<Ask>,
    pub question: Option<Question>,
    pub busy: bool,
    starting: bool,
    pub working: bool,
    pub needs_login: bool,
    pending_prompt: Option<String>,
    turn_start: Option<Instant>,
    last_output: Instant,
    warned_quiet: bool,
    pub stt: Stt,
    /// What Voxtype does, whoever started it.
    pub voxtype: stt::State,
    /// The window had the focus when the recording started: omanta shows
    /// it. Otherwise Voxtype's overlay shows it, and omanta does not.
    recording_here: bool,
    /// The terminal window has the keyboard focus.
    focused: bool,
    /// Omarchy's dictation keys are there: hold Right Alt to talk.
    pub push_to_talk: bool,
    /// The background is see-through now: give it back on exit.
    see_through_set: bool,
    /// How far the wheel has walked along the status line while the voice
    /// speaks, 0 to 1: the voice's progress, smoothed.
    pub walk: f64,
    /// The agent's model and effort, shown before the voice status.
    pub model: String,
    /// When the chat opened: the clock for the animations.
    pub born: Instant,
    /// Something on the screen moves: draw frames faster.
    animate: Arc<AtomicBool>,
    /// The live microphone level while Voxtype records.
    pub levels: levels::Levels,
    pub voice: Voice,
    voice_tx: Sender<VoiceCmd>,
    pub input: LineInput,
    last_typed: Option<Instant>,
    /// Whether the text in the message line came from dictation.
    dictation: Dictation,
    /// Sentences sent to the voice and not heard yet: (id, message, offset
    /// in its text where the sentence ends).
    reveals: std::collections::VecDeque<(u64, usize, usize)>,
    next_say_id: u64,
    /// When the reveals last moved, to give up waiting on a voice that stalls.
    reveal_moved: Instant,
    pub scroll: usize,
    pub overlay: Overlay,
    pub theme: Theme,
    pub talk_key: KeySpec,
    pub tick: u64,
    quit: bool,
    tx: Sender<Msg>,
}

fn sessions_file() -> PathBuf {
    paths::state_dir().join("sessions.json")
}

pub fn run(talk_now: bool) -> Result<()> {
    let socket = paths::chat_socket();
    let listener = match ipc::bind(&socket) {
        Ok(l) => l,
        Err(_) => {
            // One chat at a time: hand over to the one that runs.
            let action = if talk_now { "talk" } else { "open" };
            let _ = ipc::call(&socket, &Request::Chat { action: action.into() }, Duration::from_secs(1));
            anyhow::bail!("omanta is already open in another terminal");
        }
    };

    let (tx, rx) = channel::<Msg>();
    let (voice_tx, voice_rx) = channel::<VoiceCmd>();
    spawn_voice_thread(voice_rx, tx.clone());
    {
        let tx = tx.clone();
        thread::spawn(move || {
            ipc::serve(listener, move |req| match req {
                Request::Chat { action } => {
                    let _ = tx.send(Msg::External(action));
                    json!({"ok": true})
                }
                _ => json!({"ok": false, "error": "this is the chat, not the daemon"}),
            })
        });
    }
    {
        let tx = tx.clone();
        thread::spawn(move || loop {
            match event::read() {
                Ok(TermEvent::Key(k)) if k.kind != KeyEventKind::Release => {
                    if tx.send(Msg::Key(k)).is_err() {
                        break;
                    }
                }
                Ok(TermEvent::Paste(s)) => {
                    let _ = tx.send(Msg::Paste(s));
                }
                Ok(TermEvent::Resize(..)) => {
                    let _ = tx.send(Msg::Redraw);
                }
                Ok(TermEvent::FocusGained) => {
                    let _ = tx.send(Msg::Focus(true));
                }
                Ok(TermEvent::FocusLost) => {
                    let _ = tx.send(Msg::Focus(false));
                }
                Ok(_) => {}
                Err(_) => break,
            }
        });
    }
    {
        let tx = tx.clone();
        thread::spawn(move || {
            while tx.send(Msg::Tick).is_ok() {
                thread::sleep(Duration::from_millis(250));
            }
        });
    }
    // Watches Voxtype's state file, and draws about 15 frames a second while
    // something moves: Voxtype records or transcribes, or the wheel turns.
    // Idle, it only reads a small file.
    let animate = Arc::new(AtomicBool::new(false));
    {
        let tx = tx.clone();
        let animate = animate.clone();
        thread::spawn(move || {
            let mut last = stt::State::Idle;
            loop {
                let now = stt::voxtype_state();
                if now != last && tx.send(Msg::Voxtype(now)).is_err() {
                    break;
                }
                last = now;
                let moving = now != stt::State::Idle || animate.load(Ordering::Relaxed);
                if moving && tx.send(Msg::Frame).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(if moving { 66 } else { 100 }));
            }
        });
    }

    // The first run after an install: start the voice service, so the chat
    // needs no setup steps. The daemon downloads the voice model itself.
    thread::spawn(|| {
        if ipc::call(&paths::daemon_socket(), &Request::Status, Duration::from_secs(1)).is_err() {
            start_voice_service();
        }
    });

    let config = Config::load();
    let mut app = App::new(config, voice_tx, tx, animate);
    if talk_now {
        app.talk();
    }

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    // The title lets `omanta dictate` find this window in any terminal.
    execute!(stdout, terminal::SetTitle("omanta"), terminal::EnterAlternateScreen, EnableBracketedPaste, EnableFocusChange)?;
    // Tells alt+space and esc apart cleanly where the terminal supports it.
    let enhanced = terminal::supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        execute!(stdout, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
    }
    let mut term = Terminal::new(CrosstermBackend::new(stdout))?;
    app.see_through();
    let result = app.event_loop(&mut term, rx);
    app.restore_background();

    let mut stdout = io::stdout();
    if enhanced {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout, DisableBracketedPaste, DisableFocusChange, terminal::LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
    app.shutdown();
    let _ = std::fs::remove_file(&socket);
    result
}

/// Talks to the voice service off the UI thread: sends what to say and polls
/// its status, so a slow or missing daemon never stalls typing.
fn spawn_voice_thread(rx: Receiver<VoiceCmd>, tx: Sender<Msg>) {
    thread::spawn(move || {
        let socket = paths::daemon_socket();
        let call = |r: &Request| ipc::call(&socket, r, Duration::from_millis(800));
        loop {
            // Often, so a sentence shows the moment its voice starts.
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(VoiceCmd::Say(text, id)) => {
                    let _ = call(&Request::Say { text, queue: true, id });
                }
                Ok(VoiceCmd::Stop) => {
                    let _ = call(&Request::Stop);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            let voice = match call(&Request::Status) {
                Ok(v) => Voice { reachable: true, status: serde_json::from_value(v).unwrap_or_default() },
                Err(_) => Voice::default(),
            };
            if tx.send(Msg::Voice(voice)).is_err() {
                break;
            }
        }
    });
}

impl App {
    fn new(config: Config, voice_tx: Sender<VoiceCmd>, tx: Sender<Msg>, animate: Arc<AtomicBool>) -> App {
        let catalog = agent::catalog();
        let agent = catalog.iter().find(|a| a.id == config.chat.agent).unwrap_or(&catalog[0]).clone();
        let sessions = std::fs::read_to_string(sessions_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let talk_key = KeySpec::parse(&config.chat.talk_key).unwrap_or_else(|| KeySpec::parse("alt+space").unwrap());
        App {
            config,
            catalog,
            agent,
            link: None,
            process: None,
            generation: 0,
            session: None,
            sessions,
            messages: Vec::new(),
            reply_open: false,
            splitter: Splitter::default(),
            unsaid: Vec::new(),
            always: HashSet::new(),
            allow_all: false,
            ask: None,
            question: None,
            busy: false,
            starting: false,
            working: false,
            needs_login: false,
            pending_prompt: None,
            turn_start: None,
            last_output: Instant::now(),
            warned_quiet: false,
            stt: Stt::new(),
            voxtype: stt::State::Idle,
            recording_here: false,
            focused: true,
            push_to_talk: paths::find_program("omarchy-dictation").is_some(),
            see_through_set: false,
            walk: 0.0,
            model: String::new(),
            born: Instant::now(),
            animate,
            levels: levels::Levels::start(),
            voice: Voice::default(),
            voice_tx,
            input: LineInput::default(),
            last_typed: None,
            dictation: Dictation::default(),
            reveals: std::collections::VecDeque::new(),
            // Ids keep growing across runs, so a fresh chat never looks heard already.
            next_say_id: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64 * 1000)
                .unwrap_or(1),
            reveal_moved: Instant::now(),
            scroll: 0,
            overlay: Overlay::None,
            theme: Theme::load(),
            talk_key,
            tick: 0,
            quit: false,
            tx,
        }
    }

    fn event_loop(&mut self, term: &mut Terminal<CrosstermBackend<io::Stdout>>, rx: Receiver<Msg>) -> Result<()> {
        term.draw(|f| ui::draw(f, self))?;
        while let Ok(msg) = rx.recv() {
            self.handle(msg);
            // Take everything waiting before drawing once.
            while let Ok(msg) = rx.try_recv() {
                self.handle(msg);
            }
            if self.quit {
                break;
            }
            self.animate.store(ui::moving(self), Ordering::Relaxed);
            term.draw(|f| ui::draw(f, self))?;
        }
        Ok(())
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Key(k) => self.key(k),
            Msg::Paste(s) => {
                if let Overlay::Settings(v) = &mut self.overlay {
                    v.paste(&s);
                } else {
                    self.input.insert_str(&s.replace('\n', " "));
                    self.dictation.pasted();
                    self.typed();
                }
            }
            Msg::Redraw => {}
            Msg::Agent(generation, out) => {
                if generation == self.generation {
                    self.agent_output(out);
                }
            }
            Msg::Heard(result) => {
                self.stt.finished();
                match result {
                    stt::Result::Text(t) => self.send(&t),
                    stt::Result::Nothing => {}
                    stt::Result::Problem(p) => self.note(&p),
                }
            }
            Msg::Voice(v) => {
                self.voice = v;
                self.reveal_heard();
            }
            Msg::External(action) => match action.as_str() {
                "talk" => self.talk(),
                "stop" => self.stop(),
                other => {
                    if let (Some(list), Overlay::Settings(view)) = (other.strip_prefix("voices:"), &mut self.overlay) {
                        view.set_voices(list.split(',').map(str::to_string).collect());
                    }
                }
            },
            Msg::Tick => self.on_tick(),
            Msg::Voxtype(state) => {
                if state == stt::State::Recording {
                    self.levels.clear();
                    if self.stt.state() == stt::State::Idle {
                        self.recording_here = self.focused;
                    }
                }
                self.voxtype = state;
            }
            Msg::Frame => self.step_walk(),
            Msg::Focus(on) => self.focused = on,
        }
    }

    fn on_tick(&mut self) {
        self.tick += 1;
        if self.tick % 8 == 0 {
            if self.theme.refresh() {
                self.see_through();
            }
        }
        // Dictated text is sent once it stops changing; typed text waits for enter.
        if let Some(left) = self.send_countdown() {
            if left.is_zero() {
                let text = self.input.text();
                self.input.clear();
                self.last_typed = None;
                self.dictation.reset();
                self.send(&text);
            }
        }
        // The voice has stopped while text still waits for it: show the text.
        if !self.reveals.is_empty() && !self.voice.status.speaking && self.reveal_moved.elapsed() > Duration::from_secs(2) {
            self.reveal_all();
        }
        if self.busy && !self.warned_quiet && self.ask.is_none() && self.question.is_none() && self.last_output.elapsed() > Duration::from_secs(120) {
            self.warned_quiet = true;
            self.note(&format!("{} has been quiet for two minutes. Press esc to stop it if it is stuck.", self.agent.name));
        }
    }

    fn typed(&mut self) {
        self.last_typed = Some(Instant::now());
        if self.input.is_empty() {
            self.dictation.reset();
        }
    }

    /// How long until the dictated text in the message line is sent; None
    /// when nothing will be sent on its own.
    pub fn send_countdown(&self) -> Option<Duration> {
        let after = self.config.chat.send_after;
        let t = self.last_typed?;
        // In "your own dictation" mode everything in the line counts as dictated.
        let dictated = self.dictation.dictated || self.config.chat.input == "dictation";
        if after == 0 || !dictated || self.input.is_empty() || !matches!(self.overlay, Overlay::None) {
            return None;
        }
        // "Instant": a dictation tool types its text in one burst; a short
        // quiet spell means it is done.
        let wait = if after == 1 { Duration::from_millis(300) } else { Duration::from_secs(after as u64) };
        Some(wait.saturating_sub(t.elapsed()))
    }

    // ------------------------------------------------------------------ keys

    fn key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);

        if ctrl && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('q')) {
            self.quit = true;
            return;
        }
        match &mut self.overlay {
            Overlay::Help => {
                self.overlay = Overlay::None;
                return;
            }
            Overlay::Settings(view) => {
                if let Some(action) = view.key(&k, &mut self.config, &self.catalog) {
                    self.settings_action(action);
                }
                return;
            }
            Overlay::None => {}
        }

        if self.talk_key.matches(&k) {
            self.talk();
            return;
        }
        match k.code {
            KeyCode::Esc => self.stop(),
            KeyCode::Tab | KeyCode::BackTab => self.toggle_auto(),
            KeyCode::F(1) => self.overlay = Overlay::Help,
            KeyCode::Enter => {
                let text = self.input.text();
                self.input.clear();
                self.last_typed = None;
                self.dictation.reset();
                self.send(&text);
            }
            KeyCode::Char('o') if ctrl => self.open_settings(),
            KeyCode::Char('n') if ctrl => self.new_chat(),
            KeyCode::Char('r') if ctrl => self.resume(),
            KeyCode::Char('l') if ctrl => self.open_login(),
            KeyCode::Char('y') if alt && self.ask.is_some() => self.allow(false),
            KeyCode::Char('a') if alt && self.ask.is_some() => self.allow(true),
            KeyCode::Char('n') if alt && self.ask.is_some() => self.deny(None),
            KeyCode::Char('s') if alt && self.question.is_some() => self.choose(""),
            KeyCode::Char(c) if alt && c.is_ascii_digit() && self.question.is_some() => {
                let i = c.to_digit(10).unwrap_or(0) as usize;
                if let Some(choice) = self.question.as_ref().and_then(|q| q.choices.get(i.wrapping_sub(1))).cloned() {
                    self.choose(&choice);
                }
            }
            KeyCode::PageUp => self.scroll += 10,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Up => self.scroll += 1,
            KeyCode::Down => self.scroll = self.scroll.saturating_sub(1),
            _ => {
                if self.input.key(&k) {
                    if matches!(k.code, KeyCode::Char(_)) && !ctrl && !alt {
                        self.dictation.char_at(Instant::now());
                    }
                    self.typed();
                }
            }
        }
    }

    /// Auto mode: every action the agent asks for is allowed, for this run.
    /// Turned on while the agent waits for an OK, it gives that OK too.
    fn toggle_auto(&mut self) {
        self.allow_all = !self.allow_all;
        if self.allow_all && self.ask.is_some() {
            self.allow(false);
        }
    }

    fn open_settings(&mut self) {
        self.catalog = agent::catalog();
        let mut view = SettingsView::new(&self.config);
        view.allow_all = self.allow_all;
        self.overlay = Overlay::Settings(view);
        // Ask the daemon for its voices in the background.
        let tx = self.tx.clone();
        thread::spawn(move || {
            if let Ok(v) = ipc::call(&paths::daemon_socket(), &Request::Voices, Duration::from_secs(2)) {
                let voices: Vec<String> = v["voices"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                if !voices.is_empty() {
                    let _ = tx.send(Msg::External(format!("voices:{}", voices.join(","))));
                }
            }
        });
    }

    fn settings_action(&mut self, action: settings::Action) {
        match action {
            settings::Action::Close => self.overlay = Overlay::None,
            settings::Action::Saved { agent_changed } => {
                if let Err(e) = self.config.save() {
                    self.note(&format!("Could not save the settings: {e}"));
                }
                self.talk_key = KeySpec::parse(&self.config.chat.talk_key).unwrap_or(self.talk_key.clone());
                self.see_through();
                if agent_changed {
                    self.new_chat();
                    if let Some(a) = self.catalog.iter().find(|a| a.id == self.config.chat.agent) {
                        self.agent = a.clone();
                    }
                }
            }
            settings::Action::AllowAll(on) => self.allow_all = on,
            settings::Action::Login => {
                self.overlay = Overlay::None;
                self.open_login();
            }
            settings::Action::StartVoice => start_voice_service(),
            settings::Action::TestVoice => self.say("This is how replies will sound."),
        }
    }

    // ----------------------------------------------------------- the process

    fn start_process(&mut self) {
        let Some(program) = paths::find_program(self.agent.bin) else {
            self.note(&format!("{} is not installed: {} is not on your PATH.", self.agent.name, self.agent.bin));
            return;
        };
        self.model.clear();
        let mut link = agent::create(&self.agent);
        let args = link.arguments(self.session.as_deref());
        let cwd = self.config.workdir();
        self.generation += 1;
        let generation = self.generation;
        let tx = self.tx.clone();
        match Process::spawn(&program, &args, &cwd, move |out| {
            let _ = tx.send(Msg::Agent(generation, out));
        }) {
            Ok(p) => {
                self.process = Some(p);
                self.starting = true;
                let events = link.open(self.session.as_deref(), &cwd.to_string_lossy());
                self.link = Some(link);
                self.flush_link();
                for e in events {
                    self.agent_event(e);
                }
            }
            Err(e) => self.note(&format!("Could not start {}: {e}", self.agent.name)),
        }
    }

    fn kill_process(&mut self) {
        self.generation += 1;
        if let Some(p) = self.process.take() {
            thread::spawn(move || p.stop());
        }
        self.link = None;
        self.starting = false;
        self.set_busy(false);
    }

    /// Writes what the link wants to say to the agent.
    fn flush_link(&mut self) {
        let lines = self.link.as_mut().map(|l| l.drain()).unwrap_or_default();
        if let Some(p) = self.process.as_mut() {
            for line in lines {
                p.write_line(&line);
            }
        }
    }

    fn agent_output(&mut self, out: Output) {
        match out {
            Output::Line(line) => {
                self.last_output = Instant::now();
                self.warned_quiet = false;
                let events = self.link.as_mut().map(|l| l.read(&line)).unwrap_or_default();
                self.flush_link();
                for e in events {
                    self.agent_event(e);
                }
            }
            Output::Exit(stderr) => {
                let code = self.process.as_mut().and_then(|p| p.exit_code());
                self.process = None;
                self.link = None;
                self.starting = false;
                let why = stderr.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("").trim().to_string();
                if looks_like_login(&stderr) {
                    self.login_needed();
                } else if self.busy || code.is_some_and(|c| c != 0) {
                    let msg = if why.is_empty() { format!("{} stopped.", self.agent.name) } else { format!("{} stopped: {why}", self.agent.name) };
                    self.note(&msg);
                }
                self.ask = None;
                self.question = None;
                self.pending_prompt = None;
                self.set_busy(false);
            }
        }
    }

    fn agent_event(&mut self, e: Event) {
        match e {
            Event::Session(id) => {
                self.session = Some(id.clone());
                self.sessions.insert(self.agent.id.to_string(), id);
                save_sessions(&self.sessions);
            }
            Event::Ready => {
                self.starting = false;
                if let Some(text) = self.pending_prompt.take() {
                    if let Some(l) = self.link.as_mut() {
                        l.prompt(&text);
                    }
                    self.flush_link();
                }
            }
            Event::ResumeFailed => self.note("Could not continue the last conversation; this is a new one."),
            Event::Text(delta) => {
                // Space before a reply starts is no part of it.
                if !self.reply_open && delta.trim().is_empty() {
                    return;
                }
                self.append_reply(&delta);
                let parts = self.splitter.feed(&delta);
                self.unsaid.extend(parts);
            }
            Event::Break => self.end_reply(),
            Event::Thinking => self.working = false,
            Event::Working => self.working = true,
            Event::Step(act) => {
                self.end_reply();
                self.push(Kind::Step, act.doing());
                self.working = true;
            }
            Event::Ask { id, act, detail, key } => {
                self.end_reply();
                if self.allow_all || agent::covered(&key, &self.always) {
                    if let Some(l) = self.link.as_mut() {
                        l.answer(&id, true, false, None);
                    }
                    self.flush_link();
                    return;
                }
                self.say(&format!("{} wants to {}. Allow it?", self.agent.name, act.to_do()));
                self.ask = Some(Ask { id, act, detail, key });
            }
            Event::Question { id, text, choices } => {
                self.end_reply();
                let spoken = if choices.is_empty() { text.clone() } else { format!("{text} Choices: {}.", choices.join(", ")) };
                self.say(&spoken);
                self.question = Some(Question { id, text, choices });
            }
            Event::Done { error, cancelled } => {
                self.end_reply();
                self.ask = None;
                self.question = None;
                self.set_busy(false);
                if cancelled {
                    self.note("stopped");
                } else if let Some(err) = error {
                    if looks_like_login(&err) {
                        self.login_needed();
                    } else {
                        let short: String = err.chars().take(300).collect();
                        self.note(&format!("{} failed: {short}", self.agent.name));
                    }
                    self.say("Sorry, that did not work.");
                }
            }
            Event::Model(label) => self.model = label,
            Event::Retrying(err) => self.note(&format!("retrying: {err}")),
            Event::Failed(message) => {
                if looks_like_login(&message) {
                    self.login_needed();
                } else if message.is_empty() {
                    self.note(&format!("{} could not start a conversation.", self.agent.name));
                } else {
                    self.note(&format!("{} could not start: {message}", self.agent.name));
                }
                self.pending_prompt = None;
                self.kill_process();
            }
        }
    }

    // ---------------------------------------------------------- conversation

    fn send(&mut self, raw: &str) {
        let text = raw.trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(ask) = &self.ask {
            let _ = ask;
            self.push(Kind::User, text.clone());
            match speech::parse_answer(&text) {
                Answer::Allow => self.allow(false),
                Answer::Always => self.allow(true),
                Answer::Deny => self.deny(None),
                Answer::Instead => self.deny(Some(&text)),
            }
            return;
        }
        if self.question.is_some() {
            self.choose(&text);
            return;
        }
        self.prompt(&text);
    }

    fn prompt(&mut self, text: &str) {
        self.push(Kind::User, text.to_string());
        self.voice_stop();
        self.splitter.reset();
        self.unsaid.clear();
        self.needs_login = false;
        self.working = false;
        self.scroll = 0;
        self.set_busy(true);
        if self.process.is_none() {
            self.start_process();
        }
        if self.process.is_none() {
            self.set_busy(false);
            return;
        }
        if self.starting {
            self.pending_prompt = Some(text.to_string());
        } else if let Some(l) = self.link.as_mut() {
            l.prompt(text);
            self.flush_link();
        }
    }

    /// The talk key: start listening, or stop and send.
    pub fn talk(&mut self) {
        if self.config.chat.input != "key" {
            self.note("Speech input is set to your own dictation: dictate into the message line.");
            return;
        }
        match self.stt.state() {
            stt::State::Recording => {
                if let Some(job) = self.stt.stop() {
                    let tx = self.tx.clone();
                    thread::spawn(move || {
                        let _ = tx.send(Msg::Heard(job()));
                    });
                }
            }
            stt::State::Transcribing => {}
            stt::State::Idle => {
                // Talking over the voice stops it, as in a conversation.
                self.voice_stop();
                let engine = self.config.chat.stt.clone();
                // Voxtype's overlay shows only when this window cannot.
                self.recording_here = self.focused;
                if let Err(e) = self.stt.start(&engine, !self.focused) {
                    self.note(&e);
                }
            }
        }
    }

    fn allow(&mut self, always: bool) {
        let Some(ask) = self.ask.take() else { return };
        if always {
            self.always.insert(ask.key.clone());
        }
        if let Some(l) = self.link.as_mut() {
            l.answer(&ask.id, true, always, None);
        }
        self.flush_link();
        self.voice_stop();
    }

    fn deny(&mut self, instead: Option<&str>) {
        let Some(ask) = self.ask.take() else { return };
        if let Some(l) = self.link.as_mut() {
            l.answer(&ask.id, false, false, instead);
        }
        self.flush_link();
        self.voice_stop();
    }

    fn choose(&mut self, answer: &str) {
        let Some(q) = self.question.take() else { return };
        if answer.is_empty() {
            self.note("skipped the question");
        } else {
            self.push(Kind::User, answer.to_string());
        }
        if let Some(l) = self.link.as_mut() {
            l.answer_question(&q.id, answer);
        }
        self.flush_link();
        self.voice_stop();
    }

    /// Esc: stop the voice, the recording, and the agent's turn.
    pub fn stop(&mut self) {
        self.stt.cancel();
        self.voice_stop();
        if self.busy {
            if let Some(l) = self.link.as_mut() {
                l.cancel();
            }
            self.flush_link();
            self.pending_prompt = None;
        }
    }

    fn new_chat(&mut self) {
        self.stt.cancel();
        self.voice_stop();
        self.kill_process();
        self.session = None;
        self.messages.clear();
        self.reply_open = false;
        self.splitter.reset();
        self.unsaid.clear();
        self.always.clear();
        self.ask = None;
        self.question = None;
        self.pending_prompt = None;
        self.needs_login = false;
        self.scroll = 0;
    }

    fn resume(&mut self) {
        let Some(last) = self.sessions.get(self.agent.id).cloned() else {
            self.note("There is no earlier conversation to continue.");
            return;
        };
        self.new_chat();
        self.session = Some(last);
        self.note(&format!("continuing the last conversation with {}", self.agent.name));
    }

    pub fn can_resume(&self) -> bool {
        self.sessions.get(self.agent.id).is_some_and(|s| Some(s) != self.session.as_ref())
    }

    /// Opens the agent's own TUI in a new terminal, to log in there.
    fn open_login(&mut self) {
        let login = self.agent.login;
        let Some(program) = paths::find_program(login) else {
            self.note(&format!("{login} is not installed."));
            return;
        };
        let cwd = self.config.workdir();
        let mut cmd = if paths::find_program("omarchy-launch-tui").is_some() {
            let mut c = std::process::Command::new("omarchy-launch-tui");
            c.arg(&program).args(self.agent.login_args);
            c
        } else {
            let mut c = std::process::Command::new("xdg-terminal-exec");
            c.arg("-e").arg(&program).args(self.agent.login_args);
            c
        };
        let _ = cmd
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        self.note(&format!("Log in to {} in the new terminal, then send your message again.", self.agent.name));
    }

    fn login_needed(&mut self) {
        self.needs_login = true;
        self.note(&format!("{} is not logged in. Press ctrl+l to log in once in its own terminal app.", self.agent.name));
    }

    fn say(&mut self, text: &str) {
        if !self.config.chat.speak_replies {
            return;
        }
        let spoken = speech::speakable(text);
        if !spoken.is_empty() {
            let _ = self.voice_tx.send(VoiceCmd::Say(spoken, None));
        }
    }

    /// Whether replies show with the voice: on, and the voice is ready.
    fn synced(&self) -> bool {
        self.config.chat.speak_replies && self.voice.reachable && self.voice.status.ready && self.voice.status.problem.is_empty()
    }

    /// Says the whole reply in one go; its text shows once its voice starts.
    fn say_reply(&mut self, parts: Vec<(String, usize)>) {
        if !self.config.chat.speak_replies {
            return;
        }
        let Some(&(_, end)) = parts.last() else { return };
        let spoken = parts
            .iter()
            .map(|(p, _)| speech::speakable(p))
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if spoken.is_empty() {
            return;
        }
        let idx = self.messages.len().saturating_sub(1);
        let waits = self.reply_open && self.messages.get(idx).is_some_and(|m| m.shown.is_some());
        if !waits {
            let _ = self.voice_tx.send(VoiceCmd::Say(spoken, None));
            return;
        }
        self.next_say_id += 1;
        let id = self.next_say_id;
        if self.reveals.is_empty() {
            self.reveal_moved = Instant::now();
        }
        self.reveals.push_back((id, idx, end));
        let _ = self.voice_tx.send(VoiceCmd::Say(spoken, Some(id)));
    }

    /// Shows the text of every sentence the voice has started.
    fn reveal_heard(&mut self) {
        if !self.synced() {
            if !self.reveals.is_empty() || self.messages.iter().any(|m| m.shown.is_some()) {
                self.reveal_all();
            }
            return;
        }
        let heard = self.voice.status.heard;
        while let Some(&(id, idx, end)) = self.reveals.front() {
            if id > heard {
                break;
            }
            self.reveals.pop_front();
            self.reveal_moved = Instant::now();
            if let Some(m) = self.messages.get_mut(idx) {
                if let Some(shown) = m.shown {
                    m.shown = Some(shown.max(end).min(m.text.len()));
                }
            }
        }
        // A finished reply whose last sentence is heard shows whole.
        let open = if self.reply_open { self.messages.len().checked_sub(1) } else { None };
        for (i, m) in self.messages.iter_mut().enumerate() {
            if Some(i) != open && m.shown.is_some() && !self.reveals.iter().any(|r| r.1 == i) {
                m.shown = None;
            }
        }
    }

    fn reveal_all(&mut self) {
        self.reveals.clear();
        for m in &mut self.messages {
            m.shown = None;
        }
    }

    fn voice_stop(&mut self) {
        let _ = self.voice_tx.send(VoiceCmd::Stop);
        self.reveal_all();
    }

    /// A reply waits for its first sentence to be heard.
    pub fn waiting_for_voice(&self) -> bool {
        self.messages.iter().any(|m| m.shown == Some(0))
    }

    fn note(&mut self, text: &str) {
        self.push(Kind::Note, text.to_string());
    }

    fn push(&mut self, kind: Kind, text: String) {
        self.reply_open = false;
        self.messages.push(Message { kind, text, shown: None });
    }

    fn append_reply(&mut self, delta: &str) {
        if !self.reply_open {
            // With the voice on, the reply shows sentence by sentence as it is heard.
            let shown = self.synced().then_some(0);
            self.messages.push(Message { kind: Kind::Agent, text: delta.to_string(), shown });
            self.reply_open = true;
        } else if let Some(m) = self.messages.last_mut() {
            m.text.push_str(delta);
        }
    }

    /// The reply in progress is complete: say what is left of it.
    fn end_reply(&mut self) {
        let mut parts = std::mem::take(&mut self.unsaid);
        parts.extend(self.splitter.flush());
        self.say_reply(parts);
        if self.reply_open {
            let idx = self.messages.len() - 1;
            let len = self.messages[idx].text.len();
            // What follows the last spoken sentence (code, a table) shows with it.
            match self.reveals.iter_mut().rev().find(|r| r.1 == idx) {
                Some(last) => last.2 = len,
                None => self.messages[idx].shown = None,
            }
        }
        self.reply_open = false;
    }

    fn set_busy(&mut self, busy: bool) {
        if busy && !self.busy {
            self.turn_start = Some(Instant::now());
            self.last_output = Instant::now();
            self.warned_quiet = false;
        }
        self.busy = busy;
        if !busy {
            self.working = false;
            self.turn_start = None;
        }
    }

    pub fn seconds(&self) -> u64 {
        self.turn_start.map(|t| t.elapsed().as_secs()).unwrap_or(0)
    }

    /// The microphone records: omanta's talk key or any Voxtype recording.
    /// Only a recording that started while this window had the focus.
    pub fn listening(&self) -> bool {
        self.recording_here && (self.stt.state() == stt::State::Recording || self.voxtype == stt::State::Recording)
    }

    /// What the conversation is doing, for the status line.
    pub fn state(&self) -> &'static str {
        if self.listening() {
            return "listening";
        }
        if self.recording_here && (self.stt.state() == stt::State::Transcribing || self.voxtype == stt::State::Transcribing) {
            return "transcribing";
        }
        if self.ask.is_some() {
            "needs your OK"
        } else if self.question.is_some() {
            "needs your answer"
        } else if self.starting && self.busy {
            "starting"
        } else if self.busy {
            if self.working {
                "working"
            } else {
                "thinking"
            }
        } else if self.waiting_for_voice() {
            "thinking"
        } else if self.voice.status.speaking {
            "speaking"
        } else {
            "idle"
        }
    }

    /// Moves the walking wheel toward the voice's progress. A new text
    /// starts it again from the left.
    fn step_walk(&mut self) {
        if self.state() != "speaking" {
            self.walk = 0.0;
            return;
        }
        let target = self.voice.status.progress;
        if target < self.walk - 0.3 {
            self.walk = target;
        } else {
            self.walk += (target - self.walk) * 0.25;
        }
    }

    /// Makes the terminal's background see-through, in the theme's color.
    /// foot reads the alpha from OSC 11 (URxvt's "[percent]" form); other
    /// terminals ignore it. Off gives the terminal its own background back.
    fn see_through(&mut self) {
        use std::io::Write;
        let t = self.config.chat.transparency;
        if t <= 0.0 && !self.see_through_set {
            return;
        }
        let code = if t <= 0.0 {
            "\x1b]111\x1b\\".to_string()
        } else {
            let alpha = ((1.0 - t) * 100.0).round() as u32;
            format!("\x1b]11;[{alpha}]{}\x1b\\", self.theme.background)
        };
        self.see_through_set = t > 0.0;
        let mut out = io::stdout();
        let _ = out.write_all(code.as_bytes());
        let _ = out.flush();
    }

    /// Gives the terminal its own background back.
    fn restore_background(&mut self) {
        if self.see_through_set {
            self.config.chat.transparency = 0.0;
            self.see_through();
        }
    }

    fn shutdown(&mut self) {
        self.stt.cancel();
        self.voice_stop();
        if let Some(p) = self.process.take() {
            p.stop();
        }
    }
}

fn save_sessions(sessions: &HashMap<String, String>) {
    let path = sessions_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(sessions).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn looks_like_login(message: &str) -> bool {
    let m = message.to_lowercase();
    ["log in", "login", "logged in", "not logged", "authenticat", "api key", "unauthorized", "401", "credential", "sign in", "oauth token"]
        .iter()
        .any(|n| m.contains(n))
}

/// Starts the voice service: the systemd user unit when it is installed
/// (enabled, so it also starts at every login), otherwise `omanta daemon` in
/// the background.
pub fn start_voice_service() {
    let unit = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "omanta.service"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !unit {
        if let Ok(exe) = std::env::current_exe() {
            let mut cmd = std::process::Command::new(exe);
            cmd.arg("daemon").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            unsafe {
                use std::os::unix::process::CommandExt;
                cmd.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
            let _ = cmd.spawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_login_errors() {
        assert!(looks_like_login("Invalid API key · Please run /login"));
        assert!(looks_like_login("Authentication required"));
        assert!(!looks_like_login("file not found"));
    }
}
