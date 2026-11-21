//! Text between the agent and the voice: sentences for speaking, Markdown made
//! sayable, and what a spoken answer to "Allow it?" means.

/// Cuts a streamed reply into sentences, so the first one is spoken while the
/// agent still writes the rest. A code block is never read out: one short note
/// stands in for it. Each sentence comes with the byte offset in the reply
/// where it ends, so the chat can show the text up to the sentence it hears.
#[derive(Default)]
pub struct Splitter {
    buffer: String,
    /// Bytes of the reply already cut off the front of `buffer`.
    consumed: usize,
    in_code: bool,
}

pub const CODE_NOTE: &str = "(The code is on the screen.)";

impl Splitter {
    /// Adds more of the reply; returns the sentences it completes.
    pub fn feed(&mut self, text: &str) -> Vec<(String, usize)> {
        self.buffer.push_str(text);
        let mut out = Vec::new();
        loop {
            if self.in_code {
                let Some(close) = self.buffer.find("```") else { return out };
                let cut = match self.buffer[close..].find('\n') {
                    Some(nl) => close + nl + 1,
                    None => self.buffer.len(),
                };
                self.cut(cut);
                self.in_code = false;
                continue;
            }
            let fence = self.buffer.find("```");
            let end = sentence_end(&self.buffer);
            if let Some(f) = fence {
                if end.is_none_or(|c| f < c) {
                    let before = self.buffer[..f].to_string();
                    push_trimmed(&mut out, &before, self.consumed + f);
                    // The code shows when the note for it is heard.
                    self.cut(f + 3);
                    out.push((CODE_NOTE.to_string(), self.consumed));
                    self.in_code = true;
                    continue;
                }
            }
            let Some(end) = end else { return out };
            let sentence = self.buffer[..end].to_string();
            self.cut(end);
            push_trimmed(&mut out, &sentence, self.consumed);
        }
    }

    fn cut(&mut self, n: usize) {
        self.buffer.drain(..n);
        self.consumed += n;
    }

    /// The reply is done: returns what is left.
    pub fn flush(&mut self) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        if !self.in_code {
            let rest = std::mem::take(&mut self.buffer);
            push_trimmed(&mut out, &rest, self.consumed + rest.len());
        }
        self.reset();
        out
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.consumed = 0;
        self.in_code = false;
    }
}

fn push_trimmed(out: &mut Vec<(String, usize)>, s: &str, end: usize) {
    let t = s.trim();
    if !t.is_empty() {
        out.push((t.to_string(), end));
    }
}

/// The byte index just past the first sentence end: `.`, `!`, `?` or `…`
/// (with closing quotes or brackets) before whitespace, or a line break.
fn sentence_end(s: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (pos, c) = chars[i];
        if c == '\n' {
            return Some(pos + 1);
        }
        if matches!(c, '.' | '!' | '?' | '…') {
            let mut j = i;
            while j + 1 < chars.len() && matches!(chars[j + 1].1, '.' | '!' | '?' | '…' | '"' | '\'' | ')' | ']' | '”' | '’') {
                j += 1;
            }
            if j + 1 < chars.len() && chars[j + 1].1.is_whitespace() {
                return Some(chars[j].0 + chars[j].1.len_utf8());
            }
            i = j;
        }
        i += 1;
    }
    None
}

/// What text-to-speech should say for a line of Markdown: no symbols, a link
/// as its text, a bare URL as "a link". Empty when nothing sayable is left.
pub fn speakable(markdown: &str) -> String {
    let line = markdown.trim();
    // Tables and rules are for the eyes.
    if line.starts_with('|') && line.ends_with('|') {
        return String::new();
    }
    if line.len() >= 3 && line.chars().all(|c| matches!(c, '-' | '*' | '_' | ' ')) {
        return String::new();
    }
    let mut s = line.to_string();
    // Headings, quotes and list markers.
    s = s.trim_start_matches('#').trim_start().to_string();
    s = s.trim_start_matches('>').trim_start().to_string();
    for marker in ["- ", "* ", "+ ", "• "] {
        if let Some(rest) = s.strip_prefix(marker) {
            s = rest.to_string();
        }
    }
    if let Some(dot) = s.find(['.', ')']) {
        if dot > 0 && dot <= 3 && s[..dot].chars().all(|c| c.is_ascii_digit()) && s[dot + 1..].starts_with(' ') {
            s = s[dot + 2..].to_string();
        }
    }
    s = replace_links(&s);
    s = s
        .split_whitespace()
        .map(|w| if w.starts_with("http://") || w.starts_with("https://") { "a link" } else { w })
        .collect::<Vec<_>>()
        .join(" ");
    for mark in ["**", "__", "~~", "`", "*"] {
        s = s.replace(mark, "");
    }
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().any(char::is_alphanumeric) {
        s
    } else {
        String::new()
    }
}

