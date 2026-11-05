//! Filters: RBJ biquad (TDF2), TPT state-variable filter, one-pole, and a
//! DC blocker.

/// Which filter response [`Biquad::design`] should produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BiquadKind {
    /// Lowpass (unity gain at DC).
    Lowpass,
    /// Highpass (unity gain at Nyquist).
    Highpass,
    /// Bandpass with unity peak gain (constant-skirt-off form).
    Bandpass,
    /// Notch (kills a narrow band around `f0`).
    Notch,
    /// Allpass (unity magnitude, phase shift around `f0`).
    Allpass,
    /// Peaking EQ (boosts/cuts around `f0` by `gain_db`).
    PeakingEq,
}

/// Shared Nyquist guard: the frequency at which a filter may be designed.
///
/// Frequencies above Nyquist make RBJ coefficients unstable (poles leave
/// the unit circle), so `f0` is clamped to `[1.0, nyquist * 0.4995]`.
/// The conservative margin also keeps modulated designs away from the
/// degenerate region near Nyquist.
fn clamp_frequency(f0: f32, sample_rate: f32) -> f32 {
    let nyquist = sample_rate * 0.5;
    f0.clamp(1.0, nyquist * 0.4995)
}

/// A second-order IIR filter in transposed direct form II.
///
/// Coefficients follow the RBJ audio-EQ-cookbook formulas; lowpass and
/// highpass numerators use the `cos(w0)` form (`[(1-c)/2, (1-c), (1-c)/2]`),
/// which stays correctly scaled for every cutoff.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// Creates a filter from raw coefficients, normalized by `a0`.
    ///
    /// # Panics
    ///
    /// Panics if `a0` is (near) zero.
    pub fn from_coefficients(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        assert!(a0.abs() > 1.0e-12, "biquad a0 must be non-zero");
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Designs a filter of `kind` at `f0` Hz and quality `q`.
    ///
    /// `gain_db` is only used by [`BiquadKind::PeakingEq`]. `f0` is clamped
    /// to `[1.0, nyquist * 0.4995]` and `q` to at least 0.001, so every
    /// design is stable by construction.
    pub fn design(kind: BiquadKind, sample_rate: f32, f0: f32, q: f32, gain_db: f32) -> Self {
        let f0 = clamp_frequency(f0, sample_rate);
        let q = q.max(0.001);
        let w0 = 2.0 * std::f32::consts::PI * f0 / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);

        let (b0, b1, b2, a0, a1, a2) = match kind {
            BiquadKind::Lowpass => {
                let one_minus_c = 1.0 - cos_w0;
                (
                    one_minus_c / 2.0,
                    one_minus_c,
                    one_minus_c / 2.0,
                    1.0 + alpha,
                    -2.0 * cos_w0,
                    1.0 - alpha,
                )
            }
            BiquadKind::Highpass => {
                let one_plus_c = 1.0 + cos_w0;
                (
                    one_plus_c / 2.0,
                    -one_plus_c,
                    one_plus_c / 2.0,
                    1.0 + alpha,
                    -2.0 * cos_w0,
                    1.0 - alpha,
                )
            }
            BiquadKind::Bandpass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha),
            BiquadKind::Notch => (
                1.0,
                -2.0 * cos_w0,
                1.0,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
            BiquadKind::Allpass => (
                1.0 - alpha,
                -2.0 * cos_w0,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
            BiquadKind::PeakingEq => {
                let a = 10.0_f32.powf(gain_db / 40.0);
                (
                    1.0 + alpha * a,
                    -2.0 * cos_w0,
                    1.0 - alpha * a,
                    1.0 + alpha / a,
                    -2.0 * cos_w0,
                    1.0 - alpha / a,
                )
            }
        };
        Self::from_coefficients(b0, b1, b2, a0, a1, a2)
    }

    /// Processes one sample through the TDF2 recurrence.
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    /// Clears the filter state (coefficients are kept).
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// The three simultaneous outputs of an [`Svf`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SvfOutputs {
    /// Lowpass output.
    pub low: f32,
    /// Bandpass output.
    pub band: f32,
    /// Highpass output.
    pub high: f32,
}

