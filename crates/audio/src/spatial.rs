//! Spatial voice: every peer the listener placed on the plane around
//! them is heard from there, so two or three people talking at once
//! are told apart by direction, the way a room does it.
//!
//! The listener sits at the centre facing "up" (+y). A peer at
//! `[x, y]` (`PeerAudioSettings::position`) comes from the azimuth
//! `atan2(x, y)` — 0 straight ahead, +90° the right, ±180° behind —
//! and the distance from the centre is how much of that direction is
//! applied: at the centre none (the voice is the same in both ears,
//! sample for sample what it was without this), at the rim all of it.
//!
//! The rendering is the spherical-head model of Brown and Duda (1998):
//! no measured HRTF set to ship or license, and a few multiply-adds a
//! sample per placed peer on the output worker.
//!
//! - **Time difference** — the far ear hears the voice up to ~0.65 ms
//!   later (Woodworth's formula on an 8.75 cm head): a fractional delay
//!   on each ear.
//! - **Head shadow** — a first-order filter per ear that lifts the
//!   highs up to +6 dB at the near ear and cuts them down to −20 dB at
//!   the far one, flat below ~1 kHz.
//! - **Behind** — a gentle extra high cut for sources behind the
//!   listener, which a bare head model cannot tell from the front.
//!
//! All of it moves smoothly: a dragged peer glides at most a couple of
//! samples of delay per 10 ms hop, so a move is heard as a move and not
//! as clicks. Headphones are what this is for; on speakers the level
//! difference still puts a voice left or right. A mono output plays
//! nobody spatially (`AdaptiveJitterBuffer::set_stereo`): the two ears'
//! delays summed back to one channel would only colour the voice.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use crate::OPUS_SAMPLE_RATE;

/// The model's head: radius (m) and the speed of sound (m/s).
const HEAD_RADIUS: f32 = 0.0875;
const SPEED_OF_SOUND: f32 = 343.0;
/// Head shadow at its deepest, and where (Brown–Duda's `α_min`, `θ_min`).
const ALPHA_MIN: f32 = 0.1;
const THETA_MIN: f32 = 150.0 * PI / 180.0;
/// The delay line both ears read: the longest delay (~32 samples at
/// 48 kHz) and the interpolation's one more, with room. A power of two.
const DELAY_LEN: usize = 64;
const DELAY_MASK: usize = DELAY_LEN - 1;
/// Nearer the centre than this, a peer is not placed at all.
const CENTRE: f32 = 0.05;
/// How far the rendering may move in one hop (10 ms): a delay glides at
/// ~0.4 % pitch at most, and a swing from one side to the other takes
/// ~160 ms.
const MAX_DELAY_STEP: f32 = 2.0;
const MAX_ALPHA_STEP: f32 = 0.1;
const MAX_REAR_STEP: f32 = 0.05;
/// The behind cue: up to half of what is above ~4 kHz.
const REAR_DEPTH: f32 = 0.5;
const REAR_CUTOFF_HZ: f32 = 4_000.0;

/// One ear's share of a direction.
#[derive(Debug, Clone, Copy, PartialEq)]
struct EarParams {
    /// High-frequency gain of the head shadow (1 = flat).
    alpha: f32,
    /// Delay in samples.
    delay: f32,
}

/// A direction as rendered: left ear, right ear, the behind cue.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Params {
    ears: [EarParams; 2],
    rear: f32,
}

impl Params {
    /// Both ears the same, unfiltered: an unplaced peer.
    const NEUTRAL: Self = Self {
        ears: [EarParams {
            alpha: 1.0,
            delay: 0.0,
        }; 2],
        rear: 0.0,
    };

