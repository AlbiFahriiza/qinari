//! The 29 procedural sound-effect recipes.
//!
//! Every recipe is a pure function of a [`RenderContext`]: the validated
//! parameters, the sample rate, the channel count, the frame count, and a
//! seed. Random decisions never touch system randomness; each recipe
//! derives independent [`Qrng`] streams from the context seed through
//! text tags (`recipe:subsystem:seed`), so two renders with the same seed
//! are bit-identical, and changing one layer's draw count never shifts
//! another layer's stream.
//!
//! Recipes accumulate layers into a [`Bus`] with [`Bus::add_panned`] /
//! the internal `add_layer` helper (equal-power panning, drift-capable),
//! then hand the buffer back. Shaped-noise helpers from
//! `qinari_dsp::noise` peak-normalize to 0.7; recipes scale them by design
//! levels instead of renormalizing.
//!
//! Two timing shapes exist: beds (rain, wind, fire, ...) fill the whole
//! requested duration with edge fades, and one-shots (laser, `ui_click`,
//! ...) place their event at frame 0 and let the bus truncate whatever
//! runs past the requested length.

// The 29 render functions match the `Recipe::render` fn-pointer type whose
// `Result` return is part of the crate contract: recipes are allowed to
// fail even though none of today's implementations can, so clippy's
// `unnecessary_wraps` is a planned false positive here.
#![allow(clippy::unnecessary_wraps)]

use crate::params::spec;
use crate::params::{ParamSet, ParamSpec};
use qinari_dsp::buffer::AudioBuffer;
use qinari_dsp::env::{Adsr, Percussive};
use qinari_dsp::filter::{Biquad, BiquadKind, Svf};
use qinari_dsp::fx::{Crush, Drive, Tremolo};
use qinari_dsp::lfo::{Lfo, LfoShape};
use qinari_dsp::noise::{noise_burst, swept_noise, NoiseGen, NoiseKind};
use qinari_dsp::osc::{fast_sine, Osc, OscKind};
use qinari_dsp::pan::pan;
use qinari_dsp::rng::{Qrng, Seed};
use strsim::levenshtein;

/// Butterworth quality for standard 2nd-order designs.
const BUTTERWORTH: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Everything a recipe needs to render one sound.
#[derive(Debug, Clone)]
pub struct RenderContext {
    /// Validated parameters for the recipe.
    pub params: ParamSet,
    /// Reproducibility seed; every random decision in the render derives
    /// from it.
    pub seed: u64,
    /// Sample rate in hertz.
    pub sr: u32,
    /// Output channel count (1 or 2).
    pub channels: usize,
    /// Output length in frames.
    pub frames: usize,
}

/// A render bus accumulating layers into an output buffer.
///
/// Layers are added per frame or as panned slices; out-of-range writes are
/// ignored, which is how one-shots get truncated to the requested length.
#[derive(Debug, Clone)]
pub struct Bus {
    sr: u32,
    channels: usize,
    planes: Vec<Vec<f32>>,
}

impl Bus {
    /// Creates a silent bus of `channels` channels and `frames` frames.
    pub fn new(channels: usize, frames: usize, sr: u32) -> Self {
        Self {
            sr,
            channels,
            planes: vec![vec![0.0; frames]; channels.max(1)],
        }
    }

    /// Adds `value` to `channel` at `frame`; out-of-range writes are
    /// ignored.
    pub fn add(&mut self, frame: usize, channel: usize, value: f32) {
        if let Some(plane) = self.planes.get_mut(channel) {
            if let Some(slot) = plane.get_mut(frame) {
                *slot += value;
            }
        }
    }

    /// Adds `mono` starting at `frame`, equal-power panned to `pos`.
    ///
    /// With a mono bus `pos` is ignored and the signal is added at full
    /// level, so mono renders stay level-consistent however layers are
    /// panned.
    pub fn add_panned(&mut self, frame: usize, pos: f32, mono: &[f32]) {
        for (i, &sample) in mono.iter().enumerate() {
            if self.channels == 1 {
                self.add(frame + i, 0, sample);
            } else {
                let (left, right) = pan(sample, pos);
                self.add(frame + i, 0, left);
                self.add(frame + i, 1, right);
            }
        }
    }

    /// Builds the accumulated buffer.
    ///
    /// # Panics
    ///
    /// Panics if the bus was created with a zero sample rate; `render`
    /// rejects that before any bus exists.
    pub fn buffer(&self) -> AudioBuffer {
        AudioBuffer::from_channels(self.planes.clone(), self.sr)
            .expect("bus planes are equal-length by construction")
    }
}

/// One sound-effect recipe.
#[derive(Debug)]
pub struct Recipe {
    /// Recipe id as the caller writes it.
    pub id: &'static str,
    /// One-line description shown by listings.
    pub description: &'static str,
    /// Declared parameters (possibly empty).
    pub params: &'static [ParamSpec],
    /// The render function.
    pub render: fn(&RenderContext) -> Result<AudioBuffer, String>,
}

/// The full recipe catalog, in canonical order.
pub fn recipes() -> &'static [Recipe] {
    &RECIPES
}

/// Returns the recipe with that exact id.
pub fn find_recipe(id: &str) -> Option<&'static Recipe> {
    recipes().iter().find(|recipe| recipe.id == id)
}

/// Returns up to `n` recipe ids within Levenshtein distance 3 of `typo`,
/// nearest first (ties in catalog order).
pub fn suggest_recipe(typo: &str, n: usize) -> Vec<&'static str> {
    let mut scored: Vec<(usize, usize, &'static str)> = recipes()
        .iter()
        .enumerate()
        .map(|(index, recipe)| (levenshtein(typo, recipe.id), index, recipe.id))
        .collect();
    scored.sort_by_key(|&(distance, index, _)| (distance, index));
    scored
        .into_iter()
        .take_while(|&(distance, _, _)| distance <= 3)
        .map(|(_, _, id)| id)
        .take(n)
        .collect()
}

/// Converts `secs` to a frame count at `sr`, at least one frame.
pub fn seconds_to_frames(secs: f64, sr: u32) -> usize {
    ((secs * f64::from(sr)).round() as usize).max(1)
}

/// Renders a sine ping with a T60-calibrated exponential decay.
///
/// `freq` is clamped below the DSP Nyquist guard band so low-sample-rate
/// renders stay clean; the sine's initial phase is drawn from `rng`.
pub fn ping(rng: &mut Qrng, sr: f32, freq: f32, t60: f32) -> Vec<f32> {
    let mut osc = Osc::new(OscKind::Sine, guard_hz(freq, sr), sr, rng);
    let mut env = Percussive::from_t60(sr, t60);
    env.trigger();
    let frames = ((1.2 * t60 * sr).ceil() as usize).max(4);
    (0..frames).map(|_| osc.process() * env.process()).collect()
}

/// Draws event positions (ascending frame indices) for a Poisson process.
///
/// `rate_per_sec` is the expected event count per second; the process is
/// memoryless, so a render's events depend only on `rng`. A non-positive
/// rate yields no events.
pub fn poisson_events(rng: &mut Qrng, frames: usize, sr: u32, rate_per_sec: f32) -> Vec<usize> {
    let mut events = Vec::new();
    if rate_per_sec <= 0.0 || frames == 0 {
        return events;
    }
    let mean_gap = sr as f32 / rate_per_sec;
    let mut cursor = 0.0f32;
    while cursor < frames as f32 {
        // 1 - next_f32 lies in (0, 1]; ln(1) = 0 can stack two events on
        // the same frame, which the accumulator simply sums.
        let draw = 1.0 - rng.next_f32();
        events.push(cursor as usize);
        cursor += -draw.ln() * mean_gap;
    }
    events
}

/// Clamps a design frequency into the DSP Nyquist guard band.
fn guard_hz(freq: f32, sr: f32) -> f32 {
    freq.clamp(1.0, sr * 0.249_75)
}

/// Derives an independent generator for one recipe subsystem.
///
/// Streams are keyed by text (recipe, tag, seed): determinism survives
/// recipe edits because no stream depends on another's draw count.
fn tagged_rng(recipe: &str, tag: &str, ctx: &RenderContext) -> Qrng {
    let seed = ctx.seed;
    Qrng::from_seed(Seed::from_text(&format!(
        "qinari-sfx:{recipe}:{tag}:{seed}"
    )))
}

/// Adds `samples` at `start`, scaled by `gain` and panned from `pan_from`
/// to `pan_to` across the slice, reaching `pan_to` exactly at the last
/// sample (equal endpoints give a fixed position).
fn add_layer(bus: &mut Bus, start: usize, samples: &[f32], gain: f32, pan_from: f32, pan_to: f32) {
    let span = samples.len().max(1);
    for (i, &sample) in samples.iter().enumerate() {
        let t = (i + 1) as f32 / span as f32;
        let pos = pan_from + (pan_to - pan_from) * t;
        bus.add_panned(start + i, pos, &[sample * gain]);
    }
}

