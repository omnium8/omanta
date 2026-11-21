//! An agent over ACP, the Agent Client Protocol: JSON-RPC 2.0 on stdio. ACP
//! has no system prompt, so the first prompt of a conversation carries the
//! voice rules.

use std::collections::{HashMap, VecDeque};

use serde_json::{json, Value};

use super::{always_key, file_name, first_string, short, Act, Event, Link};
use crate::speech::VOICE_PROMPT;

/// What an answer to one of our calls is for.
enum Pending {
    Initialize,
    Authenticate,
    Load,
    New,
    Prompt,
}

pub struct Acp {
    serve: Vec<String>,
    cwd: String,
    session: Option<String>,
    loading: bool,
    primed: bool,
    /// The login methods the agent offered in `initialize`.
    auth_methods: Vec<String>,
    /// `authenticate` was tried for this process, so a second refusal is final.
    tried_auth: bool,
    ready: bool,
    in_flight: bool,
    next_id: i64,
    calls: HashMap<i64, Pending>,
    queue: VecDeque<String>,
    /// Our id -> the request's JSON-RPC id and its options.
    asked: HashMap<String, (Value, Vec<Value>)>,
    /// Our id -> the request's id, the form's one field, whether it takes a list.
    questions: HashMap<String, (Value, String, bool)>,
    outbox: Vec<String>,
}

/// An ACP tool call as an `Act`.
pub fn act(u: &Value) -> Act {
    let kind = u["kind"].as_str().unwrap_or("");
    let mut title = u["title"].as_str().unwrap_or("").trim().to_string();
    let raw = &u["rawInput"];
    let path = u["locations"]
        .as_array()
        .and_then(|l| l.iter().find_map(|x| x["path"].as_str().filter(|p| !p.is_empty())))
        .map(str::to_string)
        .unwrap_or_else(|| first_string(raw, &["path", "file_path"]));
    match kind {
        "execute" => {
            if let Some(t) = title.strip_prefix("$ ") {
                title = t.to_string();
            }
            let d = raw["description"].as_str().unwrap_or("");
            let what = if !d.is_empty() { d.to_string() } else if !title.is_empty() { title } else { raw["command"].as_str().unwrap_or("").to_string() };
            Act::new("run", short(&what, 60))
        }
        "read" | "edit" | "delete" => {
            let verb = match kind {
                "read" => "read",
                "edit" => "edit",
                _ => "delete",
            };
            let what = if !path.is_empty() {
                file_name(&path)
            } else if title.eq_ignore_ascii_case(kind) {
                String::new()
            } else {
                short(&title, 60)
            };
            Act::new(verb, what)
        }
        "search" => Act::new("search", short(&title, 50)),
        "fetch" => Act::new("web", short(&title, 50)),
        "think" => Act::new("plan", ""),
        _ => by_name(&title, raw, &path).unwrap_or_else(|| Act::new("tool", short(if title.is_empty() { kind } else { &title }, 60))),
    }
}

/// Some agents (Grok) send no kind, only a tool name like `read_file` or
/// `run_terminal_command`: read what it does from the name.
fn by_name(title: &str, raw: &Value, path: &str) -> Option<Act> {
    let name = title.to_lowercase();
    let file = || if path.is_empty() { String::new() } else { file_name(path) };
    let has = |words: &[&str]| words.iter().any(|w| name.contains(w));
    if has(&["terminal", "command", "shell", "bash", "exec"]) {
        let c = first_string(raw, &["description", "command", "cmd"]);
        return Some(Act::new("run", short(&c, 60)));
    }
    if has(&["delete", "remove"]) {
        return Some(Act::new("delete", file()));
    }
    if has(&["edit", "replace", "patch", "str_replace"]) {
        return Some(Act::new("edit", file()));
    }
    if has(&["write", "create_file"]) {
        return Some(Act::new("write", file()));
    }
    if has(&["read", "view", "cat"]) {
        return Some(Act::new("read", file()));
    }
    if has(&["web", "fetch", "browse"]) {
        return Some(Act::new("web", short(&first_string(raw, &["query", "url"]), 50)));
    }
    if has(&["search", "grep", "glob", "find", "list"]) {
        return Some(Act::new("search", short(&first_string(raw, &["pattern", "query", "path"]), 50)));
    }
    None
}

/// What an error says went wrong: its data's details or message when it has
/// them (Cursor puts "run 'agent login' first" there), on one line.
pub fn reason(error: &Value) -> String {
    for text in [&error["data"]["details"], &error["data"]["message"], &error["message"]] {
        let t = text.as_str().unwrap_or("").trim();
        if !t.is_empty() {
            return t.split_whitespace().collect::<Vec<_>>().join(" ");
        }
    }
    String::new()
}

