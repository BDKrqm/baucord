//! The replay buffer: the last five minutes of the call, kept in memory
//! and written to disk when asked — one WAV per person, aligned, and a
//! mix.
//!
//! Nothing touches the disk until [`Replay::save`]; what is kept is only
//! what was *said*. Each person's decoded voice (before their volume,
//! equalizer and place, so the files are as they sent it) and our own
//! microphone (after the chain, as it went out) are tapped on their real
//! threads by a [`ReplayTap`]: one record per hop into a lock-free byte
//! ring, nothing allocated, nothing waited for, and nothing at all while
//! a person is silent. A thread of its own drains the rings into a
//! [`Store`] of one-second pages per person and trims it to the window,
//! so five minutes of ten people is about as much memory as they spoke:
//! 96 KB for each second of speech, never more than [`MEMORY_CAP`].
//!
//! Positions are samples on one timeline, the engine's clock (`Instant`
//! at creation, 48 kHz): each tap places its hops on it ([`ReplayTap`]'s
//! `place`), so the output worker's hops and the microphone's frames —
//! two threads, two clocks — line up in the files to within the ring
//! delay, tens of milliseconds, and a stalled device re-anchors instead
//! of shifting everything after it.
//!
//! The record format in the ring (little endian): the source id (`u64`;
//! [`ME`] is the microphone), the position (`u64`), the sample count
//! (`u16`), then that many `i16`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::OPUS_SAMPLE_RATE;

/// How much of the call a save keeps.
pub const WINDOW_SECS: u64 = 300;
const RATE: u64 = OPUS_SAMPLE_RATE as u64;
const WINDOW_SAMPLES: u64 = WINDOW_SECS * RATE;
/// Pages are one second: whole pages are what a snapshot shares.
const PAGE_SAMPLES: usize = RATE as usize;
/// The most the store holds, however much is said: the oldest page goes
/// first. About ten minutes of speech from ten people at once.
pub const MEMORY_CAP: usize = 192 * 1024 * 1024;
/// A hop is speech when its peak reaches this (about −60 dBFS)…
const LOUD_PEAK: f32 = 0.001;
/// …and the recording goes on for this long after the last one, so a
/// word's soft tail is not cut.
const HANG_SAMPLES: usize = RATE as usize / 5;
/// The microphone's id in the rings and the store. Sessions are small
/// numbers the server hands out, so it never collides with a peer.
pub const ME: u64 = u64::MAX;
/// Records this near each other (1 ms) are one stretch of speech.
const CONTIGUOUS: u64 = RATE / 1_000;
/// A tap re-anchors to the clock when its hops fall this far behind it
/// (100 ms: a stalled device) or ahead (500 ms)…
const BEHIND: i64 = (RATE / 10) as i64;
const AHEAD: i64 = (RATE / 2) as i64;
/// …and wakes the drain thread at most this often.
const WAKE_EVERY: Duration = Duration::from_millis(100);
const HEADER: usize = 8 + 8 + 2;
/// The longest record: the microphone's 20 ms frame (960 samples), with
/// room.
const MAX_RECORD: usize = 1_920;
/// The ring of the output worker's tap, and of the microphone's.
pub const OUTPUT_RING: usize = 2 * 1024 * 1024;
pub const INPUT_RING: usize = 512 * 1024;

/// Whether a source is speaking: the hangover that keeps the end of a
/// word. Held by whoever taps (a peer's buffer, the input worker).
#[derive(Debug, Default, Clone, Copy)]
pub struct Hangover(u32);

impl Hangover {
    /// Whether this hop is kept: loud, or within the hangover of a loud
    /// one.
    pub fn keep(&mut self, samples: &[f32]) -> bool {
        let hang = u32::try_from(HANG_SAMPLES / samples.len().max(1)).unwrap_or(0);
        if samples.iter().any(|s| s.abs() >= LOUD_PEAK) {
            self.0 = hang;
            true
        } else if self.0 > 0 {
            self.0 -= 1;
            true
        } else {
            false
        }
    }
}

/// What the drain thread is told.
enum Msg {
    /// A tap pushed something.
    Data,
    /// A new tap's ring.
    Register(Consumer<u8>),
    /// Forget everything (the buffer was switched off).
    Clear,
    Save(SaveJob),
    Shutdown,
}

