//! Equalizers on the output side, both on the output worker:
//!
//! - [`PeerEq`] — one peer's own five bands, as the listener set them
//!   for that person (`PeerAudioSettings::eq`): a thin voice given body,
//!   a boomy one cleaned up, a muffled microphone brought forward.
//!   Applied after the peer's volume and before their limiter, so a
//!   boost cannot get past the hearing protection.
//! - [`ToneFilter`] — the voice tone over the whole voice mix, both ears
//!   alike ([`VoiceTone`]: natural, crisp, bass boosted). Notifications
//!   and a watched stream's sound do not go through it.
//!
//! Every band is an RBJ "Audio EQ Cookbook" biquad, in transposed direct
//! form II. A band at 0 dB is skipped, and an equalizer with every band
//! at 0 dB passes the samples through untouched.

use std::f32::consts::{FRAC_1_SQRT_2, TAU};

use crate::OPUS_SAMPLE_RATE;

/// How many bands a peer's equalizer has.
pub const EQ_BANDS: usize = 5;
/// Their centres (Hz): a low shelf, three peaks, a high shelf.
pub const EQ_BAND_HZ: [f32; EQ_BANDS] = [100.0, 300.0, 1_000.0, 3_000.0, 8_000.0];
/// How far a band goes either way (dB).
pub const EQ_MAX_DB: f32 = 12.0;
/// The peaks' width: ~1.4 octaves, so neighbouring bands overlap into
/// a smooth curve rather than five bumps.
const PEAK_Q: f32 = 1.0;
/// Under this a band is flat.
const FLAT_DB: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    LowShelf,
    Peak,
    HighShelf,
}

/// One band's shape, centre, gain and width.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    shape: Shape,
    hz: f32,
    db: f32,
    q: f32,
}

/// A biquad's coefficients, normalized by `a0`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Coeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coeffs {
    const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// The cookbook's formulas at 48 kHz. Shelves take `q` as their
    /// slope's Q (`1/√2`: the steepest without a bump).
    fn design(band: Band) -> Self {
        let a = 10f32.powf(band.db / 40.0);
        let w0 = TAU * band.hz / OPUS_SAMPLE_RATE as f32;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * band.q);
        let (b0, b1, b2, a0, a1, a2) = match band.shape {
            Shape::Peak => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
            Shape::LowShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + s),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - s),
                    (a + 1.0) + (a - 1.0) * cos + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - s,
                )
            }
            Shape::HighShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - s),
                    (a + 1.0) - (a - 1.0) * cos + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - s,
                )
            }
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }
}

/// One biquad's memory (transposed direct form II).
#[derive(Debug, Clone, Copy, Default)]
struct State {
    z1: f32,
    z2: f32,
}

impl State {
    #[inline]
    fn step(&mut self, c: &Coeffs, x: f32) -> f32 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }
}

/// The shape of peer band `i`.
fn peer_band(i: usize, db: f32) -> Band {
    let shape = match i {
        0 => Shape::LowShelf,
        i if i == EQ_BANDS - 1 => Shape::HighShelf,
        _ => Shape::Peak,
    };
    Band {
        shape,
        hz: EQ_BAND_HZ[i],
        db,
        q: if shape == Shape::Peak {
            PEAK_Q
        } else {
            FRAC_1_SQRT_2
        },
    }
}

/// One peer's five-band equalizer (mono: before their place in the
/// voice space is applied).
pub struct PeerEq {
    gains: [f32; EQ_BANDS],
    coeffs: [Coeffs; EQ_BANDS],
    state: [State; EQ_BANDS],
}

impl Default for PeerEq {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerEq {
    #[must_use]
    pub fn new() -> Self {
        Self {
            gains: [0.0; EQ_BANDS],
            coeffs: [Coeffs::IDENTITY; EQ_BANDS],
            state: [State::default(); EQ_BANDS],
        }
    }

    /// The bands' gains in dB (each kept within ±[`EQ_MAX_DB`]); the
    /// bands that changed are designed again. A band switched to flat
    /// forgets its memory.
    pub fn set(&mut self, gains: [f32; EQ_BANDS]) {
        for (i, &db) in gains.iter().enumerate() {
            let db = if db.is_finite() {
                db.clamp(-EQ_MAX_DB, EQ_MAX_DB)
            } else {
                0.0
            };
            if db.to_bits() == self.gains[i].to_bits() {
                continue;
            }
            self.gains[i] = db;
            if db.abs() < FLAT_DB {
                self.coeffs[i] = Coeffs::IDENTITY;
                self.state[i] = State::default();
            } else {
                self.coeffs[i] = Coeffs::design(peer_band(i, db));
            }
        }
    }

