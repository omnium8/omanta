//! Pause other media (MPRIS players) while omanta speaks, then resume what we
//! paused. Uses `busctl`, which systemd always provides, with a short timeout:
//! a player that does not answer must not hold up the voice.

use std::process::{Command, Stdio};

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const OBJECT: &str = "/org/mpris/MediaPlayer2";
const IFACE: &str = "org.mpris.MediaPlayer2.Player";

fn busctl(args: &[&str]) -> Option<String> {
    let out = Command::new("busctl").args(["--user", "--timeout=1"]).args(args).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

fn players() -> Vec<String> {
    let Some(list) = busctl(&["list", "--no-pager", "--no-legend"]) else { return vec![] };
    let mut names: Vec<String> = list
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|n| n.starts_with(PREFIX))
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Pauses every player that is playing; returns them, to resume later.
/// Players the user paused themselves are left alone.
pub fn pause_playing() -> Vec<String> {
    players()
        .into_iter()
        .filter(|name| {
            busctl(&["get-property", name, OBJECT, IFACE, "PlaybackStatus"]).is_some_and(|s| s.contains("Playing"))
        })
        .inspect(|name| {
            let _ = busctl(&["call", name, OBJECT, IFACE, "Pause"]);
        })
        .collect()
}

pub fn resume(players: &[String]) {
    for name in players {
        let _ = busctl(&["call", name, OBJECT, IFACE, "Play"]);
    }
}
