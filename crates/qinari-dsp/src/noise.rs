//! Noise generators and noise-based sound sources.

use crate::filter::{Biquad, BiquadKind, Svf};
use crate::rng::Qrng;

/// Spectral character of a [`NoiseGen`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseKind {
    /// White noise, uniform in `[-1, 1]`.
    White,
    /// Pink noise (-3 dB/octave) via Paul Kellet's economical filter.
    Pink,
}

/// Internal flavor of a [`NoiseGen`] (the fractal and Gaussian palettes
/// take parameters, so they get their own constructors).
#[derive(Debug, Clone)]
enum Flavor {
    White,
    Pink,
    /// 1/f^alpha between white and brown: an octave-spaced bank of
    /// one-pole lowpasses with geometrically falling weights.
    Fractal {
        /// One-pole per octave, low corner first.
        poles: Vec<crate::filter::OnePole>,
        /// Per-stage gain (weight times analytic variance normalization).
        scales: Vec<f32>,
        /// Normalization of the whole bank to the target RMS.
        output_scale: f32,
    },
    /// Unit-variance Gaussian white noise (Box-Muller).
    Gaussian {
        spare: Option<f32>,
    },
}

/// A sample-at-a-time noise generator.
///
/// Pink noise uses Paul Kellet's filter chain (scaled into roughly `[-1, 1]`);
/// the filter is BIBO-stable, so output stays bounded for any input.
/// [`NoiseGen::fractal`] adds the 1/f^α palette between white and brown
/// and [`NoiseGen::gaussian`] the Gaussian marginal (both approximately
/// 0.4 RMS; individual samples may exceed ±1, as real Gaussian noise
/// does).
#[derive(Debug, Clone)]
pub struct NoiseGen {
    flavor: Flavor,
    rng: Qrng,
    b0: f32,
    b1: f32,
    b2: f32,
    b3: f32,
    b4: f32,
    b5: f32,
    b6: f32,
}

/// Bottom corner of the fractal noise bank in hertz.
const FRACTAL_BOTTOM_HZ: f32 = 20.0;
/// Output RMS target of the fractal and Gaussian palettes.
const SOFT_RMS: f32 = 0.4;

/// Analytic input-to-output variance gain of a one-pole lowpass with
/// per-sample coefficient `a` (white input).
fn one_pole_variance_gain(a: f32) -> f32 {
    (1.0 - a) / (1.0 + a)
}

impl NoiseGen {
    /// Creates a generator of `kind` owning `rng`.
    pub fn new(kind: NoiseKind, rng: Qrng) -> Self {
        let flavor = match kind {
            NoiseKind::White => Flavor::White,
            NoiseKind::Pink => Flavor::Pink,
        };
        Self {
            flavor,
            rng,
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
            b3: 0.0,
            b4: 0.0,
            b5: 0.0,
            b6: 0.0,
        }
    }

    /// Creates a 1/f^α noise generator (α in `[0, 2]`, clamped).
    ///
    /// The filtered construction runs one independent one-pole lowpass
    /// per octave from [`FRACTAL_BOTTOM_HZ`] toward Nyquist, each driven
    /// by its own white stream and weighted by `2^(-αk/2)` so the
    /// realized power spectrum falls as `1/f^α` across the bank
    /// (α = 1 is pink, α = 2 brown; α = 0 degenerates to white).
    /// Output RMS is approximately [`SOFT_RMS`].
    pub fn fractal(alpha: f32, rng: Qrng) -> Self {
        const DESIGN_RATE: f32 = 44_100.0;
        const TOP_HZ: f32 = 11_000.0;
        let alpha = alpha.clamp(0.0, 2.0);
        if alpha <= 0.02 {
            return Self::new(NoiseKind::White, rng);
        }
        // The bank cannot know the caller's sample rate, so the stages
        // are designed at a 44.1 kHz reference and the list is cut once a
        // corner would pass a quarter of that rate. At other rates the
        // effective corners shift proportionally (a one-pole keeps its
        // per-sample coefficient), which only rescales the octaves — the
        // per-octave slope is untouched.
        let top_hz = TOP_HZ;
        let mut poles = Vec::new();
        let mut scales = Vec::new();
        let mut corner = FRACTAL_BOTTOM_HZ;
        let mut total_variance = 0.0f32;
        while corner <= top_hz {
            let a = (-2.0 * std::f32::consts::PI * corner / DESIGN_RATE).exp();
            let gain = one_pole_variance_gain(a);
            // Stage weight only: the per-stage spectra must keep their
            // natural slope (normalizing each stage to unit variance
            // would boost the low stages' 1/f^2 tails and swamp the
            // bank). The total output variance is compensated once,
            // below, with the analytic per-stage gains.
            let weight = 2.0_f32.powf(-alpha * poles.len() as f32 / 2.0);
            // Uniform [-1, 1) draws carry variance 1/3.
            total_variance += weight * weight * gain / 3.0;
            scales.push(weight);
            poles.push(crate::filter::OnePole::new(DESIGN_RATE, corner));
            corner *= 2.0;
        }
        let output_scale = SOFT_RMS / total_variance.sqrt();
        Self {
            flavor: Flavor::Fractal {
                poles,
                scales,
                output_scale,
            },
            rng,
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
            b3: 0.0,
            b4: 0.0,
            b5: 0.0,
            b6: 0.0,
        }
    }