/// Whether an error asks for a login, as ACP's -32000 "Authentication required" does.
fn wants_auth(error: &Value) -> bool {
    error["code"] == -32000 || reason(error).to_lowercase().contains("auth")
}

impl Acp {
    pub fn new(serve: Vec<String>) -> Acp {
        Acp {
            serve,
            cwd: String::new(),
            session: None,
            loading: false,
            primed: false,
            auth_methods: Vec::new(),
            tried_auth: false,
            ready: false,
            in_flight: false,
            next_id: 0,
            calls: HashMap::new(),
            queue: VecDeque::new(),
            asked: HashMap::new(),
            questions: HashMap::new(),
            outbox: Vec::new(),
        }
    }

    fn send(&mut self, mut v: Value) {
        v["jsonrpc"] = json!("2.0");
        self.outbox.push(v.to_string());
    }

    fn call(&mut self, method: &str, params: Value, pending: Pending) {
        self.next_id += 1;
        let id = self.next_id;
        self.calls.insert(id, pending);
        self.send(json!({"id": id, "method": method, "params": params}));
    }

    fn reply(&mut self, id: Value, result: Value) {
        self.send(json!({"id": id, "result": result}));
    }

    fn new_session(&mut self) {
        let params = json!({"cwd": self.cwd, "mcpServers": []});
        self.call("session/new", params, Pending::New);
    }

    fn become_ready(&mut self, out: &mut Vec<Event>) {
        self.ready = true;
        out.push(Event::Ready);
        self.next();
    }

    fn next(&mut self) {
        if !self.ready || self.in_flight {
            return;
        }
        let Some(text) = self.queue.pop_front() else { return };
        self.in_flight = true;
        let said = if self.primed { text } else { format!("({VOICE_PROMPT})\n\n{text}") };
        self.primed = true;
        let params = json!({"sessionId": self.session, "prompt": [{"type": "text", "text": said}]});
        self.call("session/prompt", params, Pending::Prompt);
    }

    fn answered(&mut self, pending: Pending, result: &Value, error: &Value) -> Vec<Event> {
        let mut out = Vec::new();
        let failed = !error.is_null();
        match pending {
            Pending::Initialize => {
                self.auth_methods = result["authMethods"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                if failed {
                    out.push(Event::Failed(reason(error)));
                } else if let Some(id) = self.session.clone() {
                    self.loading = true;
                    let params = json!({"sessionId": id, "cwd": self.cwd, "mcpServers": []});
                    self.call("session/load", params, Pending::Load);
                } else {
                    self.new_session();
                }
            }
            Pending::Load => {
                self.loading = false;
                if failed {
                    self.session = None;
                    self.primed = false;
                    out.push(Event::ResumeFailed);
                    self.new_session();
                } else {
                    self.become_ready(&mut out);
                }
            }
            Pending::New => match result["sessionId"].as_str().filter(|s| !s.is_empty()) {
                Some(id) => {
                    self.session = Some(id.to_string());
                    out.push(Event::Session(id.to_string()));
                    self.become_ready(&mut out);
                }
                // A login made in the agent's own TUI still has to be picked up
                // with `authenticate`; then the session is asked for once more.
                None if !self.tried_auth && !self.auth_methods.is_empty() && wants_auth(error) => {
                    self.tried_auth = true;
                    let method = self.auth_methods[0].clone();
                    self.call("authenticate", json!({"methodId": method}), Pending::Authenticate);
                }
                None => out.push(Event::Failed(reason(error))),
            },
            Pending::Authenticate => {
                if failed {
                    out.push(Event::Failed(reason(error)));
                } else {
                    self.new_session();
                }
            }
            Pending::Prompt => {
                self.in_flight = false;
                out.push(Event::Done {
                    error: failed.then(|| reason(error)),
                    cancelled: result["stopReason"] == "cancelled",
                });
                self.next();
            }
        }
        out
    }

    fn update(&mut self, u: &Value) -> Vec<Event> {
        match u["sessionUpdate"].as_str().unwrap_or("") {
            "agent_message_chunk" => {
                let c = &u["content"];
                let t = c["text"].as_str().unwrap_or("");
                // Copilot's note of a file it changed, sent as reply text: not for reading out.
                let notice = t.trim().starts_with("Info: /") && !t.trim().contains(' ');
                if c["type"] == "text" && !notice {
                    vec![Event::Text(t.to_string())]
                } else {
                    vec![]
                }
            }
            "tool_call" => vec![Event::Working, Event::Step(act(u))],
            "agent_thought_chunk" => vec![Event::Thinking],
            _ => vec![],
        }
    }
}

impl Link for Acp {
    fn arguments(&self, _resume: Option<&str>) -> Vec<String> {
        self.serve.clone()
    }

