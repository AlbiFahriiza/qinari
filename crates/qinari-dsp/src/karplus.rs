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
        assert!(
            peak <= 1.0,
            "excitation noise must stay in +/-1, peak {peak}"
        );

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

        // Physical cross-check: the excitation is one broadband noise burst,
        // so only a modest fraction of its energy lands in the fundamental mode.
        let at_fundamental = goertzel_magnitude(&note, 220.0, SR);
        let at_below = goertzel_magnitude(&note, 165.0, SR);
        let at_above = goertzel_magnitude(&note, 275.0, SR);
        let neighbor = at_below.max(at_above);
        assert!(
            at_fundamental > 10.0 * neighbor,
            "fundamental {at_fundamental} vs neighbor {neighbor} (selectivity < 10x)"
        );

        // The excitation is white noise: its rms must stay near 1/sqrt(3).
        let d = (SR / 220.0).round() as usize;
        let excitation = &note[..d];
        let rms = (excitation.iter().map(|&s| f64::from(s * s)).sum::<f64>() / d as f64).sqrt();
        assert!(
            (0.4..0.75).contains(&rms),
            "excitation rms {rms} (expected uniform +/-1 noise)"
        );

        // Calibrated physics from the spec: the fundamental of a 220 Hz
        // pluck reads 0.015-0.05 in goertzel amplitude. The measurement
        // window is the first half-second, where the note lives (the mode
        // e-folds in t60/6.91 ~ 0.29 s, so a whole-note window would read
        // the decayed average instead).
        let early = &note[..24_000];
        let early_fundamental = goertzel_magnitude(early, 220.0, SR);
        assert!(
            (0.015..=0.05).contains(&early_fundamental),
            "early fundamental {early_fundamental}, expected 0.015-0.05"
        );
        let early_below = goertzel_magnitude(early, 165.0, SR);
        let early_above = goertzel_magnitude(early, 275.0, SR);
        let early_neighbor = early_below.max(early_above);
        assert!(
            early_fundamental > 10.0 * early_neighbor,
            "early fundamental {early_fundamental} vs neighbor {early_neighbor} (selectivity < 10x)"
        );
    }

    #[test]
    fn pluck_retunes_with_frequency() {
        let note_220 = pluck(SR, 220.0, 2.0, &mut Qrng::new(303));
        let note_440 = pluck(SR, 440.0, 2.0, &mut Qrng::new(303));

        // The loop modes live at multiples of sr/D, so retuning to 440
        // removes the ODD multiples of 220 from the loop. The 440-note must
        // therefore be near-silent at 220 and 660 Hz, where the 220-note
        // still rings (440 itself is a mode of both, so it is not a
        // discriminator).
        let at = |note: &[f32], freq: f32| goertzel_magnitude(note, freq, SR);
        let low_of_220 = at(&note_220, 220.0);
        let low_of_440 = at(&note_440, 220.0);
        assert!(
            low_of_440 < 0.25 * low_of_220,
            "220 Hz: 440-note {low_of_440} vs 220-note {low_of_220}"
        );
        let mid_of_220 = at(&note_220, 660.0);
        let mid_of_440 = at(&note_440, 660.0);
        assert!(
            mid_of_440 < 0.25 * mid_of_220,
            "660 Hz: 440-note {mid_of_440} vs 220-note {mid_of_220}"
        );
        // And the retuned note rings hard at its own fundamental.
        let own = at(&note_440, 440.0);
        assert!(
            own > 0.25 * low_of_220,
            "440-note fundamental {own} vs 220-note {low_of_220}"
        );
    }
}
