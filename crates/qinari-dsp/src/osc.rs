//! Oscillators: `PolyBLEP`-corrected waveforms, FM, and pitch helpers.

use crate::rng::Qrng;
use std::sync::OnceLock;

/// Waveform selection for [`Osc`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OscKind {
    /// Sine.
    Sine,
    /// Band-limited sawtooth.
    Saw,
    /// Band-limited square (50% duty).
    Square,
    /// Band-limited triangle (leaky-integrated square).
    Triangle,
    /// Band-limited pulse with adjustable width.
    Pulse,
}

/// Converts a MIDI note number (69 = A4 = 440 Hz) to a frequency in hertz.
pub fn midi_to_freq(midi: f32) -> f32 {
    440.0 * 2.0_f32.powf((midi - 69.0) / 12.0)
}

/// Converts a frequency in hertz to a MIDI note number.
pub fn freq_to_midi(freq: f32) -> f32 {
    69.0 + 12.0 * (freq / 440.0).log2()
}

/// Size of the sine lookup table (plus one extra entry so linear
/// interpolation can read one past the end).
const SINE_TABLE_SIZE: usize = 2048;

/// Builds the sine table: `table[i] = sin(2*pi*i / N)` plus one wrapped
/// entry at the end.
fn build_sine_table() -> [f32; SINE_TABLE_SIZE + 1] {
    let mut table = [0.0f32; SINE_TABLE_SIZE + 1];
    for (i, slot) in table.iter_mut().enumerate() {
        *slot = (2.0 * std::f32::consts::PI * i as f32 / SINE_TABLE_SIZE as f32).sin();
    }
    table
}

/// Fast sine lookup with linear interpolation.
///
/// `phase` is in cycles (`0.0` to `1.0` is one full period) and is wrapped
/// for you, so any real `f32` phase is valid. Interpolation error is below
/// `-120 dB` relative to full scale.
pub fn fast_sine(phase: f32) -> f32 {
    static TABLE: OnceLock<[f32; SINE_TABLE_SIZE + 1]> = OnceLock::new();
    let table = TABLE.get_or_init(build_sine_table);
    let wrapped = phase - phase.floor();
    let position = wrapped * SINE_TABLE_SIZE as f32;
    let index = position as usize;
    let frac = position - index as f32;
    let a = table[index];
    let b = table[index + 1];
    a + (b - a) * frac
}

/// Classic two-segment `PolyBLEP` correction for a unit-height step.
///
/// `t` is the phase in `[0, 1)` and `dt` the phase increment per sample;
/// the returned value must be subtracted at a falling (-2) discontinuity
/// or added at a rising (+2) one.
fn polyblep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

/// A band-limited oscillator with random initial phase and optional glide.
///
/// Saw, square and pulse use `PolyBLEP` correction; triangle integrates
/// the corrected square through a leaky integrator (1 s time constant),
/// which is stable and decays at roughly 12 dB/octave like an ideal
/// triangle. The initial phase is drawn from `rng` so several detuned
/// oscillators do not phase-lock.
#[derive(Debug, Clone)]
pub struct Osc {
    kind: OscKind,
    sample_rate: f32,
    phase: f32,
    freq: f32,
    glide: Option<(f32, f32)>,
    pulse_width: f32,
    tri_state: f32,
}

impl Osc {
    /// Creates an oscillator of `kind` at `freq` Hz.
    pub fn new(kind: OscKind, freq: f32, sample_rate: f32, rng: &mut Qrng) -> Self {
        Self {
            kind,
            sample_rate,
            phase: rng.next_f32(),
            freq,
            glide: None,
            pulse_width: 0.5,
            tri_state: 0.0,
        }
    }

    /// Sets the frequency immediately, cancelling any glide.
    pub fn set_freq(&mut self, freq: f32) {
        self.freq = freq;
        self.glide = None;
    }

