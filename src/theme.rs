//! Colors for the TUI. Text keeps the terminal's own colors, which already
//! follow the Omarchy theme. The accent and the muted gray come from the
//! current theme's colors.toml and follow a theme switch live.

use std::fs;
use std::time::SystemTime;

use ratatui::style::Color;

use crate::paths;

pub struct Theme {
    pub accent: Color,
    pub muted: Color,
    pub foreground: Color,
    /// The theme's background as "#rrggbb", for the see-through background.
    pub background: String,
    /// The theme's colors and their bright versions, in rainbow order.
    pub spectrum: Vec<(Color, Color)>,
    stamp: Option<SystemTime>,
}

fn hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_matches(['"', '\'']).strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn read(key: &str, text: &str) -> Option<Color> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| hex(v)).flatten()
    })
}

impl Theme {
    pub fn load() -> Theme {
        let mut t = Theme { accent: Color::Yellow, muted: Color::DarkGray, foreground: Color::White, background: String::new(), spectrum: Vec::new(), stamp: None };
        t.refresh();
        t
    }

    /// Re-reads the theme when Omarchy switched it. Cheap: one stat. True
    /// when the theme changed.
    pub fn refresh(&mut self) -> bool {
        let path = paths::omarchy_colors();
        let stamp = fs::metadata(&path).and_then(|m| m.modified()).ok();
        if stamp == self.stamp && stamp.is_some() {
            return false;
        }
        self.stamp = stamp;
        let text = fs::read_to_string(&path).unwrap_or_default();
        self.accent = read("accent", &text).unwrap_or(Color::Yellow);
        self.muted = read("muted", &text).or_else(|| read("dark_foreground", &text)).unwrap_or(Color::DarkGray);
        self.foreground = read("foreground", &text).unwrap_or(Color::White);
        self.background = match read("background", &text) {
            Some(Color::Rgb(r, g, b)) => format!("#{r:02x}{g:02x}{b:02x}"),
            _ => "#000000".into(),
        };
        self.spectrum = ["blue", "cyan", "green", "yellow", "orange", "red", "magenta"]
            .iter()
            .filter_map(|name| {
                let normal = read(name, &text)?;
                Some((normal, read(&format!("bright_{name}"), &text).unwrap_or(normal)))
            })
            .collect();
        true
    }

    /// A color for a drawing in the theme's colors. `pos` goes once around
    /// them from 0 to 1, and repeats. `level` 0 is muted, 0.5 the theme
    /// color, 1 its bright version. Without theme colors: the accent.
    pub fn spectrum_color(&self, pos: f64, level: f64) -> Color {
        let level = level.clamp(0.0, 1.0);
        let (normal, bright) = if self.spectrum.is_empty() {
            (self.accent, self.foreground)
        } else {
            let n = self.spectrum.len();
            let p = pos.rem_euclid(1.0) * n as f64;
            let (i, f) = (p as usize % n, p.fract());
            let (a, b) = (self.spectrum[i], self.spectrum[(i + 1) % n]);
            (mix(a.0, b.0, f), mix(a.1, b.1, f))
        };
        if level < 0.5 {
            mix(self.muted, normal, level / 0.5)
        } else {
            mix(normal, bright, (level - 0.5) / 0.5)
        }
    }
}

fn mix(a: Color, b: Color, t: f64) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ if t < 0.5 => a,
        _ => b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_colors() {
        let text = "mode = \"dark\"\naccent = \"#ff9f0a\"\nmuted = '#8e8e93'\n";
        assert_eq!(read("accent", text), Some(Color::Rgb(0xff, 0x9f, 0x0a)));
        assert_eq!(read("muted", text), Some(Color::Rgb(0x8e, 0x8e, 0x93)));
        assert_eq!(read("blue", text), None);
    }

    #[test]
    fn spectrum_goes_around_the_theme_colors() {
        let mut t = Theme::load();
        t.muted = Color::Rgb(0, 0, 0);
        t.spectrum = vec![(Color::Rgb(100, 0, 0), Color::Rgb(200, 0, 0)), (Color::Rgb(0, 100, 0), Color::Rgb(0, 200, 0))];
        assert_eq!(t.spectrum_color(0.0, 0.5), Color::Rgb(100, 0, 0));
        assert_eq!(t.spectrum_color(0.5, 1.0), Color::Rgb(0, 200, 0));
        assert_eq!(t.spectrum_color(1.25, 0.5), Color::Rgb(50, 50, 0));
        assert_eq!(t.spectrum_color(0.0, 0.0), Color::Rgb(0, 0, 0));
    }
}