/// A save: where, whose files are called what, and what to do when it
/// is done (on the saving thread).
pub struct SaveJob {
    pub dir: PathBuf,
    /// A name for each person's file; `ME` is ours. Missing: `peer-<id>`.
    pub labels: HashMap<u64, String>,
    pub done: Box<dyn FnOnce(Result<ClipInfo, ReplayError>) + Send>,
}

/// What a save wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipInfo {
    pub dir: PathBuf,
    /// People's files (ours included), not counting the mix.
    pub tracks: usize,
    pub seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayError {
    /// Nobody said anything in the window.
    Nothing,
    /// The disk would not take it.
    Io(String),
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nothing => f.write_str("nothing was said in the last five minutes"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

/// The replay buffer: owns the drain thread. One per engine.
pub struct Replay {
    tx: Sender<Msg>,
    enabled: Arc<AtomicBool>,
    t0: Instant,
    thread: Option<JoinHandle<()>>,
}

impl Replay {
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        let (tx, rx) = bounded(64);
        let enabled = Arc::new(AtomicBool::new(enabled));
        let t0 = Instant::now();
        let thread = thread::Builder::new()
            .name("replay".into())
            .spawn(move || run(&rx, t0))
            .ok();
        Self {
            tx,
            enabled,
            t0,
            thread,
        }
    }

    /// A tap for a worker thread, with a ring of `bytes`. Dropping the tap
    /// ends its ring; what it left is still stored.
    #[must_use]
    pub fn tap(&self, bytes: usize) -> ReplayTap {
        let (producer, consumer) = RingBuffer::new(bytes.max(HEADER + MAX_RECORD * 2));
        let _ = self.tx.send(Msg::Register(consumer));
        let now = Instant::now();
        ReplayTap {
            producer,
            wake: self.tx.clone(),
            enabled: self.enabled.clone(),
            t0: self.t0,
            anchor: wall_samples(self.t0, now),
            last_wake: now,
        }
    }

    /// Keep listening (`true`) or stop and forget everything.
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        if !on {
            let _ = self.tx.send(Msg::Clear);
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Write the last five minutes. Returns at once; `job.done` runs on
    /// the thread that writes.
    pub fn save(&self, job: SaveJob) {
        if let Err(e) = self.tx.send(Msg::Save(job))
            && let Msg::Save(job) = e.into_inner()
        {
            (job.done)(Err(ReplayError::Io(
                "the replay buffer is not running".into(),
            )));
        }
    }
}

impl Drop for Replay {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// `now` on the timeline of `t0`, in samples.
fn wall_samples(t0: Instant, now: Instant) -> i64 {
    let micros = now.saturating_duration_since(t0).as_micros();
    i64::try_from(micros * u128::from(RATE) / 1_000_000).unwrap_or(i64::MAX)
}

/// One thread's way into the buffer. Real-time safe: no locks, no
/// allocation, no waiting.
pub struct ReplayTap {
    producer: Producer<u8>,
    wake: Sender<Msg>,
    enabled: Arc<AtomicBool>,
    t0: Instant,
    /// Samples to add to a local position to get a timeline position.
    anchor: i64,
    last_wake: Instant,
}

impl ReplayTap {
    /// A hop of `samples` from `id`, starting `local` samples into this
    /// tap's own run (the caller counts what it has processed). Nothing
    /// is kept while the buffer is off or `id` is silent (`hang`).
    pub fn push(&mut self, id: u64, local: u64, samples: &[f32], hang: &mut Hangover) {
        if samples.is_empty() || !self.enabled.load(Ordering::Relaxed) || !hang.keep(samples) {
            return;
        }
        let now = Instant::now();
        let pos = self.place(local, wall_samples(self.t0, now));
        self.write(id, pos, samples);
        if now.saturating_duration_since(self.last_wake) >= WAKE_EVERY {
            self.last_wake = now;
            let _ = self.wake.try_send(Msg::Data);
        }
    }

    /// Where `local` goes on the timeline: where this tap has been
    /// putting things, unless that has drifted from the clock by more
    /// than the tolerance — a stall, a device that came back — and then
    /// it is the clock's.
    fn place(&mut self, local: u64, wall: i64) -> u64 {
        let local = i64::try_from(local).unwrap_or(i64::MAX);
        let mut pos = self.anchor.saturating_add(local);
        let lead = pos - wall;
        if !(-BEHIND..=AHEAD).contains(&lead) {
            self.anchor = wall.saturating_sub(local);
            pos = wall;
        }
        u64::try_from(pos.max(0)).unwrap_or(0)
    }

    fn write(&mut self, id: u64, pos: u64, samples: &[f32]) {
        let n = samples.len().min(MAX_RECORD);
        let len = HEADER + 2 * n;
        // Whole records only: a full ring loses this hop, never half of
        // it.
        if self.producer.slots() < len {
            return;
        }
        let mut rec = [0u8; HEADER + 2 * MAX_RECORD];
        rec[0..8].copy_from_slice(&id.to_le_bytes());
        rec[8..16].copy_from_slice(&pos.to_le_bytes());
        rec[16..18].copy_from_slice(&u16::try_from(n).unwrap_or(0).to_le_bytes());
        for (dst, s) in rec[HEADER..len].chunks_exact_mut(2).zip(samples) {
            dst.copy_from_slice(&to_i16(*s).to_le_bytes());
        }
        let _ = self.producer.push_partial_slice(&rec[..len]);
    }
}

fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16
}

// ── The store ───────────────────────────────────────────────────────────

/// A stretch of one person's speech: samples from `start` on.
struct Page {
    start: u64,
    data: Vec<i16>,
}

impl Page {
    fn end(&self) -> u64 {
        self.start + self.data.len() as u64
    }
}

#[derive(Default)]
struct Track {
    /// Finished pages, oldest first.
    pages: VecDeque<Arc<Page>>,
    /// The one being filled.
    open: Option<Page>,
}

struct Store {
    tracks: HashMap<u64, Track>,
    bytes: usize,
    /// [`MEMORY_CAP`], smaller in the tests.
    cap: usize,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            tracks: HashMap::new(),
            bytes: 0,
            cap: MEMORY_CAP,
        }
    }
}

