//! Requantization dither with optional second-order noise shaping.

use crate::rng::Qrng;

/// TPDF requantizer, optionally with second-order noise shaping.
///
/// Every sample is quantized to `bits` bits (1..=24, clamped) over `[-1, 1]`
/// after adding TPDF dither of +/-1 LSB peak (two uniform draws in `[0, 1)`
/// re-centred), the textbook dither amplitude that decorrelates the
/// quantization error.
///
/// With `noise_shaped` the quantizer input is `x' = x - 2*e1 + e2` (the
/// history is SUBTRACTED; getting this sign backwards turns the shaping into
/// a low-pass that concentrates noise in-band instead of pushing it out) and
/// the fed-back error is `e = q - x'`, which includes the dither. The output
/// noise is then `(dither + roundoff)` filtered by `(1 - z^-1)^2`, a
/// high-pass that suppresses in-band noise by more than 20 dB at 1.5 kHz and
/// about 60 dB at 300 Hz.
///
/// Inputs are expected in `[-1, 1]`; outputs land on the quantization grid
/// within `[-1, 1 + 1 LSB)`.
#[derive(Debug, Clone)]
pub struct Dither {
    levels: f32,
    noise_shaped: bool,
    rng: Qrng,
    e1: f32,
    e2: f32,
}

impl Dither {
    /// Creates a requantizer for `bits` bits (clamped to 1..=24).
    pub fn new(bits: u32, noise_shaped: bool, rng: Qrng) -> Self {
        let bits = bits.clamp(1, 24);
        Self {
            levels: (1u32 << (bits - 1)) as f32,
            noise_shaped,
            rng,
            e1: 0.0,
            e2: 0.0,
        }
    }

