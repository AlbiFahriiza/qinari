//! Spectral and statistical analysis of sample slices.

/// Returns the Goertzel power of `samples` at `freq`.
///
/// The power is the mean square of the signal component matched to `freq`:
/// a pure sine of amplitude `a` reads `a^2 / 2`, independent of phase and
/// of the number of samples. Internally the recurrence runs in `f64` to
/// keep cancellation clean; the result is returned as `f32`.
pub fn goertzel_power(samples: &[f32], freq: f32, sample_rate: f32) -> f32 {
    let n = samples.len();
    if n == 0 {
        return 0.0;
    }
    let omega = 2.0 * std::f64::consts::PI * f64::from(freq) / f64::from(sample_rate);
    let coeff = 2.0 * omega.cos();
    let mut s1 = 0.0f64;
    let mut s2 = 0.0f64;
    for &sample in samples {
        let s0 = f64::from(sample) + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    let normalized = 2.0 * power / (n as f64 * n as f64);
    normalized as f32
}

/// Returns the Goertzel magnitude of `samples` at `freq`.
///
/// This is the amplitude of the sinusoidal component at `freq`: a pure
/// sine of amplitude `a` reads `a`, independent of phase and length.
pub fn goertzel_magnitude(samples: &[f32], freq: f32, sample_rate: f32) -> f32 {
    goertzel_power(samples, freq, sample_rate).sqrt()
}

/// Returns the zero-crossing rate: sign changes between consecutive
/// samples divided by the number of intervals.
///
/// A sine of frequency `f` has a zero-crossing rate of approximately
/// `2 * f / sample_rate`. Returns `0.0` for fewer than two samples.
pub fn zcr(samples: &[f32]) -> f32 {
    if samples.len() < 2 {
        return 0.0;
    }
    let mut crossings = 0usize;
    for pair in samples.windows(2) {
        let a = pair[0] >= 0.0;
        let b = pair[1] >= 0.0;
        if a != b {
            crossings += 1;
        }
    }
    crossings as f32 / (samples.len() - 1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(freq: f32, amp: f32, frames: usize, phase: f32) -> Vec<f32> {
        (0..frames)
            .map(|n| {
                amp * (2.0 * std::f32::consts::PI * (freq * n as f32 / SR + phase)).sin()
            })
            .collect()
    }

    #[test]
    fn goertzel_reads_amplitude_and_is_phase_invariant() {
        let n = 48_000;
        for phase in [0.0, 0.13, 0.5, 0.9] {
            let samples = sine(1000.0, 0.5, n, phase);
            let magnitude = goertzel_magnitude(&samples, 1000.0, SR);
            assert!(
                (magnitude - 0.5).abs() < 0.005,
                "phase {phase}: magnitude {magnitude}"
            );
            let power = goertzel_power(&samples, 1000.0, SR);
            assert!(
                (power - 0.125).abs() < 0.002,
                "phase {phase}: power {power}"
            );
        }
    }

    #[test]
    fn goertzel_detects_tone_far_above_neighbor_bin() {
        // 1000 Hz over exactly 1 s is on-bin at 48 kHz (bin spacing 1 Hz).
        let samples = sine(1000.0, 1.0, 48_000, 0.0);
        let at_tone = goertzel_power(&samples, 1000.0, SR);
        let at_neighbor = goertzel_power(&samples, 990.0, SR);
        assert!(
            at_tone > 100.0 * at_neighbor,
            "tone power {at_tone}, neighbor power {at_neighbor}"
        );
    }

    #[test]
    fn zcr_separates_low_and_high_tones_and_tracks_frequency() {
        let low = sine(200.0, 0.8, 48_000, 0.0);
        let high = sine(2000.0, 0.8, 48_000, 0.0);
        let zcr_low = zcr(&low);
        let zcr_high = zcr(&high);
        assert!(zcr_high > 5.0 * zcr_low);
        // Expected: 2 * f / sr.
        assert!((zcr_low - 2.0 * 200.0 / SR).abs() < 1e-3, "zcr_low {zcr_low}");
        assert!((zcr_high - 2.0 * 2000.0 / SR).abs() < 1e-3, "zcr_high {zcr_high}");
        assert_eq!(zcr(&[0.5]), 0.0);
        assert_eq!(zcr(&[]), 0.0);
        assert_eq!(zcr(&[0.5, 0.5, 0.5]), 0.0);
    }
}