    /// The rendering for `position`; `None` at the centre.
    fn at(position: [f32; 2]) -> Option<Self> {
        let [x, y] = position;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let distance = x.hypot(y);
        if distance < CENTRE {
            return None;
        }
        let amount = distance.min(1.0);
        let azimuth = x.atan2(y);
        // Left ear at −90°, right at +90°.
        let theta = [
            angle_between(azimuth, -FRAC_PI_2),
            angle_between(azimuth, FRAC_PI_2),
        ];
        // Relative to the nearer ear: a voice straight ahead is not
        // delayed at all, as an unplaced one is not.
        let delay = theta.map(delay_samples);
        let nearer = delay[0].min(delay[1]);
        let ear = |i: usize| EarParams {
            alpha: 1.0 + amount * (shadow_alpha(theta[i]) - 1.0),
            delay: amount * (delay[i] - nearer),
        };
        Some(Self {
            ears: [ear(0), ear(1)],
            rear: amount * REAR_DEPTH * (-azimuth.cos()).max(0.0),
        })
    }

    /// One hop's move from here toward `goal`, no further than the
    /// steps allow; exactly `goal` once within a step.
    fn toward(self, goal: Self) -> Self {
        let ear = |from: EarParams, to: EarParams| EarParams {
            alpha: step(from.alpha, to.alpha, MAX_ALPHA_STEP),
            delay: step(from.delay, to.delay, MAX_DELAY_STEP),
        };
        Self {
            ears: [
                ear(self.ears[0], goal.ears[0]),
                ear(self.ears[1], goal.ears[1]),
            ],
            rear: step(self.rear, goal.rear, MAX_REAR_STEP),
        }
    }
}

fn step(from: f32, to: f32, max: f32) -> f32 {
    if (to - from).abs() <= max {
        to
    } else {
        from + max.copysign(to - from)
    }
}

fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

/// The angle between two directions, 0…π.
fn angle_between(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(TAU);
    d.min(TAU - d)
}

/// Head shadow's high-frequency gain at `theta` from the ear's axis:
/// 2 facing the ear, 0.1 at 150° (Brown–Duda).
fn shadow_alpha(theta: f32) -> f32 {
    (1.0 + ALPHA_MIN / 2.0) + (1.0 - ALPHA_MIN / 2.0) * (theta / THETA_MIN * PI).cos()
}

/// How late the sound reaches an ear at `theta` from its axis, in
/// samples: straight along the surface of the head past 90°
/// (Woodworth).
fn delay_samples(theta: f32) -> f32 {
    let path = if theta < FRAC_PI_2 {
        1.0 - theta.cos()
    } else {
        1.0 + theta - FRAC_PI_2
    };
    path * HEAD_RADIUS / SPEED_OF_SOUND * OPUS_SAMPLE_RATE as f32
}

/// One ear's filter state.
#[derive(Debug, Clone, Copy, Default)]
struct Ear {
    /// The head shadow's last input and output.
    x1: f32,
    y1: f32,
    /// The behind cue's low-pass.
    lp: f32,
}

/// One peer's spatial rendering, kept by the jitter buffer next to its
/// decoder.
pub(crate) struct Spatializer {
    /// Where the peer is heard from now, and where it is going (`None`
    /// = nowhere: back to the centre).
    current: Params,
    target: Option<Params>,
    /// The voice, newest at `write`; both ears read it at their delays.
    line: [f32; DELAY_LEN],
    write: usize,
    ears: [Ear; 2],
    /// The line or the filters hold something (`silence` clears them).
    dirty: bool,
    /// The head shadow's bilinear coefficients: `K = fs·a/c`, and
    /// `1/(1+K)`, `a1 = (1−K)/(1+K)`; and the behind cue's one-pole.
    k: f32,
    norm: f32,
    a1: f32,
    rear_coeff: f32,
}

impl Default for Spatializer {
    fn default() -> Self {
        Self::new()
    }
}

impl Spatializer {
    pub(crate) fn new() -> Self {
        let fs = OPUS_SAMPLE_RATE as f32;
        let k = fs * HEAD_RADIUS / SPEED_OF_SOUND;
        let norm = 1.0 / (1.0 + k);
        Self {
            current: Params::NEUTRAL,
            target: None,
            line: [0.0; DELAY_LEN],
            write: 0,
            ears: [Ear::default(); 2],
            dirty: false,
            k,
            norm,
            a1: (1.0 - k) * norm,
            rear_coeff: 1.0 - (-TAU * REAR_CUTOFF_HZ / fs).exp(),
        }
    }