    /// Processes one sample.
    pub fn process(&mut self, x: f32) -> f32 {
        let lsb = 1.0 / self.levels;
        let dither = (self.rng.next_f32() + self.rng.next_f32() - 1.0) * lsb;
        let input = if self.noise_shaped {
            x - 2.0 * self.e1 + self.e2
        } else {
            x
        };
        let quantized = ((input + dither) * self.levels).round() / self.levels;
        if self.noise_shaped {
            let error = quantized - input;
            self.e2 = self.e1;
            self.e1 = error;
        }
        quantized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::goertzel_magnitude;

    const SR: f32 = 48_000.0;

    /// The calibrated signal of the spec: 1 kHz at -60 dBFS for 0.5 s.
    fn quiet_tone() -> Vec<f32> {
        let amplitude = 10.0_f32.powf(-60.0 / 20.0);
        (0..(0.5 * SR) as usize)
            .map(|n| amplitude * (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / SR).sin())
            .collect()
    }

    fn render(bits: u32, shaped: bool, seed: u64) -> Vec<f32> {
        let mut dither = Dither::new(bits, shaped, Qrng::new(seed));
        quiet_tone().iter().map(|&x| dither.process(x)).collect()
    }

    /// In-band noise power (300..=1500 Hz, excluding the 1 kHz tone).
    fn in_band_noise(samples: &[f32]) -> f32 {
        [300.0, 500.0, 700.0, 900.0, 1100.0, 1300.0, 1500.0]
            .iter()
            .map(|&freq| goertzel_power_at(samples, freq))
            .sum()
    }

    fn goertzel_power_at(samples: &[f32], freq: f32) -> f32 {
        crate::analysis::goertzel_power(samples, freq, SR)
    }

    #[test]
    fn output_stays_on_the_quantization_grid() {
        let out = render(16, false, 5);
        for &sample in &out {
            let steps = sample * 32_768.0;
            assert!(
                (steps - steps.round()).abs() < 1.0e-6,
                "value {sample} is off the 16-bit grid"
            );
        }
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn shaped_in_band_noise_is_at_least_five_times_lower() {
        // Calibrated physics from the spec: 16-bit, -60 dBFS 1 kHz, 0.5 s;
        // shaped in-band (300..1500 Hz) noise >= 5x lower than flat.
        let flat = render(16, false, 6);
        let shaped = render(16, true, 6);
        assert!(flat.iter().all(|s| s.is_finite()));
        assert!(shaped.iter().all(|s| s.is_finite()));

        let flat_noise = in_band_noise(&flat);
        let shaped_noise = in_band_noise(&shaped);
        assert!(
            shaped_noise * 5.0 <= flat_noise,
            "in-band noise: shaped {shaped_noise} vs flat {flat_noise} (< 5x)"
        );
        // The tone itself must survive both treatments.
        for (label, samples) in [("flat", &flat), ("shaped", &shaped)] {
            let tone = goertzel_magnitude(samples, 1000.0, SR);
            assert!(
                (tone - 0.001).abs() < 0.00015,
                "{label} tone {tone}, expected 0.001"
            );
        }
    }

    #[test]
    fn no_dc_bias() {
        let lsb = 1.0 / 32_768.0;
        for shaped in [false, true] {
            let out = render(16, shaped, 7);
            let mean: f64 = out.iter().map(|&s| f64::from(s)).sum::<f64>() / out.len() as f64;
            assert!(
                mean.abs() < 0.1 * lsb,
                "shaped={shaped} mean {mean} ({:.3} LSB)",
                mean / lsb
            );
        }
    }

    #[test]
    fn flat_dither_on_silence_has_three_levels_and_half_lsb_rms() {
        // Pins the dither amplitude convention: TPDF of +/-1 LSB peak makes
        // the requantizer emit only {-1, 0, +1} LSB with rms 0.5 LSB.
        let mut dither = Dither::new(16, false, Qrng::new(9));
        let out: Vec<f32> = (0..24_000).map(|_| dither.process(0.0)).collect();
        let lsb = 1.0 / 32_768.0;
        let mut hits = [0usize; 3];
        for &sample in &out {
            let steps = (sample / lsb).round() as i32;
            assert!(steps == -1 || steps == 0 || steps == 1, "value {sample}");
            hits[(steps + 1) as usize] += 1;
        }
        assert!(
            hits[1] > hits[0] + hits[2],
            "zero must dominate, got {hits:?}"
        );
        let rms = (out.iter().map(|&s| f64::from(s * s)).sum::<f64>() / out.len() as f64).sqrt();
        assert!(
            (0.4..0.6).contains(&(rms / f64::from(lsb))),
            "rms {rms} ({:.3} LSB)",
            rms / f64::from(lsb)
        );
    }

    #[test]
    fn shaped_low_frequency_noise_is_suppressed() {
        // Direct check of the noise-transfer function on silence: with the
        // history SUBTRACTED the shaped output must sit far below the flat
        // floor at low frequencies. The wrong feedback sign would instead
        // amplify LF noise about 16x above the flat floor.
        let mut flat = Dither::new(16, false, Qrng::new(10));
        let mut shaped = Dither::new(16, true, Qrng::new(10));
        let flat_out: Vec<f32> = (0..24_000).map(|_| flat.process(0.0)).collect();
        let shaped_out: Vec<f32> = (0..24_000).map(|_| shaped.process(0.0)).collect();
        for &freq in &[200.0, 400.0] {
            let flat_noise = goertzel_power_at(&flat_out, freq);
            let shaped_noise = goertzel_power_at(&shaped_out, freq);
            assert!(
                shaped_noise * 20.0 <= flat_noise,
                "{freq} Hz: shaped {shaped_noise} vs flat {flat_noise}"
            );
        }
    }

    #[test]
    fn dither_is_deterministic_per_seed() {
        let a = render(16, true, 42);
        let b = render(16, true, 42);
        let c = render(16, true, 43);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn twentyfour_bits_is_near_transparent() {
        let input = quiet_tone();
        let out = render(24, true, 11);
        let mut worst = 0.0f32;
        for (&x, &y) in input.iter().zip(&out) {
            worst = worst.max((x - y).abs());
        }
        // 24-bit LSB is 1.2e-7; the shaped dither stays within a few LSB.
        assert!(worst > 0.0, "output must not bypass the quantizer");
        assert!(worst < 1.0e-6, "worst deviation {worst}");
    }
}
