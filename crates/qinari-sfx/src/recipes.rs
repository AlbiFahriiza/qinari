//! The 30 procedural sound-effect recipes.
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

// The 30 render functions match the `Recipe::render` fn-pointer type whose
// `Result` return is part of the crate contract: recipes are allowed to
// fail even though none of today's implementations can, so clippy's
// `unnecessary_wraps` is a planned false positive here.
#![allow(clippy::unnecessary_wraps)]
use crate::params::spec;
use crate::params::{ParamSet, ParamSpec};
use qinari_dsp::buffer::AudioBuffer;
use qinari_dsp::env::{friedlander, Adsr, Percussive, Segment, SegmentShape, SegmentSpec};
use qinari_dsp::events::{cox_events, renewal_events};
use qinari_dsp::filter::{Biquad, BiquadKind, Svf};
use qinari_dsp::fx::{Crush, Drive, Tremolo};
use qinari_dsp::latent::LatentField;
use qinari_dsp::modal::{Material, ModalBank};
use qinari_dsp::noise::{
    band_limited_gaussian, noise_burst, noise_burst_attacked, swept_noise, NoiseGen, NoiseKind,
};
use qinari_dsp::osc::{fast_sine, Osc, OscKind};
use qinari_dsp::pan::pan;
use qinari_dsp::rng::{Qrng, Seed};
use qinari_dsp::spatial::{decorrelate, DistanceFilter, EarlyReflections};
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
/// rate yields no events. Kept for the genuinely high-rate streams
/// (`RESEARCH-AUDIO-REALISM.md` §R 2.4); clustered and drift-coupled
/// fields use the renewal and Cox processes in `qinari_dsp::events`.
pub fn poisson_events(rng: &mut Qrng, frames: usize, sr: u32, rate_per_sec: f32) -> Vec<usize> {
    qinari_dsp::events::poisson_events(rng, frames, sr, rate_per_sec)
}

/// Clamps a latent-field reading into `0..1` for depth modulation.
///
/// Fields are calibrated so ±1σ lands on the span edges; readings can
/// exceed them (as in any Gaussian process), and depths must stay in
/// range.
fn unit_depth(field_value: f32) -> f32 {
    field_value.clamp(0.0, 1.0)
}