/// Renders a sine whose pitch glides from `f0` to `f1` (exponential in
/// hertz) under a T60 envelope; the tail rings past the glide.
fn sub_sweep(rng: &mut Qrng, sr: f32, f0: f32, f1: f32, glide_secs: f32, t60: f32) -> Vec<f32> {
    let mut osc = Osc::new(OscKind::Sine, guard_hz(f0, sr), sr, rng);
    let mut env = Percussive::from_t60(sr, t60);
    env.trigger();
    let frames = ((glide_secs.max(1.2 * t60) * sr).ceil() as usize).max(4);
    let ratio = f1 / f0;
    (0..frames)
        .map(|i| {
            let t = i as f32 / sr;
            let freq = f0 * ratio.powf(t / glide_secs);
            osc.set_freq(guard_hz(freq, sr));
            osc.process() * env.process()
        })
        .collect()
}

/// Applies short equal in/out fades (in milliseconds) to a buffer.
fn fade_edges(buffer: &mut AudioBuffer, sr: u32, in_ms: f32, out_ms: f32) {
    let ms = |value: f32| (value * sr as f32 / 1000.0) as usize;
    buffer.fade_in(ms(in_ms));
    buffer.fade_out(ms(out_ms));
}

/// Quantizes the whole bus in place (used by the glitch recipe).
fn crush_in_place(bus: &mut Bus, bits: f32) {
    let mut crush = Crush::new(bits);
    for plane in &mut bus.planes {
        for sample in plane.iter_mut() {
            *sample = crush.process(*sample);
        }
    }
}

static RAIN_PARAMS: &[ParamSpec] = &[
    spec!(
        intensity,
        Float,
        0.5,
        0.0,
        1.0,
        "Rain density; raises the bed level, brightness and droplet rate."
    ),
    spec!(
        distance,
        Float,
        0.3,
        0.0,
        1.0,
        "Distance to the rain field; dulls the bed and quiets the droplets."
    ),
];

fn render_rain(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let intensity = ctx.params.get("intensity") as f32;
    let distance = ctx.params.get("distance") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // Bed: pink noise shaped by the plan's cutoffs, breathing with a
    // 0.07 Hz gust.
    let bed_cutoff = (1500.0 + 6500.0 * intensity - 1200.0 * distance).clamp(220.0, 9000.0);
    let hp_cutoff = (400.0 - 250.0 * distance).clamp(60.0, 600.0);
    let bed_level = 0.06 + 0.14 * intensity;
    let mut bed = NoiseGen::new(NoiseKind::Pink, tagged_rng("rain", "bed", ctx));
    let mut bed_lowpass = Biquad::design(BiquadKind::Lowpass, sr, bed_cutoff, BUTTERWORTH, 0.0);
    let mut bed_highpass = Biquad::design(BiquadKind::Highpass, sr, hp_cutoff, BUTTERWORTH, 0.0);
    let bed_samples: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let gust = 0.8 + 0.45 * (0.5 + 0.5 * fast_sine(0.07 * t));
            bed_highpass.process(bed_lowpass.process(bed.process())) * bed_level * gust
        })
        .collect();
    bus.add_panned(0, 0.0, &bed_samples);

    // Droplets: Poisson pings panned across the field, some heavy.
    let mut drops = tagged_rng("rain", "drops", ctx);
    let events = poisson_events(&mut drops, ctx.frames, ctx.sr, 18.0 + 95.0 * intensity);
    let attenuation = 1.0 - 0.55 * distance;
    for &at in &events {
        let heavy = drops.next_f32() < 0.08;
        let (freq, t60, level) = if heavy {
            (
                900.0 + drops.next_f32() * 700.0,
                0.1 + drops.next_f32() * 0.05,
                0.09 + drops.next_f32() * 0.08,
            )
        } else {
            (
                2200.0 + drops.next_f32() * 4500.0,
                0.035 + drops.next_f32() * 0.075,
                0.02 + drops.next_f32() * 0.07,
            )
        };
        let pos = drops.next_range(-0.85, 0.85);
        let drop = ping(&mut drops, sr, freq, t60);
        add_layer(&mut bus, at, &drop, level * attenuation, pos, pos);
    }

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 120.0, 120.0);
    Ok(out)
}

static WIND_PARAMS: &[ParamSpec] = &[spec!(
    intensity,
    Float,
    0.5,
    0.0,
    1.0,
    "Wind strength; raises level, brightness and gust depth."
)];

fn render_wind(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let intensity = ctx.params.get("intensity") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // Body: a wandering lowpass band, gusting at 0.11 Hz.
    let body_level = 0.05 + 0.16 * intensity;
    let mut body = NoiseGen::new(NoiseKind::Pink, tagged_rng("wind", "body", ctx));
    let mut body_svf = Svf::new(sr, 700.0, 1.1);
    let body_samples: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let wander = 0.5 + 0.5 * fast_sine(0.06 * t);
            body_svf.set(sr, 450.0 + 800.0 * intensity * wander, 1.1);
            let gust = 0.65 + 0.35 * (0.5 + 0.5 * fast_sine(0.11 * t + 1.3));
            body_svf.process(body.process()).low * body_level * gust
        })
        .collect();
    bus.add_panned(0, 0.0, &body_samples);

    // Whistle: a resonant band riding above the body.
    let whistle_level = 0.012 + 0.05 * intensity;
    let mut whistle = NoiseGen::new(NoiseKind::White, tagged_rng("wind", "whistle", ctx));
    let mut whistle_svf = Svf::new(sr, 1600.0, 4.0);
    let whistle_samples: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let wander = 0.5 + 0.5 * fast_sine(0.045 * t + 0.7);
            whistle_svf.set(sr, 1300.0 + 900.0 * intensity * wander, 4.0);
            whistle_svf.process(whistle.process()).band * whistle_level
        })
        .collect();
    bus.add_panned(0, 0.0, &whistle_samples);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 150.0, 150.0);
    Ok(out)
}

static THUNDER_PARAMS: &[ParamSpec] = &[spec!(
    distance,
    Float,
    0.35,
    0.0,
    1.0,
    "Distance to the strike; softens the crack and lengthens the rumble."
)];

fn render_thunder(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let distance = ctx.params.get("distance") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // Sub thump under everything.
    let sub = sub_sweep(
        &mut tagged_rng("thunder", "sub", ctx),
        sr,
        52.0,
        30.0,
        0.35,
        0.6,
    );
    add_layer(&mut bus, 0, &sub, 0.4, 0.0, 0.0);

    // The crack: bright up close, dull and stretched when far.
    let crack_cutoff = (2800.0 * (1.0 - 0.6 * distance) + 400.0).clamp(500.0, 3200.0);
    let crack = noise_burst(
        sr,
        &mut tagged_rng("thunder", "crack", ctx),
        crack_cutoff,
        0.02 + 0.02 * (1.0 - distance),
    );
    add_layer(&mut bus, 0, &crack, 0.9 - 0.55 * distance, -0.1, 0.15);

    // Rumble: a lowpassed pink bed that builds slowly when far.
    let rumble_t60 = 1.6 + 1.4 * distance;
    let rumble_frames = (1.2 * rumble_t60 * sr) as usize;
    let onset_tau = 0.12 + 0.5 * distance;
    let mut rumble_gen = NoiseGen::new(NoiseKind::Pink, tagged_rng("thunder", "rumble", ctx));
    let mut rumble_lp = Biquad::design(
        BiquadKind::Lowpass,
        sr,
        (300.0 - 220.0 * distance).clamp(80.0, 300.0),
        BUTTERWORTH,
        0.0,
    );
    let mut rumble_env = Percussive::from_t60(sr, rumble_t60);
    rumble_env.trigger();
    let rumble: Vec<f32> = (0..rumble_frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let onset = 1.0 - (-t / onset_tau).exp();
            rumble_lp.process(rumble_gen.process())
                * rumble_env.process()
                * onset
                * (0.5 - 0.3 * distance)
        })
        .collect();
    add_layer(&mut bus, 0, &rumble, 1.0, 0.0, 0.0);

    // One rolling echo down the tail.
    let echo_t60 = 1.2;
    let echo_frames = (1.2 * echo_t60 * sr) as usize;
    let mut echo_gen = NoiseGen::new(NoiseKind::Pink, tagged_rng("thunder", "echo", ctx));
    let mut echo_lp = Biquad::design(BiquadKind::Lowpass, sr, 120.0, BUTTERWORTH, 0.0);
    let mut echo_env = Percussive::from_t60(sr, echo_t60);
    echo_env.trigger();
    let echo: Vec<f32> = (0..echo_frames)
        .map(|_| {
            echo_lp.process(echo_gen.process()) * echo_env.process() * 0.16 * (1.0 - 0.5 * distance)
        })
        .collect();
    add_layer(&mut bus, (1.05 * sr) as usize, &echo, 1.0, 0.1, -0.1);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 10.0);
    Ok(out)
}

static OCEAN_PARAMS: &[ParamSpec] = &[spec!(
    intensity,
    Float,
    0.5,
    0.0,
    1.0,
    "Sea energy; raises swell level and spray."
)];