    /// Every band flat: [`Self::process`] does nothing.
    #[must_use]
    pub fn is_flat(&self) -> bool {
        self.gains.iter().all(|g| g.abs() < FLAT_DB)
    }

    /// Equalize `samples` in place.
    pub fn process(&mut self, samples: &mut [f32]) {
        for ((c, state), g) in self.coeffs.iter().zip(&mut self.state).zip(&self.gains) {
            if g.abs() < FLAT_DB {
                continue;
            }
            for s in samples.iter_mut() {
                *s = state.step(c, *s);
            }
        }
    }
}

/// The voice tone: one colour over everyone's voice, picked in
/// Settings → Audio (`audio.voice_tone` in `settings.json`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceTone {
    /// As sent: no equalizer at all (the default).
    #[default]
    Natural,
    /// Clearer words: less rumble, more presence and air.
    Crisp,
    /// Fuller voices: more low end, a touch less mud.
    BassBoost,
}

impl VoiceTone {
    /// Every tone, in the order the settings picker shows them.
    pub const ALL: [Self; 3] = [Self::Natural, Self::Crisp, Self::BassBoost];

    /// Position in [`Self::ALL`] (the picker index).
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// The tone at picker index `i`; out of range → `Natural`.
    #[must_use]
    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or_default()
    }

    fn bands(self) -> &'static [Band] {
        const CRISP: [Band; 3] = [
            Band {
                shape: Shape::LowShelf,
                hz: 120.0,
                db: -3.0,
                q: FRAC_1_SQRT_2,
            },
            Band {
                shape: Shape::Peak,
                hz: 3_200.0,
                db: 4.0,
                q: 0.8,
            },
            Band {
                shape: Shape::HighShelf,
                hz: 8_000.0,
                db: 3.0,
                q: FRAC_1_SQRT_2,
            },
        ];
        const BASS: [Band; 2] = [
            Band {
                shape: Shape::LowShelf,
                hz: 110.0,
                db: 7.0,
                q: FRAC_1_SQRT_2,
            },
            Band {
                shape: Shape::Peak,
                hz: 350.0,
                db: -1.5,
                q: 1.0,
            },
        ];
        match self {
            Self::Natural => &[],
            Self::Crisp => &CRISP,
            Self::BassBoost => &BASS,
        }
    }

    /// Wire value for the engine's atomic (`PeerTable`).
    #[must_use]
    pub fn to_u8(self) -> u8 {
        // Three variants: the index always fits.
        self.index() as u8
    }

    /// The tone [`Self::to_u8`] made.
    #[must_use]
    pub fn from_u8(v: u8) -> Self {
        Self::from_index(usize::from(v))
    }
}

/// Most bands a tone has.
const TONE_BANDS: usize = 3;

/// The voice tone over the stereo voice mix.
pub struct ToneFilter {
    tone: VoiceTone,
    coeffs: [Coeffs; TONE_BANDS],
    bands: usize,
    /// Per ear, per band.
    state: [[State; TONE_BANDS]; 2],
}

