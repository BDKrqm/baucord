//! The voice sink: PCM from the bot, Opus to the room, one 20 ms frame
//! every 20 ms. The bot pushes audio as fast as it likes (a decoder's
//! output, a file); the pacer here keeps a short queue and sends one
//! frame per slot, the way the audio engine's capture clock does for a
//! microphone, so every listener's jitter buffer sees a steady stream.
//!
//! Mono, 48 kHz, `Application::Audio` (music, not voice), 64 kbit/s
//! by default: a 20 ms frame is ~160 bytes, under the wire's 512-byte
//! frame cap with room for the encoder's peaks. The speaking light
//! goes on with the first frame and off a moment after the last.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use log::{info, warn};

use crate::net::{Command, Link};
use crate::session::{VOICE_FRAME, VOICE_FRAME_SAMPLES};

/// Samples per frame (20 ms at 48 kHz).
pub const FRAME_SAMPLES: usize = VOICE_FRAME_SAMPLES as usize;
/// Audio queued ahead of the clock before the bot's socket is made to
/// wait: ten seconds of frames.
pub const QUEUE_FRAMES: usize = 500;
/// Silence for this long turns the speaking light off.
const SPEAKING_HOLD: Duration = Duration::from_millis(200);
/// A pacer that fell this far behind its clock (the thread was not
/// scheduled) restarts the clock instead of bursting frames.
const CATCH_UP: Duration = Duration::from_millis(100);
/// The bitrates a bot may ask for; a 20 ms frame at the top is ~320
/// bytes, inside the wire's cap.
pub const KBPS_RANGE: std::ops::RangeInclusive<u32> = 16..=128;

/// One frame of samples on its way to the pacer, stamped with the
/// [`Feeder`]'s generation: a stop bumps the generation, and every
/// frame still queued from before is dropped on sight — so a stop
/// takes effect now, not after the ten seconds already queued play.
#[derive(Debug)]
pub struct PcmChunk {
    generation: u32,
    samples: Vec<f32>,
}

/// The sink's input side: slices any run of samples into frames,
/// waits when ten seconds are queued (the bot's back-pressure), and
/// stops what is queued.
pub struct Feeder {
    tx: Sender<PcmChunk>,
    generation: Arc<AtomicU32>,
    /// Frames fed and not yet played (or dropped by a stop).
    pending: Arc<AtomicUsize>,
    carry: Vec<f32>,
}

impl Feeder {
    /// The feeder and the pacer's input, made together so they share
    /// the generation.
    #[must_use]
    pub fn pair() -> (Self, Pacer) {
        let (tx, rx) = crossbeam_channel::bounded(QUEUE_FRAMES);
        let generation = Arc::new(AtomicU32::new(0));
        let pending = Arc::new(AtomicUsize::new(0));
        (
            Self {
                tx,
                generation: Arc::clone(&generation),
                pending: Arc::clone(&pending),
                carry: Vec::new(),
            },
            Pacer {
                rx,
                generation,
                pending,
            },
        )
    }

    /// Queue samples (f32, 48 kHz, mono), whole frames at a time; the
    /// rest waits for the next call. Blocks while the queue is full.
    /// `false` once the pacer is gone.
    pub fn feed(&mut self, samples: &[f32]) -> bool {
        self.carry.extend_from_slice(samples);
        let generation = self.generation.load(Ordering::Relaxed);
        let mut offset = 0;
        while self.carry.len() - offset >= FRAME_SAMPLES {
            let frame = self.carry[offset..offset + FRAME_SAMPLES].to_vec();
            offset += FRAME_SAMPLES;
            self.pending.fetch_add(1, Ordering::Relaxed);
            if self
                .tx
                .send(PcmChunk {
                    generation,
                    samples: frame,
                })
                .is_err()
            {
                self.pending.fetch_sub(1, Ordering::Relaxed);
                self.carry.clear();
                return false;
            }
        }
        self.carry.drain(..offset);
        true
    }

    /// Drop everything queued, the partial frame included.
    pub fn stop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.carry.clear();
    }

    /// Frames fed and not yet played: how far ahead of the room the
    /// feeders are, in 20 ms frames. Zero once the last frame went
    /// out — or the moment after a stop.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }

    /// Another feeder on the same sink (one per bot connection).
    #[must_use]
    pub fn another(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            generation: Arc::clone(&self.generation),
            pending: Arc::clone(&self.pending),
            carry: Vec::new(),
        }
    }
}

/// The pacer's input side.
pub struct Pacer {
    rx: Receiver<PcmChunk>,
    generation: Arc<AtomicU32>,
    pending: Arc<AtomicUsize>,
}

impl Pacer {
    /// Frames fed and not yet taken (the tests look).
    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.rx.len()
    }
}

