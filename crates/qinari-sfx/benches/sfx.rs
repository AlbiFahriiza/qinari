//! Criterion benchmarks for qinari-sfx recipes and ambient beds.
//!
//! Recipes render 10 s and the storm bed 60 s of stereo 44.1 kHz audio.
//! Both groups run at sample size 10 because a single render already
//! takes tens to hundreds of milliseconds.

use criterion::{criterion_group, criterion_main, Criterion};
use qinari_sfx::{render, render_ambient, RenderRequest};
use std::hint::black_box;

/// Bench sample rate in hertz (the CLI's default delivery rate).
const SR: u32 = 44_100;
/// Bench channel count.
const CHANNELS: usize = 2;
/// Recipe render length in seconds.
const RECIPE_SECS: f64 = 10.0;
/// Ambient bed length in seconds.
const AMBIENT_SECS: f64 = 60.0;

/// Builds a default-parameter recipe request for `recipe`.
fn recipe_request(recipe: &str) -> RenderRequest {
    RenderRequest {
        recipe: recipe.to_string(),
        params: Vec::new(),
        duration_secs: RECIPE_SECS,
        seed: 1,
        sr: SR,
        ch: CHANNELS,
    }
}

fn bench_recipes(c: &mut Criterion) {
    let mut group = c.benchmark_group("recipes");
    group.sample_size(10);
    for recipe in ["rain", "thunder", "explosion", "laser", "footsteps"] {
        let request = recipe_request(recipe);
        group.bench_function(recipe, |b| {
            b.iter(|| render(black_box(&request)).expect("recipe renders"));
        });
    }
    group.finish();
}

fn bench_ambient(c: &mut Criterion) {
    let mut group = c.benchmark_group("ambient");
    group.sample_size(10);
    group.bench_function("storm_60s", |b| {
        b.iter(|| render_ambient("storm", AMBIENT_SECS, 1, SR, CHANNELS).expect("storm renders"));
    });
    group.finish();
}

criterion_group!(benches, bench_recipes, bench_ambient);
criterion_main!(benches);