/// What a save writes: the tracks' pages that fall in `[from, to)`.
struct Snapshot {
    from: u64,
    to: u64,
    tracks: Vec<(u64, Vec<Arc<Page>>)>,
}

impl Store {
    fn push(&mut self, id: u64, pos: u64, samples: &[i16]) {
        let track = self.tracks.entry(id).or_default();
        let joins = track
            .open
            .as_ref()
            .is_some_and(|o| pos.abs_diff(o.end()) <= CONTIGUOUS && o.data.len() < PAGE_SAMPLES);
        if joins {
            if let Some(open) = &mut track.open {
                open.data.extend_from_slice(samples);
            }
        } else {
            if let Some(open) = track.open.take() {
                track.pages.push_back(Arc::new(open));
            }
            let mut data = Vec::with_capacity(PAGE_SAMPLES);
            data.extend_from_slice(samples);
            track.open = Some(Page { start: pos, data });
        }
        self.bytes += samples.len() * 2;
        if track
            .open
            .as_ref()
            .is_some_and(|o| o.data.len() >= PAGE_SAMPLES)
            && let Some(open) = track.open.take()
        {
            track.pages.push_back(Arc::new(open));
        }
    }

    /// Drop what is older than the window as of `now`, and the oldest
    /// pages while the store is over its cap.
    fn trim(&mut self, now: u64) {
        let horizon = now.saturating_sub(WINDOW_SAMPLES);
        for track in self.tracks.values_mut() {
            while track.pages.front().is_some_and(|p| p.end() <= horizon) {
                if let Some(p) = track.pages.pop_front() {
                    self.bytes = self.bytes.saturating_sub(p.data.len() * 2);
                }
            }
        }
        while self.bytes > self.cap && self.evict_oldest() {}
        self.tracks
            .retain(|_, t| t.open.is_some() || !t.pages.is_empty());
    }

    fn evict_oldest(&mut self) -> bool {
        let oldest = self
            .tracks
            .iter()
            .filter_map(|(id, t)| t.pages.front().map(|p| (p.start, *id)))
            .min();
        let Some((_, id)) = oldest else {
            return false;
        };
        if let Some(p) = self.tracks.get_mut(&id).and_then(|t| t.pages.pop_front()) {
            self.bytes = self.bytes.saturating_sub(p.data.len() * 2);
        }
        true
    }

    fn clear(&mut self) {
        self.tracks.clear();
        self.bytes = 0;
    }