    /// Creates a unit-variance Gaussian white-noise generator.
    ///
    /// Pairs of uniforms become Box-Muller deviates; the second of each
    /// pair is cached, so the stream consumes two uniforms every other
    /// sample. Output RMS is approximately [`SOFT_RMS`] (the raw
    /// deviates are scaled to sit at the level of the other palettes).
    pub fn gaussian(rng: Qrng) -> Self {
        Self {
            flavor: Flavor::Gaussian { spare: None },
            rng,
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
            b3: 0.0,
            b4: 0.0,
            b5: 0.0,
            b6: 0.0,
        }
    }

    /// Returns the next noise sample.
    pub fn process(&mut self) -> f32 {
        let white = self.rng.next_f32() * 2.0 - 1.0;
        match &mut self.flavor {
            Flavor::White => white,
            Flavor::Pink => {
                // Economical pink filter from Paul Kellet, scaled to keep the
                // output within about +/-1.
                self.b0 = 0.99886 * self.b0 + white * 0.055_517_9;
                self.b1 = 0.99332 * self.b1 + white * 0.075_075_9;
                self.b2 = 0.96900 * self.b2 + white * 0.153_852;
                self.b3 = 0.86650 * self.b3 + white * 0.310_485_6;
                self.b4 = 0.55000 * self.b4 + white * 0.532_952_2;
                self.b5 = -0.7616 * self.b5 - white * 0.016_898;
                let sum = self.b0
                    + self.b1
                    + self.b2
                    + self.b3
                    + self.b4
                    + self.b5
                    + self.b6
                    + white * 0.5362;
                self.b6 = white * 0.115_926;
                sum * 0.25
            }
            Flavor::Fractal {
                poles,
                scales,
                output_scale,
            } => {
                let mut sum = 0.0f32;
                for (pole, &scale) in poles.iter_mut().zip(scales.iter()) {
                    // Independent stream per octave: the weights only make
                    // the power spectrum add up when the stages are
                    // uncorrelated.
                    let draw = self.rng.next_f32() * 2.0 - 1.0;
                    sum += pole.process(draw) * scale;
                }
                sum * *output_scale
            }
            Flavor::Gaussian { spare } => {
                if let Some(cached) = *spare {
                    *spare = None;
                    cached * SOFT_RMS
                } else {
                    // Box-Muller on two uniforms in (0, 1].
                    let u1 = 1.0 - self.rng.next_f32();
                    let u2 = self.rng.next_f32();
                    let radius = (-2.0 * u1.ln()).sqrt();
                    let angle = 2.0 * std::f32::consts::PI * u2;
                    let (sin, cos) = angle.sin_cos();
                    *spare = Some(radius * sin);
                    radius * cos * SOFT_RMS
                }
            }
        }
    }
}

/// Scales `samples` so the largest absolute sample equals `target`.
///
/// Near-silent input is left untouched to avoid amplifying pure rounding
/// noise.
fn peak_normalize_to(samples: &mut [f32], target: f32) {
    let peak = samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    if peak > 1.0e-9 {
        let gain = target / peak;
        for sample in samples.iter_mut() {
            *sample *= gain;
        }
    }
}

