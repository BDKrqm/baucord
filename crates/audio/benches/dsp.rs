//! Per-sample paths of the voice pipeline, timed.
//!
//! `cargo bench -p audio` — or `cargo bench -p audio -- resampler` for one
//! group. Both paths run on real-time threads with a 20 ms budget, so a
//! regression here is a dropout, not a slow build: the numbers are the
//! thing to compare before and after touching `resampler.rs`,
//! `transport/jitter_buffer.rs` or the Opus settings.
//!
//! - `resampler`: one 10 ms device chunk through the sinc resampler, the
//!   input (44.1 kHz → 48 kHz) and output (48 kHz → 44.1 kHz) builds
//!   `input.rs` and `output.rs` use for a non-48 kHz device.
//! - `jitter_buffer`: one 20 ms tick of the output worker — a packet per
//!   speaking peer pushed, one stereo frame pulled and mixed — for one
//!   and for four peers, and for four placed in the voice space.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use audio::resampler::{build_input_resampler, build_output_resampler, frames_for_ms};
use audio::transport::jitter_buffer::{AdaptiveJitterBuffer, PeerTable};
use audio::{AudioPacket, FRAME_SIZE};
use audioadapter_buffers::direct::SequentialSliceOfVecs;
use criterion::{criterion_group, criterion_main, Criterion};
use rubato::Resampler;

/// A 10 ms mono chunk of a 440 Hz tone at `rate`.
fn tone(rate: u32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let t = i as f32 / rate as f32;
            (t * 440.0 * std::f32::consts::TAU).sin() * 0.5
        })
        .collect()
}

fn resampler(c: &mut Criterion) {
    let mut group = c.benchmark_group("resampler");

    // Input path: a 44.1 kHz device callback's worth of frames to 48 kHz.
    let chunk = frames_for_ms(44_100, 10);
    let mut r = build_input_resampler(44_100, chunk, 1).expect("input resampler");
    let input = vec![tone(44_100, r.input_frames_next())];
    let mut output = vec![vec![0.0f32; r.output_frames_max()]];
    group.bench_function("input_44100_to_48k_10ms", |b| {
        b.iter(|| {
            let n = r.input_frames_next();
            let expected = r.output_frames_next();
            let ia = SequentialSliceOfVecs::new(&input, 1, n).expect("input adapter");
            let mut oa =
                SequentialSliceOfVecs::new_mut(&mut output, 1, expected).expect("output adapter");
            let (_, out_len) = r.process_into_buffer(&ia, &mut oa, None).expect("resample");
            black_box(out_len)
        });
    });

    // Output path: the stereo 48 kHz mix (the two ears of spatial voice)
    // down to a 44.1 kHz device, 10 ms of device frames per call.
    let mut r = build_output_resampler(44_100, chunk, 2).expect("output resampler");
    let input = vec![tone(48_000, r.input_frames_max()); 2];
    let mut output = vec![vec![0.0f32; chunk]; 2];
    group.bench_function("output_48k_to_44100_10ms", |b| {
        b.iter(|| {
            let n = r.input_frames_next();
            let ia = SequentialSliceOfVecs::new(&input, 2, n).expect("input adapter");
            let mut oa =
                SequentialSliceOfVecs::new_mut(&mut output, 2, chunk).expect("output adapter");
            let (_, out_len) = r.process_into_buffer(&ia, &mut oa, None).expect("resample");
            black_box(out_len)
        });
    });

    group.finish();
}

/// A ring of real Opus payloads (voice mode, 48 kHz mono, one 20 ms frame each)
/// so a bench iteration pays for a real decode, not for an empty packet.
fn payloads() -> Vec<Vec<u8>> {
    let mut enc = opus::Encoder::new(48_000, opus::Channels::Mono, opus::Application::Voip)
        .expect("opus encoder");
    let mut buf = vec![0u8; 1024];
    (0..50u32)
        .map(|frame| {
            let pcm: Vec<f32> = (0..FRAME_SIZE)
                .map(|i| {
                    #[allow(clippy::cast_precision_loss)]
                    let t = (frame as usize * FRAME_SIZE + i) as f32 / 48_000.0;
                    (t * 300.0 * std::f32::consts::TAU).sin() * 0.4
                })
                .collect();
            let n = enc.encode_float(&pcm, &mut buf).expect("opus encode");
            buf[..n].to_vec()
        })
        .collect()
}

/// One output-worker tick with `peers` speaking: push each peer's packet,
/// pull one mixed stereo frame. `placed`: every peer sits somewhere in
/// the voice space (`spatial.rs`, the costly path), else all are in the
/// middle (a plain copy into both ears).
fn tick(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    peers: u64,
    placed: bool,
) {
    let payloads = payloads();
    let table = Arc::new(PeerTable::default());
    let mut jb = AdaptiveJitterBuffer::new(48_000, 10, table.clone());
    jb.set_stereo(true);
    if placed {
        for ssrc in 1..=peers {
            let side = if ssrc % 2 == 0 { 0.7 } else { -0.7 };
            table.set(
                ssrc,
                audio::PeerAudioSettings {
                    position: [side, 0.5],
                    ..audio::PeerAudioSettings::default()
                },
            );
        }
    }
    let mut left = vec![0.0f32; FRAME_SIZE];
    let mut right = vec![0.0f32; FRAME_SIZE];
    let mut seq: u32 = 0;
    #[allow(clippy::cast_possible_truncation)]
    let frame = FRAME_SIZE as u32;

    let name = if placed {
        format!("tick_{peers}_peer_placed")
    } else {
        format!("tick_{peers}_peer")
    };
    group.bench_function(name, |b| {
        b.iter(|| {
            let now = Instant::now();
            let payload = &payloads[seq as usize % payloads.len()];
            for ssrc in 1..=peers {
                jb.push_packet(
                    AudioPacket::new(seq, seq.wrapping_mul(frame), ssrc, payload.clone()),
                    now,
                );
            }
            seq = seq.wrapping_add(1);
            jb.pull_mixed(FRAME_SIZE, &mut left, &mut right);
            black_box((left[0], right[0]))
        });
    });
}

fn jitter_buffer(c: &mut Criterion) {
    let mut group = c.benchmark_group("jitter_buffer");
    tick(&mut group, 1, false);
    tick(&mut group, 4, false);
    tick(&mut group, 4, true);
    group.finish();
}

criterion_group!(benches, resampler, jitter_buffer);
criterion_main!(benches);