    fn open(&mut self, resume: Option<&str>, cwd: &str) -> Vec<Event> {
        self.cwd = cwd.to_string();
        self.session = resume.map(str::to_string);
        self.primed = resume.is_some();
        let capabilities = json!({
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false,
            // Forms, for questions: without them some agents ask as a permission request.
            "elicitation": {"form": {}}
        });
        self.call("initialize", json!({"protocolVersion": 1, "clientCapabilities": capabilities}), Pending::Initialize);
        vec![]
    }

    fn prompt(&mut self, text: &str) {
        self.queue.push_back(text.to_string());
        self.next();
    }

    fn cancel(&mut self) {
        self.queue.clear();
        if let Some(id) = self.session.clone() {
            self.send(json!({"method": "session/cancel", "params": {"sessionId": id}}));
        }
    }

    fn answer(&mut self, id: &str, allow: bool, always: bool, _instead: Option<&str>) {
        let Some((rpc_id, options)) = self.asked.remove(id) else { return };
        let want: &[&str] = match (allow, always) {
            (true, true) => &["allow_always", "allow_once"],
            (true, false) => &["allow_once", "allow_always"],
            _ => &["reject_once", "reject_always"],
        };
        let pick = want.iter().find_map(|k| options.iter().find(|o| o["kind"] == *k));
        let outcome = match pick {
            Some(o) => json!({"outcome": "selected", "optionId": o["optionId"]}),
            None => json!({"outcome": "cancelled"}),
        };
        self.reply(rpc_id, json!({"outcome": outcome}));
    }

    fn answer_question(&mut self, id: &str, answer: &str) {
        let Some((rpc_id, field, many)) = self.questions.remove(id) else { return };
        if answer.is_empty() {
            self.reply(rpc_id, json!({"action": "decline"}));
            return;
        }
        let value = if many { json!([answer]) } else { json!(answer) };
        self.reply(rpc_id, json!({"action": "accept", "content": {field: value}}));
    }