    /// Where to hear this peer from: their saved position, or `None`
    /// (spatial voice off, a mono output) for the centre. The voice
    /// glides there over the next hops.
    pub(crate) fn place(&mut self, position: Option<[f32; 2]>) {
        self.target = position.and_then(Params::at);
    }

    /// Unplaced and arrived: the voice passes through untouched.
    fn is_flat(&self) -> bool {
        self.target.is_none() && self.current == Params::NEUTRAL
    }

    /// Nothing from this peer this hop. What is left in the line would
    /// play stale at their next word, so it goes; and they are heard
    /// from where they were put, at once, rather than gliding there
    /// once they speak.
    pub(crate) fn silence(&mut self) {
        if self.dirty {
            self.line = [0.0; DELAY_LEN];
            self.ears = [Ear::default(); 2];
            self.dirty = false;
        }
        self.current = self.target.unwrap_or(Params::NEUTRAL);
    }

    /// Add `input` (this peer's mono hop) to `left` and `right` as heard
    /// from where they are.
    pub(crate) fn render(&mut self, input: &[f32], left: &mut [f32], right: &mut [f32]) {
        if self.is_flat() {
            for ((&s, l), r) in input.iter().zip(left.iter_mut()).zip(right.iter_mut()) {
                *l += s;
                *r += s;
            }
            return;
        }
        let n = input.len().min(left.len()).min(right.len());
        self.render_placed(input.iter().copied(), n, left, right);
    }

    /// Nothing from this peer this hop: what the line and the filters
    /// still hold (the far ear is up to ~30 samples behind) plays out
    /// into `left` and `right`, then [`Self::silence`].
    pub(crate) fn flush(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.dirty && !self.is_flat() {
            let n = left.len().min(right.len());
            self.render_placed(std::iter::repeat(0.0), n, left, right);
        }
        self.silence();
    }

    fn render_placed(
        &mut self,
        input: impl Iterator<Item = f32>,
        n: usize,
        left: &mut [f32],
        right: &mut [f32],
    ) {
        let from = self.current;
        let to = from.toward(self.target.unwrap_or(Params::NEUTRAL));
        let inv = 1.0 / n.max(1) as f32;
        for (i, ((s, l), r)) in input
            .zip(left.iter_mut())
            .zip(right.iter_mut())
            .take(n)
            .enumerate()
        {
            self.line[self.write] = s;
            let t = (i + 1) as f32 * inv;
            let rear = lerp(from.rear, to.rear, t);
            *l += self.ear(0, from.ears[0], to.ears[0], t, rear);
            *r += self.ear(1, from.ears[1], to.ears[1], t, rear);
            self.write = (self.write + 1) & DELAY_MASK;
        }
        self.current = to;
        self.dirty = true;
        if self.is_flat() {
            // Back at the centre: pass-through from the next hop on.
            self.silence();
        }
    }

    /// One ear's sample for the newest input, `t` of the way through
    /// this hop's glide.
    fn ear(&mut self, which: usize, from: EarParams, to: EarParams, t: f32, rear: f32) -> f32 {
        let x = self.tap(lerp(from.delay, to.delay, t));
        let alpha = lerp(from.alpha, to.alpha, t);
        let b0 = (1.0 + alpha * self.k) * self.norm;
        let b1 = (1.0 - alpha * self.k) * self.norm;
        let ear = &mut self.ears[which];
        let y = b0 * x + b1 * ear.x1 - self.a1 * ear.y1;
        ear.x1 = x;
        ear.y1 = y;
        ear.lp += self.rear_coeff * (y - ear.lp);
        y - rear * (y - ear.lp)
    }

