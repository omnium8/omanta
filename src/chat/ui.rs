//! Drawing. Rounded boxes like Omarchy's TUIs, the theme accent for what
//! matters, and the terminal's own colors for everything else.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::settings::ROWS;
use super::{App, Kind, Overlay};

/// A stick that turns, one step at a time.
const STICK: [char; 4] = ['|', '/', '-', '\\'];

/// The turning wheel: a stick with a two-step trail, newest on the right.
/// Each of the three has its own theme color, and the colors move around
/// the theme's palette. It turns backward while omanta thinks and forward
/// while the voice speaks.
fn wheel(app: &App, backward: bool) -> Vec<Span<'static>> {
    let t = app.born.elapsed().as_secs_f64();
    let step = (t * 8.0) as usize;
    (0..3)
        .map(|i| {
            let k = step + i;
            let c = if backward { STICK[(4 - k % 4) % 4] } else { STICK[k % 4] };
            let color = app.theme.spectrum_color(t * 0.12 + i as f64 * 0.1, [0.4, 0.7, 1.0][i]);
            Span::styled(c.to_string(), Style::new().fg(color).bold())
        })
        .collect()
}

/// Dense to faint, without the blank: the trail is never empty.
const TRAIL: &[u8] = b".:-=+*#%@";

/// The walking wheel, `room` cells wide: the forward wheel at the voice's
/// progress, and behind it a rippling trail that paints the theme's colors
/// along the row, brighter near the wheel.
fn walk_line(app: &App, room: usize) -> Line<'static> {
    let t = app.born.elapsed().as_secs_f64();
    let track = room.saturating_sub(2).max(4);
    let head = ((app.walk.clamp(0.0, 1.0) * (track - 3) as f64).round() as usize).min(track - 3);
    let along = |x: f64| x / track as f64 + t * 0.04;
    let mut spans = vec![Span::raw(" ")];
    for x in 0..head {
        let near = (-((head - x) as f64) / 5.0).exp();
        let ripple = 0.3 + 0.22 * (x as f64 * 0.55 - t * 7.0).sin();
        let v = (ripple + 0.55 * near).clamp(0.0, 1.0);
        let c = TRAIL[((v * (TRAIL.len() - 1) as f64).round() as usize).min(TRAIL.len() - 1)] as char;
        spans.push(Span::styled(c.to_string(), Style::new().fg(app.theme.spectrum_color(along(x as f64), 0.3 + 0.65 * v))));
    }
    let step = (t * 8.0) as usize;
    for i in 0..3 {
        let c = STICK[(step + i) % 4];
        let color = app.theme.spectrum_color(along((head + i) as f64), [0.75, 0.9, 1.0][i]);
        spans.push(Span::styled(c.to_string(), Style::new().fg(color).bold()));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// Whether something on the screen moves and needs quick frames.
pub fn moving(app: &App) -> bool {
    matches!(app.state(), "listening" | "transcribing" | "thinking" | "working" | "starting" | "speaking")
}
/// ASCII characters from faint to dense.
const RAMP: &[u8] = b" .:-=+*#%@";

/// How loud the drawn voice is at column `x`, at `t` seconds: bursts like
/// syllables under a slower swell like words, scrolling from right to left.
/// Drawn, for when the live level from Voxtype does not come.
fn voice_level(x: f64, t: f64) -> f64 {
    let u = x + t * 14.0;
    let syllable = (u * 0.42).sin().abs();
    let word = 0.5 + 0.5 * (u * 0.09 + 1.7).sin();
    let phrase = 0.65 + 0.35 * (u * 0.023).sin();
    (syllable * word * phrase).powf(0.6)
}

/// A low line with a bump that sweeps across it: the speech turns into text.
fn sweep_level(x: f64, t: f64, width: f64) -> f64 {
    let head = (t * 22.0) % (width + 12.0) - 6.0;
    0.12 + 0.6 * (1.0 - (x - head).abs() / 6.0).max(0.0)
}

/// One cell of a waveform mirrored around the middle row: dense near the
/// middle and the peaks, faint at the edges. `dy` is the distance from the
/// middle row, 0 to 1. Returns the character and how bright it is.
fn wave_cell(level: f64, dy: f64) -> (char, f64) {
    let v = (level * 1.35 - dy * 0.95).clamp(0.0, 1.0);
    let v = if dy == 0.0 { v.max(0.12) } else { v };
    let i = ((v * (RAMP.len() - 1) as f64).round() as usize).min(RAMP.len() - 1);
    (RAMP[i] as char, v)
}

/// The waveform as lines of colored ASCII, `rows` high and `width` wide.
pub fn ascii_wave(app: &App, listening: bool, width: usize, rows: usize) -> Vec<Line<'static>> {
    let t = app.born.elapsed().as_secs_f64();
    let middle = (rows.max(1) - 1) as f64 / 2.0;
    // The live microphone level when Voxtype's audio bridge sends it: the
    // newest sound on the right. Otherwise a drawn voice.
    let live = if listening { app.levels.recent(width) } else { None };
    let (levels, shift): (Vec<f64>, f64) = match live {
        Some((steps, count)) => {
            let pad = width - steps.len();
            let levels = (0..width).map(|x| if x < pad { 0.0 } else { steps[x - pad] }).collect();
            (levels, count as f64)
        }
        None if listening => ((0..width).map(|x| voice_level(x as f64, t)).collect(), t * 14.0),
        None => ((0..width).map(|x| sweep_level(x as f64, t, width as f64)).collect(), t * 8.0),
    };
    // The colors move with the sound: once around the theme every 60 cells.
    (0..rows)
        .map(|r| {
            // With an even number of rows, the two middle rows are both the middle.
            let off = ((r as f64 - middle).abs() - if rows % 2 == 0 { 0.5 } else { 0.0 }).max(0.0);
            let dy = if rows <= 2 { 0.0 } else { off / ((rows - 1) / 2) as f64 / 1.5 };
            Line::from(
                levels
                    .iter()
                    .enumerate()
                    .map(|(x, &level)| {
                        let (c, v) = wave_cell(level, dy);
                        let color = app.theme.spectrum_color((x as f64 + shift) / 60.0, 0.25 + v * 0.75);
                        Span::styled(c.to_string(), Style::new().fg(color))
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// Whether the message line shows the waveform instead of the text.
fn animating(app: &App) -> bool {
    matches!(app.state(), "listening" | "transcribing") && app.input.is_empty()
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let card = card_height(app, area.width);
    let [conversation, cards, input] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(card), Constraint::Length(app.config.chat.wave_rows.clamp(1, 4) as u16 + 2)]).areas(area);

    draw_conversation(f, app, conversation);
    if card > 0 {
        draw_card(f, app, cards);
    }
    draw_input(f, app, input);

    match &app.overlay {
        Overlay::Help => draw_help(f, app, area),
        Overlay::Settings(_) => draw_settings(f, app, area),
        Overlay::None => {}
    }
}

fn boxed<'a>(app: &App) -> Block<'a> {
    Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(app.theme.muted))
}

fn status_span(app: &App) -> Vec<Span<'static>> {
    let accent = app.theme.accent;
    let muted = app.theme.muted;
    let state = app.state();
    let blink = (app.tick / 2) % 2 == 0;
    let mark: Vec<Span<'static>> = match state {
        "listening" => vec![Span::styled("~", Style::new().fg(accent))],
        "transcribing" | "thinking" | "working" | "starting" => wheel(app, true),
        "speaking" => wheel(app, false),
        "needs your OK" | "needs your answer" => vec![Span::styled("◆", Style::new().fg(if blink { accent } else { muted }))],
        _ => vec![Span::styled("○", Style::new().fg(muted))],
    };
    let mut text = match state {
        "listening" => format!("listening · {} to send", app.config.chat.talk_key),
        "speaking" => format!("speaking · {} to talk over it", app.config.chat.talk_key),
        "idle" if app.config.chat.input == "key" => format!("{} to talk", app.config.chat.talk_key),
        "idle" => "dictate or type, then enter".to_string(),
        other => other.to_string(),
    };
    if app.busy && app.seconds() > 0 {
        text.push_str(&format!(" · {}:{:02}", app.seconds() / 60, app.seconds() % 60));
    }
    let mut spans = vec![Span::raw(" ")];
    spans.extend(mark);
    spans.extend([
        Span::raw(" "),
        Span::styled(text, Style::new().fg(if state == "idle" { muted } else { accent })),
        Span::raw(" "),
    ]);
    spans
}

fn voice_span(app: &App) -> Span<'static> {
    let muted = Style::new().fg(app.theme.muted);
    if !app.config.chat.speak_replies {
        return Span::styled(" voice off ", muted);
    }
    let v = &app.voice;
    if !v.reachable {
        return Span::styled(" voice service off · ctrl+o ", Style::new().fg(app.theme.accent));
    }
    if !v.status.problem.is_empty() {
        let p: String = v.status.problem.chars().take(48).collect();
        return Span::styled(format!(" voice: {p} "), Style::new().fg(app.theme.accent));
    }
    match v.status.phase.as_str() {
        "ready" => Span::styled(" voice ready ", muted),
        "downloading" => Span::styled(" downloading the voice (340 MB)… ", muted),
        "installing" => Span::styled(" installing the voice… ", muted),
        _ => Span::styled(" loading the voice… ", muted),
    }
}