    fn read(&mut self, line: &str) -> Vec<Event> {
        let Ok(ev) = serde_json::from_str::<Value>(line) else { return vec![] };
        let method = ev["method"].as_str().unwrap_or("");
        let id = ev.get("id").cloned();
        // An answer to one of our calls.
        if method.is_empty() {
            if let Some(pending) = id.as_ref().and_then(Value::as_i64).and_then(|i| self.calls.remove(&i)) {
                return self.answered(pending, &ev["result"], &ev["error"]);
            }
            return vec![];
        }
        let params = &ev["params"];
        match (method, id) {
            ("session/update", _) => {
                if self.loading {
                    vec![]
                } else {
                    self.update(&params["update"])
                }
            }
            ("session/request_permission", Some(rpc_id)) => {
                let call = &params["toolCall"];
                let ours = format!("p{rpc_id}");
                let raw = &call["rawInput"];
                let key = if call["kind"] == "execute" && raw.get("command").is_some() {
                    always_key("Bash", raw["command"].as_str().unwrap_or(""))
                } else {
                    format!("acp:{}", call["kind"].as_str().unwrap_or(""))
                };
                let detail = first_string(raw, &["command", "path", "file_path", "url"]);
                self.asked.insert(ours.clone(), (rpc_id, params["options"].as_array().cloned().unwrap_or_default()));
                vec![Event::Ask { id: ours, act: act(call), detail, key }]
            }
            ("elicitation/create", Some(rpc_id)) => {
                // One answer in words is something to say; anything else is declined.
                let fields = params["requestedSchema"]["properties"].as_object().cloned().unwrap_or_default();
                let mode_ok = params["mode"].as_str().unwrap_or("form") == "form";
                let single = (fields.len() == 1).then(|| fields.iter().next().map(|(k, v)| (k.clone(), v.clone()))).flatten();
                let Some((name, field)) = single.filter(|_| mode_ok) else {
                    self.reply(rpc_id, json!({"action": "decline"}));
                    return vec![];
                };
                let many = field["type"] == "array";
                let value = if many { field["items"].clone() } else { field };
                if value["type"] != "string" {
                    self.reply(rpc_id, json!({"action": "decline"}));
                    return vec![];
                }
                let choices = value["enum"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let ours = format!("q{rpc_id}");
                self.questions.insert(ours.clone(), (rpc_id, name, many));
                vec![Event::Question { id: ours, text: params["message"].as_str().unwrap_or("").to_string(), choices }]
            }
            (_, Some(rpc_id)) => {
                // fs/… and terminal/…, which we said we do not offer.
                let message = format!("{method} is not supported");
                self.send(json!({"id": rpc_id, "error": {"code": -32601, "message": message}}));
                vec![]
            }
            _ => vec![],
        }
    }

    fn drain(&mut self) -> Vec<String> {
        std::mem::take(&mut self.outbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last(a: &mut Acp) -> Value {
        serde_json::from_str(a.drain().last().unwrap()).unwrap()
    }

    #[test]
    fn opens_prompts_and_asks() {
        let mut a = Acp::new(vec!["acp".into()]);
        a.open(None, "/home/x");
        assert_eq!(last(&mut a)["method"], "initialize");
        a.prompt("hello"); // waits until the session exists
        a.read(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#);
        assert_eq!(last(&mut a)["method"], "session/new");
        let ev = a.read(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"abc"}}"#);
        assert_eq!(ev, vec![Event::Session("abc".into()), Event::Ready]);
        let sent = last(&mut a);
        assert_eq!(sent["method"], "session/prompt");
        let said = sent["params"]["prompt"][0]["text"].as_str().unwrap();
        assert!(said.starts_with("(You are in a voice conversation"));
        assert!(said.ends_with("hello"));

        let ev = a.read(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"abc","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hi!"}}}}"#);
        assert_eq!(ev, vec![Event::Text("Hi!".into())]);

        let ev = a.read(r#"{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{"sessionId":"abc","toolCall":{"kind":"execute","title":"$ npm test","rawInput":{"command":"npm test"}},"options":[{"optionId":"y","kind":"allow_once"},{"optionId":"Y","kind":"allow_always"},{"optionId":"n","kind":"reject_once"}]}}"#);
        let Event::Ask { id, act, .. } = &ev[0] else { panic!() };
        assert_eq!(act.verb, "run");
        a.answer(&id.clone(), true, true, None);
        let reply = last(&mut a);
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["result"]["outcome"]["optionId"], "Y");

        let ev = a.read(r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#);
        assert_eq!(ev, vec![Event::Done { error: None, cancelled: false }]);
    }

    #[test]
    fn refuses_client_methods() {
        let mut a = Acp::new(vec![]);
        a.read(r#"{"jsonrpc":"2.0","id":9,"method":"fs/read_text_file","params":{}}"#);
        assert_eq!(last(&mut a)["error"]["code"], -32601);
    }

    #[test]
    fn falls_back_to_a_new_session() {
        let mut a = Acp::new(vec![]);
        a.open(Some("old"), "/x");
        a.drain();
        a.read(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        assert_eq!(last(&mut a)["method"], "session/load");
        let ev = a.read(r#"{"jsonrpc":"2.0","id":2,"error":{"code":-1,"message":"gone"}}"#);
        assert_eq!(ev, vec![Event::ResumeFailed]);
        assert_eq!(last(&mut a)["method"], "session/new");
    }

    #[test]
    fn authenticates_when_a_session_needs_a_login() {
        let mut a = Acp::new(vec![]);
        a.open(None, "/x");
        a.drain();
        a.read(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"authMethods":[{"id":"cursor_login"}]}}"#);
        assert_eq!(last(&mut a)["method"], "session/new");
        let ev = a.read(r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"Authentication required"}}"#);
        assert!(ev.is_empty());
        let auth = last(&mut a);
        assert_eq!(auth["method"], "authenticate");
        assert_eq!(auth["params"]["methodId"], "cursor_login");
        a.read(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#);
        assert_eq!(last(&mut a)["method"], "session/new");
        // A second refusal is final, with the agent's own words.
        let ev = a.read(r#"{"jsonrpc":"2.0","id":4,"error":{"code":-32000,"message":"Authentication required","data":{"message":"Please run 'agent login' first."}}}"#);
        assert_eq!(ev, vec![Event::Failed("Please run 'agent login' first.".into())]);
    }

    #[test]
    fn labels_tool_calls() {
        assert_eq!(act(&json!({"kind":"read","title":"read"})), Act::new("read", ""));
        assert_eq!(act(&json!({"kind":"edit","locations":[{"path":"/a/b.rs"}]})), Act::new("edit", "b.rs"));
        assert_eq!(act(&json!({"kind":"other","title":"read_file","rawInput":{"path":"/w/hello.txt"}})), Act::new("read", "hello.txt"));
        assert_eq!(act(&json!({"title":"run_terminal_command","rawInput":{"command":"ls -la"}})), Act::new("run", "ls -la"));
    }
}