/// Run the pacer until every feeder is gone.
pub fn run(pacer: &Pacer, link: &Link, kbps: u32) {
    let pcm = &pacer.rx;
    let mut encoder =
        match opus::Encoder::new(48_000, opus::Channels::Mono, opus::Application::Audio) {
            Ok(encoder) => encoder,
            Err(e) => {
                warn!("no Opus encoder: {e}; voice is off");
                return;
            }
        };
    let kbps = kbps.clamp(*KBPS_RANGE.start(), *KBPS_RANGE.end());
    if let Err(e) = encoder.set_bitrate(opus::Bitrate::Bits(
        i32::try_from(kbps * 1000).unwrap_or(64_000),
    )) {
        warn!("could not set the Opus bitrate: {e}");
    }
    info!("voice: Opus {kbps} kbit/s, mono, 20 ms frames");
    let mut queue: VecDeque<PcmChunk> = VecDeque::new();
    let mut out = vec![0u8; protocol::VOICE_MAX_FRAME_SIZE];
    let mut speaking = false;
    let mut next = Instant::now();
    loop {
        let now = Instant::now();
        if queue.is_empty() {
            // Nothing to play: wait for audio, and after a moment of
            // silence turn the light off.
            let wait = if speaking {
                SPEAKING_HOLD
            } else {
                Duration::from_hours(1)
            };
            match pcm.recv_timeout(wait) {
                Ok(chunk) => {
                    take(&mut queue, chunk, pacer);
                    // The clock starts with the first frame of a run;
                    // a frame that lands while the last slot is still
                    // pending keeps the schedule (the queue is empty
                    // between two frames of a stream that arrives
                    // one chunk at a time).
                    if !queue.is_empty() {
                        next = next.max(Instant::now());
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if speaking {
                        speaking = false;
                        link.send(Command::Speaking(false));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
            continue;
        }
        if now < next {
            // Gather while the slot is not due.
            match pcm.recv_timeout(next - now) {
                Ok(chunk) => take(&mut queue, chunk, pacer),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            continue;
        }
        let Some(frame) = queue.pop_front() else {
            continue;
        };
        pacer.pending.fetch_sub(1, Ordering::Relaxed);
        if frame.generation != pacer.generation.load(Ordering::Relaxed) {
            // Stopped since it was queued: gone without taking a slot.
            continue;
        }
        match encoder.encode_float(&frame.samples, &mut out) {
            Ok(len) => {
                if !speaking {
                    speaking = true;
                    link.send(Command::Speaking(true));
                }
                link.send(Command::Voice(out[..len].to_vec()));
            }
            Err(e) => warn!("Opus encode failed: {e}"),
        }
        next += VOICE_FRAME;
        if now.saturating_duration_since(next) > CATCH_UP {
            next = now;
        }
    }
}

fn take(queue: &mut VecDeque<PcmChunk>, chunk: PcmChunk, pacer: &Pacer) {
    if chunk.generation != pacer.generation.load(Ordering::Relaxed) {
        // Stopped since it was fed: gone, and no longer pending.
        pacer.pending.fetch_sub(1, Ordering::Relaxed);
        return;
    }
    if chunk.samples.len() == FRAME_SAMPLES {
        queue.push_back(chunk);
    } else {
        pacer.pending.fetch_sub(1, Ordering::Relaxed);
        warn!(
            "a {}-sample chunk is not a frame; dropped",
            chunk.samples.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames go out one per 20 ms however fast they come in, the
    /// light on with the first and off after the last.
    #[test]
    fn frames_are_paced_at_twenty_milliseconds() {
        let (link, commands, _waker) = crate::net::link().expect("a link");
        let (mut feeder, pacer) = Feeder::pair();
        let pacer = std::thread::spawn(move || run(&pacer, &link, 64));
        // Half a second of silence, pushed at once, in odd pieces.
        assert!(feeder.feed(&vec![0.0; FRAME_SAMPLES * 25 - 100]));
        assert!(feeder.feed(&[0.0; 100]));
        let started = Instant::now();
        let mut voice_at = Vec::new();
        let mut lights = Vec::new();
        while let Ok(command) = commands.recv_timeout(Duration::from_secs(2)) {
            match command {
                Command::Voice(_) => voice_at.push(started.elapsed()),
                Command::Speaking(on) => lights.push((started.elapsed(), on)),
                _ => {}
            }
            if lights.len() == 2 {
                break;
            }
        }
        assert_eq!(feeder.pending(), 0, "every frame was played");
        drop(feeder);
        pacer.join().expect("the pacer ends with its feeders");
        assert_eq!(voice_at.len(), 25, "every frame was sent");
        let span = voice_at[24].saturating_sub(voice_at[0]);
        assert!(
            span >= Duration::from_millis(400) && span <= Duration::from_millis(700),
            "25 frames took {span:?}, not ~480 ms"
        );
        assert!(lights[0].1, "the light went on first");
        assert!(!lights[1].1, "and off after the last frame");
        assert!(lights[1].0 >= voice_at[24] + SPEAKING_HOLD);
    }
}
