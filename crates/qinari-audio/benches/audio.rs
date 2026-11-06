//! Criterion benchmarks for the qinari-audio delivery pipeline.
//!
//! Resampling converts a 5 s stereo buffer from 44.1 kHz to 48 kHz,
//! mastering runs the default house chain over 10 s, and the mp3 case
//! encodes 5 s to a file in the system temp directory.

use criterion::{criterion_group, criterion_main, Criterion};
use qinari_audio::{master, resample, write_mp3, MasteringSettings, Mp3Bitrate};
use qinari_dsp::buffer::AudioBuffer;
use std::hint::black_box;

/// Renders the same sine tone on every channel.
fn tone_buffer(
    freq: f32,
    amplitude: f32,
    secs: f32,
    channels: usize,
    sample_rate: u32,
) -> AudioBuffer {
    let sr = sample_rate as f32;
    let frames = (secs * sr).round() as usize;
    let channel: Vec<f32> = (0..frames)
        .map(|n| amplitude * (2.0 * std::f32::consts::PI * freq * n as f32 / sr).sin())
        .collect();
    AudioBuffer::from_channels(vec![channel; channels], sample_rate)
        .expect("tone buffer is well formed")
}

fn bench_resample(c: &mut Criterion) {
    let input = tone_buffer(997.0, 0.5, 5.0, 2, 44_100);
    c.bench_function("resample_44100_to_48000_5s", |b| {
        b.iter(|| resample(black_box(&input), 48_000).expect("resample succeeds"));
    });
}

fn bench_master(c: &mut Criterion) {
    let input = tone_buffer(997.0, 0.5, 10.0, 2, 48_000);
    let settings = MasteringSettings::default();
    c.bench_function("master_10s_default_chain", |b| {
        b.iter(|| master(black_box(&input), black_box(&settings)));
    });
}

fn bench_write_mp3(c: &mut Criterion) {
    let input = tone_buffer(997.0, 0.5, 5.0, 2, 44_100);
    let mut path = std::env::temp_dir();
    path.push("qinari-audio-bench.mp3");
    c.bench_function("write_mp3_5s_320kbps", |b| {
        b.iter(|| {
            write_mp3(black_box(&path), black_box(&input), Mp3Bitrate::Kbps320)
                .expect("mp3 encode succeeds");
        });
    });
}

criterion_group!(benches, bench_resample, bench_master, bench_write_mp3);
criterion_main!(benches);