/// Builds the shared latent field for one bed render, tilted toward
/// 1/f.
///
/// Every layer of the recipe (bed gain, cutoffs, gust depth, event
/// rate) reads THIS field, so cross-band envelope correlation and
/// event/bed co-variation exist by construction
/// (`DESIGN-REALISM-ENGINE.md` §7). The span is `0..1` (the ±1σ range
/// of the calibrated field); the tilt flattens the field's modulation
/// spectrum so no recipe carries an exactly-periodic gain line.
fn bed_latent_tilted(recipe: &str, ctx: &RenderContext, corner_hz: f32, tilt: f32) -> LatentField {
    LatentField::tilted(
        &mut tagged_rng(recipe, "latent", ctx),
        ctx.sr as f32,
        ctx.frames,
        corner_hz,
        tilt,
        0.0,
        1.0,
    )
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

/// Strikes a material bank with a few milliseconds of white-noise force.
/// The bank projects the excitation onto every mode's damped basis, so
/// the strike shapes the inter-mode energy; its single global gain sets
/// the layer level while per-mode relative gains pass through untouched
/// (plan 0.2.0 P0-1 — the 0.1.5 peak normalization to 0.7 destroyed the
/// strike-energy information, finding F3). The strike point is drawn
/// around the object's middle, varying per hit like Pauletto & Barahona's
/// per-hit randomization.
fn material_strike(
    rng: &mut Qrng,
    sr: f32,
    material: Material,
    ring_secs: f32,
) -> Result<Vec<f32>, String> {
    let bank = ModalBank::material(material, rng);
    let excitation: Vec<f32> = (0..(0.004 * sr) as usize)
        .map(|_| rng.next_range(-1.0, 1.0))
        .collect();
    let position = 0.35 + 0.3 * rng.next_f32();
    bank.strike_at(sr, &excitation, ring_secs, position)
        .map_err(|error| error.to_string())
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

/// Renders a sine ping with a short linear attack ramp before the T60
/// decay — the transient shape real droplet onsets carry
/// (`RESEARCH-AUDIO-REALISM.md` §R 2.2).
pub fn ping_attacked(rng: &mut Qrng, sr: f32, freq: f32, t60: f32, attack_secs: f32) -> Vec<f32> {
    let mut osc = Osc::new(OscKind::Sine, guard_hz(freq, sr), sr, rng);
    let tau = t60 / 6.91;
    let mut env = Percussive::shaped(sr, attack_secs, tau, tau, 0.5);
    env.trigger();
    let frames = ((1.2 * t60 * sr).ceil() as usize).max(4);
    (0..frames).map(|_| osc.process() * env.process()).collect()
}

fn render_rain(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let intensity = ctx.params.get("intensity") as f32;
    let distance = ctx.params.get("distance") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // One latent field drives the bed gain, the bed cutoff and the
    // droplet rate together (replaces the old 0.07 Hz gust sinusoid and
    // the independent Poisson stream).
    let latent = bed_latent_tilted("rain", ctx, 0.18, 0.5);

    // Bed: pink noise shaped by the plan's cutoffs, breathing on the
    // shared field.
    let bed_cutoff = (1500.0 + 6500.0 * intensity - 1200.0 * distance).clamp(220.0, 9000.0);
    let hp_cutoff = (400.0 - 250.0 * distance).clamp(60.0, 600.0);
    let bed_level = 0.06 + 0.14 * intensity;
    let mut bed = NoiseGen::new(NoiseKind::Pink, tagged_rng("rain", "bed", ctx));
    // Svf for the lowpass: the corner breathes with the field, and only
    // the TPT core retunes without state discontinuities.
    let mut bed_lowpass = Svf::new(sr, bed_cutoff, BUTTERWORTH);
    let mut bed_highpass = Biquad::design(BiquadKind::Highpass, sr, hp_cutoff, BUTTERWORTH, 0.0);
    let bed_samples: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let depth = unit_depth(latent.value_at(frame));
            let gust = 0.8 + 0.45 * depth;
            // The cutoff breathes on the same field (±12%), so the
            // bands co-vary by construction.
            let breathe = 1.0 + 0.12 * (depth - 0.5);
            bed_lowpass.set(sr, bed_cutoff * breathe, BUTTERWORTH);
            bed_highpass.process(bed_lowpass.process(bed.process()).low) * bed_level * gust
        })
        .collect();
    bus.add_panned(0, 0.0, &bed_samples);

    // Droplets: a Cox process whose rate rides the same field — the
    // shower thickens with the gusts. Panned across the field, some
    // heavy.
    let mut drops = tagged_rng("rain", "drops", ctx);
    let mean_rate = 18.0 + 95.0 * intensity;
    let events = cox_events(&mut drops, ctx.frames, ctx.sr, mean_rate, |frame| {
        0.35 + 1.4 * unit_depth(latent.value_at(frame))
    });
    let attenuation = 1.0 - 0.55 * distance;
    for &at in &events {
        let heavy = drops.next_f32() < 0.08;
        let (freq, t60, level, attack) = if heavy {
            (
                900.0 + drops.next_f32() * 700.0,
                0.1 + drops.next_f32() * 0.05,
                0.09 + drops.next_f32() * 0.08,
                0.0012,
            )
        } else {
            (
                2200.0 + drops.next_f32() * 4500.0,
                0.035 + drops.next_f32() * 0.075,
                0.02 + drops.next_f32() * 0.07,
                0.0006,
            )
        };
        let pos = drops.next_range(-0.85, 0.85);
        let drop = ping_attacked(&mut drops, sr, freq, t60, attack);
        add_layer(&mut bus, at, &drop, level * attenuation, pos, pos);
    }

    let mut out = bus.buffer();
    // Bed decorrelation: shared below 500 Hz, independent above.
    decorrelate(&mut out, 500.0, sr, &mut tagged_rng("rain", "decor", ctx));
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

    // One tilted latent drives the band wander, the gust gain and the
    // whistle together (tilt: wind turbulence carries mid-band energy;
    // the old 0.06/0.11/0.045 Hz sinusoids are gone).
    let latent = bed_latent_tilted("wind", ctx, 0.12, 0.55);

    // Body: a wandering lowpass band whose centre and gain ride the
    // shared field.
    let body_level = 0.05 + 0.16 * intensity;
    let mut body = NoiseGen::new(NoiseKind::Pink, tagged_rng("wind", "body", ctx));
    let mut body_svf = Svf::new(sr, 700.0, 1.1);
    let body_samples: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let depth = unit_depth(latent.value_at(frame));
            body_svf.set(sr, 450.0 + 800.0 * intensity * depth, 1.1);
            let gust = 0.65 + 0.35 * depth;
            body_svf.process(body.process()).low * body_level * gust
        })
        .collect();
    bus.add_panned(0, 0.0, &body_samples);

    // Whistle: a resonant band riding above the body on the same field.
    let whistle_level = 0.012 + 0.05 * intensity;
    let mut whistle = NoiseGen::new(NoiseKind::White, tagged_rng("wind", "whistle", ctx));
    let mut whistle_svf = Svf::new(sr, 1600.0, 4.0);
    let whistle_samples: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let depth = unit_depth(latent.value_at(frame));
            whistle_svf.set(sr, 1300.0 + 900.0 * intensity * depth, 4.0);
            whistle_svf.process(whistle.process()).band * whistle_level
        })
        .collect();
    bus.add_panned(0, 0.0, &whistle_samples);

    let mut out = bus.buffer();
    decorrelate(&mut out, 500.0, sr, &mut tagged_rng("wind", "decor", ctx));
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

/// The thunder rumble bed: a fractal (α≈2) bed below 300 Hz that decays
/// with the last arrival, its tail breathing on a latent field.
fn thunder_rumble(ctx: &RenderContext, sr: f32, distance: f32) -> Vec<f32> {
    let rumble_t60 = 1.6 + 1.4 * distance;
    let rumble_frames = (1.2 * rumble_t60 * sr) as usize;
    let onset_tau = 0.12 + 0.5 * distance;
    let tail = bed_latent_tilted("thunder", ctx, 0.3, 0.45);
    let mut source = NoiseGen::fractal(2.0, sr, tagged_rng("thunder", "rumble", ctx));
    let mut lowpass = Biquad::design(BiquadKind::Lowpass, sr, 300.0, BUTTERWORTH, 0.0);
    let mut highpass = Biquad::design(BiquadKind::Highpass, sr, 30.0, BUTTERWORTH, 0.0);
    let mut decay_shape = Percussive::from_t60(sr, rumble_t60);
    decay_shape.trigger();
    (0..rumble_frames)
        .map(|frame| {
            let t = frame as f32 / sr;
            let onset = 1.0 - (-t / onset_tau).exp();
            let breathe = 0.75 + 0.5 * unit_depth(tail.value_at(frame));
            lowpass.process(highpass.process(source.process()))
                * decay_shape.process()
                * onset
                * breathe
                * (0.5 - 0.3 * distance)
        })
        .collect()
}

