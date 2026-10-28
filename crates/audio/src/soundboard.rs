//! The soundboard: short clips played into the call, and into our own
//! ears if we want to hear them too.
//!
//! A clip is a file (WAV, MP3, OGG, FLAC, M4A) decoded once to mono
//! 48 kHz [`Clip`]; the client's voice thread keeps the decoded clips. Playing one is a
//! [`ClipCmd`] to the two places that mix it: the input worker, which adds
//! it to the microphone's frame after the processing chain — so the gate,
//! the denoiser and the compressor do not touch it, and Opus sends it like
//! any other sound — and, to hear it ourselves, the output worker, which
//! adds it to both ears. Each keeps its own [`ClipMixer`].
//!
//! The microphone path is silenced while we are muted or out of a voice
//! room (`InputConfig::silenced`): there the clip plays for nobody, which
//! is what a mute should mean. The frontend does not offer it then.

use std::path::Path;
use std::sync::Arc;

use audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Async, Resampler};
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

use crate::OPUS_SAMPLE_RATE;

/// The longest clip: a soundboard is for sounds, not songs, and a clip
/// is held decoded (192 KB a second).
pub const MAX_SECONDS: usize = 60;
/// The largest file looked at.
pub const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
/// Clips playing at once; a new one past this takes the oldest's place.
const MAX_PLAYING: usize = 8;
/// Input frames per resampler call.
const RESAMPLE_CHUNK: usize = 1_024;

/// A decoded clip: mono, 48 kHz.
#[derive(Clone, Debug)]
pub struct Clip {
    pub samples: Arc<[f32]>,
}

impl Clip {
    /// Its length in seconds.
    #[must_use]
    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / OPUS_SAMPLE_RATE as f32
    }
}

/// Why a file is not a clip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipError {
    /// More than [`MAX_FILE_BYTES`].
    TooBig,
    /// Longer than [`MAX_SECONDS`].
    TooLong,
    /// Not audio we can read, or it would not open.
    Unreadable(String),
}

impl std::fmt::Display for ClipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooBig => write!(f, "the file is over {} MB", MAX_FILE_BYTES / (1024 * 1024)),
            Self::TooLong => write!(f, "the clip is over {MAX_SECONDS} seconds"),
            Self::Unreadable(e) => f.write_str(e),
        }
    }
}

/// What the mixers are told.
#[derive(Clone, Debug)]
pub enum ClipCmd {
    /// Start `clip`, at `gain`.
    Play { clip: Arc<[f32]>, gain: f32 },
    /// Stop every clip.
    Stop,
}

struct Playing {
    samples: Arc<[f32]>,
    cursor: usize,
    gain: f32,
}

/// The clips playing in one stream, mixed in hop by hop.
pub struct ClipMixer {
    playing: Vec<Playing>,
}

impl Default for ClipMixer {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipMixer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            playing: Vec::with_capacity(MAX_PLAYING),
        }
    }

    pub fn handle(&mut self, cmd: ClipCmd) {
        match cmd {
            ClipCmd::Play { clip, gain } => {
                if self.playing.len() >= MAX_PLAYING {
                    self.playing.remove(0);
                }
                self.playing.push(Playing {
                    samples: clip,
                    cursor: 0,
                    gain: gain.clamp(0.0, 2.0),
                });
            }
            ClipCmd::Stop => self.playing.clear(),
        }
    }

    /// Nothing is playing: there is nothing to add.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.playing.is_empty()
    }

    /// Add the playing clips to `out` (one hop, mono); a clip that ends
    /// leaves. The sum may pass ±1: the caller limits it.
    pub fn mix_into(&mut self, out: &mut [f32]) {
        for p in &mut self.playing {
            let rest = &p.samples[p.cursor.min(p.samples.len())..];
            for (o, s) in out.iter_mut().zip(rest) {
                *o += s * p.gain;
            }
            p.cursor += out.len().min(rest.len());
        }
        self.playing.retain(|p| p.cursor < p.samples.len());
    }
}

// ── Decoding ────────────────────────────────────────────────────────────

