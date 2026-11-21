//! The sentence queue. One thread synthesizes each text with the sidecar's
//! streaming endpoint and appends the audio to the sink. A stop bumps the
//! generation, which drops what was queued before it and cuts the stream in
//! progress.

use std::collections::VecDeque;
use std::io::Read;
use std::time::Instant;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::json;

use super::audio::Audio;
use super::media;
use super::sidecar::{self, Shared};
use crate::config::Watched;

/// When each sentence becomes audible. Sentences append to one sink and play
/// back to back, so a sentence starts when the audio before it ends.
#[derive(Default)]
struct Timeline {
    /// When the audio queued so far will have played out.
    end: Option<Instant>,
    /// Sentences not heard yet: (id, when its first sample plays).
    starts: VecDeque<(u64, Instant)>,
    heard: u64,
    /// The texts in the sink, for how far the voice is.
    utterances: VecDeque<Utterance>,
    /// Seconds of audio per character, learned from finished texts.
    seconds_per_char: f64,
}

/// One text in the sink.
struct Utterance {
    /// When its first sample plays.
    start: Instant,
    /// Seconds of audio synthesized so far.
    seconds: f64,
    chars: usize,
    /// All of its audio is synthesized.
    done: bool,
}

impl Timeline {
    /// Audio of `seconds` was appended; returns when it starts to play.
    fn append(&mut self, seconds: f64) -> Instant {
        let now = Instant::now();
        let start = self.end.filter(|e| *e > now).unwrap_or(now);
        self.end = Some(start + Duration::from_secs_f64(seconds));
        start
    }

    fn heard(&mut self) -> u64 {
        let now = Instant::now();
        while let Some(&(id, at)) = self.starts.front() {
            if at > now {
                break;
            }
            self.heard = self.heard.max(id);
            self.starts.pop_front();
        }
        self.heard
    }

    fn stop(&mut self) {
        self.end = None;
        self.starts.clear();
        self.utterances.clear();
    }

    /// A text's first audio, of `seconds`, plays from `start`.
    fn begin(&mut self, start: Instant, seconds: f64, chars: usize) {
        self.utterances.push_back(Utterance { start, seconds, chars: chars.max(1), done: false });
    }

    /// More audio of the newest text.
    fn grow(&mut self, seconds: f64) {
        if let Some(u) = self.utterances.back_mut() {
            u.seconds += seconds;
        }
    }

    /// The newest text is synthesized whole: learn how long a character takes.
    fn finish(&mut self) {
        if let Some(u) = self.utterances.back_mut() {
            u.done = true;
            let rate = u.seconds / u.chars as f64;
            self.seconds_per_char = if self.seconds_per_char == 0.0 { rate } else { self.seconds_per_char * 0.6 + rate * 0.4 };
        }
    }

    /// How far the text that plays now is, 0 to 1. While it is still
    /// synthesized, its length is estimated from its characters.
    fn progress(&mut self) -> f64 {
        let now = Instant::now();
        while self.utterances.len() > 1 && self.utterances[1].start <= now {
            self.utterances.pop_front();
        }
        let Some(u) = self.utterances.front() else { return 0.0 };
        if u.start > now {
            return 0.0;
        }
        let per_char = if self.seconds_per_char > 0.0 { self.seconds_per_char } else { 0.07 };
        let total = if u.done { u.seconds } else { u.seconds.max(u.chars as f64 * per_char) };
        ((now - u.start).as_secs_f64() / total.max(0.01)).clamp(0.0, 1.0)
    }
}

pub struct Speaker {
    items: Mutex<VecDeque<(u64, String, Option<u64>)>>,
    timeline: Mutex<Timeline>,
    ready: Condvar,
    generation: AtomicU64,
    /// The generation being synthesized, +1 (0: none), so a cancelled
    /// stream that is still closing does not count as speaking.
    synthesizing: AtomicU64,
    problem: Mutex<String>,
    pub audio: Audio,
}

impl Speaker {
    pub fn new(audio: Audio) -> Arc<Speaker> {
        Arc::new(Speaker {
            items: Mutex::new(VecDeque::new()),
            timeline: Mutex::new(Timeline::default()),
            ready: Condvar::new(),
            generation: AtomicU64::new(0),
            synthesizing: AtomicU64::new(0),
            problem: Mutex::new(String::new()),
            audio,
        })
    }