/// A zero-delay-feedback (TPT) state-variable filter.
///
/// Wiring follows Andrew Simper's TPT SVF (as used in FunDSP): `g` is the
/// bilinear pre-warped integrator gain, `k = 1/q` the damping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Svf {
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
    ic1: f32,
    ic2: f32,
}

impl Svf {
    /// Creates a filter at `f0` Hz with quality `q`.
    ///
    /// `f0` is clamped by the shared Nyquist guard.
    pub fn new(sample_rate: f32, f0: f32, q: f32) -> Self {
        let mut filter = Self {
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            k: 0.0,
            ic1: 0.0,
            ic2: 0.0,
        };
        filter.set(sample_rate, f0, q);
        filter
    }

    /// Retunes the filter while preserving the internal state, so a sweep
    /// is continuous.
    ///
    /// `f0` is clamped by the shared Nyquist guard and `q` to at least
    /// 0.001.
    pub fn set(&mut self, sample_rate: f32, f0: f32, q: f32) {
        let f0 = clamp_frequency(f0, sample_rate);
        let q = q.max(0.001);
        let g = (std::f32::consts::PI * f0 / sample_rate).tan();
        self.k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    /// Processes one sample, returning low, band and high outputs.
    pub fn process(&mut self, x: f32) -> SvfOutputs {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        SvfOutputs {
            low: v2,
            band: v1,
            high: x - self.k * v1 - v2,
        }
    }
}

/// A one-pole lowpass (or highpass) filter: `y[n] = a*y[n-1] + (1-a)*x[n]`
/// with `a = exp(-2*pi*f0/sr)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnePole {
    a: f32,
    z: f32,
    invert: bool,
}

impl OnePole {
    /// Creates a lowpass filter at `cutoff_hz`.
    ///
    /// `cutoff_hz` is clamped by the shared Nyquist guard.
    pub fn new(sample_rate: f32, cutoff_hz: f32) -> Self {
        let cutoff_hz = clamp_frequency(cutoff_hz, sample_rate);
        Self {
            a: (-2.0 * std::f32::consts::PI * cutoff_hz / sample_rate).exp(),
            z: 0.0,
            invert: false,
        }
    }

    /// Creates a highpass filter at `cutoff_hz` (`x` minus the lowpass).
    ///
    /// `cutoff_hz` is clamped by the shared Nyquist guard.
    pub fn new_highpass(sample_rate: f32, cutoff_hz: f32) -> Self {
        let mut filter = Self::new(sample_rate, cutoff_hz);
        filter.invert = true;
        filter
    }

    /// Processes one sample.
    pub fn process(&mut self, x: f32) -> f32 {
        let low = self.a * self.z + (1.0 - self.a) * x;
        self.z = low;
        if self.invert {
            x - low
        } else {
            low
        }
    }

    /// Clears the filter state.
    pub fn reset(&mut self) {
        self.z = 0.0;
    }
}

