//! The live microphone level while Voxtype records, for the waveform.
//!
//! Voxtype's `voxtype-audio-bridge` reads the daemon's audio socket and
//! prints one JSON line per audio frame: `{"peak":0.42,"rms":0.18,"vad":1,
//! "ts_ms":1234}`. omanta keeps one bridge running while the chat is open
//! and keeps the loudest peak of each short step.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::paths::find_program;

/// One column of the waveform holds this much sound.
const STEP_MS: u64 = 40;
/// Steps to keep: more than any terminal is wide.
const KEEP: usize = 512;
/// With no frame for this long, the bridge is not there or not working.
const STALE: Duration = Duration::from_millis(400);

#[derive(Default)]
struct Inner {
    /// The loudest peak of each step, oldest first.
    steps: VecDeque<f32>,
    step: Option<u64>,
    /// Steps ever added: lets the colors move with the sound.
    count: u64,
    last: Option<Instant>,
}

#[derive(Clone, Default)]
pub struct Levels {
    inner: Arc<Mutex<Inner>>,
}

impl Levels {
    /// Starts the bridge in the background. Without it, `recent` is None.
    pub fn start() -> Levels {
        let levels = Levels::default();
        if find_program("voxtype-audio-bridge").is_some() {
            let inner = levels.inner.clone();
            thread::spawn(move || run_bridge(inner));
        }
        levels
    }

    /// Forgets the last recording, when a new one starts.
    pub fn clear(&self) {
        let mut i = self.inner.lock().unwrap();
        i.steps.clear();
        i.step = None;
    }

    /// The last `n` steps (or fewer), each 0 to 1 on a decibel scale, and
    /// the step count. None when no frames come.
    pub fn recent(&self, n: usize) -> Option<(Vec<f64>, u64)> {
        let i = self.inner.lock().unwrap();
        if i.last.is_none_or(|t| t.elapsed() > STALE) {
            return None;
        }
        let skip = i.steps.len().saturating_sub(n);
        Some((i.steps.iter().skip(skip).map(|&p| loudness(p)).collect(), i.count))
    }
}

/// A peak amplitude (0 to 1) as 0 to 1 on a decibel scale: -48 dB and
/// quieter is 0, -8 dB and louder is 1. Speech is far quieter than full
/// scale, and the ear hears loudness on such a scale.
pub fn loudness(peak: f32) -> f64 {
    if peak <= 0.0 {
        return 0.0;
    }
    ((20.0 * (peak as f64).log10() + 48.0) / 40.0).clamp(0.0, 1.0)
}

fn add(inner: &Mutex<Inner>, peak: f32, ts_ms: u64) {
    let mut i = inner.lock().unwrap();
    let step = ts_ms / STEP_MS;
    i.last = Some(Instant::now());
    if i.step == Some(step) {
        if let Some(last) = i.steps.back_mut() {
            *last = last.max(peak);
        }
        return;
    }
    i.step = Some(step);
    i.steps.push_back(peak);
    i.count += 1;
    while i.steps.len() > KEEP {
        i.steps.pop_front();
    }
}

fn run_bridge(inner: Arc<Mutex<Inner>>) {
    let mut wait = Duration::from_secs(1);
    loop {
        let mut cmd = Command::new("voxtype-audio-bridge");
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        // The bridge must not outlive the chat. This thread lives as long as the chat.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        if let Ok(mut child) = cmd.spawn() {
            let started = Instant::now();
            if let Some(out) = child.stdout.take() {
                for line in BufReader::new(out).lines() {
                    let Ok(line) = line else { break };
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                    if let (Some(peak), Some(ts)) = (v["peak"].as_f64(), v["ts_ms"].as_u64()) {
                        add(&inner, peak as f32, ts);
                    }
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            // A bridge that ran a while was fine: try again soon.
            if started.elapsed() > Duration::from_secs(30) {
                wait = Duration::from_secs(1);
            }
        }
        thread::sleep(wait);
        wait = (wait * 2).min(Duration::from_secs(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loudness_is_on_a_decibel_scale() {
        assert_eq!(loudness(0.0), 0.0);
        assert_eq!(loudness(0.001), 0.0);
        assert!((loudness(0.1) - 0.7).abs() < 0.01);
        assert_eq!(loudness(1.0), 1.0);
    }

    #[test]
    fn keeps_the_loudest_peak_of_each_step() {
        let levels = Levels::default();
        add(&levels.inner, 0.1, 0);
        add(&levels.inner, 0.3, 10);
        add(&levels.inner, 0.2, 45);
        let (steps, count) = levels.recent(10).unwrap();
        assert_eq!(steps, vec![loudness(0.3), loudness(0.2)]);
        assert_eq!(count, 2);
        assert_eq!(levels.recent(1).unwrap().0.len(), 1);
        levels.clear();
        assert!(levels.recent(10).unwrap().0.is_empty());
    }
}
