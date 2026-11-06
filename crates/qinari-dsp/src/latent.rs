//! One shared slow stochastic process per render: the latent field.
//!
//! Real beds do not breathe on independent, exactly-periodic LFOs: their
//! level, brightness, gust depth and event rates co-vary on one slow
//! stochastic process, and listeners judge exactly that cross-band
//! correlation (`McDermott` & `Simoncelli` — see
//! `docs/RESEARCH-AUDIO-REALISM.md` §R 1.1/2.1). [`LatentField`] is that
//! process: white noise drawn from a [`Qrng`] stream, filtered by chained
//! one-pole lowpasses, and calibrated so the amplitude it *actually
//! realizes over the render window* is exact.
//!
//! Calibration is two-pass by design. The infinite-limit analytic
//! variance of the filter chain does not predict the realized amplitude
//! on 20–60 s renders at low corners (measured ratio 0.63–1.10× across
//! 0.05–0.5 Hz during the design window — `DESIGN-REALISM-ENGINE.md` §7),
//! so the field is normalized by the mean and standard deviation it
//! realizes over the actual buffer before the affine map. The
//! construction stays a pure function of the seed.
//!
//! Corners below ~0.1 Hz are avoided for renders ≤ 60 s (the process has
//! not reached anything like steady state inside the window); longer
//! renders may use them. The optional [`LatentField::tilted`] variant
//! adds octave-spaced one-pole stages that flatten the modulation
//! spectrum toward 1/f.

use crate::rng::Qrng;

/// Control rate of the field in hertz.
///
/// The field is generated and stored at this rate and linearly
/// interpolated for audio-frame reads: a corner of at most a few hertz
/// carries no information at audio resolution, and a 3600 s ambient
/// render must not allocate one `f32` per audio frame.
const CONTROL_RATE: f32 = 200.0;

/// One-pole lowpass for sub-hertz corners at the control rate.
///
/// [`crate::filter::OnePole`] clamps its corner to at least 1 Hz (an
/// audio-rate guard against degenerate designs), which would silently
/// distort a 0.05 Hz field corner at the 200 Hz control rate, so the
/// field implements the recurrence itself.
#[derive(Debug, Clone, Copy)]
struct SlowPole {
    a: f32,
    z: f32,
}

impl SlowPole {
    fn new(corner_hz: f32) -> Self {
        let a = (-2.0 * std::f32::consts::PI * corner_hz / CONTROL_RATE).exp();
        Self { a, z: 0.0 }
    }

    fn process(&mut self, x: f32) -> f32 {
        self.z = self.a * self.z + (1.0 - self.a) * x;
        self.z
    }
}

/// Number of chained one-pole lowpasses in the base field.
const POLE_COUNT: usize = 2;

/// Lowest corner (hertz) a field may use.
const MIN_CORNER_HZ: f32 = 0.01;

/// Highest corner (hertz) a field (or tilt stage) may use.
const MAX_CORNER_HZ: f32 = 25.0;

/// Clamps a requested corner into the designable range.
fn clamp_corner(corner_hz: f32) -> f32 {
    corner_hz.clamp(MIN_CORNER_HZ, MAX_CORNER_HZ)
}

/// A precomputed slow stochastic control signal covering one render.
///
/// The field is an affine map of a normalized lowpass-filtered noise
/// signal: over the render window its mean is exactly `(lo + hi) / 2`
/// and its standard deviation is exactly `(hi - lo) / 2`, so `lo`/`hi`
/// describe the ±1σ span (individual samples may exceed it, as in any
/// Gaussian process). All reads are O(1) and allocation-free.
#[derive(Debug, Clone, PartialEq)]
pub struct LatentField {
    /// Mapped control samples at [`CONTROL_RATE`].
    samples: Vec<f32>,
    /// Audio frames per control sample.
    stride: f32,
    /// Midpoint of the mapped span (mean of the field by construction).
    midpoint: f32,
    /// Render length the field was built for, in audio frames.
    frames: usize,
    /// Next absolute audio frame for sequential [`LatentField::process`]
    /// reads (kept integer so sequential and random access agree exactly).
    cursor: usize,
}