fn render_thunder(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let distance = ctx.params.get("distance") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // The tortuous-channel model (Roy 1980 via §R 2.3): a strike is not
    // one crack but 4-8 arrivals from channel segments at different
    // distances, spread over seconds — the spread grows with distance.
    let mut arrivals_rng = tagged_rng("thunder", "arrivals", ctx);
    let count = 4 + arrivals_rng.next_int_range(0, 4) as usize;
    let spread = 1.0 + 4.0 * distance;
    let mut gaps: Vec<f32> = (0..count.saturating_sub(1))
        .map(|_| 0.25 + arrivals_rng.next_f32() * 1.75)
        .collect();
    let gap_sum: f32 = gaps.iter().sum();
    if gap_sum > 0.0 {
        for gap in &mut gaps {
            *gap *= spread / gap_sum;
        }
    }

    // Sub thump under the first arrival.
    let sub = sub_sweep(
        &mut tagged_rng("thunder", "sub", ctx),
        sr,
        52.0,
        30.0,
        0.35,
        0.6,
    );
    add_layer(&mut bus, 0, &sub, 0.4, 0.0, 0.0);

    // One clap shape shared by the arrivals, scaled per arrival.
    let clap_specs = [
        SegmentSpec {
            target: 1.0,
            secs: 0.008,
            shape: SegmentShape::Linear,
        },
        SegmentSpec {
            target: 0.55,
            secs: 0.3,
            shape: SegmentShape::Smooth,
        },
        SegmentSpec {
            target: 0.0,
            secs: 0.7,
            shape: SegmentShape::Linear,
        },
    ];
    let mut clap_env = Segment::new(&clap_specs, sr);
    let clap_frames = (clap_env.total_secs(sr) * sr) as usize;

    let mut at = 0usize;
    for arrival in 0..count {
        // Later arrivals come from farther along the channel: distance
        // and therefore attenuation grow with the arrival index.
        let arrival_distance =
            (distance + (arrival as f32 / count as f32) * 0.45 + arrivals_rng.next_f32() * 0.1)
                .clamp(0.0, 1.0);
        let atten = 1.0 / (1.0 + 4.0 * arrival_distance);
        let loudness = 0.7 + 0.3 * arrivals_rng.next_f32();

        // Clap body: clap energy concentrates below ~300 Hz (Bodhika
        // 2019 via §R 2.3).
        let clap = band_limited_gaussian(sr, &mut arrivals_rng, 25.0, 280.0, clap_frames);
        clap_env.trigger();
        let shaped: Vec<f32> = clap.iter().map(|s| s * clap_env.process()).collect();
        let pan = arrivals_rng.next_range(-0.25, 0.25);
        add_layer(&mut bus, at, &shaped, atten * loudness, pan, pan);

        // Only the near field hears the HF crack of the first break.
        if arrival == 0 && distance < 0.5 {
            let crack_cutoff = (2800.0 * (1.0 - 0.6 * distance) + 400.0).clamp(500.0, 3200.0);
            let crack = noise_burst_attacked(
                sr,
                &mut arrivals_rng,
                crack_cutoff,
                0.02 + 0.02 * (1.0 - distance),
                0.0006,
            );
            add_layer(
                &mut bus,
                at,
                &crack,
                (0.9 - 0.9 * distance) * loudness,
                pan,
                pan,
            );
        }

        if arrival < gaps.len() {
            at += (gaps[arrival] * sr) as usize;
        }
    }

    // Rumble: a fractal (α≈2) bed below 300 Hz that decays with the
    // last arrival; its tail breathes on a latent field.
    let rumble = thunder_rumble(ctx, sr, distance);
    add_layer(&mut bus, 0, &rumble, 1.0, 0.0, 0.0);

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

    // The fire's flare level: one tilted latent (corner ~1.4 Hz, tilt
    // filling the flicker band) replaces the 0.8 Hz sample-and-hold.
    let flare = bed_latent_tilted("fire", ctx, 1.4, 0.65);

    // Hiss bed flickering on the flare field.
    let hiss_level = 0.015 + 0.05 * intensity;
    let mut hiss = NoiseGen::new(NoiseKind::Pink, tagged_rng("fire", "hiss", ctx));
    let mut hiss_hp = Biquad::design(BiquadKind::Highpass, sr, 1200.0, BUTTERWORTH, 0.0);
    let hiss_samples: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let jitter = 0.7 + 0.3 * unit_depth(flare.value_at(frame));
            hiss_hp.process(hiss.process()) * hiss_level * jitter
        })
        .collect();
    bus.add_panned(0, 0.0, &hiss_samples);

    // Crackle ticks ride the flare as a Cox process — bursts cluster on
    // the flare-ups. Attacked bursts carry a real onset.
    let mut crackle = tagged_rng("fire", "crackle", ctx);
    let tick_events = cox_events(
        &mut crackle,
        ctx.frames,
        ctx.sr,
        6.0 + 42.0 * intensity,
        |frame| 0.3 + 1.7 * unit_depth(flare.value_at(frame)),
    );
    for &at in &tick_events {
        let cutoff = 2200.0 + crackle.next_f32() * 3800.0;
        let decay = 0.006 + crackle.next_f32() * 0.02;
        let attack = 0.0003 + crackle.next_f32() * 0.0005;
        let tick = noise_burst_attacked(sr, &mut crackle, cutoff, decay, attack);
        let loud = crackle.next_f32();
        let pos = crackle.next_range(-0.8, 0.8);
        add_layer(&mut bus, at, &tick, 0.05 + 0.32 * loud * loud, pos, pos);
    }

    // Deep pops from the coals: a renewal stream with a heavy tail
    // (CV 1.6) — pops bunch up like real coal shifts.
    let pop_events = renewal_events(&mut crackle, ctx.frames, ctx.sr, 0.7 + 1.3 * intensity, 1.6);
    for &at in &pop_events {
        let cutoff = 380.0 + crackle.next_f32() * 250.0;
        let decay = 0.04 + crackle.next_f32() * 0.03;
        let pop = noise_burst_attacked(sr, &mut crackle, cutoff, decay, 0.0015);
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
    decorrelate(&mut out, 500.0, sr, &mut tagged_rng("fire", "decor", ctx));
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

    // The flow surge: one tilted latent (the old 0.9 Hz and 0.23 Hz
    // wobble sinusoids are gone) drives the burble centre, the bed
    // level and the bubble rate together.
    let flow = bed_latent_tilted("stream", ctx, 0.5, 0.7);

    // Burble: a resonant band whose centre wanders on the flow field.
    let burble_level = 0.05 + 0.13 * intensity;
    let mut burble = NoiseGen::new(NoiseKind::White, tagged_rng("stream", "burble", ctx));
    let mut burble_svf = Svf::new(sr, 900.0, 2.8);
    let burble_samples: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let depth = unit_depth(flow.value_at(frame));
            burble_svf.set(sr, 700.0 + 750.0 * depth, 2.8);
            burble_svf.process(burble.process()).band * burble_level
        })
        .collect();
    bus.add_panned(0, 0.0, &burble_samples);

    // Body: a soft low bed underneath, breathing on the same field.
    let mut body = NoiseGen::new(NoiseKind::Pink, tagged_rng("stream", "body", ctx));
    let mut body_lp = Biquad::design(BiquadKind::Lowpass, sr, 800.0, BUTTERWORTH, 0.0);
    let body_samples: Vec<f32> = (0..ctx.frames)
        .map(|frame| {
            let depth = unit_depth(flow.value_at(frame));
            body_lp.process(body.process()) * 0.035 * (0.7 + 0.6 * depth)
        })
        .collect();
    bus.add_panned(0, 0.0, &body_samples);

    // Bubbles: a Cox process on the flow field — bubble bursts cluster
    // with the surges.
    let mut bubbles = tagged_rng("stream", "bubbles", ctx);
    let events = cox_events(
        &mut bubbles,
        ctx.frames,
        ctx.sr,
        2.5 + 7.0 * intensity,
        |frame| 0.3 + 1.5 * unit_depth(flow.value_at(frame)),
    );
    for &at in &events {
        let freq = 700.0 + bubbles.next_f32() * 1900.0;
        let t60 = 0.05 + bubbles.next_f32() * 0.06;
        let droplet = ping_attacked(&mut bubbles, sr, freq, t60, 0.0008);
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
    decorrelate(&mut out, 500.0, sr, &mut tagged_rng("stream", "decor", ctx));
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

static BIRDSONG_PARAMS: &[ParamSpec] = &[
    spec!(
        activity,
        Float,
        0.6,
        0.0,
        1.0,
        "How busy the field is; sets the mean phrase rate."
    ),
    spec!(
        variety,
        Float,
        0.5,
        0.0,
        1.0,
        "How many distinct songs share the field."
    ),
];

/// The Podos (1997) performance envelope: trill rate times frequency
/// bandwidth cannot exceed this (notes/s × Hz). Fast-and-wide songs are
/// physiologically impossible, so the two are drawn jointly, never
/// independently.
const PODOS_RATE_BW_LIMIT: f32 = 50_000.0;

/// Draws a joint (trill rate, bandwidth) pair inside the Podos envelope:
/// bandwidth first, then the rate ceiling that bandwidth leaves.
fn podos_draw(rng: &mut Qrng) -> (f32, f32) {
    let bandwidth = rng.next_range(350.0, 4_250.0);
    let max_rate = (PODOS_RATE_BW_LIMIT / bandwidth).min(26.0);
    let rate = rng.next_range(2.5, max_rate.max(3.0));
    debug_assert!(rate * bandwidth <= PODOS_RATE_BW_LIMIT * 1.001);
    (rate, bandwidth)
}

/// One source-filter note: a buzzy syringeal source gliding `f0`→`f1`
/// through swept vocal-tract formants (Nowicki's helium result: oscine
/// song is source × filter, so bare sines are structurally wrong).
struct BirdNote {
    /// Glide start frequency in hertz.
    f0: f32,
    /// Glide end frequency in hertz.
    f1: f32,
    /// Note length in seconds.
    dur: f32,
    /// Vibrato rate in hertz (0 = none).
    vib_hz: f32,
    /// Vibrato depth as a fractional pitch wobble.
    vib_depth: f32,
}

/// Renders one note: a band-limited sawtooth source under an exponential
/// glide (with optional vibrato), filtered through three swept SVF
/// formants whose frequencies track the source, under a short
/// attack/release envelope. The formant set (ratios, Qs, weights) is
/// drawn per motif so each song has its own tract shape.
fn bird_note(
    rng: &mut Qrng,
    sr: f32,
    note: &BirdNote,
    formants: &[(f32, f32, f32); 3],
) -> Vec<f32> {
    let two_pi = 2.0 * std::f32::consts::PI;
    let mut source = Osc::new(OscKind::Saw, guard_hz(note.f0, sr), sr, rng);
    let mut tract = [
        Svf::new(sr, guard_hz(note.f0 * formants[0].0, sr), formants[0].1),
        Svf::new(sr, guard_hz(note.f0 * formants[1].0, sr), formants[1].1),
        Svf::new(sr, guard_hz(note.f0 * formants[2].0, sr), formants[2].1),
    ];
    let frames = ((note.dur * sr).ceil() as usize).max(4);
    let attack = (0.003 * sr) as usize;
    let release = ((0.25 * note.dur * sr) as usize).max(4);
    let ratio = note.f1 / note.f0;
    (0..frames)
        .map(|i| {
            let t = i as f32 / sr;
            let glide = note.f0 * ratio.powf(t / note.dur);
            let wobble = 1.0 + note.vib_depth * (two_pi * note.vib_hz * t).sin();
            let hz = guard_hz(glide * wobble, sr);
            source.set_freq(hz);
            let x = source.process();
            let mut out = 0.0_f32;
            for (filter, &(ratio, q, weight)) in tract.iter_mut().zip(formants) {
                filter.set(sr, hz * ratio, q);
                out += filter.process(x).band * weight;
            }
            // Short attack and release so notes click neither on nor off.
            let edge = if i < attack {
                i as f32 / attack as f32
            } else if i + release > frames {
                (frames - i) as f32 / release as f32
            } else {
                1.0
            };
            out * edge
        })
        .collect()
}

/// One motif as a template, not samples: the note specs, the gap after
/// each note, the mix weight, the per-motif formant set, and the jointly
/// drawn (rate, bandwidth) pair the notes were built inside. Every
/// repetition re-renders the template with micro-jitter, so nothing in
/// the field ever repeats verbatim.
struct MotifSpec {
    /// Note templates in singing order.
    notes: Vec<BirdNote>,
    /// Gap after each note in seconds.
    gaps: Vec<f32>,
    /// Mix weight (trills read softer than staircase calls).
    weight: f32,
    /// The motif's vocal-tract formants: (ratio over source f, Q, mix).
    formants: [(f32, f32, f32); 3],
    /// Trill rate in notes/s (drawn jointly with the bandwidth).
    rate_hz: f32,
    /// Frequency bandwidth in hertz the song spans.
    bandwidth_hz: f32,
}

/// Builds one song motif: the classic three shapes (staircase, trill,
/// downslur) with note timing derived from the drawn rate and pitch
/// spread from the drawn bandwidth — the Podos constraint couples them
/// by construction.
fn song_motif(songs: &mut Qrng) -> MotifSpec {
    let kind = songs.next_f32();
    let base = 2_100.0 + songs.next_f32() * 2_700.0;
    let (rate, bandwidth) = podos_draw(songs);
    let period = 1.0 / rate;
    // The tract: three formants at 1–3.4× the source, each motif's own.
    let formants = [
        (songs.next_range(1.0, 1.3), songs.next_range(4.0, 8.0), 1.0),
        (
            songs.next_range(1.8, 2.6),
            songs.next_range(5.0, 10.0),
            songs.next_range(0.5, 1.0),
        ),
        (
            songs.next_range(2.8, 3.6),
            songs.next_range(6.0, 12.0),
            songs.next_range(0.2, 0.6),
        ),
    ];
    let mut notes: Vec<BirdNote> = Vec::new();
    let mut gaps: Vec<f32> = Vec::new();
    let weight;
    if kind < 0.38 {
        // Staircase: 3-7 rising steps spanning the drawn bandwidth.
        weight = 1.0;
        let steps = 3 + songs.next_int_range(0, 4) as usize;
        for s in 0..steps {
            let f0 = base + bandwidth * s as f32 / steps as f32;
            let f1 = f0 + bandwidth / steps as f32;
            notes.push(BirdNote {
                f0,
                f1,
                dur: 0.55 * period,
                vib_hz: 0.0,
                vib_depth: 0.0,
            });
            gaps.push(0.45 * period);
        }
    } else if kind < 0.7 {
        // Trill: fast ticks — the drawn rate is high, so the Podos draw
        // already forced the bandwidth narrow.
        weight = 0.55;
        let ticks = 10 + songs.next_int_range(0, 14) as usize;
        for s in 0..ticks {
            let f0 = base + bandwidth * s as f32 / ticks as f32;
            notes.push(BirdNote {
                f0,
                f1: f0 + bandwidth / ticks as f32,
                dur: 0.6 * period,
                vib_hz: 0.0,
                vib_depth: 0.0,
            });
            gaps.push(0.4 * period);
        }
    } else {
        // Downslur: 2-4 long falling calls with vibrato, wide only when
        // the rate drawn for them is slow.
        weight = 0.8;
        let calls = 2 + songs.next_int_range(0, 2) as usize;
        for _ in 0..calls {
            let f0 = base + bandwidth * 0.5;
            let f1 = (base - bandwidth * 0.5).max(900.0);
            notes.push(BirdNote {
                f0,
                f1,
                dur: 1.6 * period,
                vib_hz: 16.0 + songs.next_f32() * 12.0,
                vib_depth: 0.02,
            });
            gaps.push(0.8 * period);
        }
    }
    MotifSpec {
        notes,
        gaps,
        weight,
        formants,
        rate_hz: rate,
        bandwidth_hz: bandwidth,
    }
}

/// Renders one occurrence of a motif with micro-jitter: ±3% pitch and
/// ±8% timing per repetition (real bouts repeat motifs with
/// micro-variation), returning per-note samples and their start offsets
/// in seconds relative to the phrase start.
fn motif_occurrence(rng: &mut Qrng, sr: f32, motif: &MotifSpec) -> Vec<(usize, Vec<f32>)> {
    let pitch = 1.0 + rng.next_range(-0.03, 0.03);
    let mut out = Vec::with_capacity(motif.notes.len());
    let mut cursor = 0.0_f32;
    for note in &motif.notes {
        let jittered = BirdNote {
            f0: note.f0 * pitch,
            f1: note.f1 * pitch,
            dur: note.dur * (1.0 + rng.next_range(-0.04, 0.04)),
            vib_hz: note.vib_hz,
            vib_depth: note.vib_depth,
        };
        out.push((
            (cursor * sr) as usize,
            bird_note(rng, sr, &jittered, &motif.formants),
        ));
        cursor += jittered.dur + motif.gaps[out.len() - 1] * (1.0 + rng.next_range(-0.08, 0.08));
    }
    out
}

fn render_birdsong(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let activity = ctx.params.get("activity") as f32;
    let variety = ctx.params.get("variety") as f32;
    let mut bus = Bus::new(ctx.channels, ctx.frames, ctx.sr);

    // The singer's repertoire: motif templates, built once per render.
    let mut songs = tagged_rng("birdsong", "songs", ctx);
    let song_count = 1 + (variety * 5.0).round() as usize;
    let motifs: Vec<MotifSpec> = (0..song_count).map(|_| song_motif(&mut songs)).collect();

    // Phrase field: a Cox process — the phrase rate rides a tilted
    // latent field, so bursts cluster and lull like real singing instead
    // of the metronome Poisson stream this replaces.
    let mut latent_rng = tagged_rng("birdsong", "latent", ctx);
    let latent = LatentField::tilted(&mut latent_rng, sr, ctx.frames, 0.25, 0.35, 0.3, 1.7);
    let mut field = tagged_rng("birdsong", "field", ctx);
    let rate = 0.08 + 0.55 * activity;
    let mut events = cox_events(&mut field, ctx.frames, ctx.sr, rate, |frame| {
        latent.value_at(frame)
    });
    let opening = (field.next_range(0.15, 0.85) * sr) as usize;
    events.push(opening);
    events.sort_unstable();

    // The singer works through the repertoire with a small transition
    // matrix — mostly repeat the current motif (species syntax),
    // sometimes switch — and every repetition is re-rendered with
    // micro-jitter, so the same tune never repeats verbatim.
    let mut jitter = tagged_rng("birdsong", "jitter", ctx);
    let mut current = field.next_int_range(0, (motifs.len() - 1) as u64) as usize;
    let mut pan = 0.0_f32;
    for &at in &events {
        if motifs.len() > 1 && field.next_f32() < 0.35 {
            let mut next = current;
            while next == current {
                next = field.next_int_range(0, (motifs.len() - 1) as u64) as usize;
            }
            current = next;
        }
        let motif = &motifs[current];
        // The joint draw is enforced at render time too, not just in
        // tests: no fast-and-wide song may ever reach the field.
        debug_assert!(
            motif.rate_hz * motif.bandwidth_hz <= PODOS_RATE_BW_LIMIT * 1.001,
            "motif outside the Podos envelope"
        );
        pan = (pan + field.next_range(-0.35, 0.35)).clamp(-0.8, 0.8);
        let level = (0.11 + 0.1 * field.next_f32()) * motif.weight;
        for (offset, note) in motif_occurrence(&mut jitter, sr, motif) {
            add_layer(&mut bus, at + offset, &note, level, pan, pan);
        }
    }

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 120.0, 120.0);
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

    // The physics-shaped event (§R 2.3): sub-ms shock front, Friedlander
    // body with its rarefaction (negative) phase, 2-4 distance-scaled
    // ground-reflection taps, air absorption over the propagation
    // distance, and a fractal rumble whose tail rides a latent field.
    // `size` maps to peak pressure and low-frequency proportion;
    // `distance` maps to t*, tap delays and absorption.
    let t_star = 0.06 + 0.42 * distance + 0.12 * size;
    let peak_pressure = 0.9 - 0.35 * distance;

    // Direct path assembled in mono, then shaped by the environment.
    let mut event_rng = tagged_rng("explosion", "event", ctx);
    let body_frames = ((4.5 * t_star) * sr).ceil() as usize;
    let mut event = vec![0.0f32; body_frames];

    // 1. Shock front: sub-ms broadband click, hard onset.
    let shock_cutoff = (8000.0 - 5000.0 * distance + 2000.0 * size).clamp(1500.0, 9000.0);
    let shock = noise_burst_attacked(sr, &mut event_rng, shock_cutoff, 0.006, 0.0004);
    for (slot, &sample) in event.iter_mut().zip(shock.iter()) {
        *slot += sample * peak_pressure;
    }

    // 2. Friedlander body: the bipolar P(t) shapes a band-limited
    // Gaussian carrier; the negative lobe is physical, not decorative.
    let body_cutoff = (1600.0 * (1.0 - 0.55 * distance) + 900.0 * size).clamp(300.0, 6000.0);
    let body = band_limited_gaussian(sr, &mut event_rng, 30.0, body_cutoff, body_frames);
    for (n, slot) in event.iter_mut().enumerate() {
        let t = n as f32 / sr;
        *slot += body[n] * friedlander(t, t_star) * peak_pressure * 1.4;
    }

    // 3. Ground reflections (Mach-stem sum): 2-4 taps at
    // distance-scaled delays with falling gains.
    let tap_count = 2 + event_rng.next_int_range(0, 2) as usize;
    let mut taps: Vec<(f32, f32)> = Vec::with_capacity(tap_count);
    let tap_base = 0.018 + 0.06 * distance;
    for tap in 0..tap_count {
        let delay = tap_base * (1.0 + tap as f32) * (0.8 + 0.4 * event_rng.next_f32());
        let gain = 0.45 * 0.6_f32.powi(tap as i32) * (1.0 - 0.4 * distance);
        taps.push((delay, gain));
    }
    let mut reflections = EarlyReflections::new(sr, &taps);
    for slot in &mut event {
        *slot = reflections.process(*slot);
    }

    // 4. Air absorption + propagation: delay, 1/d gain and HF rolloff
    // over the source distance (8 m near field to 60 m at distance 1).
    let mut air = DistanceFilter::from_meters(sr, 8.0 + 52.0 * distance);
    for slot in &mut event {
        *slot = air.process(*slot);
    }
    add_layer(&mut bus, 0, &event, 1.0, 0.0, 0.1);

    // 5. Sub boom: driven so it reads on small speakers (kept from the
    // classic recipe; the size-dependent LF proportion).
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

    // 6. Rumble: fractal (α≈2) bed band-limited to 30-300 Hz, onset
    // locked to the body, tail driven by the latent field.
    let rumble_t60 = 1.0 + 1.8 * size + 0.8 * distance;
    let rumble_frames = (1.2 * rumble_t60 * sr) as usize;
    let tail = bed_latent_tilted("explosion", ctx, 0.25, 0.45);
    let mut blast_source = NoiseGen::fractal(2.0, sr, tagged_rng("explosion", "rumble", ctx));
    let mut blast_lowpass = Biquad::design(BiquadKind::Lowpass, sr, 300.0, BUTTERWORTH, 0.0);
    let mut blast_highpass = Biquad::design(BiquadKind::Highpass, sr, 30.0, BUTTERWORTH, 0.0);
    let mut blast_shape = Percussive::shaped(sr, 0.02, rumble_t60 / 6.91, rumble_t60 / 3.0, 0.35);
    blast_shape.trigger();
    let rumble: Vec<f32> = (0..rumble_frames)
        .map(|frame| {
            let breathe = 0.7 + 0.5 * unit_depth(tail.value_at(frame));
            blast_lowpass.process(blast_highpass.process(blast_source.process()))
                * blast_shape.process()
                * breathe
                * (0.3 + 0.2 * size - 0.15 * distance)
        })
        .collect();
    add_layer(&mut bus, 0, &rumble, 1.0, 0.0, 0.0);

    // 7. Debris: ticks scattering after the blast (past the body, so
    // the negative-phase window stays clean), distance-attenuated.
    let debris_frames = (1.6 * sr) as usize;
    let rate = ((4.0 + 10.0 * size) * (1.0 - 0.6 * distance)).max(0.5);
    let mut debris = tagged_rng("explosion", "debris", ctx);
    let events = poisson_events(&mut debris, debris_frames, ctx.sr, rate);
    for &event_at in &events {
        let at = (0.5 * sr) as usize + event_at;
        let cutoff = 2500.0 + debris.next_f32() * 4500.0;
        let decay = 0.02 + debris.next_f32() * 0.03;
        let attack = 0.0004 + debris.next_f32() * 0.0012;
        let tick = noise_burst_attacked(sr, &mut debris, cutoff, decay, attack);
        let pos = debris.next_range(-0.8, 0.8);
        let level = (0.05 + 0.2 * debris.next_f32()) * (1.0 - 0.5 * distance);
        add_layer(&mut bus, at, &tick, level, pos, pos);
    }

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 10.0);
    Ok(out)
}

