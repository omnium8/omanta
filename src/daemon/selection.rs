//! The selected text on Wayland: the primary selection first (a mouse
//! highlight lands there), then Ctrl+C through wtype or ydotool and the
//! clipboard, which is put back afterwards.

use std::io::Write;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use crate::paths::find_program;

fn wl_paste(primary: bool) -> Option<String> {
    find_program("wl-paste")?;
    let mut cmd = Command::new("wl-paste");
    cmd.arg("--no-newline").stderr(Stdio::null());
    if primary {
        cmd.arg("--primary");
    }
    let out = cmd.output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    (out.status.success() && !text.trim().is_empty()).then_some(text)
}

fn wl_copy(text: &str) {
    let Ok(mut child) = Command::new("wl-copy").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    else {
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    let _ = child.wait();
}

fn quiet(cmd: &mut Command) -> bool {
    cmd.stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn synthesize_copy() -> bool {
    if find_program("wtype").is_some() {
        // Release what the Hyprland chord may still hold before Ctrl+C.
        quiet(Command::new("wtype").args(["-m", "alt", "-m", "shift", "-m", "ctrl", "-m", "win"]));
        sleep(Duration::from_millis(40));
        return quiet(Command::new("wtype").args(["-M", "ctrl", "-k", "c", "-m", "ctrl"]));
    }
    if find_program("ydotool").is_some() {
        return quiet(Command::new("ydotool").args(["key", "29:1", "46:1", "46:0", "29:0"]));
    }
    false
}

pub fn selected_text() -> Option<String> {
    // Hyprland may still be releasing the modifier keys of the chord.
    sleep(Duration::from_millis(250));
    if let Some(t) = wl_paste(true) {
        return Some(t);
    }
    let previous = wl_paste(false).unwrap_or_default();
    let _ = quiet(Command::new("wl-copy").arg("--clear"));
    sleep(Duration::from_millis(60));
    let copied = if synthesize_copy() {
        sleep(Duration::from_millis(220));
        wl_paste(false)
    } else {
        None
    };
    if !previous.is_empty() {
        wl_copy(&previous);
    }
    copied
}