/// A leaky-integrator differentiator DC blocker: `y = x - x1 + R*y1` with
/// `R = 0.995`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DcBlocker {
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    /// Creates a DC blocker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Processes one sample.
    pub fn process(&mut self, x: f32) -> f32 {
        const R: f32 = 0.995;
        let y = x - self.x1 + R * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }

    /// Clears the filter state.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::goertzel_magnitude;

    const SR: f32 = 48_000.0;

    /// Renders `freq` through `filter` and returns the steady-state
    /// amplitude measured by Goertzel on the second half.
    fn response_amplitude<F: FnMut(f32) -> f32>(mut filter: F, freq: f32) -> f32 {
        let n = 48_000;
        let mut out = Vec::with_capacity(n / 2);
        for i in 0..n {
            let x = (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin();
            let y = filter(x);
            if i >= n / 2 {
                out.push(y);
            }
        }
        goertzel_magnitude(&out, freq, SR)
    }

    #[test]
    fn biquad_lowpass_passes_low_and_attenuates_high() {
        let at_500 = {
            let mut f = Biquad::design(
                BiquadKind::Lowpass,
                SR,
                1000.0,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
            );
            response_amplitude(|x| f.process(x), 500.0)
        };
        let at_4000 = {
            let mut f = Biquad::design(
                BiquadKind::Lowpass,
                SR,
                1000.0,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
            );
            response_amplitude(|x| f.process(x), 4000.0)
        };
        assert!(at_500 > 0.9, "500 Hz passed at {at_500}");
        assert!(at_4000 < 0.2, "4 kHz passed at {at_4000}");
        let attenuation_db = 20.0 * (at_4000 / at_500).log10();
        assert!(
            attenuation_db < -10.0,
            "attenuation was {attenuation_db} dB"
        );
    }

    #[test]
    fn biquad_highpass_mirrors_lowpass() {
        let at_500 = {
            let mut f = Biquad::design(
                BiquadKind::Highpass,
                SR,
                1000.0,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
            );
            response_amplitude(|x| f.process(x), 500.0)
        };
        let at_4000 = {
            let mut f = Biquad::design(
                BiquadKind::Highpass,
                SR,
                1000.0,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
            );
            response_amplitude(|x| f.process(x), 4000.0)
        };
        assert!(at_4000 > 0.9, "4 kHz passed at {at_4000}");
        // A 2nd-order highpass is only -12.3 dB at half the cutoff
        // (measured 0.24); -20 dB there would need a 4th-order filter.
        assert!(at_500 < 0.3, "500 Hz passed at {at_500}");
        let attenuation_db = 20.0 * (at_500 / at_4000).log10();
        assert!(
            attenuation_db < -10.0,
            "attenuation was {attenuation_db} dB"
        );
    }

    #[test]
    fn biquad_bandpass_notch_allpass_and_peaking_behave() {
        let mut bp = Biquad::design(BiquadKind::Bandpass, SR, 1000.0, 2.0, 0.0);
        assert!(response_amplitude(|x| bp.process(x), 1000.0) > 0.85);
        let mut bp = Biquad::design(BiquadKind::Bandpass, SR, 1000.0, 2.0, 0.0);
        assert!(response_amplitude(|x| bp.process(x), 250.0) < 0.45);

        let mut notch = Biquad::design(BiquadKind::Notch, SR, 1000.0, 5.0, 0.0);
        let rejected = response_amplitude(|x| notch.process(x), 1000.0);
        let mut notch = Biquad::design(BiquadKind::Notch, SR, 1000.0, 5.0, 0.0);
        let passed = response_amplitude(|x| notch.process(x), 2000.0);
        assert!(rejected < 0.05, "notch rejected at {rejected}");
        assert!(passed > 0.7, "notch passed at {passed}");

        let mut ap = Biquad::design(
            BiquadKind::Allpass,
            SR,
            1000.0,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        );
        let unity = response_amplitude(|x| ap.process(x), 1000.0);
        assert!((unity - 1.0).abs() < 0.02, "allpass magnitude {unity}");

        let mut peak = Biquad::design(BiquadKind::PeakingEq, SR, 1000.0, 1.0, 6.0);
        let boosted = response_amplitude(|x| peak.process(x), 1000.0);
        let mut peak = Biquad::design(BiquadKind::PeakingEq, SR, 1000.0, 1.0, 6.0);
        let untouched = response_amplitude(|x| peak.process(x), 4000.0);
        assert!(
            (boosted - 10.0_f32.powf(6.0 / 20.0)).abs() < 0.1,
            "boost {boosted}"
        );
        assert!((untouched - 1.0).abs() < 0.15, "far band {untouched}");
    }

    #[test]
    fn biquad_above_nyquist_stays_finite_and_bounded() {
        // Regression: designing at f0=6000 with sr=8000 (f0 > Nyquist) used
        // to produce poles outside the unit circle and blow up.
        let mut f = Biquad::design(
            BiquadKind::Lowpass,
            8000.0,
            6000.0,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        );
        let mut rng = crate::rng::Qrng::new(31);
        let mut peak = 0.0f32;
        for _ in 0..8000 {
            let noise = rng.next_f32() * 2.0 - 1.0;
            let y = f.process(noise);
            assert!(y.is_finite());
            peak = peak.max(y.abs());
        }
        assert!(peak < 10.0, "peak was {peak}");

        // The clamped design (~2 kHz lowpass) still attenuates 3.5 kHz.
        let at_3500 = {
            let mut f = Biquad::design(
                BiquadKind::Lowpass,
                8000.0,
                6000.0,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
            );
            let n = 32_000;
            let mut out = Vec::with_capacity(n / 2);
            for i in 0..n {
                let x = (2.0 * std::f32::consts::PI * 3500.0 * i as f32 / 8000.0).sin();
                let y = f.process(x);
                if i >= n / 2 {
                    out.push(y);
                }
            }
            goertzel_magnitude(&out, 3500.0, 8000.0)
        };
        assert!(
            at_3500 < 0.5,
            "clamped lowpass let 3.5 kHz through at {at_3500}"
        );
    }

    #[test]
    fn svf_wires_low_band_high_correctly() {
        let measure = |freq: f32, sr: f32, out: fn(SvfOutputs) -> f32| {
            let mut svf = Svf::new(sr, 1000.0, std::f32::consts::FRAC_1_SQRT_2);
            response_amplitude(|x| out(svf.process(x)), freq)
        };
        let low_at_200 = measure(200.0, SR, |o| o.low);
        let high_at_200 = measure(200.0, SR, |o| o.high);
        assert!(
            low_at_200 > 10.0 * high_at_200,
            "low {low_at_200} vs high {high_at_200} at 200 Hz"
        );
        let low_at_5000 = measure(5000.0, SR, |o| o.low);
        let high_at_5000 = measure(5000.0, SR, |o| o.high);
        assert!(
            high_at_5000 > 10.0 * low_at_5000,
            "high {high_at_5000} vs low {low_at_5000} at 5 kHz"
        );
        // Band output peaks between the two extremes.
        let band_at_1000 = measure(1000.0, SR, |o| o.band);
        let band_at_200 = measure(200.0, SR, |o| o.band);
        let band_at_5000 = measure(5000.0, SR, |o| o.band);
        assert!(band_at_1000 > 3.0 * band_at_200.max(band_at_5000));
    }

    #[test]
    fn svf_set_preserves_state_and_continues_filtering() {
        let mut svf = Svf::new(SR, 1000.0, std::f32::consts::FRAC_1_SQRT_2);
        let mut rng = crate::rng::Qrng::new(3);
        for _ in 0..10_000 {
            let x = rng.next_f32() * 2.0 - 1.0;
            svf.process(x);
        }
        svf.set(SR, 4000.0, std::f32::consts::FRAC_1_SQRT_2);
        // After retuning, a 200 Hz tone must come out strongly on low.
        let at_200 = response_amplitude(|x| svf.process(x).low, 200.0);
        assert!(at_200 > 0.8, "low after set() was {at_200}");
    }

    #[test]
    fn one_pole_attenuates_highs_and_mirror_highpass() {
        let mut lp = OnePole::new(SR, 200.0);
        assert!(response_amplitude(|x| lp.process(x), 50.0) > 0.9);
        let mut lp = OnePole::new(SR, 200.0);
        let hf = response_amplitude(|x| lp.process(x), 5000.0);
        assert!(hf < 0.1, "5 kHz through lowpass was {hf}");

        let mut hp = OnePole::new_highpass(SR, 200.0);
        assert!(response_amplitude(|x| hp.process(x), 5000.0) > 0.9);
        let mut hp = OnePole::new_highpass(SR, 200.0);
        assert!(response_amplitude(|x| hp.process(x), 50.0) < 0.35);
    }

    #[test]
    fn biquad_identity_coefficients_pass_signal_through_exactly() {
        let mut identity = Biquad::from_coefficients(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        for step in 0..1000 {
            let input = (std::f32::consts::PI * step as f32 * 0.01).sin();
            let output = identity.process(input);
            assert_eq!(output.to_bits(), input.to_bits(), "step {step}");
        }
    }

    #[test]
    fn biquad_reset_restores_the_initial_response() {
        let mut warmed = Biquad::design(
            BiquadKind::Lowpass,
            SR,
            2000.0,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        );
        let mut source = crate::rng::Qrng::new(4);
        for _ in 0..5_000 {
            let noise = source.next_f32() * 2.0 - 1.0;
            warmed.process(noise);
        }
        warmed.reset();
        let mut fresh = Biquad::design(
            BiquadKind::Lowpass,
            SR,
            2000.0,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        );
        for step in 0..1_000 {
            let input = (2.0 * std::f32::consts::PI * 440.0 * step as f32 / SR).sin();
            let from_reset = warmed.process(input);
            let from_fresh = fresh.process(input);
            assert_eq!(from_reset.to_bits(), from_fresh.to_bits(), "step {step}");
        }
    }

    #[test]
    fn biquad_lowpass_has_unity_dc_gain_and_highpass_blocks_dc() {
        // With the wrong [1, 2, 1] lowpass numerators the DC gain would be
        // 2/(1-cos(w0)), about 234 at 1 kHz — this pins the (1-c)/2 form.
        let mut lp = Biquad::design(
            BiquadKind::Lowpass,
            SR,
            1000.0,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        );
        let mut hp = Biquad::design(
            BiquadKind::Highpass,
            SR,
            1000.0,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        );
        let mut lp_out = 0.0;
        let mut hp_out = 0.0;
        for _ in 0..48_000 {
            lp_out = lp.process(1.0);
            hp_out = hp.process(1.0);
        }
        assert!((lp_out - 1.0).abs() < 1.0e-3, "lowpass DC residue {lp_out}");
        assert!(hp_out.abs() < 1.0e-3, "highpass DC residue {hp_out}");
    }

    #[test]
    fn svf_band_and_low_gain_at_center_equal_q() {
        // The analog prototype evaluated at the (pre-warped) center is
        // exactly 1/k = q for both the band and the low output.
        for q in [0.5f32, 2.0] {
            let mut svf = Svf::new(SR, 1000.0, q);
            let band = response_amplitude(|x| svf.process(x).band, 1000.0);
            assert!((band - q).abs() < 0.1, "q {q}: band {band}");
            let mut svf = Svf::new(SR, 1000.0, q);
            let low = response_amplitude(|x| svf.process(x).low, 1000.0);
            assert!((low - q).abs() < 0.1, "q {q}: low {low}");
        }
    }

    #[test]
    fn one_pole_meets_half_power_at_cutoff() {
        let mut lp = OnePole::new(SR, 200.0);
        let at_cutoff = response_amplitude(|x| lp.process(x), 200.0);
        let expected = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (at_cutoff - expected).abs() < 0.05,
            "response at cutoff was {at_cutoff}, expected ~{expected}"
        );
    }

    #[test]
    fn svf_and_one_pole_above_nyquist_stay_finite() {
        // Nyquist guard regression for the designs not covered above.
        let mut svf = Svf::new(8000.0, 6000.0, 0.5);
        let mut pole = OnePole::new(8000.0, 6000.0);
        let mut rng = crate::rng::Qrng::new(23);
        let mut peak_svf = 0.0f32;
        let mut peak_pole = 0.0f32;
        for _ in 0..16_000 {
            let noise = rng.next_f32() * 2.0 - 1.0;
            let outputs = svf.process(noise);
            peak_svf = peak_svf
                .max(outputs.low.abs())
                .max(outputs.band.abs())
                .max(outputs.high.abs());
            peak_pole = peak_pole.max(pole.process(noise).abs());
        }
        assert!(peak_svf < 10.0, "svf peak was {peak_svf}");
        assert!(peak_pole < 2.0, "one-pole peak was {peak_pole}");
    }

    #[test]
    fn dc_blocker_kills_dc_and_passes_audio() {
        let mut blocker = DcBlocker::new();
        let mut last = 0.0;
        for _ in 0..20_000 {
            last = blocker.process(1.0);
        }
        assert!(last.abs() < 1.0e-3, "DC residue {last}");

        let mut blocker = DcBlocker::new();
        let at_200 = response_amplitude(|x| blocker.process(x), 200.0);
        assert!(at_200 > 0.9, "200 Hz passed at {at_200}");
    }
}
