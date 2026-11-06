//! Criterion benchmarks for qinari-music track renders.
//!
//! Each style composes 30 s of stereo 44.1 kHz audio at its typical
//! tempo. The group runs at sample size 10 because a full render takes
//! a second or more.

use criterion::{criterion_group, criterion_main, Criterion};
use qinari_dsp::rng::Seed;
use qinari_music::{render, TrackRequest};
use std::hint::black_box;

/// Bench sample rate in hertz (the CLI's default delivery rate).
const SR: u32 = 44_100;
/// Track length in seconds.
const TRACK_SECS: f32 = 30.0;

fn bench_styles(c: &mut Criterion) {
    let mut group = c.benchmark_group("styles");
    group.sample_size(10);
    // (id, bpm) pairs carry each style's typical tempo from STYLES.
    for (style, bpm) in [("lofi", 82.0), ("synthwave", 110.0), ("cinematic", 88.0)] {
        let request = TrackRequest {
            style: style.to_string(),
            duration_secs: TRACK_SECS,
            bpm,
            seed: Seed::from_u64(1),
            sample_rate: SR,
            channels: 2,
        };
        group.bench_function(style, |b| {
            b.iter(|| render(black_box(&request)).expect("style renders"));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_styles);
criterion_main!(benches);