fn draw_conversation(f: &mut Frame, app: &App, area: Rect) {
    let title = Line::from(vec![Span::raw(" "), Span::styled("omanta", Style::new().fg(app.theme.accent).bold()), Span::raw(" ")]);
    let agent = Line::from(Span::styled(format!(" {} ", app.agent.name), Style::new().bold())).alignment(Alignment::Right);
    let mut block = boxed(app).title(title).title(agent);
    if app.state() == "speaking" {
        // The status line becomes a track the wheel walks along with the voice.
        let accent = Style::new().fg(app.theme.accent);
        let voice = voice_span(app);
        let long = format!(" speaking · {} to talk over it", app.config.chat.talk_key);
        let fits = |text: &str| (area.width as usize).saturating_sub(text.width() + voice.width() + 3) >= 24;
        let text = if fits(&long) { long } else { " speaking".to_string() };
        let room = (area.width as usize).saturating_sub(text.width() + voice.width() + 3);
        block = block
            .title_bottom(walk_line(app, room))
            .title_bottom(Line::from(vec![Span::styled(text, accent), voice]).alignment(Alignment::Right));
    } else {
        block = block
            .title_bottom(Line::from(status_span(app)))
            .title_bottom(Line::from(voice_span(app)).alignment(Alignment::Right));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);

    let width = inner.width.saturating_sub(2) as usize;
    let lines = conversation_lines(app, width.max(10));
    let height = inner.height as usize;
    if lines.is_empty() {
        let hint = if app.config.chat.input == "key" {
            format!("Press {} and talk to {}. Press it again to send.", app.config.chat.talk_key, app.agent.name)
        } else {
            format!("Dictate into the message line, or type, and talk to {}.", app.agent.name)
        };
        let mut text = vec![Line::raw(""); height / 2];
        text.push(Line::styled(hint, Style::new().fg(app.theme.muted)).alignment(Alignment::Center));
        if app.can_resume() {
            text.push(Line::styled("ctrl+r continues the last conversation", Style::new().fg(app.theme.muted)).alignment(Alignment::Center));
        }
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), inner);
        return;
    }
    // Anchor to the bottom, `scroll` lines up from the end.
    let max_scroll = lines.len().saturating_sub(height);
    let scroll = app.scroll.min(max_scroll);
    let start = lines.len().saturating_sub(height + scroll);
    let end = (start + height).min(lines.len());
    let shown: Vec<Line> = lines[start..end].to_vec();
    let pad = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    f.render_widget(Paragraph::new(shown), pad);
    if scroll > 0 {
        let more = Line::styled(format!(" ↓ {scroll} more "), Style::new().fg(app.theme.accent)).alignment(Alignment::Right);
        f.render_widget(Paragraph::new(more), Rect { y: inner.y + inner.height.saturating_sub(1), height: 1, ..inner });
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    textwrap::wrap(text, width.max(4)).into_iter().map(|c| c.into_owned()).collect()
}