/// Decode `path` to a [`Clip`].
///
/// # Errors
///
/// [`ClipError`]: too big, too long, or not audio.
pub fn decode_file(path: &Path) -> Result<Clip, ClipError> {
    let meta = std::fs::metadata(path)
        .map_err(|e| ClipError::Unreadable(format!("{}: {e}", path.display())))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(ClipError::TooBig);
    }
    let file = std::fs::File::open(path)
        .map_err(|e| ClipError::Unreadable(format!("{}: {e}", path.display())))?;
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let unreadable =
        |what: &str, e: &dyn std::fmt::Display| ClipError::Unreadable(format!("{what} ({e})"));
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| unreadable("not a format that can be read", &e))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| ClipError::Unreadable("no audio in it".into()))?;
    let track_id = track.id;
    let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
        return Err(ClipError::Unreadable("no audio in it".into()));
    };
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| unreadable("no decoder for this codec", &e))?;

    let limit = MAX_SECONDS * OPUS_SAMPLE_RATE as usize;
    let mut out: Vec<f32> = Vec::new();
    let mut resampler: Option<ToRate> = None;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut mono: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(unreadable("reading it", &e)),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buffer = match decoder.decode(&packet) {
            Ok(buffer) => buffer,
            // A bad packet is a click, not the end of the clip.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(e) => return Err(unreadable("decoding it", &e)),
        };
        let rate = downmix(&buffer, &mut interleaved, &mut mono);
        if rate == 0 || mono.is_empty() {
            continue;
        }
        if rate == OPUS_SAMPLE_RATE {
            out.extend_from_slice(&mono);
        } else {
            let r = match &mut resampler {
                Some(r) if r.rate == rate => r,
                _ => resampler.insert(ToRate::new(rate)?),
            };
            r.push(&mono, &mut out);
        }
        if out.len() > limit {
            return Err(ClipError::TooLong);
        }
    }
    // The resampler holds back less than a chunk: let it out.
    if let Some(r) = &mut resampler {
        r.push(&vec![0.0; RESAMPLE_CHUNK * 2], &mut out);
    }
    if out.len() > limit {
        return Err(ClipError::TooLong);
    }
    if out.is_empty() {
        return Err(ClipError::Unreadable("it has no sound".into()));
    }
    Ok(Clip {
        samples: out.into(),
    })
}

/// The decoded buffer averaged to mono into `mono`; its sample rate (0
/// if it has no channels).
fn downmix(
    buffer: &GenericAudioBufferRef<'_>,
    interleaved: &mut Vec<f32>,
    mono: &mut Vec<f32>,
) -> u32 {
    let spec = buffer.spec();
    let channels = spec.channels().count();
    interleaved.clear();
    mono.clear();
    if channels == 0 {
        return 0;
    }
    buffer.copy_to_vec_interleaved(interleaved);
    let scale = 1.0 / channels as f32;
    mono.extend(
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() * scale),
    );
    spec.rate()
}

/// Mono at `rate` → mono at 48 kHz, the engine's own input resampler
/// (sinc, fixed input chunks).
struct ToRate {
    rate: u32,
    inner: Async<f32>,
    pending: Vec<f32>,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
}

impl ToRate {
    fn new(rate: u32) -> Result<Self, ClipError> {
        let inner = crate::resampler::build_input_resampler(rate, RESAMPLE_CHUNK, 1)
            .map_err(|e| ClipError::Unreadable(format!("no resampler from {rate} Hz ({e})")))?;
        let output = vec![vec![0.0f32; inner.output_frames_max()]];
        Ok(Self {
            rate,
            inner,
            pending: Vec::with_capacity(RESAMPLE_CHUNK * 2),
            input: vec![Vec::with_capacity(RESAMPLE_CHUNK)],
            output,
        })
    }