    /// The window ending at `now`. Pages are shared, not copied; the
    /// pages being filled are closed (the next hop opens new ones).
    fn snapshot(&mut self, now: u64) -> Snapshot {
        let from = now.saturating_sub(WINDOW_SAMPLES);
        let mut tracks = Vec::new();
        for (id, t) in &mut self.tracks {
            if let Some(open) = t.open.take() {
                t.pages.push_back(Arc::new(open));
            }
            let pages: Vec<Arc<Page>> = t
                .pages
                .iter()
                .filter(|p| p.end() > from && p.start < now)
                .cloned()
                .collect();
            if !pages.is_empty() {
                tracks.push((*id, pages));
            }
        }
        tracks.sort_by_key(|(id, _)| *id);
        Snapshot {
            from,
            to: now,
            tracks,
        }
    }
}

// ── The drain thread ────────────────────────────────────────────────────

fn run(rx: &Receiver<Msg>, t0: Instant) {
    let mut rings: Vec<Consumer<u8>> = Vec::new();
    let mut store = Store::default();
    let mut scratch: Vec<u8> = Vec::new();
    let mut samples: Vec<i16> = Vec::new();
    let now = || u64::try_from(wall_samples(t0, Instant::now()).max(0)).unwrap_or(0);

    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Register(c) => rings.push(c),
            Msg::Data => {}
            Msg::Clear => {
                drain(&mut rings, &mut store, &mut scratch, &mut samples);
                store.clear();
            }
            Msg::Save(job) => {
                drain(&mut rings, &mut store, &mut scratch, &mut samples);
                let snapshot = store.snapshot(now());
                // On a thread of its own: the rings keep draining.
                thread::Builder::new()
                    .name("replay-save".into())
                    .spawn(move || {
                        let result = write_clip(&snapshot, &job.dir, &job.labels);
                        (job.done)(result);
                    })
                    .ok();
            }
            Msg::Shutdown => break,
        }
        drain(&mut rings, &mut store, &mut scratch, &mut samples);
        store.trim(now());
    }
}

/// Everything the rings hold into the store; the rings of taps that are
/// gone and empty go.
fn drain(
    rings: &mut Vec<Consumer<u8>>,
    store: &mut Store,
    scratch: &mut Vec<u8>,
    samples: &mut Vec<i16>,
) {
    for ring in rings.iter_mut() {
        let n = ring.slots();
        if n == 0 {
            continue;
        }
        scratch.resize(n, 0);
        let (popped, _) = ring.pop_partial_slice(&mut scratch[..n]);
        let mut rest: &[u8] = popped;
        // Records are written whole, so they are read whole; a stray
        // tail would be dropped.
        while rest.len() >= HEADER {
            let id = u64::from_le_bytes(rest[0..8].try_into().unwrap_or([0; 8]));
            let pos = u64::from_le_bytes(rest[8..16].try_into().unwrap_or([0; 8]));
            let count = usize::from(u16::from_le_bytes([rest[16], rest[17]]));
            let end = HEADER + 2 * count;
            if rest.len() < end {
                break;
            }
            samples.clear();
            samples.extend(
                rest[HEADER..end]
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]])),
            );
            store.push(id, pos, samples);
            rest = &rest[end..];
        }
    }
    rings.retain(|r| !(r.is_abandoned() && r.slots() == 0));
}

// ── Writing the files ───────────────────────────────────────────────────

/// A name a Windows file can have: no separators or reserved characters,
/// not empty, not a device name, 40 characters at most.
fn safe_stem(label: &str) -> String {
    const DEVICES: [&str; 8] = ["con", "prn", "aux", "nul", "com1", "com2", "lpt1", "lpt2"];
    let cleaned: String = label
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(40)
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    if cleaned.is_empty() {
        return "peer".into();
    }
    if DEVICES.contains(&cleaned.to_lowercase().as_str()) {
        return format!("{cleaned}_");
    }
    cleaned
}

/// `stem.wav`, or `stem-2.wav`… when taken.
fn unique_name(stem: &str, used: &mut HashSet<String>) -> String {
    let mut name = format!("{stem}.wav");
    let mut n = 2;
    while !used.insert(name.to_lowercase()) {
        name = format!("{stem}-{n}.wav");
        n += 1;
    }
    name
}

/// `len` samples of the track's speech from `from` on, silence between.
fn render(pages: &[Arc<Page>], from: u64, len: usize) -> Vec<i16> {
    let mut out = vec![0i16; len];
    let to = from + len as u64;
    for p in pages {
        let a = p.start.max(from);
        let b = p.end().min(to);
        if a >= b {
            continue;
        }
        let src = &p.data[usize::try_from(a - p.start).unwrap_or(0)..];
        let dst = usize::try_from(a - from).unwrap_or(0);
        let n = usize::try_from(b - a).unwrap_or(0);
        out[dst..dst + n].copy_from_slice(&src[..n]);
    }
    out
}