impl Default for ToneFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl ToneFilter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tone: VoiceTone::Natural,
            coeffs: [Coeffs::IDENTITY; TONE_BANDS],
            bands: 0,
            state: [[State::default(); TONE_BANDS]; 2],
        }
    }

    /// Switch to `tone` (nothing to do if it already is).
    pub fn set(&mut self, tone: VoiceTone) {
        if tone == self.tone {
            return;
        }
        self.tone = tone;
        let bands = tone.bands();
        self.bands = bands.len().min(TONE_BANDS);
        for (c, band) in self.coeffs.iter_mut().zip(bands) {
            *c = Coeffs::design(*band);
        }
        self.state = [[State::default(); TONE_BANDS]; 2];
    }

    /// Colour both ears in place.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (ear, samples) in self.state.iter_mut().zip([left, right]) {
            for (c, state) in self.coeffs[..self.bands].iter().zip(ear.iter_mut()) {
                for s in samples.iter_mut() {
                    *s = state.step(c, *s);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// |H| in dB at `hz`, from the coefficients.
    fn response_db(c: &Coeffs, hz: f32) -> f32 {
        let w = f64::from(TAU * hz / OPUS_SAMPLE_RATE as f32);
        let (b0, b1, b2) = (f64::from(c.b0), f64::from(c.b1), f64::from(c.b2));
        let (a1, a2) = (f64::from(c.a1), f64::from(c.a2));
        // z^-1 = e^{-jw}
        let (c1, s1, c2, s2) = (w.cos(), -w.sin(), (2.0 * w).cos(), -(2.0 * w).sin());
        let num = ((b0 + b1 * c1 + b2 * c2).powi(2) + (b1 * s1 + b2 * s2).powi(2)).sqrt();
        let den = ((1.0 + a1 * c1 + a2 * c2).powi(2) + (a1 * s1 + a2 * s2).powi(2)).sqrt();
        (20.0 * (num / den).log10()) as f32
    }

    fn sine(hz: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (TAU * hz * i as f32 / OPUS_SAMPLE_RATE as f32).sin() * 0.25)
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn each_band_does_what_it_says() {
        for (i, &hz) in EQ_BAND_HZ.iter().enumerate() {
            let c = Coeffs::design(peer_band(i, 6.0));
            let at = response_db(&c, hz);
            match i {
                // Shelves: half their gain at the corner, all of it
                // well past it.
                0 => {
                    assert!((at - 3.0).abs() < 0.5, "low shelf at corner {at}");
                    assert!((response_db(&c, 20.0) - 6.0).abs() < 0.3);
                }
                4 => {
                    assert!((at - 3.0).abs() < 0.5, "high shelf at corner {at}");
                    assert!((response_db(&c, 20_000.0) - 6.0).abs() < 0.6);
                }
                _ => assert!((at - 6.0).abs() < 0.05, "peak {hz} Hz: {at}"),
            }
            // Far from a peak, nothing.
            if (1..4).contains(&i) {
                assert!(response_db(&c, hz * 16.0).abs() < 0.5);
                assert!(response_db(&c, hz / 16.0).abs() < 0.5);
            }
        }
    }

    #[test]
    fn a_flat_equalizer_leaves_the_samples_alone() {
        let mut eq = PeerEq::new();
        eq.set([0.0; EQ_BANDS]);
        assert!(eq.is_flat());
        let input = sine(440.0, 960);
        let mut out = input.clone();
        eq.process(&mut out);
        assert!(input
            .iter()
            .zip(&out)
            .all(|(a, b)| a.to_bits() == b.to_bits()));
    }

    #[test]
    fn a_boost_and_a_cut_are_heard() {
        let mut eq = PeerEq::new();
        eq.set([0.0, 0.0, 0.0, 9.0, 0.0]);
        let mut boosted = sine(3_000.0, 9_600);
        eq.process(&mut boosted);
        let gain = 20.0 * (rms(&boosted[4_800..]) / rms(&sine(3_000.0, 4_800))).log10();
        assert!((gain - 9.0).abs() < 0.5, "+9 dB band gave {gain}");
        eq.set([0.0, -9.0, 0.0, 0.0, 0.0]);
        let mut cut = sine(300.0, 9_600);
        eq.process(&mut cut);
        let gain = 20.0 * (rms(&cut[4_800..]) / rms(&sine(300.0, 4_800))).log10();
        assert!((gain + 9.0).abs() < 0.5, "-9 dB band gave {gain}");
    }

    #[test]
    fn gains_are_kept_in_range() {
        let mut eq = PeerEq::new();
        eq.set([40.0, -40.0, f32::NAN, 0.0, 0.0]);
        assert_eq!(eq.gains[0].to_bits(), EQ_MAX_DB.to_bits());
        assert_eq!(eq.gains[1].to_bits(), (-EQ_MAX_DB).to_bits());
        assert_eq!(eq.gains[2].to_bits(), 0f32.to_bits());
    }

    #[test]
    fn the_tones_lean_the_way_their_names_say() {
        let level = |tone: VoiceTone, hz: f32| {
            let mut f = ToneFilter::new();
            f.set(tone);
            let (mut l, mut r) = (sine(hz, 9_600), sine(hz, 9_600));
            f.process(&mut l, &mut r);
            assert!(l.iter().zip(&r).all(|(a, b)| a.to_bits() == b.to_bits()));
            20.0 * (rms(&l[4_800..]) / rms(&sine(hz, 4_800))).log10()
        };
        assert!(level(VoiceTone::Natural, 40.0).abs() < 1e-4);
        assert!(level(VoiceTone::Natural, 4_000.0).abs() < 1e-4);
        // Well below the shelves' corners (a shelf has only half its
        // gain at the corner itself).
        assert!(level(VoiceTone::Crisp, 3_200.0) > 3.0);
        assert!(level(VoiceTone::Crisp, 40.0) < -2.0);
        assert!(level(VoiceTone::BassBoost, 40.0) > 5.0);
        assert!(level(VoiceTone::BassBoost, 3_200.0).abs() < 0.5);
    }

    #[test]
    fn tones_round_trip_through_the_picker_and_the_atomic() {
        for tone in VoiceTone::ALL {
            assert_eq!(VoiceTone::from_index(tone.index()), tone);
            assert_eq!(VoiceTone::from_u8(tone.to_u8()), tone);
        }
        assert_eq!(VoiceTone::from_index(9), VoiceTone::Natural);
    }
}