    /// Feed `samples`; what comes out so far at 48 kHz is appended to `out`.
    fn push(&mut self, samples: &[f32], out: &mut Vec<f32>) {
        self.pending.extend_from_slice(samples);
        loop {
            let needed = self.inner.input_frames_next();
            if self.pending.len() < needed {
                break;
            }
            self.input[0].clear();
            self.input[0].extend_from_slice(&self.pending[..needed]);
            self.pending.drain(..needed);
            let expected = self.inner.output_frames_next();
            self.output[0].resize(expected, 0.0);
            let Ok(input) = SequentialSliceOfVecs::new(&self.input, 1, needed) else {
                break;
            };
            let Ok(mut output) = SequentialSliceOfVecs::new_mut(&mut self.output, 1, expected)
            else {
                break;
            };
            match self.inner.process_into_buffer(&input, &mut output, None) {
                Ok((_, produced)) => out.extend_from_slice(&self.output[0][..produced]),
                Err(_) => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn temp_file(tag: &str, ext: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        std::env::temp_dir().join(format!(
            "baucord-clip-{tag}-{}-{}.{ext}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// A WAV of a 440 Hz sine, `seconds` long, at `rate`, `channels` wide.
    fn write_sine(path: &Path, rate: u32, channels: u16, seconds: f32) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).expect("create wav");
        let frames = (rate as f32 * seconds) as usize;
        for i in 0..frames {
            let t = i as f32 / rate as f32;
            let s = ((t * 440.0 * std::f32::consts::TAU).sin() * 16_000.0) as i16;
            for _ in 0..channels {
                w.write_sample(s).expect("sample");
            }
        }
        w.finalize().expect("finalize");
    }

    #[test]
    fn a_wav_at_48k_comes_back_as_it_is() {
        let path = temp_file("48k", "wav");
        write_sine(&path, 48_000, 1, 0.5);
        let clip = decode_file(&path).expect("decodes");
        let _ = std::fs::remove_file(&path);
        assert_eq!(clip.samples.len(), 24_000);
        let peak = clip.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 16_000.0 / 32_768.0).abs() < 0.01, "peak {peak}");
    }

    #[test]
    fn stereo_44k_becomes_mono_48k_of_the_same_length() {
        let path = temp_file("44k", "wav");
        write_sine(&path, 44_100, 2, 1.0);
        let clip = decode_file(&path).expect("decodes");
        let _ = std::fs::remove_file(&path);
        // One second, within the resampler's chunk granularity.
        let n = clip.samples.len() as i64;
        assert!((n - 48_000).abs() < 2_500, "{n} samples for one second");
        assert!((clip.seconds() - 1.0).abs() < 0.06);
        // The tone survived: its energy is where a 0.49 amplitude sine's is.
        let mid = &clip.samples[12_000..36_000];
        let rms = (mid.iter().map(|s| s * s).sum::<f32>() / mid.len() as f32).sqrt();
        assert!((rms - 0.49 / 2f32.sqrt()).abs() < 0.03, "rms {rms}");
    }

    #[test]
    fn a_long_file_is_refused() {
        let path = temp_file("long", "wav");
        write_sine(&path, 8_000, 1, MAX_SECONDS as f32 + 5.0);
        let r = decode_file(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(r.err(), Some(ClipError::TooLong));
    }

    #[test]
    fn something_that_is_not_audio_is_refused() {
        let path = temp_file("junk", "mp3");
        std::fs::write(
            &path,
            b"this is not audio at all, just words, repeated. ".repeat(200),
        )
        .expect("write");
        let r = decode_file(&path);
        let _ = std::fs::remove_file(&path);
        assert!(matches!(r, Err(ClipError::Unreadable(_))), "{r:?}");
        assert!(matches!(
            decode_file(Path::new("no-such-clip.wav")),
            Err(ClipError::Unreadable(_))
        ));
    }

    #[test]
    fn a_clip_is_added_hop_by_hop_and_leaves_when_done() {
        let clip: Arc<[f32]> = vec![0.5f32; 1_000].into();
        let mut mixer = ClipMixer::new();
        assert!(mixer.is_idle());
        mixer.handle(ClipCmd::Play { clip, gain: 0.5 });
        let mut hop = vec![0.1f32; 480];
        mixer.mix_into(&mut hop);
        assert!(
            hop.iter().all(|s| (s - 0.35).abs() < 1e-6),
            "0.1 + 0.5 × 0.5"
        );
        let mut hop = vec![0.0f32; 480];
        mixer.mix_into(&mut hop);
        assert!(hop.iter().all(|s| (s - 0.25).abs() < 1e-6), "0.5 × 0.5");
        assert!(!mixer.is_idle(), "40 samples are left");
        let mut hop = vec![0.0f32; 480];
        mixer.mix_into(&mut hop);
        assert!(
            hop[..40].iter().all(|s| (s - 0.25).abs() < 1e-6),
            "the last 40"
        );
        assert!(hop[40..].iter().all(|s| s.abs() < 1e-9), "then silence");
        assert!(mixer.is_idle(), "it left");
    }

    #[test]
    fn stop_silences_everything_and_the_oldest_makes_room() {
        let mut mixer = ClipMixer::new();
        let clip: Arc<[f32]> = vec![0.1f32; 48_000].into();
        for _ in 0..MAX_PLAYING + 3 {
            mixer.handle(ClipCmd::Play {
                clip: clip.clone(),
                gain: 1.0,
            });
        }
        assert_eq!(mixer.playing.len(), MAX_PLAYING);
        mixer.handle(ClipCmd::Stop);
        assert!(mixer.is_idle());
        let mut hop = vec![0.0f32; 480];
        mixer.mix_into(&mut hop);
        assert!(hop.iter().all(|s| *s == 0.0));
    }
}