/// Every message as styled, wrapped lines.
pub fn conversation_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let accent = app.theme.accent;
    let muted = app.theme.muted;
    let mut out: Vec<Line> = Vec::new();
    for (i, m) in app.messages.iter().enumerate() {
        match m.kind {
            Kind::User => {
                if i > 0 {
                    out.push(Line::raw(""));
                }
                for (j, l) in wrap(&m.text, width - 2).into_iter().enumerate() {
                    let mark = if j == 0 { "› " } else { "  " };
                    out.push(Line::from(vec![Span::styled(mark, Style::new().fg(accent).bold()), Span::styled(l, Style::new().bold())]));
                }
            }
            Kind::Agent => {
                out.push(Line::raw(""));
                let visible = match m.shown {
                    Some(n) => &m.text[..n.min(m.text.len())],
                    None => &m.text,
                };
                let visible = visible.trim_start();
                if visible.is_empty() {
                    // The voice is loading: the text comes with it.
                    let dots = ".".repeat(1 + (app.tick as usize / 2) % 3);
                    out.push(Line::styled(format!("thinking{dots}"), Style::new().fg(muted).add_modifier(Modifier::ITALIC)));
                    continue;
                }
                let mut code = false;
                for raw in visible.lines() {
                    if raw.trim_start().starts_with("```") {
                        code = !code;
                        continue;
                    }
                    if code {
                        for l in wrap(raw, width - 2) {
                            out.push(Line::from(vec![Span::styled("┃ ", Style::new().fg(muted)), Span::styled(l, Style::new().fg(Color::Cyan))]));
                        }
                        continue;
                    }
                    let heading = raw.trim_start().starts_with('#');
                    let text = if heading { raw.trim_start_matches('#').trim() } else { raw };
                    if text.trim().is_empty() {
                        out.push(Line::raw(""));
                        continue;
                    }
                    for l in wrap(text, width) {
                        let style = if heading { Style::new().bold() } else { Style::new() };
                        out.push(Line::styled(l, style));
                    }
                }
            }
            Kind::Step => {
                for l in wrap(&m.text, width - 4) {
                    out.push(Line::styled(format!("  · {l}"), Style::new().fg(muted)));
                }
            }
            Kind::Note => {
                for l in wrap(&m.text, width - 4) {
                    out.push(Line::styled(format!("  ─ {l}"), Style::new().fg(muted).add_modifier(Modifier::ITALIC)));
                }
            }
        }
    }
    out
}