fn render_ocean(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let intensity = ctx.params.get("intensity") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // One shared swell phase drives the band, the level and the spray.
    let bed_level = 0.05 + 0.16 * intensity;
    let spray_level = 0.02 + 0.06 * intensity;
    let mut body = NoiseGen::new(NoiseKind::Pink, tagged_rng("ocean", "body", ctx));
    let mut body_svf = Svf::new(sr, 600.0, 0.9);
    let mut spray = NoiseGen::new(NoiseKind::White, tagged_rng("ocean", "spray", ctx));
    let mut spray_hp = Biquad::design(BiquadKind::Highpass, sr, 1800.0, BUTTERWORTH, 0.0);
    let samples: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let swell = 0.5 + 0.5 * fast_sine(0.055 * t);
            body_svf.set(sr, 350.0 + 650.0 * swell, 0.9);
            let wave = body_svf.process(body.process()).low * bed_level * (0.6 + 0.4 * swell);
            let crest = (swell - 0.55).max(0.0);
            let hiss = spray_hp.process(spray.process()) * spray_level * crest * crest * 3.0;
            wave + hiss
        })
        .collect();
    bus.add_panned(0, 0.0, &samples);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 150.0, 150.0);
    Ok(out)
}

static FIRE_PARAMS: &[ParamSpec] = &[spec!(
    intensity,
    Float,
    0.5,
    0.0,
    1.0,
    "Fire size; raises hiss, crackle and pop rates."
)];

fn render_fire(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let intensity = ctx.params.get("intensity") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // Hiss bed flickering at sample-and-hold rate.
    let hiss_level = 0.015 + 0.05 * intensity;
    let mut hiss = NoiseGen::new(NoiseKind::Pink, tagged_rng("fire", "hiss", ctx));
    let mut hiss_hp = Biquad::design(BiquadKind::Highpass, sr, 1200.0, BUTTERWORTH, 0.0);
    let mut flicker = Lfo::new(
        LfoShape::SampleHold,
        0.8,
        sr,
        &mut tagged_rng("fire", "flicker", ctx),
    );
    let hiss_samples: Vec<f32> = (0..ctx.frames)
        .map(|_| {
            let jitter = 0.7 + 0.3 * (0.5 + 0.5 * flicker.process());
            hiss_hp.process(hiss.process()) * hiss_level * jitter
        })
        .collect();
    bus.add_panned(0, 0.0, &hiss_samples);

    // Crackle ticks; a few of them pop loudly.
    let mut crackle = tagged_rng("fire", "crackle", ctx);
    let tick_events = poisson_events(&mut crackle, ctx.frames, ctx.sr, 6.0 + 42.0 * intensity);
    for &at in &tick_events {
        let cutoff = 2200.0 + crackle.next_f32() * 3800.0;
        let decay = 0.006 + crackle.next_f32() * 0.02;
        let tick = noise_burst(sr, &mut crackle, cutoff, decay);
        let loud = crackle.next_f32();
        let pos = crackle.next_range(-0.8, 0.8);
        add_layer(&mut bus, at, &tick, 0.05 + 0.32 * loud * loud, pos, pos);
    }

    // Deep pops from the coals.
    let pop_events = poisson_events(&mut crackle, ctx.frames, ctx.sr, 0.7 + 1.3 * intensity);
    for &at in &pop_events {
        let cutoff = 380.0 + crackle.next_f32() * 250.0;
        let decay = 0.04 + crackle.next_f32() * 0.03;
        let pop = noise_burst(sr, &mut crackle, cutoff, decay);
        let pos = crackle.next_range(-0.5, 0.5);
        add_layer(
            &mut bus,
            at,
            &pop,
            0.3 + 0.25 * crackle.next_f32(),
            pos,
            pos,
        );
    }

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 100.0, 100.0);
    Ok(out)
}

static STREAM_PARAMS: &[ParamSpec] = &[spec!(
    intensity,
    Float,
    0.5,
    0.0,
    1.0,
    "Water flow; raises the burble level and bubble rate."
)];

fn render_stream(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let intensity = ctx.params.get("intensity") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // Burble: a resonant band whose centre wobbles on two slow rates.
    let burble_level = 0.05 + 0.13 * intensity;
    let mut burble = NoiseGen::new(NoiseKind::White, tagged_rng("stream", "burble", ctx));
    let mut burble_svf = Svf::new(sr, 900.0, 2.8);
    let burble_samples: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let centre = 900.0 + 550.0 * fast_sine(0.9 * t) + 380.0 * fast_sine(0.23 * t);
            burble_svf.set(sr, centre, 2.8);
            burble_svf.process(burble.process()).band * burble_level
        })
        .collect();
    bus.add_panned(0, 0.0, &burble_samples);

    // Body: a soft low bed underneath.
    let mut body = NoiseGen::new(NoiseKind::Pink, tagged_rng("stream", "body", ctx));
    let mut body_lp = Biquad::design(BiquadKind::Lowpass, sr, 800.0, BUTTERWORTH, 0.0);
    let body_samples: Vec<f32> = (0..ctx.frames)
        .map(|_| body_lp.process(body.process()) * 0.035)
        .collect();
    bus.add_panned(0, 0.0, &body_samples);

    // Bubbles: short pings scattered across the water.
    let mut bubbles = tagged_rng("stream", "bubbles", ctx);
    let events = poisson_events(&mut bubbles, ctx.frames, ctx.sr, 2.5 + 7.0 * intensity);
    for &at in &events {
        let freq = 700.0 + bubbles.next_f32() * 1900.0;
        let t60 = 0.05 + bubbles.next_f32() * 0.06;
        let droplet = ping(&mut bubbles, sr, freq, t60);
        let pos = bubbles.next_range(-0.7, 0.7);
        add_layer(
            &mut bus,
            at,
            &droplet,
            0.03 + 0.06 * bubbles.next_f32(),
            pos,
            pos,
        );
    }

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 120.0, 120.0);
    Ok(out)
}

static CRICKETS_PARAMS: &[ParamSpec] = &[spec!(
    intensity,
    Float,
    0.5,
    0.0,
    1.0,
    "Cricket density; chirp group rate."
)];

fn render_crickets(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let intensity = ctx.params.get("intensity") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // Each event is one cricket: 3-6 pulses of a ~4.2 kHz tone.
    let mut voices = tagged_rng("crickets", "voices", ctx);
    let events = poisson_events(&mut voices, ctx.frames, ctx.sr, 0.25 + 1.1 * intensity);
    for &at in &events {
        let pulses = 3 + voices.next_int_range(0, 3) as usize;
        let chirp_freq = 3850.0 + voices.next_f32() * 700.0;
        let base_level = 0.1 + 0.08 * voices.next_f32();
        let pos = voices.next_range(-0.9, 0.9);
        for pulse in 0..pulses {
            let at_pulse = at + (pulse as f32 * 0.024 * sr) as usize;
            let t60 = 0.02 + 0.012 * voices.next_f32();
            let chirp = ping(&mut voices, sr, chirp_freq, t60);
            add_layer(&mut bus, at_pulse, &chirp, base_level, pos, pos);
        }
    }

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 100.0, 100.0);
    Ok(out)
}

static EXPLOSION_PARAMS: &[ParamSpec] = &[
    spec!(
        size,
        Float,
        0.7,
        0.0,
        1.0,
        "Charge size; deepens and lengthens the blast."
    ),
    spec!(
        distance,
        Float,
        0.3,
        0.0,
        1.0,
        "Distance to the blast; dulls and stretches it."
    ),
];

fn render_explosion(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let size = ctx.params.get("size") as f32;
    let distance = ctx.params.get("distance") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // Sub boom: driven so it reads on small speakers.
    let mut drive = Drive::new(2.0);
    let boom: Vec<f32> = sub_sweep(
        &mut tagged_rng("explosion", "boom", ctx),
        sr,
        95.0,
        32.0,
        0.5,
        1.0 + 0.8 * size,
    )
    .into_iter()
    .map(|sample| drive.process(sample * (0.5 + 0.2 * size)))
    .collect();
    add_layer(&mut bus, 0, &boom, 1.0, 0.0, 0.0);

    // Blast: the broadband body of the event.
    let blast_cutoff = (1400.0 * (1.0 - 0.55 * distance) + 900.0 * size).clamp(300.0, 6000.0);
    let blast = noise_burst(
        sr,
        &mut tagged_rng("explosion", "blast", ctx),
        blast_cutoff,
        0.22 + 0.25 * size + 0.15 * distance,
    );
    add_layer(&mut bus, 0, &blast, 0.85 - 0.35 * distance, 0.0, 0.1);

    // Rumble: slow onset, long tail.
    let rumble_t60 = 1.0 + 1.8 * size + 0.8 * distance;
    let rumble_frames = (1.2 * rumble_t60 * sr) as usize;
    let mut rumble_gen = NoiseGen::new(NoiseKind::Pink, tagged_rng("explosion", "rumble", ctx));
    let mut rumble_lp = Biquad::design(
        BiquadKind::Lowpass,
        sr,
        (260.0 - 180.0 * distance).clamp(60.0, 300.0),
        BUTTERWORTH,
        0.0,
    );
    let mut rumble_env = Percussive::from_t60(sr, rumble_t60);
    rumble_env.trigger();
    let rumble: Vec<f32> = (0..rumble_frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let onset = 1.0 - (-t / (0.15 + 0.4 * distance)).exp();
            rumble_lp.process(rumble_gen.process())
                * rumble_env.process()
                * onset
                * (0.3 + 0.2 * size - 0.15 * distance)
        })
        .collect();
    add_layer(&mut bus, 0, &rumble, 1.0, 0.0, 0.0);

    // Debris: ticks scattering after the blast.
    let debris_frames = (1.4 * sr) as usize;
    let rate = ((4.0 + 10.0 * size) * (1.0 - 0.6 * distance)).max(0.5);
    let mut debris = tagged_rng("explosion", "debris", ctx);
    let events = poisson_events(&mut debris, debris_frames, ctx.sr, rate);
    for &event_at in &events {
        let at = (0.35 * sr) as usize + event_at;
        let cutoff = 2500.0 + debris.next_f32() * 4500.0;
        let decay = 0.02 + debris.next_f32() * 0.03;
        let tick = noise_burst(sr, &mut debris, cutoff, decay);
        let pos = debris.next_range(-0.8, 0.8);
        let level = (0.05 + 0.2 * debris.next_f32()) * (1.0 - 0.5 * distance);
        add_layer(&mut bus, at, &tick, level, pos, pos);
    }

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 10.0);
    Ok(out)
}