    /// Starts a linear-in-Hz glide to `freq` over `secs` seconds.
    ///
    /// A non-positive `secs` jumps straight to the target.
    pub fn glide_to(&mut self, freq: f32, secs: f32) {
        if secs <= 0.0 {
            self.set_freq(freq);
            return;
        }
        let step = (freq - self.freq) / (secs * self.sample_rate);
        self.glide = Some((freq, step));
    }

    /// Sets the pulse width for [`OscKind::Pulse`] to `[0, 1]` (clamped).
    ///
    /// Extreme widths degenerate toward a constant output; 0.5 is a square.
    pub fn set_pulse_width(&mut self, width: f32) {
        self.pulse_width = width.clamp(0.0, 1.0);
    }

    /// Returns the current frequency in hertz (glide target once reached).
    pub fn freq(&self) -> f32 {
        self.freq
    }

    /// Advances one sample and returns the waveform value.
    pub fn process(&mut self) -> f32 {
        if let Some((target, step)) = self.glide {
            self.freq += step;
            let reached = if step >= 0.0 {
                self.freq >= target
            } else {
                self.freq <= target
            };
            if reached {
                self.freq = target;
                self.glide = None;
            }
        }

        let dt = (self.freq / self.sample_rate).min(0.5);
        let phase = self.phase;
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }

        match self.kind {
            OscKind::Sine => fast_sine(phase),
            OscKind::Saw => 2.0 * phase - 1.0 - polyblep(phase, dt),
            OscKind::Square => {
                let naive = if phase < 0.5 { 1.0 } else { -1.0 };
                let falling = (phase + 0.5) % 1.0;
                naive + polyblep(phase, dt) - polyblep(falling, dt)
            }
            OscKind::Pulse => {
                let naive = if phase < self.pulse_width { 1.0 } else { -1.0 };
                let falling = (phase - self.pulse_width + 1.0) % 1.0;
                naive + polyblep(phase, dt) - polyblep(falling, dt)
            }
            OscKind::Triangle => {
                // Integrate the corrected square: 4*f/sr gives unit triangle
                // amplitude and the leak (1 s) bounds DC drift.
                let naive = if phase < 0.5 { 1.0 } else { -1.0 };
                let falling = (phase + 0.5) % 1.0;
                let square = naive + polyblep(phase, dt) - polyblep(falling, dt);
                let leak = (-1.0 / self.sample_rate).exp();
                let gain = 4.0 * self.freq / self.sample_rate;
                self.tri_state = leak * self.tri_state + gain * square;
                self.tri_state
            }
        }
    }
}

/// A simple two-operator FM oscillator.
///
/// The output is `sin(2*pi*(carrier_phase + index*sin(2*pi*mod_phase)))`
/// where the modulator runs at `carrier * ratio` hertz.
#[derive(Debug, Clone)]
pub struct FmOsc {
    carrier: f32,
    ratio: f32,
    index: f32,
    sample_rate: f32,
    carrier_phase: f32,
    mod_phase: f32,
}

impl FmOsc {
    /// Creates an FM oscillator with carrier in hertz, frequency `ratio`
    /// and modulation `index`.
    pub fn new(carrier_hz: f32, ratio: f32, index: f32, sample_rate: f32) -> Self {
        Self {
            carrier: carrier_hz,
            ratio,
            index,
            sample_rate,
            carrier_phase: 0.0,
            mod_phase: 0.0,
        }
    }