fn card_height(app: &App, width: u16) -> u16 {
    let w = width.saturating_sub(6) as usize;
    if let Some(a) = &app.ask {
        let detail = if a.detail.is_empty() { 0 } else { wrap(&a.detail, w).len().min(6) };
        return 4 + detail as u16 + wrap(&a.act.to_do(), w).len() as u16 - 1;
    }
    if let Some(q) = &app.question {
        return 4 + wrap(&q.text, w).len() as u16 + q.choices.len().min(9) as u16 - 1;
    }
    if app.needs_login {
        return 3;
    }
    0
}

fn draw_card(f: &mut Frame, app: &App, area: Rect) {
    let accent = app.theme.accent;
    let muted = Style::new().fg(app.theme.muted);
    let key = |k: &'static str| Span::styled(k, Style::new().fg(accent).bold());
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent))
        .padding(Padding::horizontal(1));
    let mut lines: Vec<Line> = Vec::new();
    if let Some(a) = &app.ask {
        let block = block.title(Line::from(Span::styled(" needs your OK ", Style::new().fg(accent).bold())));
        lines.push(Line::from(vec![Span::styled(app.agent.name, Style::new().bold()), Span::raw(format!(" wants to {}", a.act.to_do()))]));
        if !a.detail.is_empty() {
            for l in wrap(&a.detail, area.width.saturating_sub(6) as usize).into_iter().take(6) {
                lines.push(Line::styled(l, Style::new().fg(Color::Cyan)));
            }
        }
        lines.push(Line::from(vec![
            key("alt+y"), Span::styled(" allow  ", muted),
            key("alt+a"), Span::styled(" always  ", muted),
            key("alt+n"), Span::styled(" deny  ", muted),
            Span::styled("· or say yes, no, or what to do instead", muted),
        ]));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
    } else if let Some(q) = &app.question {
        let block = block.title(Line::from(Span::styled(format!(" {} asks ", app.agent.name), Style::new().fg(accent).bold())));
        lines.push(Line::styled(q.text.clone(), Style::new().bold()));
        for (i, c) in q.choices.iter().take(9).enumerate() {
            lines.push(Line::from(vec![Span::styled(format!("alt+{} ", i + 1), Style::new().fg(accent)), Span::raw(c.clone())]));
        }
        lines.push(Line::from(vec![key("alt+s"), Span::styled(" skip  · or say or type your own answer", muted)]));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
    } else if app.needs_login {
        lines.push(Line::from(vec![
            Span::raw(format!("{} is not logged in. ", app.agent.name)),
            key("ctrl+l"),
            Span::raw(" opens it in a terminal to log in once."),
        ]));
        f.render_widget(Paragraph::new(lines).block(block), area);
    }
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    if animating(app) {
        return draw_listening(f, app, area, app.state() == "listening");
    }
    let title = if let Some(left) = app.send_countdown() {
        if app.config.chat.send_after == 1 {
            " sending… ".to_string()
        } else {
            format!(" sending in {} s · keep typing or press enter ", left.as_secs() + 1)
        }
    } else if app.ask.is_some() {
        " yes, no, or what to do instead ".to_string()
    } else if app.question.is_some() {
        " your answer ".to_string()
    } else {
        format!(" message {} ", app.agent.name)
    };
    let muted = Style::new().fg(app.theme.muted);
    let k = |s: String| Span::styled(s, Style::new().fg(app.theme.accent));
    // Auto mode: off, the agent asks before each action; on, it does
    // everything, and the words take every theme color, one per letter.
    let mut auto = vec![Span::raw(" "), k("tab".into())];
    if app.allow_all {
        let words = " >> fix everything";
        let n = words.chars().count() as f64;
        auto.extend(words.chars().enumerate().map(|(i, c)| {
            Span::styled(c.to_string(), Style::new().fg(app.theme.spectrum_color(i as f64 / n, 0.85)).bold())
        }));
        auto.push(Span::raw(" "));
    } else {
        auto.push(Span::styled(" approve everything ", muted));
    }
    let auto = Line::from(auto);
    // Omarchy's push-to-talk first. What does not fit beside the auto mode
    // is left off from the end, so nothing overlaps.
    let mut keys: Vec<(String, &str)> = Vec::new();
    if app.push_to_talk {
        keys.push(("right alt".into(), "push to talk"));
    }
    keys.extend([
        (app.config.chat.talk_key.clone(), "talk"),
        ("esc".into(), "stop"),
        ("ctrl+o".into(), "settings"),
        ("f1".into(), "keys"),
    ]);
    let room = (area.width as usize).saturating_sub(auto.width() + 4);
    let size = |keys: &[(String, &str)]| keys.iter().map(|(key, what)| key.width() + what.width() + 4).sum::<usize>() + 1;
    while keys.len() > 1 && size(&keys) > room {
        keys.pop();
    }
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, what)) in keys.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", muted));
        }
        spans.push(k(key));
        spans.push(Span::styled(format!(" {what}"), muted));
    }
    spans.push(Span::raw(" "));
    let hints = Line::from(spans);
    let block = boxed(app).title(Line::styled(title, muted)).title_bottom(auto).title_bottom(hints.alignment(Alignment::Right));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width.saturating_sub(4) as usize;
    let (rows, row, col) = app.input.view_rows(width, inner.height as usize);
    let lines: Vec<Line> = rows
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let mark = if i == 0 { "› " } else { "  " };
            Line::from(vec![Span::styled(mark, Style::new().fg(app.theme.accent)), Span::raw(text)])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), Rect { x: inner.x + 1, width: inner.width.saturating_sub(1), ..inner });
    if matches!(app.overlay, Overlay::None) {
        f.set_cursor_position((inner.x + 3 + col as u16, inner.y + row as u16));
    }
}