static IMPACT_PARAMS: &[ParamSpec] = &[spec!(
    intensity,
    Float,
    0.6,
    0.0,
    1.0,
    "Impact strength; raises thump and slap levels."
)];

fn render_impact(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let intensity = ctx.params.get("intensity") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    let thump = sub_sweep(
        &mut tagged_rng("impact", "thump", ctx),
        sr,
        140.0,
        48.0,
        0.14,
        0.4,
    );
    add_layer(&mut bus, 0, &thump, 0.35 + 0.45 * intensity, 0.0, 0.0);

    let slap = noise_burst(
        sr,
        &mut tagged_rng("impact", "slap", ctx),
        1200.0 + 2600.0 * intensity,
        0.018 + 0.02 * intensity,
    );
    add_layer(&mut bus, 0, &slap, 0.55 + 0.25 * intensity, 0.0, 0.05);

    // Body: a short enveloped wash under the transient.
    let body_frames = (0.4 * sr) as usize;
    let mut body = NoiseGen::new(NoiseKind::Pink, tagged_rng("impact", "body", ctx));
    let mut body_lp = Biquad::design(BiquadKind::Lowpass, sr, 700.0, BUTTERWORTH, 0.0);
    let mut body_env = Percussive::from_t60(sr, 0.3);
    body_env.trigger();
    let body_samples: Vec<f32> = (0..body_frames)
        .map(|_| body_lp.process(body.process()) * body_env.process() * 0.2)
        .collect();
    add_layer(&mut bus, 0, &body_samples, 1.0, 0.0, 0.0);

    // A soft sigh of air after the hit.
    let sigh = swept_noise(
        sr,
        &mut tagged_rng("impact", "sigh", ctx),
        900.0,
        300.0,
        0.25,
        1.5,
    );
    add_layer(
        &mut bus,
        (0.12 * sr) as usize,
        &sigh,
        0.08 + 0.18 * intensity,
        -0.2,
        0.2,
    );

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 10.0);
    Ok(out)
}

static WHOOSH_PARAMS: &[ParamSpec] = &[spec!(
    intensity,
    Float,
    0.6,
    0.0,
    1.0,
    "Pass speed and level of the air sweep."
)];

fn render_whoosh(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let intensity = ctx.params.get("intensity") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // Up-sweep travelling left to right, down-sweep returning.
    let up = swept_noise(
        sr,
        &mut tagged_rng("whoosh", "up", ctx),
        350.0,
        2600.0,
        0.34,
        2.5,
    );
    add_layer(&mut bus, 0, &up, 0.35 + 0.45 * intensity, -0.85, 0.75);

    let down = swept_noise(
        sr,
        &mut tagged_rng("whoosh", "down", ctx),
        2400.0,
        450.0,
        0.3,
        2.2,
    );
    add_layer(
        &mut bus,
        (0.18 * sr) as usize,
        &down,
        0.3 + 0.35 * intensity,
        0.8,
        -0.7,
    );

    // A long faint air wash underneath.
    let air = noise_burst(sr, &mut tagged_rng("whoosh", "air", ctx), 900.0, 0.5);
    add_layer(&mut bus, 0, &air, 0.1 + 0.1 * intensity, 0.0, 0.0);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 10.0);
    Ok(out)
}

static SWORD_PARAMS: &[ParamSpec] = &[spec!(
    ring,
    Float,
    0.6,
    0.0,
    1.0,
    "Metallic ring level after the whoosh."
)];

fn render_sword(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let ring = ctx.params.get("ring") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    let whoosh = swept_noise(
        sr,
        &mut tagged_rng("sword", "whoosh", ctx),
        500.0,
        4200.0,
        0.3,
        3.0,
    );
    add_layer(&mut bus, 0, &whoosh, 0.55, -0.7, 0.6);

    // Two detuned Karplus-Strong strings give the metal.
    let mut metal = tagged_rng("sword", "metal", ctx);
    let base = guard_hz(1900.0 + metal.next_f32() * 700.0, sr);
    let ring_a = qinari_dsp::karplus::pluck(sr, base, 0.9, &mut metal);
    add_layer(
        &mut bus,
        (0.16 * sr) as usize,
        &ring_a,
        0.22 * ring,
        -0.15,
        0.25,
    );
    let ring_b = qinari_dsp::karplus::pluck(sr, base * 1.98, 0.7, &mut metal);
    add_layer(
        &mut bus,
        (0.16 * sr) as usize,
        &ring_b,
        0.12 * ring,
        0.25,
        -0.15,
    );

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 10.0);
    Ok(out)
}

static HIT_PARAMS: &[ParamSpec] = &[];

fn render_hit(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    let punch = sub_sweep(
        &mut tagged_rng("hit", "punch", ctx),
        sr,
        160.0,
        42.0,
        0.13,
        0.22,
    );
    add_layer(&mut bus, 0, &punch, 0.7, 0.0, 0.0);

    let slap = noise_burst(sr, &mut tagged_rng("hit", "slap", ctx), 3800.0, 0.012);
    add_layer(&mut bus, 0, &slap, 0.5, 0.0, 0.0);

    let body_frames = (0.35 * sr) as usize;
    let mut body = NoiseGen::new(NoiseKind::Pink, tagged_rng("hit", "body", ctx));
    let mut body_lp = Biquad::design(BiquadKind::Lowpass, sr, 600.0, BUTTERWORTH, 0.0);
    let mut body_env = Percussive::from_t60(sr, 0.28);
    body_env.trigger();
    let body_samples: Vec<f32> = (0..body_frames)
        .map(|_| body_lp.process(body.process()) * body_env.process() * 0.22)
        .collect();
    add_layer(&mut bus, 0, &body_samples, 1.0, 0.0, 0.0);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 10.0);
    Ok(out)
}

static SUB_DROP_PARAMS: &[ParamSpec] = &[];

fn render_sub_drop(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    let mut drive = Drive::new(2.5);
    let sub: Vec<f32> = sub_sweep(
        &mut tagged_rng("sub_drop", "sub", ctx),
        sr,
        85.0,
        28.0,
        0.7,
        1.1,
    )
    .into_iter()
    .map(|sample| drive.process(sample * 0.6))
    .collect();
    add_layer(&mut bus, 0, &sub, 1.0, 0.0, 0.0);

    let puff = noise_burst(sr, &mut tagged_rng("sub_drop", "puff", ctx), 500.0, 0.1);
    add_layer(&mut bus, 0, &puff, 0.18, 0.0, 0.0);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 10.0);
    Ok(out)
}

static LASER_PARAMS: &[ParamSpec] = &[];

fn render_laser(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // Manual two-operator FM with a falling carrier; the index collapses
    // from a noisy splash into a clean tone.
    let frames = ((0.36 * sr).ceil() as usize).max(4);
    let mut env = Percussive::from_t60(sr, 0.3);
    env.trigger();
    let mut carrier_phase = 0.0f32;
    let mut mod_phase = 0.0f32;
    let zap: Vec<f32> = (0..frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let freq = 2200.0 * (180.0f32 / 2200.0).powf(t / 0.32);
            let index = 1.7 * (-t / 0.1).exp() + 0.15;
            let tuned = guard_hz(freq, sr);
            mod_phase = (mod_phase + tuned * 2.01 / sr) % 1.0;
            carrier_phase = (carrier_phase + tuned / sr) % 1.0;
            fast_sine(carrier_phase + index * fast_sine(mod_phase)) * env.process() * 0.55
        })
        .collect();
    add_layer(&mut bus, 0, &zap, 1.0, -0.1, 0.1);

    // A bright shot transient at the head.
    let tick = noise_burst(sr, &mut tagged_rng("laser", "tick", ctx), 7000.0, 0.008);
    add_layer(&mut bus, 0, &tick, 0.15, 0.0, 0.0);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 6.0);
    Ok(out)
}

static BLASTER_PARAMS: &[ParamSpec] = &[];

