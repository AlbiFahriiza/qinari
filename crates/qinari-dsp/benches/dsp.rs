//! Criterion benchmarks for the qinari-dsp hot paths.
//!
//! Every case processes exactly one second of audio at 48 kHz: a biquad
//! lowpass over a noise buffer, a `PolyBLEP` saw render, and a full
//! integrated-LUFS measurement of a stereo tone.

use criterion::{criterion_group, criterion_main, Criterion};
use qinari_dsp::buffer::AudioBuffer;
use qinari_dsp::filter::{Biquad, BiquadKind};
use qinari_dsp::lufs::integrated_lufs;
use qinari_dsp::osc::{Osc, OscKind};
use qinari_dsp::rng::Qrng;
use std::hint::black_box;

/// Bench sample rate in hertz.
const SR: f32 = 48_000.0;
/// Frames in one second at [`SR`].
const FRAMES: usize = 48_000;

/// Renders the same sine tone on every channel at [`SR`].
fn tone_buffer(freq: f32, amplitude: f32, secs: f32, channels: usize) -> AudioBuffer {
    let frames = (secs * SR).round() as usize;
    let channel: Vec<f32> = (0..frames)
        .map(|n| amplitude * (2.0 * std::f32::consts::PI * freq * n as f32 / SR).sin())
        .collect();
    AudioBuffer::from_channels(vec![channel; channels], SR as u32)
        .expect("tone buffer is well formed")
}

fn bench_biquad_lowpass(c: &mut Criterion) {
    let mut rng = Qrng::new(11);
    let input: Vec<f32> = (0..FRAMES).map(|_| rng.next_f32() * 2.0 - 1.0).collect();
    c.bench_function("biquad_lowpass_1s", |b| {
        b.iter(|| {
            let mut filter = Biquad::design(
                BiquadKind::Lowpass,
                SR,
                1000.0,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
            );
            let mut output = Vec::with_capacity(input.len());
            for &sample in black_box(&input) {
                output.push(filter.process(sample));
            }
            output
        });
    });
}

fn bench_polyblep_saw(c: &mut Criterion) {
    c.bench_function("polyblep_saw_1s", |b| {
        b.iter(|| {
            let mut rng = Qrng::new(7);
            let mut osc = Osc::new(OscKind::Saw, 220.0, SR, &mut rng);
            let mut output = Vec::with_capacity(FRAMES);
            for _ in 0..FRAMES {
                output.push(osc.process());
            }
            output
        });
    });
}

fn bench_integrated_lufs(c: &mut Criterion) {
    let buffer = tone_buffer(997.0, 0.5, 1.0, 2);
    c.bench_function("integrated_lufs_1s", |b| {
        b.iter(|| integrated_lufs(black_box(&buffer)));
    });
}

criterion_group!(
    benches,
    bench_biquad_lowpass,
    bench_polyblep_saw,
    bench_integrated_lufs
);
criterion_main!(benches);