/// The message line while Voxtype records or transcribes: an animation in
/// place of Voxtype's own overlay.
fn draw_listening(f: &mut Frame, app: &App, area: Rect, listening: bool) {
    let muted = Style::new().fg(app.theme.muted);
    let accent = Style::new().fg(app.theme.accent);
    let own = app.stt.state() != crate::stt::State::Idle;
    let title = if !listening {
        Line::from(vec![Span::raw(" "), Span::styled("transcribing…", accent), Span::raw(" ")])
    } else {
        let mut spans = vec![
            Span::raw(" "),
            Span::styled("listening", accent),
        ];
        if own {
            spans.push(Span::styled(format!(" · {} to send", app.config.chat.talk_key), muted));
        }
        spans.push(Span::raw(" "));
        Line::from(spans)
    };
    let hints = Line::from(vec![Span::raw(" "), Span::styled("esc", accent), Span::styled(" cancel ", muted)]);
    let mut block = boxed(app).title(title);
    if own {
        block = block.title_bottom(hints.alignment(Alignment::Right));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width.saturating_sub(2) as usize;
    let lines = ascii_wave(app, listening, width, inner.height as usize);
    f.render_widget(Paragraph::new(lines), Rect { x: inner.x + 1, width: inner.width.saturating_sub(1), ..inner });
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2));
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let rows: Vec<(String, &str)> = vec![
        (app.config.chat.talk_key.clone(), "talk; press again to send"),
        ("enter".into(), "send what you typed or dictated"),
        ("tab".into(), "auto mode: allow every action without asking"),
        ("esc".into(), "stop the voice, the recording, and the agent"),
        ("alt+y / alt+a / alt+n".into(), "allow / always / deny an action"),
        ("alt+1…9 / alt+s".into(), "pick an answer / skip a question"),
        ("ctrl+n".into(), "new conversation"),
        ("ctrl+r".into(), "continue the last conversation"),
        ("ctrl+o".into(), "settings"),
        ("ctrl+l".into(), "log in to the agent (opens a terminal)"),
        ("↑ ↓ pgup pgdn".into(), "scroll"),
        ("ctrl+c / ctrl+q".into(), "quit"),
    ];
    let lines: Vec<Line> = rows
        .into_iter()
        .map(|(k, v)| Line::from(vec![Span::styled(format!("{k:>22}  "), Style::new().fg(app.theme.accent)), Span::raw(v)]))
        .collect();
    let rect = centered(area, 72, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    let block = boxed(app).title(Line::styled(" keys ", Style::new().fg(app.theme.accent).bold()));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn draw_settings(f: &mut Frame, app: &App, area: Rect) {
    let Overlay::Settings(view) = &app.overlay else { return };
    let accent = app.theme.accent;
    let muted = Style::new().fg(app.theme.muted);
    let rect = centered(area, 78, ROWS.len() as u16 + 5);
    f.render_widget(Clear, rect);
    let block = boxed(app)
        .title(Line::styled(" settings ", Style::new().fg(accent).bold()))
        .title_bottom(Line::styled(" ↑↓ choose · ←→ change · enter edit · esc close ", muted).alignment(Alignment::Right));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();
    let mut cursor = None;
    for (i, row) in ROWS.iter().enumerate() {
        let (label, value, _) = view.describe(*row, &app.config, &app.catalog, app.allow_all);
        let selected = i == view.selected;
        let label_style = if selected { Style::new().fg(accent).bold() } else { Style::new() };
        let mark = if selected { "▸ " } else { "  " };
        let value_span = if selected && view.editing.is_some() {
            let (text, col) = view.editing.as_ref().map(|e| e.view(40)).unwrap_or_default();
            cursor = Some((inner.x + 26 + col as u16, inner.y + i as u16));
            Span::styled(text, Style::new().underlined())
        } else if selected {
            Span::styled(format!("‹ {value} ›"), Style::new().fg(accent))
        } else {
            Span::raw(value)
        };
        lines.push(Line::from(vec![
            Span::styled(mark, Style::new().fg(accent)),
            Span::styled(format!("{label:<20}  "), label_style),
            value_span,
        ]));
    }
    lines.push(Line::raw(""));
    let (_, _, hint) = view.describe(view.row(), &app.config, &app.catalog, app.allow_all);
    if !view.error.is_empty() {
        lines.push(Line::styled(view.error.clone(), Style::new().fg(Color::Red)));
    } else {
        lines.push(Line::styled(hint.to_string(), muted));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner });
    if let Some(c) = cursor {
        f.set_cursor_position(c);
    }
}