static IMPACT_PARAMS: &[ParamSpec] = &[
    spec!(
        intensity,
        Float,
        0.6,
        0.0,
        1.0,
        "Impact strength; raises thump and slap levels."
    ),
    spec!(
        material,
        Int,
        0.0,
        0.0,
        3.0,
        "Struck material: 0=wood, 1=metal, 2=ceramic, 3=thud."
    ),
];

fn render_impact(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let intensity = ctx.params.get("intensity") as f32;
    let material = Material::ALL[(ctx.params.get("material").clamp(0.0, 3.0) as usize).min(3)];
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

    // Body: the struck object rings as a modal bank (van den Doel &
    // Pai) — the pink-noise wash this replaces had no spectral fine
    // structure at all.
    let body = material_strike(&mut tagged_rng("impact", "body", ctx), sr, material, 0.9)?;
    add_layer(&mut bus, 0, &body, 0.35 + 0.4 * intensity, 0.0, 0.0);

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

    // Metal ring: a modal metal bank struck at contact (van den Doel &
    // Pai) — inharmonic bell-family partials replace the two bare
    // Karplus-Strong strings.
    let ring_samples = material_strike(
        &mut tagged_rng("sword", "metal", ctx),
        sr,
        Material::Metal,
        1.4,
    )?;
    add_layer(
        &mut bus,
        (0.16 * sr) as usize,
        &ring_samples,
        0.42 * ring,
        -0.15,
        0.25,
    );

    let mut out = bus.buffer();
    fade_edges(&mut out, ctx.sr, 0.0, 10.0);
    Ok(out)
}