fn render_blaster(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // Compressed FM drop, faster and harder than the laser.
    let frames = ((0.14 * sr).ceil() as usize).max(4);
    let mut env = Percussive::from_t60(sr, 0.12);
    env.trigger();
    let mut carrier_phase = 0.0f32;
    let mut mod_phase = 0.0f32;
    let shot: Vec<f32> = (0..frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let freq = 1500.0 * (280.0f32 / 1500.0).powf(t / 0.13);
            let index = 2.4 * (-t / 0.06).exp() + 0.1;
            let tuned = guard_hz(freq, sr);
            mod_phase = (mod_phase + tuned * 2.0 / sr) % 1.0;
            carrier_phase = (carrier_phase + tuned / sr) % 1.0;
            fast_sine(carrier_phase + index * fast_sine(mod_phase)) * env.process() * 0.5
        })
        .collect();
    add_layer(&mut bus, 0, &shot, 1.0, -0.1, 0.1);

    let crack = noise_burst(sr, &mut tagged_rng("blaster", "crack", ctx), 5000.0, 0.015);
    add_layer(&mut bus, 0, &crack, 0.35, 0.0, 0.0);

    let zip = swept_noise(
        sr,
        &mut tagged_rng("blaster", "zip", ctx),
        3000.0,
        800.0,
        0.12,
        6.0,
    );
    add_layer(&mut bus, (0.05 * sr) as usize, &zip, 0.25, 0.1, -0.1);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 6.0);
    Ok(out)
}

static TELEPORT_PARAMS: &[ParamSpec] = &[];

fn render_teleport(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // Rising shimmer.
    let shimmer = swept_noise(
        sr,
        &mut tagged_rng("teleport", "shimmer", ctx),
        250.0,
        5200.0,
        0.75,
        5.0,
    );
    add_layer(&mut bus, 0, &shimmer, 0.32, -0.4, 0.4);

    // Sparkle grains thickening toward the end.
    let mut sparkle = tagged_rng("teleport", "sparkle", ctx);
    for grain in 0..14 {
        let t = 0.75 * (grain as f32 + sparkle.next_f32() * 0.85) / 14.0;
        let at = (t * sr) as usize;
        let freq = 1200.0 + sparkle.next_f32() * 5300.0;
        let t60 = 0.04 + sparkle.next_f32() * 0.12;
        let grain_samples = ping(&mut sparkle, sr, freq, t60);
        let pos = sparkle.next_range(-0.8, 0.8);
        add_layer(
            &mut bus,
            at,
            &grain_samples,
            0.04 + 0.1 * sparkle.next_f32(),
            pos,
            pos,
        );
    }

    // Arrival: a soft thump and puff.
    let arrival = sub_sweep(
        &mut tagged_rng("teleport", "arrival", ctx),
        sr,
        160.0,
        70.0,
        0.18,
        0.35,
    );
    add_layer(&mut bus, (0.72 * sr) as usize, &arrival, 0.3, 0.0, 0.0);
    let puff = noise_burst(sr, &mut tagged_rng("teleport", "puff", ctx), 400.0, 0.12);
    add_layer(&mut bus, (0.72 * sr) as usize, &puff, 0.15, 0.0, 0.0);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 10.0);
    Ok(out)
}

static SPELL_CAST_PARAMS: &[ParamSpec] = &[];

fn render_spell_cast(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    let whoosh = swept_noise(
        sr,
        &mut tagged_rng("spell_cast", "whoosh", ctx),
        450.0,
        3200.0,
        0.5,
        4.0,
    );
    add_layer(&mut bus, 0, &whoosh, 0.4, -0.5, 0.5);

    // Sparkle grains scattered across the cast.
    let mut sparkle = tagged_rng("spell_cast", "sparkle", ctx);
    for grain in 0..12 {
        let t = 0.65 * (grain as f32 + sparkle.next_f32() * 0.8) / 12.0;
        let at = (t * sr) as usize;
        let freq = 1800.0 + sparkle.next_f32() * 5700.0;
        let t60 = 0.08 + sparkle.next_f32() * 0.14;
        let grain_samples = ping(&mut sparkle, sr, freq, t60);
        let pos = sparkle.next_range(-0.8, 0.8);
        add_layer(
            &mut bus,
            at,
            &grain_samples,
            0.03 + 0.07 * sparkle.next_f32(),
            pos,
            pos,
        );
    }

    // Bell-like resolve: a fifth, with a soft thump underneath.
    let resolve_at = (0.55 * sr) as usize;
    let high = ping(
        &mut tagged_rng("spell_cast", "resolve_high", ctx),
        sr,
        1568.0,
        0.6,
    );
    add_layer(&mut bus, resolve_at, &high, 0.16, -0.1, -0.1);
    let higher = ping(
        &mut tagged_rng("spell_cast", "resolve_top", ctx),
        sr,
        2093.0,
        0.5,
    );
    add_layer(&mut bus, resolve_at, &higher, 0.09, 0.15, 0.15);
    let thump = sub_sweep(
        &mut tagged_rng("spell_cast", "resolve_sub", ctx),
        sr,
        220.0,
        110.0,
        0.2,
        0.4,
    );
    add_layer(&mut bus, resolve_at, &thump, 0.12, 0.0, 0.0);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 10.0);
    Ok(out)
}

static SONAR_PARAMS: &[ParamSpec] = &[];

fn render_sonar(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);
    let mut rng = tagged_rng("sonar", "pings", ctx);

    let main = ping(&mut rng, sr, 1150.0, 1.1);
    add_layer(&mut bus, 0, &main, 0.42, 0.0, 0.0);

    let echo_a = ping(&mut rng, sr, 1150.0, 0.9);
    add_layer(&mut bus, (0.5 * sr) as usize, &echo_a, 0.13, 0.2, 0.2);

    let echo_b = ping(&mut rng, sr, 1150.0, 0.8);
    add_layer(&mut bus, (0.95 * sr) as usize, &echo_b, 0.05, -0.2, -0.2);

    let bloop = sub_sweep(&mut rng, sr, 200.0, 80.0, 0.15, 0.4);
    add_layer(&mut bus, (0.1 * sr) as usize, &bloop, 0.15, 0.0, 0.0);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 10.0);
    Ok(out)
}

static UI_SUCCESS_PARAMS: &[ParamSpec] = &[];

fn render_ui_success(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);
    let mut rng = tagged_rng("ui_success", "blips", ctx);

    let first = ping(&mut rng, sr, 880.0, 0.16);
    add_layer(&mut bus, 0, &first, 0.4, 0.0, 0.0);

    let second = ping(&mut rng, sr, 1318.5, 0.28);
    add_layer(&mut bus, (0.15 * sr) as usize, &second, 0.42, 0.0, 0.0);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 6.0);
    Ok(out)
}

static UI_ERROR_PARAMS: &[ParamSpec] = &[];

fn render_ui_error(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);
    let mut rng = tagged_rng("ui_error", "buzz", ctx);

    // Two low buzzes, each gated by an ADSR with a short release.
    for buzz_index in 0..2 {
        let at = (buzz_index as f32 * 0.19 * sr) as usize;
        let mut low = Osc::new(OscKind::Saw, 147.0, sr, &mut rng);
        let mut mid = Osc::new(OscKind::Saw, 294.0, sr, &mut rng);
        let mut lp = Biquad::design(BiquadKind::Lowpass, sr, 1100.0, BUTTERWORTH, 0.0);
        let mut env = Adsr::new(0.008, 0.02, 0.8, 0.05, sr);
        env.trigger();
        let hold = (0.14 * sr) as usize;
        let release = (0.06 * sr) as usize;
        let mut buzz: Vec<f32> = Vec::with_capacity(hold + release);
        for _ in 0..hold {
            let sample = lp.process(low.process() + 0.6 * mid.process()) * env.process() * 0.3;
            buzz.push(sample);
        }
        env.release();
        for _ in 0..release {
            let sample = lp.process(low.process() + 0.6 * mid.process()) * env.process() * 0.3;
            buzz.push(sample);
        }
        add_layer(&mut bus, at, &buzz, 1.0, 0.0, 0.0);
    }

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 6.0);
    Ok(out)
}

static UI_CLICK_PARAMS: &[ParamSpec] = &[];

fn render_ui_click(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);
    let mut rng = tagged_rng("ui_click", "tick", ctx);

    let tick = noise_burst(sr, &mut rng, 5500.0, 0.007);
    add_layer(&mut bus, 0, &tick, 0.55, 0.0, 0.0);

    let tone = ping(&mut rng, sr, 2400.0, 0.035);
    add_layer(&mut bus, 0, &tone, 0.1, 0.0, 0.0);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 6.0);
    Ok(out)
}

static ALARM_PARAMS: &[ParamSpec] = &[spec!(
    speed,
    Float,
    0.7,
    0.2,
    3.0,
    "Sweep cycles per second of the siren."
)];