    /// The line `delay` samples back from the newest (linear
    /// interpolation between the two around it).
    fn tap(&self, delay: f32) -> f32 {
        let delay = delay.clamp(0.0, (DELAY_LEN - 2) as f32);
        let whole = delay.floor();
        let frac = delay - whole;
        let m = whole as usize;
        let a = self.line[(self.write + DELAY_LEN - m) & DELAY_MASK];
        let b = self.line[(self.write + DELAY_LEN - m - 1) & DELAY_MASK];
        a + frac * (b - a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOP: usize = 480;

    /// Deterministic noise: broadband, so both the delay and the
    /// shadow show.
    fn noise(seed: &mut u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|_| {
                *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (*seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect()
    }

    /// `hops` of noise through `s`; the last hop's two ears.
    fn run(s: &mut Spatializer, hops: usize) -> (Vec<f32>, Vec<f32>) {
        let mut seed = 7;
        let mut out = (Vec::new(), Vec::new());
        for _ in 0..hops {
            let input = noise(&mut seed, HOP);
            let mut l = vec![0.0; HOP];
            let mut r = vec![0.0; HOP];
            s.render(&input, &mut l, &mut r);
            out = (l, r);
        }
        out
    }

    fn energy(x: &[f32]) -> f32 {
        x.iter().map(|s| s * s).sum()
    }

    #[test]
    fn an_unplaced_peer_is_untouched() {
        for position in [None, Some([0.0, 0.0]), Some([0.01, -0.02])] {
            let mut s = Spatializer::new();
            s.place(position);
            let input: Vec<f32> = noise(&mut 3, HOP);
            let mut l = vec![0.5; HOP];
            let mut r = vec![0.5; HOP];
            s.render(&input, &mut l, &mut r);
            for ((&x, &l), &r) in input.iter().zip(&l).zip(&r) {
                assert_eq!(l.to_bits(), (x + 0.5).to_bits(), "{position:?}");
                assert_eq!(r.to_bits(), (x + 0.5).to_bits(), "{position:?}");
            }
        }
    }

    #[test]
    fn a_peer_on_the_right_is_louder_and_earlier_on_the_right() {
        let mut s = Spatializer::new();
        s.place(Some([1.0, 0.0]));
        s.silence(); // there at once
        let (l, r) = run(&mut s, 10);
        assert!(
            energy(&r) > 2.0 * energy(&l),
            "{} vs {}",
            energy(&r),
            energy(&l)
        );
        // The left ear hears the right ear's signal later: ~31 samples
        // of path, and the shadow's own lag on top of it at the low end
        // (the model's low-frequency difference is the larger, as a
        // real head's is).
        let lag = |d: usize| -> f32 { r[..HOP - d].iter().zip(&l[d..]).map(|(a, b)| a * b).sum() };
        let best = (0..60).max_by(|&a, &b| lag(a).total_cmp(&lag(b))).unwrap();
        assert!((26..=46).contains(&best), "lag {best}");
    }

    #[test]
    fn left_mirrors_right() {
        let mut right = Spatializer::new();
        right.place(Some([0.8, 0.3]));
        right.silence();
        let mut left = Spatializer::new();
        left.place(Some([-0.8, 0.3]));
        left.silence();
        let (rl, rr) = run(&mut right, 5);
        let (ll, lr) = run(&mut left, 5);
        for (a, b) in rl.iter().zip(&lr).chain(rr.iter().zip(&ll)) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn straight_ahead_is_the_same_in_both_ears() {
        let mut s = Spatializer::new();
        s.place(Some([0.0, 1.0]));
        s.silence();
        let (l, r) = run(&mut s, 5);
        assert!(energy(&l) > 0.0);
        for (a, b) in l.iter().zip(&r) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn behind_is_duller_than_ahead() {
        let hf = |position| {
            let mut s = Spatializer::new();
            s.place(Some(position));
            s.silence();
            let (l, _) = run(&mut s, 5);
            // First difference: a crude high-pass.
            let d: Vec<f32> = l.windows(2).map(|w| w[1] - w[0]).collect();
            energy(&d) / energy(&l)
        };
        assert!(hf([0.0, -1.0]) < hf([0.0, 1.0]));
    }

    #[test]
    fn a_move_glides_and_a_return_to_the_centre_ends_flat() {
        let mut s = Spatializer::new();
        s.place(Some([-1.0, 0.0]));
        s.silence();
        run(&mut s, 3);
        s.place(Some([1.0, 0.0]));
        // One hop moves the delays two samples, not thirty.
        run(&mut s, 1);
        assert!(s.current.ears[1].delay > 25.0, "{:?}", s.current);
        run(&mut s, 30);
        assert_eq!(s.current, s.target.unwrap());
        s.place(None);
        run(&mut s, 30);
        assert!(s.is_flat());
    }
}
