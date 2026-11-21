//! Claude Code over stream-json both ways, approvals through
//! `--permission-prompt-tool stdio`, as the Agent SDK runs it.

use std::collections::HashMap;

use serde_json::{json, Value};

use super::{always_key, file_name, first_string, short, Act, Event, Link};
use crate::speech::VOICE_PROMPT;

#[derive(Default)]
pub struct Claude {
    session: Option<String>,
    asked: HashMap<String, Value>,
    questions: HashMap<String, Value>,
    outbox: Vec<String>,
    requests: u32,
}

/// A Claude Code tool call as an `Act`.
pub fn act(tool: &str, input: &Value) -> Act {
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match tool {
        "Bash" => {
            let d = s("description");
            Act::new("run", short(if d.is_empty() { &input["command"].as_str().unwrap_or("") } else { &d }, 60))
        }
        "Read" => Act::new("read", file_name(&s("file_path"))),
        "Write" => Act::new("write", file_name(&s("file_path"))),
        "Edit" | "MultiEdit" | "NotebookEdit" => Act::new("edit", file_name(&first_string(input, &["file_path", "notebook_path"]))),
        "Glob" | "Grep" => Act::new("search", short(&s("pattern"), 50)),
        "WebSearch" => Act::new("web", short(&s("query"), 50)),
        "WebFetch" => {
            let url = s("url");
            let host = url.split("://").nth(1).unwrap_or(&url).split('/').next().unwrap_or("").to_string();
            Act::new("read", host)
        }
        "Task" | "Agent" => Act::new("subtask", short(&s("description"), 50)),
        "TodoWrite" | "ExitPlanMode" => Act::new("plan", ""),
        other => {
            // mcp__server__tool -> tool
            let name = other.strip_prefix("mcp__").and_then(|r| r.split("__").nth(1)).unwrap_or(other);
            Act::new("tool", name)
        }
    }
}

impl Claude {
    fn send(&mut self, v: Value) {
        self.outbox.push(v.to_string());
    }

    fn respond(&mut self, id: &str, response: Value) {
        self.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": response}}));
    }
}

impl Link for Claude {
    fn arguments(&self, resume: Option<&str>) -> Vec<String> {
        let mut a: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--append-system-prompt",
            VOICE_PROMPT,
            // Looking things up needs no OK; anything that changes something asks.
            "--allowedTools",
            "Read,Glob,Grep,WebSearch,WebFetch,TodoWrite",
            "--permission-prompt-tool",
            "stdio",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(r) = resume {
            a.push("--resume".into());
            a.push(r.into());
        }
        a
    }

    fn open(&mut self, resume: Option<&str>, _cwd: &str) -> Vec<Event> {
        self.session = resume.map(str::to_string);
        self.send(json!({"type": "control_request", "request_id": "init", "request": {"subtype": "initialize"}}));
        // It says nothing until the first prompt; it is ready once it runs.
        vec![Event::Ready]
    }

    fn prompt(&mut self, text: &str) {
        self.send(json!({"type": "user", "message": {"role": "user", "content": text}}));
    }

    fn cancel(&mut self) {
        self.requests += 1;
        let id = format!("stop{}", self.requests);
        self.send(json!({"type": "control_request", "request_id": id, "request": {"subtype": "interrupt"}}));
    }

    fn answer(&mut self, id: &str, allow: bool, _always: bool, instead: Option<&str>) {
        let input = self.asked.remove(id).unwrap_or_else(|| json!({}));
        let response = if allow {
            json!({"behavior": "allow", "updatedInput": input})
        } else {
            let message = match instead {
                Some(t) => format!("The user said no, and said: {t}"),
                None => "The user said no to this.".into(),
            };
            json!({"behavior": "deny", "message": message})
        };
        self.respond(id, response);
    }

    fn answer_question(&mut self, id: &str, answer: &str) {
        let mut input = self.questions.remove(id).unwrap_or_else(|| json!({}));
        if answer.is_empty() {
            self.respond(id, json!({"behavior": "deny", "message": "The user skipped the question."}));
            return;
        }
        // Every question in the call gets the one spoken answer.
        let mut answers = serde_json::Map::new();
        for q in input["questions"].as_array().cloned().unwrap_or_default() {
            if let Some(text) = q["question"].as_str() {
                answers.insert(text.to_string(), json!(answer));
            }
        }
        input["answers"] = Value::Object(answers);
        self.respond(id, json!({"behavior": "allow", "updatedInput": input}));
    }