fn render_alarm(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let speed = ctx.params.get("speed") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // One phase drives a sine and a lowpassed square in unison.
    let mut lp = Biquad::design(BiquadKind::Lowpass, sr, 2800.0, BUTTERWORTH, 0.0);
    let mut phase = 0.0f32;
    let siren: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let freq = 760.0 + 240.0 * fast_sine(speed * t);
            phase = (phase + guard_hz(freq, sr) / sr) % 1.0;
            let sine = fast_sine(phase);
            let square = if sine >= 0.0 { 1.0 } else { -1.0 };
            sine * 0.14 + lp.process(square) * 0.1
        })
        .collect();
    bus.add_panned(0, 0.0, &siren);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 100.0, 100.0);
    Ok(out)
}

static ENGINE_PARAMS: &[ParamSpec] = &[
    spec!(
        rpm,
        Int,
        2400.0,
        600.0,
        6000.0,
        "Engine speed in revolutions per minute."
    ),
    spec!(
        load,
        Float,
        0.5,
        0.0,
        1.0,
        "Engine load; raises mechanical noise and brightness."
    ),
];

fn render_engine(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let rpm = ctx.params.get("rpm") as f32;
    let load = ctx.params.get("load") as f32;
    let fire = rpm / 60.0 * 2.0; // 4-cylinder four-stroke firing rate
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    let mut saw = Osc::new(
        OscKind::Saw,
        fire,
        sr,
        &mut tagged_rng("engine", "saw", ctx),
    );
    let mut saw2 = Osc::new(
        OscKind::Saw,
        fire * 2.0,
        sr,
        &mut tagged_rng("engine", "saw2", ctx),
    );
    let mut sub = Osc::new(
        OscKind::Sine,
        (fire / 2.0).max(20.0),
        sr,
        &mut tagged_rng("engine", "sub", ctx),
    );
    let mut noise = NoiseGen::new(NoiseKind::Pink, tagged_rng("engine", "noise", ctx));
    let mut fundamental_lp = Biquad::design(BiquadKind::Lowpass, sr, 500.0, BUTTERWORTH, 0.0);
    let mut overtone_lp = Biquad::design(BiquadKind::Lowpass, sr, 900.0, BUTTERWORTH, 0.0);
    let mut noise_lp = Biquad::design(
        BiquadKind::Lowpass,
        sr,
        200.0 + 600.0 * load,
        BUTTERWORTH,
        0.0,
    );
    let body: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let flutter = 1.0 + 0.015 * fast_sine(4.3 * t);
            saw.set_freq(guard_hz(fire * flutter, sr));
            saw2.set_freq(guard_hz(fire * 2.0 * flutter, sr));
            fundamental_lp.process(saw.process()) * 0.14
                + overtone_lp.process(saw2.process()) * 0.08
                + sub.process() * 0.1
                + noise_lp.process(noise.process()) * (0.02 + 0.08 * load)
        })
        .collect();
    bus.add_panned(0, 0.0, &body);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 150.0, 150.0);
    Ok(out)
}

static FORCEFIELD_PARAMS: &[ParamSpec] = &[spec!(
    intensity,
    Float,
    0.5,
    0.0,
    1.0,
    "Field energy; raises the hum and the edge."
)];

fn render_forcefield(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let intensity = ctx.params.get("intensity") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // Two detuned saws beating against each other, plus an octave sine.
    let mut saw_a = Osc::new(
        OscKind::Saw,
        55.0,
        sr,
        &mut tagged_rng("forcefield", "a", ctx),
    );
    let mut saw_b = Osc::new(
        OscKind::Saw,
        55.35,
        sr,
        &mut tagged_rng("forcefield", "b", ctx),
    );
    let mut sine = Osc::new(
        OscKind::Sine,
        110.0,
        sr,
        &mut tagged_rng("forcefield", "sine", ctx),
    );
    let mut lp = Biquad::design(BiquadKind::Lowpass, sr, 750.0, BUTTERWORTH, 0.0);
    let energy = 0.55 + 0.75 * intensity;
    let hum: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let throb = 0.8 + 0.2 * fast_sine(0.37 * t);
            let mix = saw_a.process() * 0.05 + saw_b.process() * 0.045 + sine.process() * 0.035;
            lp.process(mix) * throb * energy
        })
        .collect();
    bus.add_panned(0, 0.0, &hum);

    // Energized edge: a high band chopped by a fast tremolo.
    let edge_level = 0.012 + 0.03 * intensity;
    let mut edge = NoiseGen::new(NoiseKind::White, tagged_rng("forcefield", "edge", ctx));
    let mut edge_hp = Biquad::design(BiquadKind::Highpass, sr, 2600.0, BUTTERWORTH, 0.0);
    let mut tremolo = Tremolo::new(
        11.0,
        0.85,
        sr,
        &mut tagged_rng("forcefield", "tremolo", ctx),
    );
    let edge_samples: Vec<f32> = (0..ctx.frames)
        .map(|_| tremolo.process(edge_hp.process(edge.process())) * edge_level)
        .collect();
    bus.add_panned(0, 0.0, &edge_samples);

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 100.0, 100.0);
    Ok(out)
}

static ROBOT_TALK_PARAMS: &[ParamSpec] = &[];

fn render_robot_talk(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let mut rng = tagged_rng("robot_talk", "syllables", ctx);
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);
    let end = ctx.frames as f32;
    let mut cursor = 0.0f32;

    // A stream of beeps: square, saw or triangle, each softly gated.
    while cursor < end {
        let at = cursor as usize;
        let pick = rng.next_f32();
        let (kind, freq) = if pick < 0.5 {
            (OscKind::Square, 260.0 + rng.next_f32() * 900.0)
        } else if pick < 0.75 {
            (OscKind::Saw, 300.0 + rng.next_f32() * 1300.0)
        } else {
            (OscKind::Triangle, 180.0 + rng.next_f32() * 600.0)
        };
        let hold = ((0.05 + rng.next_f32() * 0.17) * sr) as usize;
        let release = (0.05 * sr) as usize;
        let mut osc = Osc::new(kind, guard_hz(freq, sr), sr, &mut rng);
        let mut lp = Biquad::design(BiquadKind::Lowpass, sr, 2800.0, BUTTERWORTH, 0.0);
        let mut env = Adsr::new(0.006, 0.02, 0.75, 0.05, sr);
        env.trigger();
        let mut syllable: Vec<f32> = Vec::with_capacity(hold + release);
        for _ in 0..hold {
            syllable.push(lp.process(osc.process()) * env.process() * 0.22);
        }
        env.release();
        for _ in 0..release {
            syllable.push(lp.process(osc.process()) * env.process() * 0.22);
        }
        let pos = rng.next_range(-0.6, 0.6);
        add_layer(&mut bus, at, &syllable, 1.0, pos, pos);
        let pause = if rng.next_f32() < 0.12 {
            0.4
        } else {
            0.03 + rng.next_f32() * 0.12
        };
        cursor = at as f32 + (hold + release) as f32 + pause * sr;
    }

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 60.0, 60.0);
    Ok(out)
}

static GLITCH_PARAMS: &[ParamSpec] = &[spec!(
    intensity,
    Float,
    0.5,
    0.0,
    1.0,
    "Stutter density and level."
)];

fn render_glitch(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let intensity = ctx.params.get("intensity") as f32;
    let level = 0.55 + 0.85 * intensity;
    let mut rng = tagged_rng("glitch", "segments", ctx);
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);
    let mut cursor = 0.0f32;

    // Chopped segments: rest, noise tick, square blip, crushed wash or
    // mini riser, then the whole bus is requantized.
    while cursor < ctx.frames as f32 - 1.0 {
        let at = cursor as usize;
        let seg = ((0.03 + rng.next_f32() * 0.28) * sr) as usize;
        let pick = rng.next_f32();
        let rest_prob = 0.3 - 0.25 * intensity;
        if pick >= rest_prob {
            if pick < 0.55 {
                let cutoff = 700.0 + rng.next_f32() * 5200.0;
                let decay = 0.02 + rng.next_f32() * 0.05;
                let tick = noise_burst(sr, &mut rng, cutoff, decay);
                let pos = rng.next_range(-0.7, 0.7);
                add_layer(&mut bus, at, &tick, level, pos, pos);
            } else if pick < 0.75 {
                let freq = 90.0 + rng.next_f32() * 1900.0;
                let hold = ((0.02 + rng.next_f32() * 0.1) * sr) as usize;
                let mut osc = Osc::new(OscKind::Square, guard_hz(freq, sr), sr, &mut rng);
                let mut lp = Biquad::design(BiquadKind::Lowpass, sr, 2500.0, BUTTERWORTH, 0.0);
                let mut env = Percussive::from_t60(sr, 0.12);
                env.trigger();
                let blip: Vec<f32> = (0..hold)
                    .map(|_| lp.process(osc.process()) * env.process() * level * 0.5)
                    .collect();
                let pos = rng.next_range(-0.7, 0.7);
                add_layer(&mut bus, at, &blip, 1.0, pos, pos);
            } else if pick < 0.9 {
                let wash_frames = seg.min((0.25 * sr) as usize).max(4);
                let mut crush = Crush::new(2.5 + rng.next_f32() * 3.0);
                let wash: Vec<f32> = (0..wash_frames)
                    .map(|_| crush.process(rng.next_f32() * 2.0 - 1.0) * level * 0.3)
                    .collect();
                let pos = rng.next_range(-0.7, 0.7);
                add_layer(&mut bus, at, &wash, 1.0, pos, pos);
            } else {
                let sweep = swept_noise(sr, &mut rng, 300.0, 4000.0, seg as f32 / sr, 6.0);
                add_layer(&mut bus, at, &sweep, level * 0.5, -0.4, 0.4);
            }
        }
        cursor += seg as f32;
    }

    crush_in_place(&mut bus, 4.5);
    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 60.0, 60.0);
    Ok(out)
}