    /// Advances one sample and returns the modulated sine.
    pub fn process(&mut self) -> f32 {
        let mod_step = (self.carrier * self.ratio / self.sample_rate) % 1.0;
        self.mod_phase = (self.mod_phase + mod_step) % 1.0;
        let carrier_step = (self.carrier / self.sample_rate) % 1.0;
        self.carrier_phase = (self.carrier_phase + carrier_step) % 1.0;
        fast_sine(self.carrier_phase + self.index * fast_sine(self.mod_phase))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{goertzel_magnitude, zcr};

    const SR: f32 = 48_000.0;

    fn render(mut osc: Osc, frames: usize) -> Vec<f32> {
        (0..frames).map(|_| osc.process()).collect()
    }

    #[test]
    fn midi_conversion_round_trips() {
        assert!((midi_to_freq(69.0) - 440.0).abs() < 1e-4);
        assert!((freq_to_midi(440.0) - 69.0).abs() < 1e-4);
        assert!((midi_to_freq(81.0) - 880.0).abs() < 0.01);
        for note in [33.0, 57.5, 69.0, 76.25, 108.0] {
            assert!((freq_to_midi(midi_to_freq(note)) - note).abs() < 1e-3);
        }
    }

    #[test]
    fn fast_sine_matches_std_sin_across_the_cycle() {
        for i in 0..4096 {
            let phase = i as f32 / 4096.0;
            let expected =
                (2.0 * std::f32::consts::PI * phase).sin();
            let got = fast_sine(phase);
            assert!((got - expected).abs() < 3.0e-6, "phase {phase}: {got} vs {expected}");
        }
        // Wrapping: negative and >1 phases alias correctly.
        assert!((fast_sine(-0.25) - (-1.0)).abs() < 3.0e-6);
        assert!((fast_sine(1.75) - (-1.0)).abs() < 3.0e-6);
        assert!(fast_sine(0.0).abs() < 3.0e-6);
    }

    #[test]
    fn saw_has_strong_fundamental_and_suppressed_aliases() {
        // 1700 Hz over 1 s is on-bin; harmonic 15 (25500 Hz) folds to
        // 22500 Hz in a naive saw with amplitude ~2/(15*pi).
        let samples = render(
            Osc::new(OscKind::Saw, 1700.0, SR, &mut Qrng::new(5)),
            48_000,
        );
        let fundamental = goertzel_magnitude(&samples, 1700.0, SR);
        let alias = goertzel_magnitude(&samples, 22_500.0, SR);
        let alias2 = goertzel_magnitude(&samples, 20_800.0, SR);
        assert!(fundamental > 0.4, "fundamental {fundamental}");
        assert!(alias < 0.02, "alias at 22.5 kHz {alias}");
        assert!(alias2 < 0.02, "alias at 20.8 kHz {alias2}");
    }

    #[test]
    fn square_has_strong_fundamental_and_suppressed_aliases() {
        let samples = render(
            Osc::new(OscKind::Square, 1700.0, SR, &mut Qrng::new(6)),
            48_000,
        );
        let fundamental = goertzel_magnitude(&samples, 1700.0, SR);
        let alias = goertzel_magnitude(&samples, 22_500.0, SR);
        assert!(fundamental > 0.8, "fundamental {fundamental}");
        assert!(alias < 0.02, "alias {alias}");
        // Even harmonics of a 50% square should be near absent.
        let even = goertzel_magnitude(&samples, 3400.0, SR);
        assert!(even < 0.05, "even harmonic {even}");
    }

    #[test]
    fn triangle_decays_faster_than_saw() {
        let tri = render(
            Osc::new(OscKind::Triangle, 220.0, SR, &mut Qrng::new(7)),
            48_000,
        );
        let saw = render(
            Osc::new(OscKind::Saw, 220.0, SR, &mut Qrng::new(8)),
            48_000,
        );
        let fundamental = 220.0;
        let third = 660.0;
        let fifth = 1100.0;
        let tri_h1 = goertzel_magnitude(&tri, fundamental, SR);
        let tri_h3 = goertzel_magnitude(&tri, third, SR);
        let tri_h5 = goertzel_magnitude(&tri, fifth, SR);
        let saw_h1 = goertzel_magnitude(&saw, fundamental, SR);
        let saw_h3 = goertzel_magnitude(&saw, third, SR);
        // Triangle: harmonics fall ~12 dB/octave (h3 ~ 1/9, -19 dB).
        assert!(tri_h1 > 0.4, "triangle fundamental {tri_h1}");
        assert!(tri_h3 / tri_h1 < 0.16, "triangle h3 ratio {}", tri_h3 / tri_h1);
        assert!(tri_h5 / tri_h1 < 0.07, "triangle h5 ratio {}", tri_h5 / tri_h1);
        // Saw: harmonics fall ~6 dB/octave (h3 ~ 1/3, -9.5 dB).
        assert!(saw_h3 / saw_h1 > 0.25, "saw h3 ratio {}", saw_h3 / saw_h1);
        assert!(saw_h3 / saw_h1 > tri_h3 / tri_h1 * 1.5);

        // Long render stays bounded (leaky integrator must not blow up).
        let long = render(
            Osc::new(OscKind::Triangle, 220.0, SR, &mut Qrng::new(7)),
            48_000 * 5,
        );
        assert!(long.iter().all(|s| s.is_finite()));
        assert!(long.iter().fold(0.0f32, |m, &s| m.max(s.abs())) < 1.5);
    }

    #[test]
    fn pulse_width_controls_spectrum() {
        let narrow = render(
            {
                let mut osc = Osc::new(OscKind::Pulse, 440.0, SR, &mut Qrng::new(9));
                osc.set_pulse_width(0.1);
                osc
            },
            48_000,
        );
        let fundamental = goertzel_magnitude(&narrow, 440.0, SR);
        // A narrow pulse has strong even harmonics; 880 Hz must be present.
        let second = goertzel_magnitude(&narrow, 880.0, SR);
        assert!(fundamental > 0.3, "fundamental {fundamental}");
        assert!(second > 0.15, "second harmonic {second}");
    }

    #[test]
    fn glide_reaches_target_frequency() {
        let mut osc = Osc::new(OscKind::Saw, 220.0, SR, &mut Qrng::new(11));
        osc.glide_to(800.0, 0.5);
        let samples = render(osc, 48_000);
        // After 0.5 s the oscillator must sit exactly at 800 Hz.
        let tail = &samples[24_000..];
        let crossings = tail
            .windows(2)
            .filter(|pair| (pair[0] >= 0.0) != (pair[1] >= 0.0))
            .count();
        let measured = crossings as f32 / (tail.len() as f32 / SR);
        assert!((measured - 800.0).abs() < 10.0, "measured {measured} Hz");
        // The steady tail reads as a clean 800 Hz tone (200 on-bin cycles).
        let at_target = goertzel_magnitude(tail, 800.0, SR);
        assert!(at_target > 0.5, "magnitude at target {at_target}");
    }

    #[test]
    fn glide_reports_frequency_and_zero_glide_jumps() {
        let mut osc = Osc::new(OscKind::Square, 100.0, SR, &mut Qrng::new(12));
        assert!((osc.freq() - 100.0).abs() < 1e-6);
        osc.glide_to(300.0, 0.0);
        assert!((osc.freq() - 300.0).abs() < 1e-6, "zero-time glide must jump");
        for _ in 0..100 {
            osc.process();
        }
        assert!((osc.freq() - 300.0).abs() < 1e-6);
    }

    #[test]
    fn fm_produces_sidebands() {
        let mut fm = FmOsc::new(200.0, 3.0, 1.0, SR);
        let samples: Vec<f32> = (0..48_000).map(|_| fm.process()).collect();
        let carrier = goertzel_magnitude(&samples, 200.0, SR);
        // First sidebands at 200 +/- 600 Hz.
        let upper = goertzel_magnitude(&samples, 800.0, SR);
        let lower = goertzel_magnitude(&samples, 400.0, SR);
        assert!(carrier > 0.4, "carrier {carrier}");
        assert!(upper > 0.15, "upper sideband {upper}");
        assert!(lower > 0.15, "lower sideband {lower}");
        assert!(samples.iter().all(|s| s.is_finite()));
    }
}