static HIT_PARAMS: &[ParamSpec] = &[spec!(
    material,
    Int,
    3.0,
    0.0,
    3.0,
    "Struck material: 0=wood, 1=metal, 2=ceramic, 3=thud."
)];

fn render_hit(ctx: &RenderContext) -> Result<AudioBuffer, String> {
    let sr = ctx.sr as f32;
    let material = Material::ALL[(ctx.params.get("material").clamp(0.0, 3.0) as usize).min(3)];
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

    // Body: the struck object's modal ring (thud by default — a body
    // hit is dull; switch material for wood/metal/ceramic strikes).
    let body = material_strike(&mut tagged_rng("hit", "body", ctx), sr, material, 0.6)?;
    add_layer(&mut bus, 0, &body, 0.45, 0.0, 0.0);

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
        // Each footfall strikes a thud modal bank — a dull, heavily
        // damped object, jittered per step (van den Doel & Pai).
        let thud = material_strike(&mut rng, sr, Material::Thud, 0.16)?;
        let level = 0.3 + 0.2 * rng.next_f32();
        let pos = if step.is_multiple_of(2) {
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

static RECIPES: [Recipe; 30] = [
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
    Recipe {
        id: "birdsong",
        description: "Songbirds trade staircase chirps, trills and downslur calls across the field.",
        params: BIRDSONG_PARAMS,
        render: render_birdsong,
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
        assert_eq!(recipes().len(), 30);
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

    fn recipes_rng(tag: &str) -> Qrng {
        Qrng::from_seed(Seed::from_text(tag))
    }

    #[test]
    fn podos_draws_stay_inside_the_performance_envelope() {
        let mut rng = recipes_rng("podos-envelope");
        for _ in 0..500 {
            let (rate, bandwidth) = podos_draw(&mut rng);
            assert!(
                rate * bandwidth <= PODOS_RATE_BW_LIMIT * 1.0001,
                "rate {rate} Hz × bandwidth {bandwidth} Hz exceeds the Podos limit"
            );
            // The tradeoff itself: no fast-and-wide songs exist.
            if rate > 15.0 {
                assert!(
                    bandwidth < PODOS_RATE_BW_LIMIT / 15.0,
                    "fast ({rate} Hz) and wide ({bandwidth} Hz) at once"
                );
            }
        }
    }

    #[test]
    fn motifs_couple_timing_and_pitch_to_the_podos_draw() {
        let mut rng = recipes_rng("motif-coupling");
        for _ in 0..50 {
            let motif = song_motif(&mut rng);
            assert!(!motif.notes.is_empty());
            assert_eq!(motif.notes.len(), motif.gaps.len());
            // Timing derives from the drawn rate: staircase and trill
            // inter-onset intervals land on the period; downslur calls
            // span a couple of periods by design. Everything stays
            // period-quantized — no free-running durations.
            let period = 1.0 / motif.rate_hz;
            for (note, gap) in motif.notes.iter().zip(&motif.gaps) {
                let inter_onset = note.dur + gap;
                assert!(
                    (0.8..=2.6).contains(&(inter_onset / period)),
                    "inter-onset {inter_onset} is not period-derived (period {period})"
                );
            }
            // Pitch spread derives from the drawn bandwidth.
            let f_max = motif
                .notes
                .iter()
                .map(|n| n.f0.max(n.f1))
                .fold(f32::MIN, f32::max);
            let f_min = motif
                .notes
                .iter()
                .map(|n| n.f0.min(n.f1))
                .fold(f32::MAX, f32::min);
            assert!(
                f_max - f_min <= motif.bandwidth_hz * 1.35,
                "span {} vs drawn bandwidth {}",
                f_max - f_min,
                motif.bandwidth_hz
            );
            assert!(
                motif.rate_hz * motif.bandwidth_hz <= PODOS_RATE_BW_LIMIT * 1.0001,
                "the stored pair must satisfy the envelope"
            );
        }
    }

    #[test]
    fn motif_repetitions_differ_under_micro_jitter() {
        let mut songs = recipes_rng("jitter-songs");
        let motif = song_motif(&mut songs);
        let sr = 44_100.0_f32;
        let mut left_stream = recipes_rng("occurrence-a");
        let mut right_stream = recipes_rng("occurrence-b");
        let first = motif_occurrence(&mut left_stream, sr, &motif);
        let second = motif_occurrence(&mut right_stream, sr, &motif);
        assert_eq!(first.len(), second.len(), "same motif, same note count");
        // The acceptance target: repetitions are never verbatim. Compare
        // the longest note's audio; pitch jitter (±3%) walks the phase,
        // so the normalized correlation must sit below 1 within
        // tolerance.
        let pick = first
            .iter()
            .enumerate()
            .max_by_key(|(_, (_, samples))| samples.len())
            .map(|(i, _)| i)
            .unwrap();
        let left_note = &first[pick].1;
        let right_note = &second[pick].1;
        let overlap = left_note.len().min(right_note.len());
        let dot: f32 = left_note[..overlap]
            .iter()
            .zip(&right_note[..overlap])
            .map(|(s, t)| s * t)
            .sum();
        let left_energy: f32 = left_note[..overlap].iter().map(|s| s * s).sum();
        let right_energy: f32 = right_note[..overlap].iter().map(|s| s * s).sum();
        let corr = dot / (left_energy * right_energy).sqrt();
        assert!(
            corr.is_finite() && corr < 0.999_5,
            "repetitions must differ (correlation {corr})"
        );
        // Determinism holds: the same jitter stream renders identical
        // bytes.
        let mut replay = recipes_rng("occurrence-a");
        let again = motif_occurrence(&mut replay, sr, &motif);
        for ((_, left), (_, right)) in first.iter().zip(&again) {
            assert_eq!(left, right);
        }
    }

    #[test]
    fn bird_notes_are_source_filtered_not_bare_sines() {
        // The v1 note was a bare sine: zero energy at any harmonic. The
        // v2 source-filter note drives a sawtooth through formants, so
        // the second harmonic must carry real energy (Goertzel reads a
        // flat note at its fundamental and second harmonic).
        let mut rng = recipes_rng("note-shape");
        let formants = [(1.1, 6.0, 1.0), (2.2, 8.0, 0.7), (3.1, 10.0, 0.4)];
        let note = BirdNote {
            f0: 3_000.0,
            f1: 3_000.0,
            dur: 0.09,
            vib_hz: 0.0,
            vib_depth: 0.0,
        };
        let samples = bird_note(&mut rng, 48_000.0, &note, &formants);
        assert!(samples.len() > 3_000);
        assert!(samples.iter().all(|s| s.is_finite()));
        let fundamental = goertzel_magnitude(&samples, 3_000.0, 48_000.0);
        let harmonic = goertzel_magnitude(&samples, 6_000.0, 48_000.0);
        assert!(
            fundamental > 0.05,
            "the note must ring at its source frequency (read {fundamental})"
        );
        assert!(
            harmonic / fundamental > 0.04,
            "source-filter note must carry harmonic energy (h2/f0 = {}); a bare sine reads 0",
            harmonic / fundamental
        );
    }
}