    pub fn say(&self, text: &str, queue: bool, id: Option<u64>) -> usize {
        if !queue {
            self.stop();
        }
        let generation = self.generation.load(Ordering::SeqCst);
        let mut items = self.items.lock().unwrap();
        items.push_back((generation, text.to_string(), id));
        self.ready.notify_one();
        items.len()
    }

    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.items.lock().unwrap().clear();
        self.audio.stop();
        self.timeline.lock().unwrap().stop();
    }

    /// The id of the last sentence whose audio has started.
    pub fn heard(&self) -> u64 {
        self.timeline.lock().unwrap().heard()
    }

    /// How far the text that plays now is, 0 to 1.
    pub fn progress(&self) -> f64 {
        self.timeline.lock().unwrap().progress()
    }

    pub fn queued(&self) -> usize {
        self.items.lock().unwrap().len()
    }

    pub fn speaking(&self) -> bool {
        let synth = self.synthesizing.load(Ordering::SeqCst);
        (synth != 0 && self.current(synth - 1)) || self.audio.playing() || self.queued() > 0
    }

    /// The last synthesis error, cleared by the next success.
    pub fn problem(&self) -> String {
        self.problem.lock().unwrap().clone()
    }

    fn current(&self, generation: u64) -> bool {
        generation == self.generation.load(Ordering::SeqCst)
    }

    /// The speaker thread's loop.
    pub fn run(self: Arc<Self>, health: Shared, mut config: Watched) {
        let mut paused: Vec<String> = Vec::new();
        let mut idle_ticks = 0u32;
        loop {
            let next = {
                let mut items = self.items.lock().unwrap();
                if items.is_empty() {
                    items = self.ready.wait_timeout(items, Duration::from_millis(150)).unwrap().0;
                }
                items.pop_front()
            };
            let Some((generation, text, id)) = next else {
                // Resume the media we paused once the voice has really ended,
                // not in the short gaps between sentences.
                if !paused.is_empty() && !self.speaking() {
                    idle_ticks += 1;
                    if idle_ticks >= 4 {
                        media::resume(&paused);
                        paused.clear();
                    }
                } else {
                    idle_ticks = 0;
                }
                continue;
            };
            idle_ticks = 0;
            if !self.current(generation) {
                continue;
            }
            let c = config.refresh().clone();
            // The voice may still be starting: wait for it a while.
            for _ in 0..200 {
                if health.lock().unwrap().ready || !self.current(generation) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            if !health.lock().unwrap().ready {
                *self.problem.lock().unwrap() = "the voice is not ready".into();
                continue;
            }
            if c.voice.pause_media && paused.is_empty() {
                paused = media::pause_playing();
            }
            self.audio.set_volume(c.voice.volume);
            self.synthesizing.store(generation + 1, Ordering::SeqCst);
            let result = self.stream(generation, id, &text, &c.voice.name, c.voice.speed, &c.voice.language, c.voice.sidecar_port);
            self.synthesizing.store(0, Ordering::SeqCst);
            *self.problem.lock().unwrap() = match result {
                Ok(()) => String::new(),
                Err(e) => {
                    eprintln!("omanta: speak error: {e}");
                    e.to_string()
                }
            };
        }
    }

    /// Streams one text: a 4-byte sample rate, then frames of a 4-byte length
    /// and little-endian 16-bit PCM.
    #[allow(clippy::too_many_arguments)]
    fn stream(&self, generation: u64, id: Option<u64>, text: &str, voice: &str, speed: f32, language: &str, port: u16) -> anyhow::Result<()> {
        let response = ureq::post(&format!("{}/speak_stream", sidecar::base(port)))
            .timeout(Duration::from_secs(120))
            .send_json(json!({"text": text, "voice": voice, "speed": speed, "language": language}))?;
        let mut reader = response.into_reader();
        let mut word = [0u8; 4];
        if reader.read_exact(&mut word).is_err() {
            return Ok(()); // nothing to say
        }
        let rate = u32::from_be_bytes(word);
        let mut first = true;
        let chars = text.chars().count();
        loop {
            if !self.current(generation) {
                // Dropping the reader closes the connection; the sidecar stops.
                return Ok(());
            }
            if reader.read_exact(&mut word).is_err() {
                if self.current(generation) {
                    self.timeline.lock().unwrap().finish();
                }
                return Ok(());
            }
            let len = u32::from_be_bytes(word) as usize;
            let mut pcm = vec![0u8; len];
            reader.read_exact(&mut pcm)?;
            let samples: Vec<i16> = pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
            if self.current(generation) {
                let mut t = self.timeline.lock().unwrap();
                let seconds = samples.len() as f64 / rate.max(1) as f64;
                let start = t.append(seconds);
                if first {
                    first = false;
                    t.begin(start, seconds, chars);
                    if let Some(id) = id {
                        t.starts.push_back((id, start));
                    }
                } else {
                    t.grow(seconds);
                }
                drop(t);
                self.audio.push(rate, samples);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_start_after_the_audio_before_them() {
        let mut t = Timeline::default();
        let a = t.append(0.0);
        t.starts.push_back((1, a));
        let b = t.append(10.0); // a second sentence queued behind 10 s of audio
        let c = t.append(1.0);
        t.starts.push_back((2, c));
        assert!(b <= Instant::now());
        assert_eq!(t.heard(), 1, "the first sentence plays now, the second in 10 s");
        t.stop();
        assert_eq!(t.heard(), 1, "heard never goes back");
    }

    #[test]
    fn progress_follows_the_text_that_plays() {
        let mut t = Timeline::default();
        let now = Instant::now();
        t.begin(now - Duration::from_secs(1), 4.0, 10);
        t.finish();
        assert!((t.progress() - 0.25).abs() < 0.02, "1 s of 4 s played");
        // A text still in synthesis: its length comes from its characters.
        t.begin(now - Duration::from_secs(1), 1.0, 100);
        assert!((t.progress() - 1.0 / 40.0).abs() < 0.01, "0.4 s a character, learned: 100 characters is 40 s");
        t.stop();
        assert_eq!(t.progress(), 0.0);
    }
}
