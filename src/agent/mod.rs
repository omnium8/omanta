//! Agent CLIs in a machine-readable mode. A link holds no process: lines from
//! the agent go into `read`, and the lines it wants written collect in an
//! outbox that the caller drains, so tests feed links recorded output.

pub mod acp;
pub mod claude;
pub mod process;

use std::collections::HashSet;

use serde_json::Value;

use crate::paths;

#[derive(Clone, Debug)]
pub struct AgentInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// The program on PATH.
    pub bin: &'static str,
    /// Arguments that serve ACP on stdio; empty for Claude Code.
    pub serve: &'static [&'static str],
    /// The program that logs in, when it is not `bin` (an ACP adapter).
    pub login: &'static str,
    /// Its arguments for logging in; empty opens its TUI, which asks.
    pub login_args: &'static [&'static str],
    pub installed: bool,
}

/// Every agent omanta knows, with `installed` read from PATH. Claude Code
/// speaks stream-json; the rest serve ACP.
pub fn catalog() -> Vec<AgentInfo> {
    type Row = (&'static str, &'static str, &'static str, &'static [&'static str], &'static str, &'static [&'static str]);
    let list: [Row; 10] = [
        ("claude", "Claude Code", "claude", &[], "claude", &[]),
        ("opencode", "OpenCode", "opencode", &["acp"], "opencode", &[]),
        ("grok", "Grok", "grok", &["agent", "stdio"], "grok", &[]),
        ("cursor-agent", "Cursor Agent", "cursor-agent", &["acp"], "cursor-agent", &["login"]),
        ("gemini", "Gemini CLI", "gemini", &["--experimental-acp"], "gemini", &[]),
        ("codex", "Codex", "codex-acp", &[], "codex", &[]),
        ("copilot", "GitHub Copilot", "copilot", &["--acp"], "copilot", &[]),
        ("qwen", "Qwen Code", "qwen", &["--acp"], "qwen", &[]),
        ("kimi", "Kimi", "kimi", &["acp"], "kimi", &[]),
        ("hermes", "Hermes", "hermes-acp", &[], "hermes", &[]),
    ];
    list.iter()
        .map(|&(id, name, bin, serve, login, login_args)| AgentInfo {
            id,
            name,
            bin,
            serve,
            login,
            login_args,
            installed: paths::find_program(bin).is_some(),
        })
        .collect()
}

/// A tool call, as the chat shows and says it.
#[derive(Clone, Debug, PartialEq)]
pub struct Act {
    /// run, read, write, edit, delete, search, web, subtask, plan, tool
    pub verb: &'static str,
    /// A file name, a command's description, a query; may be empty.
    pub what: String,
}

impl Act {
    fn new(verb: &'static str, what: impl Into<String>) -> Act {
        Act { verb, what: what.into() }
    }

    /// "run a command: list files", for "Claude wants to …".
    pub fn to_do(&self) -> String {
        let w = &self.what;
        match (self.verb, w.is_empty()) {
            ("run", true) => "run a command".into(),
            ("run", false) => format!("run a command: {w}"),
            ("read", true) => "read a file".into(),
            ("write", true) => "write a file".into(),
            ("edit", true) => "edit a file".into(),
            ("delete", true) => "delete a file".into(),
            ("read" | "write" | "edit" | "delete", false) => format!("{} {w}", self.verb),
            ("search", _) => format!("search for {w}"),
            ("web", _) => format!("search the web for {w}"),
            ("subtask", _) => format!("start a subtask: {w}"),
            ("plan", _) => "go ahead with its plan".into(),
            (_, true) => "use a tool".into(),
            (_, false) => format!("use {w}"),
        }
    }

