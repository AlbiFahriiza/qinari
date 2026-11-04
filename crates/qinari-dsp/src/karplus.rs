//! Karplus-Strong plucked string synthesis.

use crate::rng::Qrng;

/// Renders a plucked string note.
///
/// A delay line of `D = round(sr / f0)` samples is excited with white
/// noise from `rng`, then looped through `y[n] = decay * 0.5 * (y[n-D] +
/// y[n-D-1])`; the half-sample average is the loop lowpass. `damping_t60`
/// maps to the round-trip gain `decay = 10^(-3*D / (sr * t60))`, so the
/// string's amplitude crosses -60 dB after `damping_t60` seconds. The
/// render runs to about 1.5 * t60 (roughly -90 dB), which is the full
/// audible life of the note.
///
/// # Panics
///
/// Panics if `f0` or `damping_t60` is not positive, or `sample_rate` is
/// not positive.
pub fn pluck(sample_rate: f32, f0: f32, damping_t60: f32, rng: &mut Qrng) -> Vec<f32> {
    assert!(sample_rate > 0.0, "sample_rate must be positive");
    assert!(f0 > 0.0, "f0 must be positive");
    assert!(damping_t60 > 0.0, "damping_t60 must be positive");

    // Guard a minimum of 2 samples so the averaging tap always has two
    // distinct entries (frequencies above Nyquist detune to sr/2).
    let d = ((sample_rate / f0).round() as usize).max(2);
    let decay = 10.0_f32.powf(-3.0 * d as f32 / (sample_rate * damping_t60));
    let total = ((1.5 * damping_t60 * sample_rate).ceil() as usize).max(4 * d);

    let mut buffer: Vec<f32> = (0..d).map(|_| rng.next_f32() * 2.0 - 1.0).collect();
    let mut out = Vec::with_capacity(total);
    let mut index = 0usize;
    for _ in 0..total {
        let current = buffer[index];
        let next = buffer[(index + 1) % d];
        out.push(current);
        buffer[index] = decay * 0.5 * (current + next);
        index += 1;
        if index == d {
            index = 0;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::goertzel_magnitude;

    const SR: f32 = 48_000.0;

    #[test]
    fn pluck_is_finite_bounded_and_decaying() {
        let mut rng = Qrng::new(101);
        let note = pluck(SR, 220.0, 2.0, &mut rng);
        assert_eq!(note.len(), (1.5 * 2.0 * SR) as usize);
        assert!(note.iter().all(|s| s.is_finite()));
        let peak = note.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(peak <= 1.0, "excitation noise must stay in +/-1, peak {peak}");

        let quarter = note.len() / 4;
        let rms = |slice: &[f32]| {
            (slice.iter().map(|&s| f64::from(s * s)).sum::<f64>() / slice.len() as f64).sqrt()
        };
        assert!(
            rms(&note[..quarter]) > 10.0 * rms(&note[3 * quarter..]),
            "note does not decay"
        );
    }

    #[test]
    fn pluck_rings_selectively_at_fundamental() {
        let mut rng = Qrng::new(202);
        let note = pluck(SR, 220.0, 2.0, &mut rng);

        // The excitation is one broadband noise burst, so only a modest
        // fraction of its energy lands in the fundamental mode.
        let at_fundamental = goertzel_magnitude(&note, 220.0, SR);
        let at_below = goertzel_magnitude(&note, 165.0, SR);
        let at_above = goertzel_magnitude(&note, 275.0, SR);
        let neighbor = at_below.max(at_above);
        assert!(
            at_fundamental > 10.0 * neighbor,
            "fundamental {at_fundamental} vs neighbor {neighbor} (selectivity < 10x)"
        );

        // Physical cross-check: the ringing amplitude must equal the
        // excitation's matched sinusoid amplitude diluted by the ratio of
        // effective decay length to note length (loop gain ~cos(pi/D)).
        let d = (SR / 220.0).round() as usize;
        let excitation = &note[..d];
        let excitation_amp = goertzel_magnitude(excitation, 220.0, SR);
        let rms = (excitation
            .iter()
            .map(|&s| f64::from(s * s))
            .sum::<f64>()
            / d as f64)
            .sqrt();
        assert!(
            (0.4..0.75).contains(&rms),
            "excitation rms {rms} (expected uniform +/-1 noise)"
        );
        let per_period = 10.0_f64.powf(-3.0 * d as f64 / (SR as f64 * 2.0))
            * (std::f64::consts::PI / d as f64).cos();
        let per_sample = per_period.powf(1.0 / d as f64);
        let effective_length = 1.0 / (1.0 - per_sample);
        let expected = f64::from(excitation_amp) * effective_length / note.len() as f64;
        assert!(
            (f64::from(at_fundamental) - expected).abs() < 0.3 * expected,
            "ringing {at_fundamental} vs predicted {expected}"
        );
    }

    #[test]
    fn pluck_retunes_with_frequency() {
        let note_220 = pluck(SR, 220.0, 2.0, &mut Qrng::new(303));
        let note_440 = pluck(SR, 440.0, 2.0, &mut Qrng::new(303));

        let p220_in_low = goertzel_magnitude(&note_220, 220.0, SR);
        let p440_in_low = goertzel_magnitude(&note_220, 440.0, SR);
        assert!(
            p220_in_low > 5.0 * p440_in_low,
            "220 Hz pluck: {p220_in_low} at 220 vs {p440_in_low} at 440"
        );

        let p220_in_high = goertzel_magnitude(&note_440, 220.0, SR);
        let p440_in_high = goertzel_magnitude(&note_440, 440.0, SR);
        assert!(
            p440_in_high > 5.0 * p220_in_high,
            "440 Hz pluck: {p440_in_high} at 440 vs {p220_in_high} at 220"
        );
        // And the peak moves up: the 440 note rings harder at 440 than the
        // 220 note does (shorter string holds proportionally more of the
        // burst energy per mode).
        assert!(p440_in_high > p220_in_low * 1.5);
    }
}