impl LatentField {
    /// Builds the base field: a two-pole lowpassed noise process mapped
    /// so ±1σ lands on `lo`/`hi`.
    ///
    /// `corner_hz` is the lowpass corner of the chain (clamped to
    /// `[0.01, 25]` Hz; the design range is 0.05–0.5). `frames` is the
    /// render length in audio frames at `sample_rate`.
    ///
    /// # Panics
    ///
    /// Panics if `lo >= hi` (an empty span cannot be mapped to).
    pub fn new(
        rng: &mut Qrng,
        sample_rate: f32,
        frames: usize,
        corner_hz: f32,
        lo: f32,
        hi: f32,
    ) -> Self {
        Self::build(rng, sample_rate, frames, corner_hz, 0.0, lo, hi)
    }

    /// Builds the base field plus an octave-bank tilt toward 1/f.
    ///
    /// `tilt` in `[0, 1]` (clamped) mixes in octave-spaced one-pole
    /// stages above `corner_hz` — equal octave weight is the classic 1/f
    /// construction — so the modulation spectrum flattens from the steep
    /// two-pole rolloff toward pink as `tilt` grows. The mixture is
    /// renormalized, so mean and ±1σ span stay exact for every `tilt`.
    ///
    /// # Panics
    ///
    /// Panics if `lo >= hi`.
    pub fn tilted(
        rng: &mut Qrng,
        sample_rate: f32,
        frames: usize,
        corner_hz: f32,
        tilt: f32,
        lo: f32,
        hi: f32,
    ) -> Self {
        Self::build(
            rng,
            sample_rate,
            frames,
            corner_hz,
            tilt.clamp(0.0, 1.0),
            lo,
            hi,
        )
    }

    /// Shared construction: base chain, optional tilt, calibration, map.
    fn build(
        rng: &mut Qrng,
        sample_rate: f32,
        frames: usize,
        corner_hz: f32,
        tilt: f32,
        lo: f32,
        hi: f32,
    ) -> Self {
        assert!(lo < hi, "LatentField requires lo < hi (got {lo} >= {hi})");
        let (center, half_span) = (lo.midpoint(hi), 0.5 * (hi - lo));
        let stride = (sample_rate / CONTROL_RATE).max(1.0);
        let control_len = if frames == 0 {
            0
        } else {
            (((frames as f32) / stride).ceil() as usize).max(1)
        };
        if control_len == 0 {
            return Self {
                samples: Vec::new(),
                stride,
                midpoint: center,
                frames: 0,
                cursor: 0,
            };
        }

        // Pass 1a: draw the base white noise and run it through the
        // chained one-pole lowpasses.
        let corner = clamp_corner(corner_hz);
        let mut poles = [SlowPole::new(corner); POLE_COUNT];
        let mut raw: Vec<f32> = Vec::with_capacity(control_len);
        for _ in 0..control_len {
            let mut value = rng.next_f32() * 2.0 - 1.0;
            for pole in &mut poles {
                value = pole.process(value);
            }
            raw.push(value);
        }

        // Pass 1b: independent unit-variance octave stages for the tilt.
        if tilt > 0.0 {
            let mut stage_corner = corner * 2.0;
            let mut stage_count = 0usize;
            while stage_corner <= MAX_CORNER_HZ {
                stage_count += 1;
                stage_corner *= 2.0;
            }
            // Equal octave weights; 1/sqrt(K) keeps the injected tilt
            // energy comparable for any stage count (the final
            // renormalization removes the absolute scale anyway).
            let weight = tilt / (stage_count as f32).sqrt();
            let mut stage_corner = corner * 2.0;
            for _ in 0..stage_count {
                let mut pole = SlowPole::new(stage_corner);
                let mut stage: Vec<f32> = Vec::with_capacity(control_len);
                let mut sum_sq = 0.0f64;
                for _ in 0..control_len {
                    let white = rng.next_f32() * 2.0 - 1.0;
                    let value = pole.process(white);
                    sum_sq += f64::from(value) * f64::from(value);
                    stage.push(value);
                }
                let variance = sum_sq / control_len as f64;
                let scale = if variance > 1.0e-12 {
                    (1.0 / variance.sqrt()) as f32
                } else {
                    0.0
                };
                for (slot, &value) in raw.iter_mut().zip(&stage) {
                    *slot += value * scale * weight;
                }
                stage_corner *= 2.0;
            }
        }

        // Pass 2: calibrate by the realized mean/std over this buffer,
        // then map onto the span. (The analytic steady-state variance is
        // wrong by up to ~1.6x on short renders — see the module docs.)
        let mean = raw.iter().map(|v| f64::from(*v)).sum::<f64>() / control_len as f64;
        let variance = raw
            .iter()
            .map(|v| {
                let d = f64::from(*v) - mean;
                d * d
            })
            .sum::<f64>()
            / control_len as f64;
        let std = variance.sqrt();
        let scale = if std > 1.0e-12 { 1.0 / std } else { 0.0 };
        for slot in &mut raw {
            *slot = (f64::from(center) + (f64::from(*slot) - mean) * scale * f64::from(half_span))
                as f32;
        }

        Self {
            samples: raw,
            stride,
            midpoint: center,
            frames,
            cursor: 0,
        }
    }