static RISER_PARAMS: &[ParamSpec] = &[spec!(
    intensity,
    Float,
    0.6,
    0.0,
    1.0,
    "Build energy; raises the final level and tick level."
)];

fn render_riser(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let intensity = ctx.params.get("intensity") as f32;
    let duration = ctx.frames as f32 / sr;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // Noise sweep and tonal gliss rising over the whole render.
    let sweep = swept_noise(
        sr,
        &mut tagged_rng("riser", "sweep", ctx),
        200.0,
        6000.0,
        duration,
        1.8,
    );
    let sweep_frames = sweep.len().max(1);
    let ramped: Vec<f32> = sweep
        .iter()
        .enumerate()
        .map(|(i, &sample)| sample * (i as f32 / sweep_frames as f32).powi(3))
        .collect();
    add_layer(&mut bus, 0, &ramped, 0.35 + 0.25 * intensity, -0.3, 0.3);

    let mut gliss_rng = tagged_rng("riser", "gliss", ctx);
    let mut osc = Osc::new(OscKind::Saw, 110.0, sr, &mut gliss_rng);
    let mut lp = Biquad::design(BiquadKind::Lowpass, sr, 4000.0, BUTTERWORTH, 0.0);
    let gliss: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let freq = 110.0 * 6.0f32.powf(t / duration);
            osc.set_freq(guard_hz(freq, sr));
            let ramp = (t / duration).powi(2);
            lp.process(osc.process()) * ramp * (0.25 + 0.2 * intensity)
        })
        .collect();
    bus.add_panned(0, -0.25, &gliss);

    // Accelerating ticks: the classic build-up pulse.
    let mut ticks = tagged_rng("riser", "ticks", ctx);
    let mut t = 0.0f32;
    let mut interval = 0.45f32;
    while t < duration {
        let tick = noise_burst(sr, &mut ticks, 3200.0, 0.03);
        let pos = ticks.next_range(-0.5, 0.5);
        add_layer(
            &mut bus,
            (t * sr) as usize,
            &tick,
            0.08 + 0.18 * (t / duration),
            pos,
            pos,
        );
        interval = (interval * 0.93).max(0.07);
        t += interval;
    }

    // No tail fade: a riser is cut at its peak by design.
    Ok(bus.buffer())
}

static HEARTBEAT_PARAMS: &[ParamSpec] = &[spec!(
    bpm,
    Float,
    60.0,
    40.0,
    120.0,
    "Heart tempo in beats per minute."
)];

fn render_heartbeat(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let bpm = ctx.params.get("bpm") as f32;
    let period = 60.0 / bpm;
    let duration = ctx.frames as f32 / sr;
    let mut rng = tagged_rng("heartbeat", "thumps", ctx);
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);
    let mut lp = Biquad::design(BiquadKind::Lowpass, sr, 180.0, BUTTERWORTH, 0.0);

    let mut t = 0.0f32;
    while t < duration {
        let at = (t * sr) as usize;
        // Lub: the strong thump, slightly left.
        let lub: Vec<f32> = sub_sweep(&mut rng, sr, 64.0, 40.0, 0.11, 0.17)
            .into_iter()
            .map(|sample| lp.process(sample * 0.55))
            .collect();
        add_layer(&mut bus, at, &lub, 1.0, -0.06, -0.06);
        let lub_body = noise_burst(sr, &mut rng, 250.0, 0.12);
        add_layer(&mut bus, at, &lub_body, 0.1, -0.06, -0.06);

        // Dub: the answer, softer and slightly right.
        let dub_at = at + (0.36f32.min(0.42 * period) * sr) as usize;
        let dub: Vec<f32> = sub_sweep(&mut rng, sr, 56.0, 38.0, 0.09, 0.13)
            .into_iter()
            .map(|sample| lp.process(sample * 0.38))
            .collect();
        add_layer(&mut bus, dub_at, &dub, 1.0, 0.06, 0.06);
        let dub_body = noise_burst(sr, &mut rng, 220.0, 0.1);
        add_layer(&mut bus, dub_at, &dub_body, 0.07, 0.06, 0.06);

        t += period;
    }

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 80.0, 80.0);
    Ok(out)
}

static FOOTSTEPS_PARAMS: &[ParamSpec] = &[spec!(rate, Float, 1.8, 0.5, 4.0, "Steps per second.")];

fn render_footsteps(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let rate = ctx.params.get("rate") as f32;
    let duration = ctx.frames as f32 / sr;
    let mut rng = tagged_rng("footsteps", "steps", ctx);
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // A step grid with jitter; feet alternate across the stereo field.
    let mut step = 0usize;
    let mut t = 0.0f32;
    while t < duration {
        let at = (t * sr) as usize;
        let thud_cutoff = 750.0 + rng.next_f32() * 700.0;
        let thud_decay = 0.045 + rng.next_f32() * 0.035;
        let thud = noise_burst(sr, &mut rng, thud_cutoff, thud_decay);
        let level = 0.3 + 0.2 * rng.next_f32();
        let pos = if step % 2 == 0 {
            -0.4 + rng.next_f32() * 0.15
        } else {
            0.4 - rng.next_f32() * 0.15
        };
        add_layer(&mut bus, at, &thud, level, pos, pos);

        // Heel click just after the thud.
        let click_cutoff = 4200.0 + rng.next_f32() * 2500.0;
        let click = noise_burst(sr, &mut rng, click_cutoff, 0.008);
        add_layer(
            &mut bus,
            at + (0.045 * sr) as usize,
            &click,
            0.1 + 0.06 * rng.next_f32(),
            pos,
            pos,
        );

        step += 1;
        t += 1.0 / rate + rng.next_f32() * 0.06 - 0.03;
    }

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 80.0, 80.0);
    Ok(out)
}