fn write_wav(path: &Path, samples: &[i16]) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: OPUS_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::create(path, spec).map_err(|e| format!("{}: {e}", path.display()))?;
    for &s in samples {
        writer
            .write_sample(s)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    writer
        .finalize()
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// One WAV per person in `dir`, all the same length and starting at the
/// same moment, and `mix.wav`.
fn write_clip(
    snapshot: &Snapshot,
    dir: &Path,
    labels: &HashMap<u64, String>,
) -> Result<ClipInfo, ReplayError> {
    let Some(first) = snapshot
        .tracks
        .iter()
        .filter_map(|(_, pages)| pages.first().map(|p| p.start))
        .min()
    else {
        return Err(ReplayError::Nothing);
    };
    // The buffer may have run less than five minutes: no empty lead. The
    // end is the clock's, or the last sample if it is later: the output
    // worker decodes a few hops ahead of the clock, and that speech is
    // as much the end of the call as the rest.
    let last = snapshot
        .tracks
        .iter()
        .filter_map(|(_, pages)| pages.last().map(|p| p.end()))
        .max()
        .unwrap_or(0);
    let from = snapshot.from.max(first);
    let to = snapshot.to.max(last);
    let len = usize::try_from(to.saturating_sub(from)).unwrap_or(0);
    if len == 0 {
        return Err(ReplayError::Nothing);
    }
    std::fs::create_dir_all(dir).map_err(|e| ReplayError::Io(format!("{}: {e}", dir.display())))?;
    let mut used = HashSet::new();
    used.insert("mix.wav".to_string());
    let mut mix = vec![0i32; len];
    for (id, pages) in &snapshot.tracks {
        let label = labels.get(id).cloned().unwrap_or_else(|| {
            if *id == ME {
                "yo".into()
            } else {
                format!("peer-{id}")
            }
        });
        let name = unique_name(&safe_stem(&label), &mut used);
        let track = render(pages, from, len);
        write_wav(&dir.join(name), &track).map_err(ReplayError::Io)?;
        for (m, s) in mix.iter_mut().zip(&track) {
            *m += i32::from(*s);
        }
    }
    let mixed: Vec<i16> = mix
        .iter()
        .map(|&m| m.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16)
        .collect();
    write_wav(&dir.join("mix.wav"), &mixed).map_err(ReplayError::Io)?;
    Ok(ClipInfo {
        dir: dir.to_path_buf(),
        tracks: snapshot.tracks.len(),
        seconds: u32::try_from(len as u64 / RATE).unwrap_or(u32::MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::AtomicU32;
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "baucord-replay-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn read_wav(path: &Path) -> Vec<i16> {
        hound::WavReader::open(path)
            .expect("open wav")
            .samples::<i16>()
            .map(|s| s.expect("sample"))
            .collect()
    }

    fn tone(len: usize, level: i16) -> Vec<i16> {
        vec![level; len]
    }

    #[test]
    fn speech_hops_join_and_a_gap_opens_a_new_page() {
        let mut store = Store::default();
        store.push(7, 1_000, &tone(480, 100));
        store.push(7, 1_480, &tone(480, 100)); // right after
        store.push(7, 1_962, &tone(480, 100)); // 2 samples of jitter: still joined
        assert_eq!(
            store.tracks[&7].open.as_ref().map(|o| o.data.len()),
            Some(1_440)
        );
        store.push(7, 20_000, &tone(480, 100)); // a pause
        let t = &store.tracks[&7];
        assert_eq!(t.pages.len(), 1, "the first stretch closed");
        assert_eq!(t.open.as_ref().map(|o| o.start), Some(20_000));
    }

    #[test]
    fn a_page_closes_at_a_second() {
        let mut store = Store::default();
        let mut pos = 0;
        while pos < PAGE_SAMPLES as u64 + 2_000 {
            store.push(1, pos, &tone(480, 50));
            pos += 480;
        }
        assert_eq!(store.tracks[&1].pages.len(), 1);
        assert!(store.tracks[&1].pages[0].data.len() >= PAGE_SAMPLES);
    }

    #[test]
    fn only_the_last_five_minutes_are_kept() {
        let mut store = Store::default();
        // Ten minutes of a second of speech every ten seconds.
        let mut t = 0u64;
        while t < 600 * RATE {
            store.push(3, t, &tone(RATE as usize / 2, 80));
            t += 10 * RATE;
        }
        let now = 600 * RATE;
        store.trim(now);
        let snap = store.snapshot(now);
        let (_, pages) = &snap.tracks[0];
        assert!(pages.iter().all(|p| p.end() > now - WINDOW_SAMPLES));
        assert!(pages
            .first()
            .is_some_and(|p| p.start >= now - WINDOW_SAMPLES - RATE));
        // Thirty of the sixty half-seconds fit in the window.
        let kept: usize = pages.iter().map(|p| p.data.len()).sum();
        assert!((29..=31).contains(&(kept / (RATE as usize / 2))), "{kept}");
        // And the store's byte count followed.
        assert!(store.bytes <= kept * 2 + 4 * RATE as usize);
    }

    #[test]
    fn the_memory_cap_drops_the_oldest_first() {
        let page = PAGE_SAMPLES;
        // A cap of ten pages, and thirteen of them said.
        let mut store = Store {
            cap: 10 * page * 2,
            ..Store::default()
        };
        let pages_over = 13;
        for i in 0..pages_over {
            let track = (i % 2) as u64 + 1;
            let start = i as u64 * page as u64;
            store.push(track, start, &vec![1; page]);
        }
        store.trim(pages_over as u64 * page as u64);
        assert!(store.bytes <= 10 * page * 2);
        let oldest = store
            .tracks
            .values()
            .filter_map(|t| t.pages.front().map(|p| p.start))
            .min()
            .expect("pages left");
        assert!(
            oldest >= 3 * page as u64,
            "the first pages went first: {oldest}"
        );
    }

    #[test]
    fn silence_is_not_kept_but_a_words_tail_is() {
        let mut hang = Hangover::default();
        let silent = vec![0.0f32; 480];
        let loud = vec![0.3f32; 480];
        assert!(!hang.keep(&silent));
        assert!(hang.keep(&loud));
        // 200 ms of 10 ms hops after it, then nothing.
        let kept = (0..40).filter(|_| hang.keep(&silent)).count();
        assert_eq!(kept, 20);
        // A 20 ms microphone frame hangs for as long in time.
        let mut hang = Hangover::default();
        assert!(hang.keep(&vec![0.3f32; 960]));
        assert_eq!(
            (0..40).filter(|_| hang.keep(&vec![0.0f32; 960])).count(),
            10
        );
    }

    #[test]
    fn a_tap_follows_the_clock_and_re_anchors_after_a_stall() {
        let replay = Replay::new(true);
        let mut tap = replay.tap(INPUT_RING);
        let wall = 48_000 * 10; // ten seconds in
        tap.anchor = wall; // as if created now
                           // Hops land where they are put while they keep up.
        assert_eq!(tap.place(0, wall), wall as u64);
        assert_eq!(tap.place(480, wall + 480), wall as u64 + 480);
        // A little ahead (the ring's fill) is fine…
        assert_eq!(tap.place(4_800, wall + 480), wall as u64 + 4_800);
        // …a stall of a second is not: it goes back to the clock.
        let later = wall + 48_000;
        assert_eq!(tap.place(5_280, later), later as u64);
        assert_eq!(tap.place(5_760, later + 480), later as u64 + 480);
    }

    #[test]
    fn what_a_tap_pushes_comes_out_of_the_save() {
        let replay = Replay::new(true);
        let mut me = replay.tap(INPUT_RING);
        let mut out = replay.tap(OUTPUT_RING);
        let mut me_hang = Hangover::default();
        let mut peer_hang = Hangover::default();
        // 600 ms of the microphone and 600 ms of a peer, together.
        for i in 0..30u64 {
            me.push(ME, i * 960, &vec![0.25f32; 960], &mut me_hang);
        }
        for i in 0..60u64 {
            out.push(42, i * 480, &vec![0.5f32; 480], &mut peer_hang);
        }
        // The clock has to have moved for there to be a window.
        thread::sleep(Duration::from_millis(40));
        let dir = temp_dir("e2e");
        let (tx, rx) = bounded(1);
        replay.save(SaveJob {
            dir: dir.clone(),
            labels: HashMap::from([(ME, "yo".to_string()), (42, "Ana/Mar\u{ed}a".to_string())]),
            done: Box::new(move |r| {
                let _ = tx.send(r);
            }),
        });
        let info = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the save answered")
            .expect("the save worked");
        assert_eq!(info.tracks, 2);
        let yo = read_wav(&dir.join("yo.wav"));
        let ana = read_wav(&dir.join("Ana_Mar\u{ed}a.wav"));
        let mix = read_wav(&dir.join("mix.wav"));
        assert_eq!(yo.len(), ana.len(), "aligned: the same length");
        assert_eq!(yo.len(), mix.len());
        assert!(yo.contains(&8_192), "the microphone's 0.25");
        assert!(ana.contains(&16_384), "the peer's 0.5");
        assert!(mix.contains(&24_576), "both at once: 0.75");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_said_is_nothing_to_save() {
        let replay = Replay::new(true);
        let dir = temp_dir("empty");
        let (tx, rx) = bounded(1);
        replay.save(SaveJob {
            dir: dir.clone(),
            labels: HashMap::new(),
            done: Box::new(move |r| {
                let _ = tx.send(r);
            }),
        });
        let r = rx.recv_timeout(Duration::from_secs(10)).expect("answered");
        assert_eq!(r, Err(ReplayError::Nothing));
        assert!(!dir.exists(), "no folder for nothing");
    }

    #[test]
    fn switched_off_it_keeps_nothing() {
        let replay = Replay::new(false);
        let mut tap = replay.tap(INPUT_RING);
        let mut hang = Hangover::default();
        for i in 0..20u64 {
            tap.push(ME, i * 960, &vec![0.5f32; 960], &mut hang);
        }
        let (tx, rx) = bounded(1);
        replay.save(SaveJob {
            dir: temp_dir("off"),
            labels: HashMap::new(),
            done: Box::new(move |r| {
                let _ = tx.send(r);
            }),
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).expect("answered"),
            Err(ReplayError::Nothing)
        );
        // And switching it off forgets what it had.
        replay.set_enabled(true);
        for i in 0..20u64 {
            tap.push(ME, i * 960, &vec![0.5f32; 960], &mut hang);
        }
        replay.set_enabled(false);
        let (tx, rx) = bounded(1);
        replay.save(SaveJob {
            dir: temp_dir("off2"),
            labels: HashMap::new(),
            done: Box::new(move |r| {
                let _ = tx.send(r);
            }),
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).expect("answered"),
            Err(ReplayError::Nothing)
        );
    }

    #[test]
    fn file_names_are_safe_and_unique() {
        assert_eq!(safe_stem("Ana/Mar\u{ed}a"), "Ana_Mar\u{ed}a");
        assert_eq!(safe_stem("  ..  "), "peer");
        assert_eq!(safe_stem("CON"), "CON_");
        assert_eq!(safe_stem(&"x".repeat(100)).chars().count(), 40);
        assert_eq!(safe_stem("a<b>c:d"), "a_b_c_d");
        let mut used = HashSet::from(["mix.wav".to_string()]);
        assert_eq!(unique_name("Ana", &mut used), "Ana.wav");
        assert_eq!(
            unique_name("ana", &mut used),
            "ana-2.wav",
            "Windows ignores case"
        );
        assert_eq!(
            unique_name("mix", &mut used),
            "mix-2.wav",
            "the mix keeps its name"
        );
    }

    #[test]
    fn a_full_ring_loses_a_hop_never_half_of_one() {
        let replay = Replay::new(true);
        // The smallest ring a tap gets: room for a couple of records.
        let mut tap = replay.tap(1);
        let mut hang = Hangover::default();
        // Far more than fits, without the drain thread being woken
        // faster than a hop: all pushes return, none panic.
        for i in 0..200u64 {
            tap.push(5, i * 960, &vec![0.4f32; 960], &mut hang);
        }
        let (tx, rx) = bounded(1);
        replay.save(SaveJob {
            dir: temp_dir("full"),
            labels: HashMap::new(),
            done: Box::new(move |r| {
                let _ = tx.send(r);
            }),
        });
        let r = rx.recv_timeout(Duration::from_secs(10)).expect("answered");
        // Whatever fit was whole records: it wrote, and the track is
        // whole samples.
        assert!(matches!(r, Ok(_) | Err(ReplayError::Nothing)), "{r:?}");
    }
}