    fn read(&mut self, line: &str) -> Vec<Event> {
        let Ok(ev) = serde_json::from_str::<Value>(line) else { return vec![] };
        let mut out = Vec::new();
        if let Some(sid) = ev["session_id"].as_str().filter(|s| !s.is_empty()) {
            if self.session.as_deref() != Some(sid) {
                self.session = Some(sid.to_string());
                out.push(Event::Session(sid.to_string()));
            }
        }
        // A subtask's own messages are its business, not the conversation's.
        let sub = !ev["parent_tool_use_id"].is_null();
        match ev["type"].as_str().unwrap_or("") {
            "system" => {
                if ev["subtype"] == "api_retry" {
                    out.push(Event::Retrying(ev["error"].as_str().unwrap_or("network").to_string()));
                }
            }
            "stream_event" if !sub => {
                let e = &ev["event"];
                match e["type"].as_str().unwrap_or("") {
                    "content_block_delta" => match e["delta"]["type"].as_str() {
                        Some("text_delta") => out.push(Event::Text(e["delta"]["text"].as_str().unwrap_or("").to_string())),
                        Some("thinking_delta") => out.push(Event::Thinking),
                        _ => {}
                    },
                    "content_block_start" if e["content_block"]["type"] == "tool_use" => out.push(Event::Working),
                    "message_start" => out.push(Event::Break),
                    _ => {}
                }
            }
            "assistant" => {
                for block in ev["message"]["content"].as_array().cloned().unwrap_or_default() {
                    if block["type"] != "tool_use" || block["name"] == "AskUserQuestion" {
                        continue;
                    }
                    let a = act(block["name"].as_str().unwrap_or(""), &block["input"]);
                    out.push(Event::Step(if sub { Act::new("subtask", a.what) } else { a }));
                }
            }
            "result" => {
                if ev["is_error"].as_bool().unwrap_or(false) {
                    let r = ev["result"].as_str().unwrap_or("");
                    let error = if r.is_empty() { ev["subtype"].as_str().unwrap_or("error") } else { r };
                    out.push(Event::Done { error: Some(error.to_string()), cancelled: false });
                } else {
                    out.push(Event::Done { error: None, cancelled: ev["subtype"] == "error_during_execution" });
                }
            }
            "control_request" => {
                let id = ev["request_id"].as_str().unwrap_or("").to_string();
                let req = &ev["request"];
                if req["subtype"] != "can_use_tool" {
                    self.send(json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": "not supported"}}));
                    return out;
                }
                let name = req["tool_name"].as_str().unwrap_or("");
                let input = req["input"].clone();
                if name == "AskUserQuestion" {
                    let q = &input["questions"][0];
                    let choices = q["options"]
                        .as_array()
                        .map(|o| o.iter().filter_map(|c| c["label"].as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    let text = q["question"].as_str().unwrap_or("").to_string();
                    self.questions.insert(id.clone(), input);
                    out.push(Event::Question { id, text, choices });
                    return out;
                }
                let detail = first_string(&input, &["command", "file_path", "url", "pattern", "plan"]);
                let key = always_key(name, input["command"].as_str().unwrap_or(""));
                let a = act(name, &input);
                self.asked.insert(id.clone(), input);
                out.push(Event::Ask { id, act: a, detail, key });
            }
            "control_response" => {
                let r = &ev["response"];
                if r["request_id"] == "init" && r["subtype"] == "error" {
                    out.push(Event::Failed(r["error"].as_str().unwrap_or("").to_string()));
                }
            }
            _ => {}
        }
        out
    }

    fn drain(&mut self) -> Vec<String> {
        std::mem::take(&mut self.outbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn streams_text_steps_and_result() {
        let mut c = Claude::default();
        assert_eq!(c.open(None, "/tmp"), vec![Event::Ready]);
        assert_eq!(parse(&c.drain()[0])["request"]["subtype"], "initialize");
        c.prompt("hi");
        assert_eq!(parse(&c.drain()[0])["message"]["content"], "hi");

        let mut ev = c.read(r#"{"type":"system","subtype":"init","session_id":"s1"}"#);
        ev.extend(c.read(r#"{"type":"stream_event","session_id":"s1","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}}"#));
        ev.extend(c.read(r#"{"type":"assistant","session_id":"s1","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls","description":"List files"}}]}}"#));
        ev.extend(c.read(r#"{"type":"result","subtype":"success","is_error":false,"session_id":"s1","result":"Hello"}"#));
        assert_eq!(
            ev,
            vec![
                Event::Session("s1".into()),
                Event::Text("Hello".into()),
                Event::Step(Act::new("run", "List files")),
                Event::Done { error: None, cancelled: false },
            ]
        );
    }

    #[test]
    fn asks_and_gets_answers() {
        let mut c = Claude::default();
        let ev = c.read(r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"git push"}}}"#);
        let Event::Ask { id, detail, key, .. } = &ev[0] else { panic!("{ev:?}") };
        assert_eq!((id.as_str(), detail.as_str(), key.as_str()), ("r1", "git push", "Bash:git"));
        c.answer("r1", true, false, None);
        let r = parse(&c.drain()[0]);
        assert_eq!(r["response"]["request_id"], "r1");
        assert_eq!(r["response"]["response"]["behavior"], "allow");
        assert_eq!(r["response"]["response"]["updatedInput"]["command"], "git push");

        c.read(r#"{"type":"control_request","request_id":"r2","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"/a/b.txt"}}}"#);
        c.answer("r2", false, false, Some("write it to c.txt"));
        let r = parse(&c.drain()[0]);
        assert_eq!(r["response"]["response"]["behavior"], "deny");
        assert!(r["response"]["response"]["message"].as_str().unwrap().contains("c.txt"));
    }

    #[test]
    fn questions_go_back_as_answers() {
        let mut c = Claude::default();
        let ev = c.read(r#"{"type":"control_request","request_id":"q1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which one?","options":[{"label":"A"},{"label":"B"}]}]}}}"#);
        assert_eq!(ev, vec![Event::Question { id: "q1".into(), text: "Which one?".into(), choices: vec!["A".into(), "B".into()] }]);
        c.answer_question("q1", "B");
        let r = parse(&c.drain()[0]);
        assert_eq!(r["response"]["response"]["updatedInput"]["answers"]["Which one?"], "B");
    }

    #[test]
    fn resumes_a_session() {
        let c = Claude::default();
        let a = c.arguments(Some("abc"));
        assert_eq!(&a[a.len() - 2..], &["--resume".to_string(), "abc".to_string()]);
    }
}
