//! Audio out. rodio's output stream cannot leave its thread, so one thread owns
//! it and takes commands. Sentences append to one sink and play back to back:
//! the next sentence is synthesized while the current one plays.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, OutputStreamHandle, Sink};

enum Cmd {
    Push(u32, Vec<i16>),
    Stop,
    Volume(f32),
}

#[derive(Clone)]
pub struct Audio {
    tx: Sender<Cmd>,
    playing: Arc<AtomicBool>,
}

fn open_output() -> Option<(OutputStream, OutputStreamHandle)> {
    // At login the audio server can be a moment behind; try a few times.
    for _ in 0..10 {
        if let Ok(o) = OutputStream::try_default() {
            return Some(o);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    None
}

impl Audio {
    pub fn new(volume: f32) -> Audio {
        let (tx, rx) = channel::<Cmd>();
        let playing = Arc::new(AtomicBool::new(false));
        let flag = playing.clone();
        std::thread::spawn(move || {
            let mut output: Option<(OutputStream, OutputStreamHandle)> = None;
            let mut sink: Option<Sink> = None;
            let mut volume = volume;
            loop {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(Cmd::Push(rate, samples)) => {
                        if sink.is_none() {
                            if output.is_none() {
                                output = open_output();
                            }
                            match output.as_ref().map(|(_s, h)| Sink::try_new(h)) {
                                Some(Ok(s)) => {
                                    s.set_volume(volume);
                                    sink = Some(s);
                                }
                                Some(Err(e)) => {
                                    eprintln!("omanta: audio sink error: {e}; reopening the output");
                                    output = None;
                                }
                                None => eprintln!("omanta: no audio output"),
                            }
                        }
                        if let Some(s) = &sink {
                            s.append(SamplesBuffer::new(1, rate, samples));
                            flag.store(true, Ordering::Relaxed);
                        }
                    }
                    Ok(Cmd::Stop) => {
                        if let Some(s) = sink.take() {
                            s.stop();
                        }
                        flag.store(false, Ordering::Relaxed);
                    }
                    Ok(Cmd::Volume(v)) => {
                        volume = v;
                        if let Some(s) = &sink {
                            s.set_volume(v);
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if sink.as_ref().is_some_and(|s| s.empty()) {
                            flag.store(false, Ordering::Relaxed);
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        Audio { tx, playing }
    }

    pub fn push(&self, rate: u32, samples: Vec<i16>) {
        self.playing.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Push(rate, samples));
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
        self.playing.store(false, Ordering::Relaxed);
    }

    pub fn set_volume(&self, v: f32) {
        let _ = self.tx.send(Cmd::Volume(v));
    }

    /// True while audio is queued or playing.
    pub fn playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }
}