    /// "reading main.rs", for the step lines.
    pub fn doing(&self) -> String {
        let w = &self.what;
        let ing = match self.verb {
            "run" => return format!("running: {w}"),
            "read" => "reading",
            "write" => "writing",
            "edit" => "editing",
            "delete" => "deleting",
            "search" => return format!("searching {w}"),
            "web" => return format!("searching the web: {w}"),
            "subtask" => return format!("subtask: {w}"),
            "plan" => return "planning".into(),
            _ => return if w.is_empty() { "using a tool".into() } else { format!("using {w}") },
        };
        if w.is_empty() {
            format!("{ing} a file")
        } else {
            format!("{ing} {w}")
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The conversation's id, to resume it later.
    Session(String),
    /// Ready for prompts.
    Ready,
    /// The session to continue is gone; a new one started.
    ResumeFailed,
    /// More of the reply.
    Text(String),
    /// A new message starts (after tool use): keep its text apart.
    Break,
    Thinking,
    Working,
    Step(Act),
    /// It asks before a tool call. `key` is what "always" allows.
    Ask { id: String, act: Act, detail: String, key: String },
    /// It asks the user a question; any answer goes, `choices` are suggestions.
    Question { id: String, text: String, choices: Vec<String> },
    /// The turn ended.
    Done { error: Option<String>, cancelled: bool },
    Retrying(String),
    /// It cannot go on (not logged in, no session).
    Failed(String),
}

pub trait Link: Send {
    /// Arguments for the agent's program, continuing session `resume`.
    fn arguments(&self, resume: Option<&str>) -> Vec<String>;
    /// The process runs: say hello.
    fn open(&mut self, resume: Option<&str>, cwd: &str) -> Vec<Event>;
    fn prompt(&mut self, text: &str);
    fn cancel(&mut self);
    fn answer(&mut self, id: &str, allow: bool, always: bool, instead: Option<&str>);
    /// An empty answer skips the question.
    fn answer_question(&mut self, id: &str, answer: &str);
    fn read(&mut self, line: &str) -> Vec<Event>;
    /// Lines to write to the agent's stdin, oldest first.
    fn drain(&mut self) -> Vec<String>;
}

pub fn create(agent: &AgentInfo) -> Box<dyn Link> {
    if agent.id == "claude" {
        Box::new(claude::Claude::default())
    } else {
        Box::new(acp::Acp::new(agent.serve.iter().map(|s| s.to_string()).collect()))
    }
}

/// Every program a command line runs: `cd x && curl y | head` is cd, curl, head.
pub fn programs(command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let normalized = command.replace("&&", "\n").replace("||", "\n").replace("$(", "\n");
    for part in normalized.split(['\n', ';', '|', '&', '`']) {
        let first = part
            .split_whitespace()
            .find(|w| !(w.contains('=') && w.split('=').next().is_some_and(|k| !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_'))));
        if let Some(word) = first {
            let program = word.trim_start_matches(['(', '{']).rsplit('/').next().unwrap_or("").to_string();
            if !program.is_empty() && !out.contains(&program) {
                out.push(program);
            }
        }
    }
    out.sort();
    out
}

/// What "always" allows for a tool call: the tool, or for a shell command
/// every program in it, so "always" for `git status` does not allow `rm`.
pub fn always_key(tool: &str, command: &str) -> String {
    if tool == "Bash" || tool == "execute" {
        format!("Bash:{}", programs(command).join(" "))
    } else {
        tool.to_string()
    }
}

pub fn covered(key: &str, always: &HashSet<String>) -> bool {
    let Some(wanted) = key.strip_prefix("Bash:") else { return always.contains(key) };
    let allowed: HashSet<&str> =
        always.iter().filter_map(|k| k.strip_prefix("Bash:")).flat_map(|k| k.split(' ')).collect();
    let wanted: Vec<&str> = wanted.split(' ').filter(|w| !w.is_empty()).collect();
    !wanted.is_empty() && wanted.iter().all(|w| allowed.contains(w))
}

pub(crate) fn file_name(path: &str) -> String {
    let p = path.trim_end_matches('/');
    p.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(path).to_string()
}

pub(crate) fn first_string(v: &Value, keys: &[&str]) -> String {
    keys.iter().filter_map(|k| v.get(*k).and_then(Value::as_str)).find(|s| !s.is_empty()).unwrap_or("").to_string()
}

pub(crate) fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_covers_only_its_programs() {
        assert_eq!(programs("cd /x && git status | head -5"), vec!["cd", "git", "head"]);
        assert_eq!(programs("FOO=1 /usr/bin/make test"), vec!["make"]);
        let always: HashSet<String> = [always_key("Bash", "git status")].into();
        assert!(covered(&always_key("Bash", "git log"), &always));
        assert!(!covered(&always_key("Bash", "git log && rm -rf x"), &always));
        assert!(covered("Edit", &["Edit".to_string()].into()));
        assert!(!covered("Write", &["Edit".to_string()].into()));
    }

    #[test]
    fn phrases_read_naturally() {
        assert_eq!(Act::new("run", "List files").to_do(), "run a command: List files");
        assert_eq!(Act::new("tool", "").to_do(), "use a tool");
        assert_eq!(Act::new("edit", "main.rs").doing(), "editing main.rs");
        assert_eq!(Act::new("read", "").doing(), "reading a file");
    }

    #[test]
    fn catalog_lists_claude_first() {
        let c = catalog();
        assert!(c.len() >= 5);
        assert_eq!(c[0].id, "claude");
    }
}