static RECIPES: [Recipe; 29] = [
    Recipe {
        id: "rain",
        description: "Steady rainfall: a gusty filtered bed with panned droplet pings.",
        params: RAIN_PARAMS,
        render: render_rain,
    },
    Recipe {
        id: "wind",
        description: "Breeze to gale: a wandering band of filtered noise with slow gusts.",
        params: WIND_PARAMS,
        render: render_wind,
    },
    Recipe {
        id: "thunder",
        description: "A lightning strike: crack, rumble and a rolling echo; distance sets the perspective.",
        params: THUNDER_PARAMS,
        render: render_thunder,
    },
    Recipe {
        id: "ocean",
        description: "Waves on a shore: a swelling bed that rises and falls with the surf.",
        params: OCEAN_PARAMS,
        render: render_ocean,
    },
    Recipe {
        id: "fire",
        description: "A burning fire: soft hiss, quick crackle ticks and occasional deep pops.",
        params: FIRE_PARAMS,
        render: render_fire,
    },
    Recipe {
        id: "stream",
        description: "A small stream: burbling resonant water over a gentle bed, with droplet bubbles.",
        params: STREAM_PARAMS,
        render: render_stream,
    },
    Recipe {
        id: "explosion",
        description: "A detonation: sub boom, blast, debris ticks and a long rumble; size and distance shape it.",
        params: EXPLOSION_PARAMS,
        render: render_explosion,
    },
    Recipe {
        id: "impact",
        description: "A heavy impact: thump, slap and body air.",
        params: IMPACT_PARAMS,
        render: render_impact,
    },
    Recipe {
        id: "whoosh",
        description: "A fast air pass: layered band sweeps travelling across the stereo field.",
        params: WHOOSH_PARAMS,
        render: render_whoosh,
    },
    Recipe {
        id: "sword",
        description: "A sword swing: a sharp whoosh with a metallic ring.",
        params: SWORD_PARAMS,
        render: render_sword,
    },
    Recipe {
        id: "laser",
        description: "A classic zapp: an FM pitch fall that starts bright and cleans to a tone.",
        params: LASER_PARAMS,
        render: render_laser,
    },
    Recipe {
        id: "blaster",
        description: "A quick shot: a compressed FM drop with a noise crack.",
        params: BLASTER_PARAMS,
        render: render_blaster,
    },
    Recipe {
        id: "teleport",
        description: "A shimmering rise with sparkle grains and a soft arrival thump.",
        params: TELEPORT_PARAMS,
        render: render_teleport,
    },
    Recipe {
        id: "forcefield",
        description: "An energized shield hum with a throbbing edge.",
        params: FORCEFIELD_PARAMS,
        render: render_forcefield,
    },
    Recipe {
        id: "robot_talk",
        description: "A stream of synthesized robot beeps and chirps.",
        params: ROBOT_TALK_PARAMS,
        render: render_robot_talk,
    },
    Recipe {
        id: "ui_success",
        description: "Two ascending sine blips (880 Hz then 1318 Hz).",
        params: UI_SUCCESS_PARAMS,
        render: render_ui_success,
    },
    Recipe {
        id: "ui_error",
        description: "A low double buzz: two short gated bursts.",
        params: UI_ERROR_PARAMS,
        render: render_ui_error,
    },
    Recipe {
        id: "ui_click",
        description: "A tiny UI tick.",
        params: UI_CLICK_PARAMS,
        render: render_ui_click,
    },
    Recipe {
        id: "sonar",
        description: "A sonar ping with fading echoes and a soft sub bloop.",
        params: SONAR_PARAMS,
        render: render_sonar,
    },
    Recipe {
        id: "alarm",
        description: "A sweeping alarm siren; speed sets the sweep rate.",
        params: ALARM_PARAMS,
        render: render_alarm,
    },
    Recipe {
        id: "engine",
        description: "A running engine: firing-rate saws over load-dependent noise.",
        params: ENGINE_PARAMS,
        render: render_engine,
    },
    Recipe {
        id: "glitch",
        description: "Digital stutter: chopped noise, square blips and a bit-crushed texture.",
        params: GLITCH_PARAMS,
        render: render_glitch,
    },
    Recipe {
        id: "riser",
        description: "A build-up spanning the whole render: rising sweep, tonal gliss and accelerating ticks.",
        params: RISER_PARAMS,
        render: render_riser,
    },
    Recipe {
        id: "hit",
        description: "A punchy hit: sub thump, slap transient and body.",
        params: HIT_PARAMS,
        render: render_hit,
    },
    Recipe {
        id: "sub_drop",
        description: "A deep descending sub with drive and a soft puff.",
        params: SUB_DROP_PARAMS,
        render: render_sub_drop,
    },
    Recipe {
        id: "heartbeat",
        description: "A lub-dub heartbeat at the given tempo.",
        params: HEARTBEAT_PARAMS,
        render: render_heartbeat,
    },
    Recipe {
        id: "footsteps",
        description: "Footsteps on a step grid, alternating left and right.",
        params: FOOTSTEPS_PARAMS,
        render: render_footsteps,
    },
    Recipe {
        id: "spell_cast",
        description: "A cast: airy whoosh, sparkle grains and a bell-like resolve.",
        params: SPELL_CAST_PARAMS,
        render: render_spell_cast,
    },
    Recipe {
        id: "crickets",
        description: "Pulsed ~4.2 kHz cricket chirps scattered across the field.",
        params: CRICKETS_PARAMS,
        render: render_crickets,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use qinari_dsp::analysis::goertzel_magnitude;

    const SR: u32 = 48_000;

    #[test]
    fn seconds_to_frames_rounds_and_never_returns_zero() {
        assert_eq!(seconds_to_frames(1.5, SR), 72_000);
        assert_eq!(seconds_to_frames(0.0, SR), 1, "zero rounds up to one frame");
        assert_eq!(seconds_to_frames(0.05, 8000), 400);
        assert_eq!(seconds_to_frames(600.0, 192_000), 115_200_000);
    }

    #[test]
    fn guard_hz_respects_the_nyquist_band() {
        assert!((guard_hz(1000.0, 8000.0) - 1000.0).abs() < 1e-6);
        assert!(guard_hz(6700.0, 8000.0) <= 8000.0 * 0.249_75 + 1e-6);
        assert!((guard_hz(0.5, 8000.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn ping_decays_and_reads_at_its_frequency() {
        let mut rng = Qrng::new(5);
        let tone = ping(&mut rng, 48000.0, 1000.0, 0.2);
        assert!(tone.iter().all(|s| s.is_finite()));
        let quarter = tone.len() / 4;
        let head_rms = (tone[..quarter]
            .iter()
            .map(|&s| f64::from(s * s))
            .sum::<f64>()
            / quarter as f64)
            .sqrt();
        let tail_rms = (tone[3 * quarter..]
            .iter()
            .map(|&s| f64::from(s * s))
            .sum::<f64>()
            / quarter as f64)
            .sqrt();
        assert!(
            head_rms > 10.0 * tail_rms,
            "head {head_rms} tail {tail_rms}"
        );
        let magnitude = goertzel_magnitude(&tone[..quarter], 1000.0, 48000.0);
        assert!(magnitude > 0.3, "ping reads {magnitude} at 1 kHz");
    }

    #[test]
    fn poisson_events_are_sorted_in_range_and_dense_enough() {
        let mut rng = Qrng::new(11);
        let events = poisson_events(&mut rng, 72_000, SR, 20.0);
        assert!(events.windows(2).all(|pair| pair[0] <= pair[1]), "sorted");
        assert!(events.iter().all(|&at| at < 72_000));
        // Expected 30 events over 1.5 s at 20/s; accept a wide band.
        assert!((12..=60).contains(&events.len()), "got {}", events.len());
        assert_eq!(poisson_events(&mut rng, 1000, SR, 0.0).len(), 0);
        assert_eq!(poisson_events(&mut rng, 0, SR, 20.0).len(), 0);
    }

    #[test]
    fn bus_accumulates_and_truncates() {
        let mut bus = Bus::new(2, 10, SR);
        bus.add(0, 0, 0.5);
        bus.add(99, 0, 1.0); // out of range: ignored
        bus.add(0, 5, 1.0); // out of range: ignored
        bus.add_panned(2, -1.0, &[0.25, 0.25]);
        let buffer = bus.buffer();
        assert_eq!(buffer.frames(), 10);
        assert!((buffer.get(0, 0) - 0.5).abs() < 1e-6);
        assert_eq!(buffer.get(0, 1), 0.0);
        // Hard left: all signal in the left plane, none in the right.
        assert!((buffer.get(0, 2) - 0.25).abs() < 1e-6);
        assert_eq!(buffer.get(1, 2), 0.0);
        assert!((buffer.get(0, 3) - 0.25).abs() < 1e-6);
        assert_eq!(buffer.get(1, 3), 0.0);
        assert_eq!(buffer.get(0, 4), 0.0, "past the slice");

        // Mono bus ignores the pan position.
        let mut mono = Bus::new(1, 4, SR);
        mono.add_panned(0, 1.0, &[0.3]);
        let mono_buffer = mono.buffer();
        assert!((mono_buffer.get(0, 0) - 0.3).abs() < 1e-6);
        assert_eq!(mono_buffer.num_channels(), 1);
    }

    #[test]
    fn add_layer_applies_gain_and_pan_drift() {
        let mut bus = Bus::new(2, 4, SR);
        add_layer(&mut bus, 0, &[0.5, 0.5, 0.5, 0.5], 0.8, -1.0, 1.0);
        let buffer = bus.buffer();
        let half_sqrt = std::f32::consts::FRAC_1_SQRT_2;
        // t = 1/4: pos -0.5, left sqrt(0.75), right sqrt(0.25).
        assert!((buffer.get(0, 0) - 0.4 * 0.75f32.sqrt()).abs() < 1e-6);
        assert!((buffer.get(1, 0) - 0.4 * 0.25f32.sqrt()).abs() < 1e-6);
        // t = 1/2: centre, -3.01 dB a side.
        assert!((buffer.get(0, 1) - 0.4 * half_sqrt).abs() < 1e-6);
        assert!((buffer.get(1, 1) - 0.4 * half_sqrt).abs() < 1e-6);
        // Last sample sits exactly at pan_to: hard right.
        assert_eq!(buffer.get(0, 3), 0.0);
        assert!(
            (buffer.get(1, 3) - 0.4).abs() < 1e-6,
            "{}",
            buffer.get(1, 3)
        );
    }

    #[test]
    fn catalog_ids_are_unique_and_complete() {
        let ids: Vec<&str> = recipes().iter().map(|recipe| recipe.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "duplicate recipe id");
        assert_eq!(recipes().len(), 29);
        assert!(find_recipe("rain").is_some());
        assert!(find_recipe("nope").is_none());
    }

    #[test]
    fn param_tables_are_well_formed() {
        for recipe in recipes() {
            let mut names: Vec<&str> = Vec::new();
            for spec_entry in recipe.params {
                assert!(
                    names.iter().all(|name| *name != spec_entry.name),
                    "{} declares {} twice",
                    recipe.id,
                    spec_entry.name
                );
                names.push(spec_entry.name);
                assert!(
                    spec_entry.min < spec_entry.max,
                    "{}: {} has an empty range",
                    recipe.id,
                    spec_entry.name
                );
                assert!(
                    (spec_entry.min..=spec_entry.max).contains(&spec_entry.default),
                    "{}: default for {} is outside its range",
                    recipe.id,
                    spec_entry.name
                );
            }
        }
    }

    #[test]
    fn suggest_recipe_orders_by_distance_and_filters() {
        let near = suggest_recipe("rian", 5);
        assert_eq!(near.first(), Some(&"rain"));
        assert!(
            suggest_recipe("zzzzzzzz", 5).is_empty(),
            "far typos yield no list"
        );
        let exact = suggest_recipe("heartbeat", 3);
        assert_eq!(exact.first(), Some(&"heartbeat"));
    }
}