    /// Returns the field value at an absolute audio frame.
    ///
    /// Reads past the end of the render hold the last value (bed tails
    /// may render slightly past the nominal length); an empty field
    /// returns the span midpoint.
    pub fn value_at(&self, frame: usize) -> f32 {
        let last = match self.samples.len() {
            0 => return self.midpoint,
            n => n - 1,
        };
        let t = frame as f32 / self.stride;
        let index = t.floor();
        let i = if index >= last as f32 {
            return self.samples[last];
        } else {
            index as usize
        };
        let frac = t - index;
        let a = self.samples[i];
        let b = self.samples[i + 1];
        a + (b - a) * frac
    }

    /// Sequentially returns the value at the next audio frame, starting
    /// at frame 0.
    pub fn process(&mut self) -> f32 {
        let value = self.value_at(self.cursor);
        self.cursor += 1;
        value
    }

    /// Returns the number of audio frames the field was built for.
    pub fn len(&self) -> usize {
        self.frames
    }

    /// Returns `true` when the field covers no frames.
    pub fn is_empty(&self) -> bool {
        self.frames == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// Mean and std of a slice, in f64.
    fn mean_std(samples: &[f32]) -> (f64, f64) {
        let n = samples.len() as f64;
        let mean = samples.iter().map(|v| f64::from(*v)).sum::<f64>() / n;
        let variance = samples
            .iter()
            .map(|v| {
                let d = f64::from(*v) - mean;
                d * d
            })
            .sum::<f64>()
            / n;
        (mean, variance.sqrt())
    }

    /// Normalized roughness: variance of the first difference over the
    /// variance — a scale-free proxy for fast modulation content.
    fn roughness(samples: &[f32]) -> f64 {
        if samples.len() < 2 {
            return 0.0;
        }
        let diffs: Vec<f64> = samples.windows(2).map(|w| f64::from(w[1] - w[0])).collect();
        let (_, var) = mean_std_by(&diffs);
        let (_, signal_var) = mean_std(samples);
        if signal_var <= 0.0 {
            0.0
        } else {
            var / signal_var
        }
    }

    fn mean_std_by(values: &[f64]) -> (f64, f64) {
        let n = values.len() as f64;
        let mean = values.iter().sum::<f64>() / n;
        let variance = values
            .iter()
            .map(|v| {
                let d = v - mean;
                d * d
            })
            .sum::<f64>()
            / n;
        (mean, variance.sqrt())
    }

    #[test]
    fn calibration_lands_mean_and_one_sigma_exactly() {
        // 120 s render, corner inside the design range: the realized
        // mean must be the span midpoint and the realized std half the
        // span — the two-pass guarantee the analytic variance cannot
        // give (design §7: ratio 0.63-1.10 without it).
        let mut rng = Qrng::new(31);
        let field = LatentField::new(&mut rng, SR, (120.0 * SR) as usize, 0.3, -1.0, 1.0);
        let (mean, std) = mean_std(&field.samples);
        assert!((mean - 0.0).abs() < 0.01, "mean {mean}");
        assert!((std - 1.0).abs() < 0.01, "std {std}");

        // A different span maps identically in scaled units.
        let mut rng = Qrng::new(31);
        let field = LatentField::new(&mut rng, SR, (120.0 * SR) as usize, 0.3, 0.2, 1.0);
        let (mean, std) = mean_std(&field.samples);
        assert!((mean - 0.6).abs() < 0.01, "mean {mean}");
        assert!((std - 0.4).abs() < 0.01, "std {std}");
    }

    #[test]
    fn same_seed_same_field_and_other_seed_differs() {
        let frames = (60.0 * SR) as usize;
        let a = LatentField::new(&mut Qrng::new(7), SR, frames, 0.2, -1.0, 1.0);
        let b = LatentField::new(&mut Qrng::new(7), SR, frames, 0.2, -1.0, 1.0);
        assert_eq!(a.samples, b.samples, "same seed must be bit-identical");
        let c = LatentField::new(&mut Qrng::new(8), SR, frames, 0.2, -1.0, 1.0);
        assert_ne!(a.samples, c.samples);
    }

    #[test]
    fn higher_corner_carries_more_fast_modulation() {
        let frames = (120.0 * SR) as usize;
        let slow = LatentField::new(&mut Qrng::new(3), SR, frames, 0.05, -1.0, 1.0);
        let fast = LatentField::new(&mut Qrng::new(3), SR, frames, 0.5, -1.0, 1.0);
        assert!(
            roughness(&fast.samples) > 4.0 * roughness(&slow.samples),
            "roughness fast {} vs slow {}",
            roughness(&fast.samples),
            roughness(&slow.samples)
        );
    }

    #[test]
    fn tilt_flattens_the_field_toward_one_over_f() {
        let frames = (120.0 * SR) as usize;
        let plain = LatentField::new(&mut Qrng::new(5), SR, frames, 0.2, -1.0, 1.0);
        let tilted = LatentField::tilted(&mut Qrng::new(5), SR, frames, 0.2, 1.0, -1.0, 1.0);
        assert!(
            roughness(&tilted.samples) > 3.0 * roughness(&plain.samples),
            "tilt roughness {} vs plain {}",
            roughness(&tilted.samples),
            roughness(&plain.samples)
        );
        // Tilt keeps the calibration exact.
        let (mean, std) = mean_std(&tilted.samples);
        assert!((mean - 0.0).abs() < 0.01, "mean {mean}");
        assert!((std - 1.0).abs() < 0.01, "std {std}");
        // Tilt 0 degenerates to the plain field for the same seed.
        let untilted = LatentField::tilted(&mut Qrng::new(5), SR, frames, 0.2, 0.0, -1.0, 1.0);
        assert_eq!(untilted.samples, plain.samples);
    }

    #[test]
    fn sequential_and_random_access_agree_exactly() {
        let mut field =
            LatentField::new(&mut Qrng::new(11), SR, (2.0 * SR) as usize, 0.4, 0.0, 2.0);
        for frame in 0..((2.0 * SR) as usize) {
            assert_eq!(
                field.process().to_bits(),
                field.value_at(frame).to_bits(),
                "frame {frame}"
            );
        }
    }

    #[test]
    fn reads_hold_the_last_value_past_the_end() {
        let mut field = LatentField::new(&mut Qrng::new(13), SR, SR as usize, 0.3, -1.0, 1.0);
        let total = 3 * SR as usize;
        for _ in 0..total {
            field.process();
        }
        let held = field.process();
        let last_control = field.samples[field.samples.len() - 1];
        // Past the end the interpolation clamps to the final control
        // sample; allow the small interpolation slope at the boundary.
        assert!((held - last_control).abs() < 0.5);
    }

    #[test]
    fn values_stay_within_gaussian_bounds() {
        let mut rng = Qrng::new(17);
        let field = LatentField::new(&mut rng, SR, (240.0 * SR) as usize, 0.2, -1.0, 1.0);
        // Unit-variance Gaussian-like process: |x| > 6 sigma has
        // probability ~2e-9 over 48k control samples.
        assert!(field.samples.iter().all(|v| v.abs() < 6.0 && v.is_finite()));
    }

    #[test]
    fn zero_frame_field_is_empty_and_returns_midpoint() {
        let field = LatentField::new(&mut Qrng::new(19), SR, 0, 0.3, -1.0, 1.0);
        assert!(field.is_empty());
        assert_eq!(field.len(), 0);
        assert_eq!(field.value_at(1000), 0.0);
        let field = LatentField::new(&mut Qrng::new(19), SR, 0, 0.3, 1.0, 3.0);
        assert_eq!(field.value_at(0), 2.0);
    }

    #[test]
    #[should_panic(expected = "LatentField requires lo < hi")]
    fn empty_span_panics() {
        LatentField::new(&mut Qrng::new(23), SR, SR as usize, 0.3, 1.0, 1.0);
    }
}
