//! Noise generators and noise-based sound sources.

use crate::filter::{Biquad, BiquadKind};
use crate::rng::Qrng;

/// Spectral character of a [`NoiseGen`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseKind {
    /// White noise, uniform in `[-1, 1]`.
    White,
    /// Pink noise (-3 dB/octave) via Paul Kellet's economical filter.
    Pink,
}

/// A sample-at-a-time noise generator.
///
/// Pink noise uses Paul Kellet's filter chain (scaled into roughly `[-1, 1]`);
/// the filter is BIBO-stable, so output stays bounded for any input.
#[derive(Debug, Clone)]
pub struct NoiseGen {
    kind: NoiseKind,
    rng: Qrng,
    b0: f32,
    b1: f32,
    b2: f32,
    b3: f32,
    b4: f32,
    b5: f32,
    b6: f32,
}

impl NoiseGen {
    /// Creates a generator of `kind` owning `rng`.
    pub fn new(kind: NoiseKind, rng: Qrng) -> Self {
        Self {
            kind,
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
        match self.kind {
            NoiseKind::White => white,
            NoiseKind::Pink => {
                // Paul Kellet's economical pink filter, scaled to keep the
                // output within about +/-1.
                self.b0 = 0.99886 * self.b0 + white * 0.0555179;
                self.b1 = 0.99332 * self.b1 + white * 0.0750759;
                self.b2 = 0.96900 * self.b2 + white * 0.1538520;
                self.b3 = 0.86650 * self.b3 + white * 0.3104856;
                self.b4 = 0.55000 * self.b4 + white * 0.5329522;
                self.b5 = -0.7616 * self.b5 - white * 0.0168980;
                let sum = self.b0 + self.b1 + self.b2 + self.b3 + self.b4 + self.b5 + self.b6
                    + white * 0.5362;
                self.b6 = white * 0.115926;
                sum * 0.25
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
pub fn noise_burst(
    sample_rate: f32,
    rng: &mut Qrng,
    cutoff_hz: f32,
    decay_secs: f32,
) -> Vec<f32> {
    if decay_secs <= 0.0 || sample_rate <= 0.0 {
        return Vec::new();
    }
    let total = (8.0 * decay_secs * sample_rate).ceil() as usize;
    let tau_samples = decay_secs * sample_rate;
    let mut filter = Biquad::design(BiquadKind::Lowpass, sample_rate, cutoff_hz, 0.7071, 0.0);
    let mut out = Vec::with_capacity(total);
    for n in 0..total {
        let white = rng.next_f32() * 2.0 - 1.0;
        let enveloped = filter.process(white) * (-n as f32 / tau_samples).exp();
        out.push(enveloped);
    }
    peak_normalize_to(&mut out, 0.7);
    out
}

/// Renders noise through a bandpass sweeping from `start_hz` to `end_hz`
/// (linear in Hz), peak-normalized to 0.7.
///
/// The filter is retuned every sample (state preserved) so the sweep is
/// continuous; `q` controls the resonance.
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
    let mut filter = Biquad::design(BiquadKind::Bandpass, sample_rate, start_hz, q, 0.0);
    let mut out = Vec::with_capacity(total);
    for n in 0..total {
        let t = n as f32 / total as f32;
        let f0 = start_hz + (end_hz - start_hz) * t;
        filter = Biquad::design(BiquadKind::Bandpass, sample_rate, f0, q, 0.0);
        let white = rng.next_f32() * 2.0 - 1.0;
        out.push(filter.process(white));
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
        let mean_square = samples.iter().map(|&s| f64::from(s * s)).sum::<f64>()
            / samples.len() as f64;
        let rms = mean_square.sqrt() as f32;
        // Uniform [-1, 1] has RMS 1/sqrt(3) = 0.5774.
        assert!(
            (rms - 0.5774).abs() < 0.02,
            "rms {rms}, expected ~0.5774"
        );
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
        assert!(!burst.is_empty());
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
        assert!(noise_burst(SR, &mut rng, 1000.0, 0.0).is_empty());
    }

    #[test]
    fn swept_noise_is_bounded_and_not_silent() {
        let mut rng = Qrng::new(88);
        let sweep = swept_noise(SR, &mut rng, 200.0, 3000.0, 0.5, 2.0);
        assert_eq!(sweep.len(), (0.5 * SR) as usize);
        assert!(sweep.iter().all(|s| s.is_finite()));
        let peak = sweep.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!((peak - 0.7).abs() < 0.01, "peak {peak}");
        let rms = (sweep.iter().map(|&s| f64::from(s * s)).sum::<f64>()
            / sweep.len() as f64)
            .sqrt();
        assert!(rms > 0.01, "rms {rms}");
        assert!(sweep.iter().fold(0.0f32, |m, &s| m.max(s.abs())) <= 0.71);
    }
}
