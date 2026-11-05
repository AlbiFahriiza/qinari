//! Low-frequency oscillators, bipolar in `[-1, 1]`.

use crate::osc::fast_sine;
use crate::rng::Qrng;

/// Waveform selection for [`Lfo`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfoShape {
    /// Sine.
    Sine,
    /// Triangle.
    Triangle,
    /// Sawtooth rising from -1 to 1.
    Saw,
    /// Square (positive first half of the cycle).
    Square,
    /// Sample & hold: a fresh random value every cycle, from the `Qrng`.
    SampleHold,
}

/// A low-frequency oscillator, processed one sample at a time.
///
/// All shapes are bipolar in `[-1, 1]`. [`LfoShape::SampleHold`] draws a
/// fresh uniform random value from its own forked `Qrng` once per cycle.
#[derive(Debug, Clone)]
pub struct Lfo {
    shape: LfoShape,
    freq: f32,
    sample_rate: f32,
    phase: f32,
    hold_samples: usize,
    hold_counter: usize,
    held: f32,
    rng: Option<Qrng>,
}

impl Lfo {
    /// Creates an LFO of `shape` at `freq_hz`.
    ///
    /// [`LfoShape::SampleHold`] draws its values from `rng`; other shapes
    /// ignore it (the parameter keeps call sites uniform).
    pub fn new(shape: LfoShape, freq_hz: f32, sample_rate: f32, rng: &mut Qrng) -> Self {
        let freq = freq_hz.max(0.0);
        let hold_samples = ((sample_rate / freq.max(1.0e-6)).round() as usize).max(1);
        let held = if shape == LfoShape::SampleHold {
            rng.next_f32() * 2.0 - 1.0
        } else {
            0.0
        };
        Self {
            shape,
            freq,
            sample_rate,
            phase: 0.0,
            hold_samples,
            hold_counter: 0,
            held,
            rng: if shape == LfoShape::SampleHold {
                Some(rng.fork())
            } else {
                None
            },
        }
    }

    /// Advances one sample and returns a value in `[-1, 1]`.
    pub fn process(&mut self) -> f32 {
        if self.shape == LfoShape::SampleHold {
            let out = self.held;
            self.hold_counter += 1;
            if self.hold_counter >= self.hold_samples {
                self.hold_counter = 0;
                if let Some(rng) = self.rng.as_mut() {
                    self.held = rng.next_f32() * 2.0 - 1.0;
                }
            }
            return out;
        }

        let phase = self.phase;
        let step = (self.freq / self.sample_rate).min(0.5);
        self.phase += step;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }

        match self.shape {
            LfoShape::Sine => fast_sine(phase),
            LfoShape::Triangle => {
                if phase < 0.5 {
                    4.0 * phase - 1.0
                } else {
                    3.0 - 4.0 * phase
                }
            }
            LfoShape::Saw => 2.0 * phase - 1.0,
            LfoShape::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoShape::SampleHold => unreachable!("handled above"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn sine_lfo_period_matches_frequency_by_zero_crossings() {
        let mut lfo = Lfo::new(LfoShape::Sine, 5.0, SR, &mut Qrng::new(1));
        let samples: Vec<f32> = (0..48_000).map(|_| lfo.process()).collect();
        let crossings = samples
            .windows(2)
            .filter(|pair| (pair[0] >= 0.0) != (pair[1] >= 0.0))
            .count() as f32;
        // Two crossings per cycle: crossings / duration = 2 * freq.
        let measured_freq = crossings / 1.0;
        assert!(
            (measured_freq - 10.0).abs() < 1.0,
            "crossings {crossings} (expected ~10)"
        );
        let period = 2.0 / crossings;
        assert!((period - 0.2).abs() < 0.02, "period {period}");
    }

    #[test]
    fn all_shapes_stay_bipolar() {
        for shape in [
            LfoShape::Sine,
            LfoShape::Triangle,
            LfoShape::Saw,
            LfoShape::Square,
            LfoShape::SampleHold,
        ] {
            let mut lfo = Lfo::new(shape, 7.3, SR, &mut Qrng::new(2));
            let mut min = f32::INFINITY;
            let mut max = f32::NEG_INFINITY;
            for _ in 0..48_000 {
                let v = lfo.process();
                assert!((-1.0..=1.0).contains(&v), "{shape:?} produced {v}");
                min = min.min(v);
                max = max.max(v);
            }
            assert!(min < -0.5, "{shape:?} min {min}");
            assert!(max > 0.5, "{shape:?} max {max}");
        }
    }

    #[test]
    fn square_lfo_spends_half_the_cycle_on_each_side() {
        let mut lfo = Lfo::new(LfoShape::Square, 10.0, SR, &mut Qrng::new(3));
        let samples: Vec<f32> = (0..4800).map(|_| lfo.process()).collect();
        let positive = samples.iter().filter(|&&s| s > 0.0).count();
        assert!(
            (positive as i32 - 2400).abs() < 10,
            "positive samples {positive}"
        );
    }

    #[test]
    fn sample_hold_changes_value_once_per_cycle() {
        let mut lfo = Lfo::new(LfoShape::SampleHold, 10.0, SR, &mut Qrng::new(4));
        let samples: Vec<f32> = (0..48_000).map(|_| lfo.process()).collect();
        // Exactly 4800 samples per hold at 48 kHz / 10 Hz. Held samples are
        // the same f32 copied, so segment boundaries are compared by bits.
        let mut runs = 1usize;
        for pair in samples.windows(2) {
            if pair[0].to_bits() != pair[1].to_bits() {
                runs += 1;
            }
        }
        assert_eq!(runs, 10, "expected 10 held segments, got {runs}");
        // Values differ between segments (broadband random draws).
        let distinct = samples
            .chunks(4800)
            .map(|chunk| chunk[0])
            .collect::<Vec<f32>>();
        for i in 1..distinct.len() {
            assert_ne!(distinct[i], distinct[i - 1], "segment {i} repeated value");
        }
    }
}