/// Renders a lowpassed noise burst with exponential decay, peak-normalized
/// to 0.7.
///
/// White noise is shaped by a 2nd-order lowpass at `cutoff_hz` (Q ~0.707)
/// and an exponential envelope whose e-folding time constant is
/// `decay_secs`; the render covers about `8 * decay_secs` (to roughly
/// -70 dB) and is then normalized so its peak is exactly 0.7. Recipes rely
/// on that normalization for predictable levels.
///
/// Returns an empty vector if `decay_secs` is not positive.
pub fn noise_burst(sample_rate: f32, rng: &mut Qrng, cutoff_hz: f32, decay_secs: f32) -> Vec<f32> {
    if decay_secs <= 0.0 || sample_rate <= 0.0 {
        return Vec::new();
    }
    let total = (8.0 * decay_secs * sample_rate).ceil() as usize;
    let tau_samples = decay_secs * sample_rate;
    let mut filter = Biquad::design(
        BiquadKind::Lowpass,
        sample_rate,
        cutoff_hz,
        std::f32::consts::FRAC_1_SQRT_2,
        0.0,
    );
    let mut out = Vec::with_capacity(total);
    for n in 0..total {
        let white = rng.next_f32() * 2.0 - 1.0;
        let enveloped = filter.process(white) * (-(n as f32) / tau_samples).exp();
        out.push(enveloped);
    }
    peak_normalize_to(&mut out, 0.7);
    out
}

/// Renders a lowpassed Gaussian noise burst with a linear attack ramp
/// and exponential decay, peak-normalized to 0.7.
///
/// Like [`noise_burst`], but the body is Gaussian (a more natural
/// marginal than uniform) and the envelope rises linearly over
/// `attack_secs` (the 0.2–5 ms transient range) before the exponential
/// decay takes over — zero-attack onsets are the classic sample-pack
/// tell (`RESEARCH-AUDIO-REALISM.md` §R 2.2).
///
/// Returns an empty vector if `decay_secs` is not positive.
pub fn noise_burst_attacked(
    sample_rate: f32,
    rng: &mut Qrng,
    cutoff_hz: f32,
    decay_secs: f32,
    attack_secs: f32,
) -> Vec<f32> {
    if decay_secs <= 0.0 || sample_rate <= 0.0 {
        return Vec::new();
    }
    let total = (8.0 * decay_secs * sample_rate).ceil() as usize;
    let tau_samples = decay_secs * sample_rate;
    let attack_samples = (attack_secs.max(0.0) * sample_rate).round() as usize;
    let mut filter = Biquad::design(
        BiquadKind::Lowpass,
        sample_rate,
        cutoff_hz,
        std::f32::consts::FRAC_1_SQRT_2,
        0.0,
    );
    let mut gaussian = NoiseGen::gaussian(rng.fork());
    let mut out = Vec::with_capacity(total);
    for n in 0..total {
        let body = filter.process(gaussian.process());
        let enveloped = if n < attack_samples {
            body * (n as f32 / attack_samples.max(1) as f32)
        } else {
            let decayed = (n - attack_samples) as f32 / tau_samples;
            body * (-decayed).exp()
        };
        out.push(enveloped);
    }
    peak_normalize_to(&mut out, 0.7);
    out
}

/// Renders `frames` of Gaussian noise band-limited to `[lo_hz, hi_hz]`,
/// peak-normalized to 0.7.
///
/// The Gaussian marginal survives the constant peak normalization; the
/// band is a highpass/lowpass biquad pair at 0.707 Q.
pub fn band_limited_gaussian(
    sample_rate: f32,
    rng: &mut Qrng,
    lo_hz: f32,
    hi_hz: f32,
    frames: usize,
) -> Vec<f32> {
    if sample_rate <= 0.0 || frames == 0 {
        return Vec::new();
    }
    let mut gaussian = NoiseGen::gaussian(rng.fork());
    let mut highpass = Biquad::design(
        BiquadKind::Highpass,
        sample_rate,
        lo_hz,
        std::f32::consts::FRAC_1_SQRT_2,
        0.0,
    );
    let mut lowpass = Biquad::design(
        BiquadKind::Lowpass,
        sample_rate,
        hi_hz.max(lo_hz + 1.0),
        std::f32::consts::FRAC_1_SQRT_2,
        0.0,
    );
    let mut out = Vec::with_capacity(frames);
    for _ in 0..frames {
        out.push(lowpass.process(highpass.process(gaussian.process())));
    }
    peak_normalize_to(&mut out, 0.7);
    out
}