/// `[text](url)` becomes `text`.
fn replace_links(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let Some(close) = after.find("](") else { break };
        let Some(end) = after[close + 2..].find(')') else { break };
        out.push_str(&rest[..open]);
        out.push_str(&after[..close]);
        rest = &after[close + 2 + end + 1..];
    }
    out.push_str(rest);
    out
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Answer {
    Allow,
    Always,
    Deny,
    /// Something else to do: passed on to the agent as the reason for "no".
    Instead,
}

/// What a spoken or typed answer to "Allow it?" means.
pub fn parse_answer(said: &str) -> Answer {
    let s: String = said
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' || c.is_whitespace() { c } else { ' ' })
        .collect();
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.is_empty() {
        return Answer::Deny;
    }
    let s = words.join(" ");
    let is = |phrases: &[&str]| phrases.iter().any(|p| s == *p || s.starts_with(&format!("{p} ")));
    if is(&["always", "yes always", "allow always", "always allow", "yeah always", "a"]) {
        return Answer::Always;
    }
    if is(&["no", "n", "nope", "nah", "deny", "don't", "do not", "stop", "cancel", "never"]) {
        return if words.len() <= 3 { Answer::Deny } else { Answer::Instead };
    }
    if is(&[
        "yes", "y", "yeah", "yep", "yup", "sure", "ok", "okay", "allow", "allow it", "go", "go ahead", "do it",
        "please do", "fine", "alright", "all right", "approve", "approved", "proceed", "continue",
    ]) {
        return if words.len() <= 4 { Answer::Allow } else { Answer::Instead };
    }
    Answer::Instead
}

/// The system prompt that makes an agent talk like a person.
pub const VOICE_PROMPT: &str = "You are in a voice conversation with the user on their Omarchy Linux desktop. They \
speak, and speech-to-text writes their words, so expect misheard words, most of all names. Your replies are read \
aloud by text-to-speech, so every word costs the user time. Talk like a person in a conversation: answer in one to \
three short sentences, and say more only when they ask. Answer, then stop: no preamble, no repeating the question, \
no offers of more help. Never use markdown, lists, headings, code blocks, tables, emoji or URLs; say paths, numbers \
and symbols the way a person says them out loud. For a task with tools, say in a few words what you will do, do the \
work without talking about each step, and at the end give the outcome in one or two sentences. When a word looks \
misheard, take the most likely meaning and go on; ask only when the meanings are very different. When the user says \
no to an action you asked about, do not do it another way: say what you would do instead, or ask. Answer in the \
language the user speaks.";

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(v: Vec<(String, usize)>) -> Vec<String> {
        v.into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn cuts_sentences_as_they_stream() {
        let mut s = Splitter::default();
        assert_eq!(s.feed("Hello there. How a"), vec![("Hello there.".to_string(), 12)]);
        assert_eq!(s.feed("re you? I am"), vec![("How are you?".to_string(), 25)]);
        assert_eq!(s.flush(), vec![("I am".to_string(), 30)]);
    }

    #[test]
    fn keeps_decimals_and_paths() {
        let mut s = Splitter::default();
        assert_eq!(texts(s.feed("Version 3.5 is in ~/.config now. ")), vec!["Version 3.5 is in ~/.config now."]);
        assert_eq!(texts(s.feed("Wait... \"Really?\" Yes")), vec!["Wait...", "\"Really?\""]);
    }

    #[test]
    fn replaces_code_blocks() {
        let mut s = Splitter::default();
        let reply = "Run this:\n```bash\nls -la\n```\nDone.\n";
        let mut got = s.feed(&reply[..20]);
        got.extend(s.feed(&reply[20..]));
        assert_eq!(texts(got.clone()), vec!["Run this:", CODE_NOTE, "Done."]);
        // The offsets reveal the reply in order and end at its end.
        assert_eq!(got[0].1, 10);
        assert_eq!(&reply[..got[2].1], "Run this:\n```bash\nls -la\n```\nDone.");
        assert!(s.flush().is_empty());
    }

    #[test]
    fn makes_markdown_sayable() {
        assert_eq!(speakable("**Bold** and `code` and [the docs](https://x.y)"), "Bold and code and the docs");
        assert_eq!(speakable("- a list item"), "a list item");
        assert_eq!(speakable("2. second step"), "second step");
        assert_eq!(speakable("## Heading"), "Heading");
        assert_eq!(speakable("See https://example.com/page now"), "See a link now");
        assert_eq!(speakable("| a | b |"), "");
        assert_eq!(speakable("---"), "");
    }

    #[test]
    fn understands_answers() {
        assert_eq!(parse_answer("Yes."), Answer::Allow);
        assert_eq!(parse_answer("y"), Answer::Allow);
        assert_eq!(parse_answer("yeah go ahead"), Answer::Allow);
        assert_eq!(parse_answer("Always"), Answer::Always);
        assert_eq!(parse_answer("yes, always"), Answer::Always);
        assert_eq!(parse_answer("No."), Answer::Deny);
        assert_eq!(parse_answer("no thanks"), Answer::Deny);
        assert_eq!(parse_answer("no, use the other folder instead please"), Answer::Instead);
        assert_eq!(parse_answer("put it in the docs folder"), Answer::Instead);
    }
}