/// Renders noise through a bandpass sweeping from `start_hz` to `end_hz`
/// (linear in Hz), peak-normalized to 0.7.
///
/// The filter is a TPT state-variable filter whose centre is retuned every
/// sample while its state is preserved (see [`Svf::set`]), so the sweep is
/// continuous and the resonance tracks the moving centre; `q` controls the
/// resonance.
pub fn swept_noise(
    sample_rate: f32,
    rng: &mut Qrng,
    start_hz: f32,
    end_hz: f32,
    secs: f32,
    q: f32,
) -> Vec<f32> {
    if secs <= 0.0 || sample_rate <= 0.0 {
        return Vec::new();
    }
    let total = (secs * sample_rate).round() as usize;
    let mut filter = Svf::new(sample_rate, start_hz, q);
    let mut out = Vec::with_capacity(total);
    for n in 0..total {
        let t = n as f32 / total as f32;
        let f0 = start_hz + (end_hz - start_hz) * t;
        filter.set(sample_rate, f0, q);
        let white = rng.next_f32() * 2.0 - 1.0;
        out.push(filter.process(white).band);
    }
    peak_normalize_to(&mut out, 0.7);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::goertzel_power;

    const SR: f32 = 48_000.0;

    #[test]
    fn white_noise_rms_matches_uniform_distribution() {
        let mut gen = NoiseGen::new(NoiseKind::White, Qrng::new(21));
        let samples: Vec<f32> = (0..48_000).map(|_| gen.process()).collect();
        assert!(samples.iter().all(|s| s.is_finite()));
        let peak = samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(peak <= 1.0);
        let mean_square =
            samples.iter().map(|&s| f64::from(s * s)).sum::<f64>() / samples.len() as f64;
        let rms = mean_square.sqrt() as f32;
        // Uniform [-1, 1] has RMS 1/sqrt(3) = 0.5774.
        assert!((rms - 0.5774).abs() < 0.02, "rms {rms}, expected ~0.5774");
    }

    #[test]
    fn pink_noise_has_more_low_than_high_energy() {
        let segments = 16;
        let seg_len = 4_800;
        let mut gen = NoiseGen::new(NoiseKind::Pink, Qrng::new(33));
        let mut lf = 0.0f64;
        let mut hf = 0.0f64;
        for _ in 0..segments {
            let block: Vec<f32> = (0..seg_len).map(|_| gen.process()).collect();
            lf += f64::from(goertzel_power(&block, 100.0, SR));
            hf += f64::from(goertzel_power(&block, 4000.0, SR));
        }
        assert!(
            lf > 8.0 * hf,
            "pink LF power {lf} vs HF power {hf} (expected ~40x)"
        );

        // White noise must be flat by comparison.
        let mut gen = NoiseGen::new(NoiseKind::White, Qrng::new(34));
        let mut lf = 0.0f64;
        let mut hf = 0.0f64;
        for _ in 0..segments {
            let block: Vec<f32> = (0..seg_len).map(|_| gen.process()).collect();
            lf += f64::from(goertzel_power(&block, 100.0, SR));
            hf += f64::from(goertzel_power(&block, 4000.0, SR));
        }
        assert!(
            (lf / hf) < 3.0 && (lf / hf) > 0.33,
            "white LF/HF ratio {}",
            lf / hf
        );
    }

    #[test]
    fn noise_burst_peaks_at_target_and_decays() {
        let mut rng = Qrng::new(77);
        let burst = noise_burst(SR, &mut rng, 2000.0, 0.25);
        // 8 * decay_secs * sr, per the render-length contract in the docs.
        assert_eq!(burst.len(), 96_000);
        assert!(burst.iter().all(|s| s.is_finite()));
        let peak = burst.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!((peak - 0.7).abs() < 0.01, "peak {peak}");

        let n = burst.len();
        let head = &burst[..n / 10];
        let tail = &burst[n - n / 10..];
        let rms = |slice: &[f32]| {
            (slice.iter().map(|&s| f64::from(s * s)).sum::<f64>() / slice.len() as f64).sqrt()
        };
        assert!(
            rms(head) > 4.0 * rms(tail),
            "head rms {} vs tail rms {}",
            rms(head),
            rms(tail)
        );

        // Degenerate parameter returns empty output.
        assert_eq!(noise_burst(SR, &mut rng, 1000.0, 0.0), [] as [f32; 0]);
    }

    #[test]
    fn swept_noise_follows_the_sweep_centers() {
        // The resonance must track the moving centre: energy measured at the
        // end of the sweep must sit near the end frequency, not near the start.
        let mut rng = Qrng::new(88);
        let sweep = swept_noise(SR, &mut rng, 200.0, 3000.0, 1.0, 4.0);
        let n = sweep.len();
        let last_quarter = &sweep[3 * n / 4..];
        let first_quarter = &sweep[..n / 4];
        let power_at = |slice: &[f32], freq: f32| goertzel_power(slice, freq, SR);
        // Near the end (2250-3000 Hz centre) the tail holds much more energy
        // at 2600 Hz than the head does, and vice versa at 350 Hz.
        let tail_high = power_at(last_quarter, 2600.0);
        let head_high = power_at(first_quarter, 2600.0);
        assert!(
            tail_high > 4.0 * head_high,
            "2600 Hz: tail {tail_high} vs head {head_high}"
        );
        let tail_low = power_at(last_quarter, 350.0);
        let head_low = power_at(first_quarter, 350.0);
        assert!(
            head_low > 4.0 * tail_low,
            "350 Hz: head {head_low} vs tail {tail_low}"
        );
    }

    #[test]
    fn swept_noise_is_bounded_and_not_silent() {
        let mut rng = Qrng::new(88);
        let sweep = swept_noise(SR, &mut rng, 200.0, 3000.0, 0.5, 2.0);
        assert_eq!(sweep.len(), (0.5 * SR) as usize);
        assert!(sweep.iter().all(|s| s.is_finite()));
        let peak = sweep.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!((peak - 0.7).abs() < 0.01, "peak {peak}");
        let rms =
            (sweep.iter().map(|&s| f64::from(s * s)).sum::<f64>() / sweep.len() as f64).sqrt();
        assert!(rms > 0.01, "rms {rms}");
        assert!(sweep.iter().fold(0.0f32, |m, &s| m.max(s.abs())) <= 0.71);
    }

    /// Octave-band power of a generator over `secs` seconds.
    fn band_power(alpha: f32, freq: f32, secs: f32, seed: u64) -> f64 {
        let mut gen = NoiseGen::fractal(alpha, Qrng::new(seed));
        let blocks = 16;
        let block_len = (secs * SR / blocks as f32) as usize;
        let mut total = 0.0f64;
        for _ in 0..blocks {
            let block: Vec<f32> = (0..block_len).map(|_| gen.process()).collect();
            total += f64::from(goertzel_power(&block, freq, SR));
        }
        total
    }

    #[test]
    fn fractal_noise_falls_one_over_f_alpha() {
        // 100 Hz vs 1600 Hz is four octaves: power should fall by about
        // 2^(4*alpha). Tolerances are wide (one-pole banks ripple), but
        // the three alphas must separate clearly.
        let seed = 51;
        for alpha in [0.5f32, 1.0, 1.7] {
            let low = band_power(alpha, 100.0, 2.0, seed);
            let high = band_power(alpha, 1600.0, 2.0, seed);
            let ratio = low / high;
            let expected = 2.0_f64.powf(4.0 * f64::from(alpha));
            assert!(
                (ratio / expected > 0.35) && (ratio / expected < 2.8),
                "alpha {alpha}: LF/HF {ratio}, expected ~{expected}"
            );
        }
        // alpha = 2 (brown) must be far steeper than alpha = 0.5.
        let brown_ratio = band_power(2.0, 100.0, 2.0, 51) / band_power(2.0, 1600.0, 2.0, 51);
        let soft_ratio = band_power(0.5, 100.0, 2.0, 51) / band_power(0.5, 1600.0, 2.0, 51);
        assert!(
            brown_ratio > 8.0 * soft_ratio,
            "brown {brown_ratio} vs soft {soft_ratio}"
        );
        // alpha = 0 is white: the ratio must be near flat.
        let white_ratio = band_power(0.0, 100.0, 2.0, 51) / band_power(0.0, 1600.0, 2.0, 51);
        assert!(
            (0.33..=3.0).contains(&white_ratio),
            "white ratio {white_ratio}"
        );
    }

    #[test]
    fn fractal_noise_is_finite_at_sane_rms() {
        let mut gen = NoiseGen::fractal(1.3, Qrng::new(52));
        let samples: Vec<f32> = (0..96_000).map(|_| gen.process()).collect();
        assert!(samples.iter().all(|s| s.is_finite()));
        let rms =
            (samples.iter().map(|&s| f64::from(s * s)).sum::<f64>() / samples.len() as f64).sqrt();
        assert!(
            (0.25..=0.6).contains(&rms),
            "fractal rms {rms}, expected ~0.4"
        );
    }

    #[test]
    fn gaussian_noise_has_gaussian_moments() {
        let mut gen = NoiseGen::gaussian(Qrng::new(53));
        let n = 96_000;
        let samples: Vec<f32> = (0..n).map(|_| gen.process()).collect();
        assert!(samples.iter().all(|s| s.is_finite()));
        let mean = samples.iter().map(|s| f64::from(*s)).sum::<f64>() / f64::from(n);
        let var = samples
            .iter()
            .map(|s| {
                let d = f64::from(*s) - mean;
                d * d
            })
            .sum::<f64>()
            / f64::from(n);
        let std = var.sqrt();
        // Scaled to SOFT_RMS = 0.4.
        assert!(mean.abs() < 0.02, "mean {mean}");
        assert!((std - 0.4).abs() < 0.03, "std {std}");
        // Gaussian crest factor: max/sigma typically 3.5-5 for 96k
        // samples; uniform noise would sit near 1.7.
        let peak = samples
            .iter()
            .fold(0.0f64, |m, s| m.max(f64::from(*s).abs()));
        let crest = peak / std;
        assert!(crest > 3.0, "crest {crest} (uniform-like?)");
        assert!(crest < 6.5, "crest {crest} (outliers too large?)");
    }

    #[test]
    fn attacked_burst_ramps_in_and_decays() {
        let mut rng = Qrng::new(77);
        let burst = noise_burst_attacked(SR, &mut rng, 2000.0, 0.25, 0.002);
        assert_eq!(burst.len(), 96_000, "same length contract as noise_burst");
        assert!(burst.iter().all(|s| s.is_finite()));
        let peak = burst.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!((peak - 0.7).abs() < 0.01, "peak {peak}");
        // The attack: the first sample is exactly zero (envelope factor 0)
        // and the first quarter of the attack window carries far less
        // energy than the post-attack body.
        let attack_n = (0.002 * SR) as usize;
        assert_eq!(burst[0], 0.0, "attack starts from silence");
        let rms = |slice: &[f32]| {
            (slice.iter().map(|&s| f64::from(s * s)).sum::<f64>() / slice.len() as f64).sqrt()
        };
        assert!(
            rms(&burst[..attack_n / 4]) < 0.6 * rms(&burst[attack_n..4 * attack_n]),
            "attack does not ramp"
        );
        // Degenerate parameters return empty output.
        assert_eq!(
            noise_burst_attacked(SR, &mut rng, 1000.0, 0.0, 0.001),
            [] as [f32; 0]
        );
    }

    #[test]
    fn band_limited_gaussian_stays_in_band() {
        let mut rng = Qrng::new(78);
        let band = band_limited_gaussian(SR, &mut rng, 400.0, 2000.0, SR as usize);
        assert_eq!(band.len(), SR as usize);
        assert!(band.iter().all(|s| s.is_finite()));
        let peak = band.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!((peak - 0.7).abs() < 0.01, "peak {peak}");
        let in_band = goertzel_power(&band[..SR as usize / 2], 1000.0, SR);
        let below = goertzel_power(&band[..SR as usize / 2], 100.0, SR);
        let above = goertzel_power(&band[..SR as usize / 2], 5000.0, SR);
        assert!(in_band > 20.0 * below, "in {in_band} vs below {below}");
        assert!(in_band > 20.0 * above, "in {in_band} vs above {above}");
    }
}
