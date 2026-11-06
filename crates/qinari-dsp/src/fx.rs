//! Effects: delay, reverb, compression, limiting, drive, crush, tremolo.

use crate::buffer::db_to_linear;
use crate::lfo::{Lfo, LfoShape};
use crate::rng::Qrng;
use std::collections::VecDeque;

/// A feedback comb delay: `y[n] = x[n] + feedback * y[n - delay]`.
///
/// `process` returns the full comb output (dry + wet). Delay times are
/// clamped to the allocated maximum and to at least one sample.
#[derive(Debug, Clone)]
pub struct Delay {
    buffer: Vec<f32>,
    sample_rate: f32,
    index: usize,
    line_samples: usize,
    feedback: f32,
}

impl Delay {
    /// Creates a delay with room for `max_secs` and the given feedback
    /// (clamped to `[0, 0.99]`).
    pub fn new(sample_rate: f32, max_secs: f32, feedback: f32) -> Self {
        let len = ((max_secs * sample_rate).ceil() as usize).max(1);
        Self {
            buffer: vec![0.0; len],
            sample_rate,
            index: 0,
            line_samples: len,
            feedback: feedback.clamp(0.0, 0.99),
        }
    }

    /// Sets the delay time in seconds (clamped to `[1 sample, max]`).
    pub fn set_delay_secs(&mut self, secs: f32) {
        let max = self.buffer.len();
        self.line_samples = ((secs * self.sample_rate).round() as usize).clamp(1, max);
    }

    /// Sets the feedback (clamped to `[0, 0.99]`).
    pub fn set_feedback(&mut self, feedback: f32) {
        self.feedback = feedback.clamp(0.0, 0.99);
    }

    /// Reads the delayed sample that the next [`Delay::process`] call
    /// will overwrite, without advancing the line.
    ///
    /// With feedback 0 this is a clean feed-forward tap `line_samples`
    /// behind the write head — the primitive the spatial-reflection
    /// helpers are built on (call it *before* `process` writes the new
    /// sample).
    pub fn tap(&self) -> f32 {
        self.buffer[self.index]
    }

    /// Processes one sample and returns dry + delayed feedback.
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x + self.feedback * self.buffer[self.index];
        self.buffer[self.index] = y;
        self.index += 1;
        if self.index >= self.line_samples {
            self.index = 0;
        }
        y
    }
}

/// One Freeverb comb filter (feedback + one-pole damping).
#[derive(Debug, Clone)]
struct Comb {
    buffer: Vec<f32>,
    index: usize,
    filter_store: f32,
}

impl Comb {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size],
            index: 0,
            filter_store: 0.0,
        }
    }

    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let output = self.buffer[self.index];
        self.filter_store = output * damp + self.filter_store * (1.0 - damp);
        self.buffer[self.index] = input + self.filter_store * feedback;
        self.index += 1;
        if self.index >= self.buffer.len() {
            self.index = 0;
        }
        output
    }
}

/// One Freeverb allpass filter (gain 0.5).
#[derive(Debug, Clone)]
struct Allpass {
    buffer: Vec<f32>,
    index: usize,
}

impl Allpass {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size],
            index: 0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.buffer[self.index];
        let output = -input + buffered;
        self.buffer[self.index] = input + 0.5 * output;
        self.index += 1;
        if self.index >= self.buffer.len() {
            self.index = 0;
        }
        output
    }
}

/// Freeverb-style Schroeder reverb (Jezar at Dreampark's classic tuning).
///
/// Per channel: 8 parallel combs (base tunings 1116, 1188, 1277, 1356,
/// 1422, 1491, 1557, 1617 scaled by `sr / 44100`, feedback 0.84, damping
/// 0.2) feeding 4 serial allpasses (556, 441, 341, 225 scaled, gain 0.5).
/// The right channel's tunings are offset by a 23-sample spread. Input is
/// scaled by Freeverb's fixed 0.015 gain and the output is dry + wet.
#[derive(Debug, Clone)]
pub struct Freeverb {
    combs_l: Vec<Comb>,
    combs_r: Vec<Comb>,
    allpasses_l: Vec<Allpass>,
    allpasses_r: Vec<Allpass>,
}

const COMB_TUNINGS: [f32; 8] = [
    1116.0, 1188.0, 1277.0, 1356.0, 1422.0, 1491.0, 1557.0, 1617.0,
];
const ALLPASS_TUNINGS: [f32; 4] = [556.0, 441.0, 341.0, 225.0];
const STEREO_SPREAD: f32 = 23.0;

impl Freeverb {
    /// Creates the reverb for `sample_rate`.
    pub fn new(sample_rate: f32) -> Self {
        let scale = sample_rate / 44_100.0;
        let scaled = |tuning: f32| ((tuning * scale).round() as usize).max(1);
        let mut combs_l = Vec::with_capacity(8);
        let mut combs_r = Vec::with_capacity(8);
        for &tuning in &COMB_TUNINGS {
            combs_l.push(Comb::new(scaled(tuning)));
            combs_r.push(Comb::new(scaled(tuning + STEREO_SPREAD)));
        }
        let mut allpasses_l = Vec::with_capacity(4);
        let mut allpasses_r = Vec::with_capacity(4);
        for &tuning in &ALLPASS_TUNINGS {
            allpasses_l.push(Allpass::new(scaled(tuning)));
            allpasses_r.push(Allpass::new(scaled(tuning + STEREO_SPREAD)));
        }
        Self {
            combs_l,
            combs_r,
            allpasses_l,
            allpasses_r,
        }
    }

    /// Processes one stereo frame, returning `(dry + wet_l, dry + wet_r)`.
    pub fn process(&mut self, left_in: f32, right_in: f32) -> (f32, f32) {
        const FIXED_GAIN: f32 = 0.015;
        const FEEDBACK: f32 = 0.84;
        const DAMP: f32 = 0.2;
        let input = left_in.midpoint(right_in) * FIXED_GAIN;
        let mut wet_l = 0.0;
        let mut wet_r = 0.0;
        for comb in &mut self.combs_l {
            wet_l += comb.process(input, FEEDBACK, DAMP);
        }
        for comb in &mut self.combs_r {
            wet_r += comb.process(input, FEEDBACK, DAMP);
        }
        for allpass in &mut self.allpasses_l {
            wet_l = allpass.process(wet_l);
        }
        for allpass in &mut self.allpasses_r {
            wet_r = allpass.process(wet_r);
        }
        (left_in + wet_l, right_in + wet_r)
    }
}

/// A feed-forward compressor with a peak envelope follower.
///
/// The sidechain tracks the rectified peak with one-pole attack/release
/// (each segment 95% complete at its nominal duration). Above
/// `threshold_db` the gain is `(x_db - threshold) * (1/ratio - 1)` plus
/// makeup, with `ratio >= 1`.
#[derive(Debug, Clone, Copy)]
pub struct Compressor {
    threshold_db: f32,
    ratio: f32,
    makeup_db: f32,
    attack_coef: f32,
    release_coef: f32,
    envelope: f32,
}

impl Compressor {
    /// Creates a compressor. `ratio` is clamped to at least 1.
    pub fn new(
        sample_rate: f32,
        threshold_db: f32,
        ratio: f32,
        attack_secs: f32,
        release_secs: f32,
        makeup_db: f32,
    ) -> Self {
        let coef = |secs: f32| {
            if secs <= 0.0 {
                1.0
            } else {
                1.0 - (-3.0 / (secs * sample_rate)).exp()
            }
        };
        Self {
            threshold_db,
            ratio: ratio.max(1.0),
            makeup_db,
            attack_coef: coef(attack_secs),
            release_coef: coef(release_secs),
            envelope: 0.0,
        }
    }

    /// Processes one sample and returns the compressed value.
    pub fn process(&mut self, x: f32) -> f32 {
        let rect = x.abs();
        let coef = if rect > self.envelope {
            self.attack_coef
        } else {
            self.release_coef
        };
        self.envelope += (rect - self.envelope) * coef;

        let mut gain_db = self.makeup_db;
        if self.envelope > 1.0e-9 {
            let env_db = 20.0 * self.envelope.log10();
            if env_db > self.threshold_db {
                gain_db += (env_db - self.threshold_db) * (1.0 / self.ratio - 1.0);
            }
        }
        x * db_to_linear(gain_db)
    }
}

/// A stereo-linked lookahead brickwall limiter.
///
/// The signal is delayed by a 5 ms lookahead; the gain target comes from
/// the sliding-window maximum of `max(|l|, |r|)` over the window ending
/// at the current input sample (monotonic deque keyed by absolute sample
/// position), so both channels always receive the same gain. The gain
/// attacks **instantly** and releases in 50 ms (95% at nominal time):
/// the emerging sample is the oldest member of the window, so the full
/// reduction is due immediately — a smoothed attack would leave a
/// level-proportional residual (a +20 dB click escaped a −1.0 ceiling
/// by more than a decibel before the attack was made instant), while
/// the lookahead places the gain dip just ahead of the transient. A
/// final clamp to the ceiling guarantees the brickwall for any input.
/// Note the inherent `lookahead`-sample latency at the output.
#[derive(Debug, Clone)]
pub struct StereoLimiter {
    ceiling: f32,
    lookahead: usize,
    release_coef: f32,
    gain: f32,
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    delay_index: usize,
    position: u64,
    window: VecDeque<(u64, f32)>,
}

impl StereoLimiter {
    /// Creates a limiter with the ceiling given in dBFS (e.g. `-1.0`).
    pub fn new(sample_rate: f32, ceiling_db: f32) -> Self {
        let lookahead = ((0.005 * sample_rate).round() as usize).max(1);
        let release_coef = 1.0 - (-3.0 / (0.050 * sample_rate)).exp();
        Self {
            ceiling: db_to_linear(ceiling_db),
            lookahead,
            release_coef,
            gain: 1.0,
            delay_l: vec![0.0; lookahead],
            delay_r: vec![0.0; lookahead],
            delay_index: 0,
            position: 0,
            window: VecDeque::new(),
        }
    }

    /// Processes one stereo frame with a single linked gain.
    pub fn process(&mut self, left_in: f32, right_in: f32) -> (f32, f32) {
        let peak = left_in.abs().max(right_in.abs());

        while let Some(&(_, back_peak)) = self.window.back() {
            if back_peak <= peak {
                self.window.pop_back();
            } else {
                break;
            }
        }
        self.window.push_back((self.position, peak));
        let window_start = self.position.saturating_sub(self.lookahead as u64);
        while let Some(&(front_pos, _)) = self.window.front() {
            if front_pos < window_start {
                self.window.pop_front();
            } else {
                break;
            }
        }

        let target = match self.window.front() {
            Some(&(_, max_peak)) if max_peak > 1.0e-9 => (self.ceiling / max_peak).min(1.0),
            _ => 1.0,
        };
        if target < self.gain {
            // Instant attack: see the struct documentation. The window
            // already holds the transient this reduction must catch.
            self.gain = target;
        } else {
            self.gain += (target - self.gain) * self.release_coef;
        }

        let out_l = self.delay_l[self.delay_index];
        let out_r = self.delay_r[self.delay_index];
        self.delay_l[self.delay_index] = left_in;
        self.delay_r[self.delay_index] = right_in;
        self.delay_index += 1;
        if self.delay_index >= self.lookahead {
            self.delay_index = 0;
        }
        self.position += 1;
        (
            (out_l * self.gain).clamp(-self.ceiling, self.ceiling),
            (out_r * self.gain).clamp(-self.ceiling, self.ceiling),
        )
    }
}

/// A `tanh` waveshaper.
///
/// Output is `tanh(drive * x) / tanh(drive)`, which maps `[-1, 1]` onto
/// `[-1, 1]`, stays near-linear for small drives and saturates hard for
/// large ones. `drive` is clamped to at least 0.01.
#[derive(Debug, Clone, Copy)]
pub struct Drive {
    drive: f32,
}

impl Drive {
    /// Creates the shaper with the given drive amount.
    pub fn new(drive: f32) -> Self {
        Self {
            drive: drive.max(0.01),
        }
    }

    /// Sets the drive amount (clamped to at least 0.01).
    pub fn set_drive(&mut self, drive: f32) {
        self.drive = drive.max(0.01);
    }

    /// Processes one sample.
    pub fn process(&mut self, x: f32) -> f32 {
        (self.drive * x).tanh() / self.drive.tanh()
    }
}

/// A bit-crusher: quantizes to `2^bits` steps over `[-1, 1]`.
///
/// `bits` is clamped to `[1, 24]`; fractional bit depths are allowed.
#[derive(Debug, Clone, Copy)]
pub struct Crush {
    levels: f32,
}

impl Crush {
    /// Creates the crusher quantizing to `bits` bits.
    pub fn new(bits: f32) -> Self {
        Self {
            levels: 2.0_f32.powf(bits.clamp(1.0, 24.0) - 1.0),
        }
    }

    /// Processes one sample.
    pub fn process(&mut self, x: f32) -> f32 {
        let step = (x * self.levels).round();
        let step = step.clamp(-self.levels, self.levels - 1.0);
        step / self.levels
    }
}

/// An LFO-modulated amplitude tremolo.
///
/// The gain is `1 - depth * (1 - mod)` where `mod` is the 0..1 LFO
/// envelope, so `depth = 1` modulates down to silence and `depth = 0`
/// is bypass.
#[derive(Debug, Clone)]
pub struct Tremolo {
    lfo: Lfo,
    depth: f32,
}

impl Tremolo {
    /// Creates a tremolo at `rate_hz` with `depth` in `[0, 1]` (clamped).
    pub fn new(rate_hz: f32, depth: f32, sample_rate: f32, rng: &mut Qrng) -> Self {
        Self {
            lfo: Lfo::new(LfoShape::Sine, rate_hz, sample_rate, rng),
            depth: depth.clamp(0.0, 1.0),
        }
    }

    /// Processes one sample.
    pub fn process(&mut self, x: f32) -> f32 {
        let modulation = self.lfo.process() * 0.5 + 0.5;
        x * (1.0 - self.depth * (1.0 - modulation))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::goertzel_magnitude;

    const SR: f32 = 48_000.0;

    fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin())
            .collect()
    }

    fn steady_amplitude(samples: &[f32], freq: f32) -> f32 {
        let half = samples.len() / 2;
        goertzel_magnitude(&samples[half..], freq, SR)
    }

    #[test]
    fn delay_feedback_loop_decays_at_the_feedback_rate() {
        let mut delay = Delay::new(1000.0, 0.5, 0.5);
        delay.set_delay_secs(0.1);
        let mut out = vec![0.0f32; 500];
        out[0] = delay.process(1.0);
        for slot in out.iter_mut().skip(1) {
            *slot = delay.process(0.0);
        }
        assert!((out[0] - 1.0).abs() < 1e-6);
        assert!((out[100] - 0.5).abs() < 1e-5, "echo 1 {}", out[100]);
        assert!((out[200] - 0.25).abs() < 1e-5, "echo 2 {}", out[200]);
        assert!((out[300] - 0.125).abs() < 1e-5, "echo 3 {}", out[300]);
        assert!((out[400] - 0.0625).abs() < 1e-5, "echo 4 {}", out[400]);
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn delay_tap_reads_the_delayed_sample_without_advancing() {
        // 0.01 s at 1 kHz is a 10-sample line.
        let mut delay = Delay::new(1000.0, 0.03, 0.0);
        delay.set_delay_secs(0.01);
        assert_eq!(delay.tap(), 0.0, "empty line reads silence");
        delay.process(1.0);
        delay.process(0.5);
        // Only two writes: the tap slot still holds silence.
        assert_eq!(delay.tap(), 0.0);
        for i in 0..12 {
            delay.process(i as f32);
        }
        // 14 writes total (inputs 1.0, 0.5, then 0..11); the write head
        // sits at ring index 4, so the tap holds the input of call #4,
        // which was 2.0 — the sample written 10 calls ago.
        assert_eq!(delay.tap(), 2.0, "tap holds the 10-sample-old input");
    }

    #[test]
    fn delay_clamps_to_maximum() {
        let mut delay = Delay::new(100.0, 0.01, 0.5); // 1 sample max
        delay.set_delay_secs(5.0);
        let y = delay.process(1.0);
        assert!((y - 1.0).abs() < 1e-6, "dry passthrough {y}");
        let y2 = delay.process(0.0);
        assert!((y2 - 0.5).abs() < 1e-5, "one-sample echo {y2}");
    }

    #[test]
    fn freeverb_impulse_response_decays_to_zero() {
        let mut reverb = Freeverb::new(SR);
        let total = 144_000; // 3 s
        let mut left = Vec::with_capacity(total);
        let mut right = Vec::with_capacity(total);
        for i in 0..total {
            let (l, r) = if i == 0 {
                reverb.process(1.0, 1.0)
            } else {
                reverb.process(0.0, 0.0)
            };
            left.push(l);
            right.push(r);
        }
        assert!(left.iter().all(|s| s.is_finite()));
        assert!(right.iter().all(|s| s.is_finite()));

        let rms = |slice: &[f32]| {
            (slice.iter().map(|&s| f64::from(s * s)).sum::<f64>() / slice.len() as f64).sqrt()
        };
        let head = rms(&left[..48_000]);
        let tail = rms(&left[96_000..]);
        // The dry impulse alone contributes 1/sqrt(48000) = 0.00456 and the
        // wet adds only a few percent, so 0.005 was beyond the physics.
        assert!(head > 0.004, "early IR rms {head}");
        assert!(tail < 1.0e-5, "late IR rms {tail}");
        // Stereo spread decorrelates the channels.
        let spread: f64 = left
            .iter()
            .zip(&right)
            .map(|(&l, &r)| f64::from(l - r) * f64::from(l - r))
            .sum();
        assert!(spread > 0.0, "channels are identical");
    }

    #[test]
    fn compressor_reduces_dynamic_range_above_threshold() {
        let render = |amp: f32| {
            let mut comp = Compressor::new(SR, -12.0, 4.0, 0.005, 0.1, 0.0);
            let input = sine(220.0, amp, 19_200); // 0.4 s
            let out: Vec<f32> = input.iter().map(|&x| comp.process(x)).collect();
            steady_amplitude(&out, 220.0)
        };
        let quiet = render(10.0_f32.powf(-20.0 / 20.0)); // -20 dBFS
        let mid = render(10.0_f32.powf(-10.0 / 20.0)); // -10 dBFS
        let loud = render(10.0_f32.powf(-4.0 / 20.0)); // -4 dBFS

        // Below threshold: near unity gain.
        let quiet_in = 10.0_f32.powf(-20.0 / 20.0);
        assert!(
            (20.0 * (quiet / quiet_in).log10()).abs() < 1.5,
            "below-threshold gain {} dB",
            20.0 * (quiet / quiet_in).log10()
        );

        let input_range_db =
            20.0 * (10.0_f32.powf(-4.0 / 20.0) / 10.0_f32.powf(-10.0 / 20.0)).log10();
        let output_range_db = 20.0 * (loud / mid).log10();
        assert_eq!(input_range_db, 6.0);
        assert!(
            output_range_db < 3.0,
            "output dynamic range {output_range_db} dB (expected ~1.5)"
        );
        assert!(
            input_range_db - output_range_db > 2.5,
            "compression was only {} dB",
            input_range_db - output_range_db
        );
        // The loud tone is actually pushed down.
        let loud_in = 10.0_f32.powf(-4.0 / 20.0);
        assert!(
            loud < loud_in * 0.8,
            "loud output {loud} vs input {loud_in}"
        );
    }

    #[test]
    fn limiter_holds_ceiling_on_full_scale_sine() {
        let ceiling = db_to_linear(-1.0);
        let mut limiter = StereoLimiter::new(SR, -1.0);
        let input = sine(1000.0, 1.0, 96_000);
        let mut peak = 0.0f32;
        for &x in &input {
            let (l, r) = limiter.process(x, x);
            assert!(l.is_finite() && r.is_finite());
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(
            peak <= ceiling * 1.02,
            "output peak {peak} vs ceiling {ceiling}"
        );
        assert!(peak > ceiling * 0.9, "output peak {peak} too quiet");
    }

    #[test]
    fn limiter_holds_ceiling_on_a_single_impulse() {
        // Regression: a full-scale single-sample transient at a released
        // gain must still emerge at or below the ceiling. The 5 ms
        // attack ramp alone cannot guarantee that (it is only ~95%
        // settled when the impulse leaves the lookahead window), so the
        // limiter needs a hard ceiling stage for the residual.
        let ceiling = db_to_linear(-1.0);
        let mut limiter = StereoLimiter::new(SR, -1.0);
        let mut peak = 0.0f32;
        for i in 0..48_000 {
            let x = if i == 1_000 { 1.0f32 } else { 0.0 };
            let (l, r) = limiter.process(x, x);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(
            peak <= ceiling * 1.001,
            "impulse escaped the ceiling: {peak} > {ceiling}"
        );
    }

    #[test]
    fn limiter_holds_ceiling_on_a_fast_attack_click() {
        // A 1 ms attack riding a 2 kHz carrier — the shape that made
        // mastered UI clicks overshoot the documented ceiling.
        let ceiling = db_to_linear(-1.0);
        let mut limiter = StereoLimiter::new(SR, -1.0);
        let mut peak = 0.0f32;
        let mut env = 0.0f32;
        for i in 0..48_000 {
            env = if i < 48 {
                i as f32 / 48.0
            } else {
                env * 0.9995
            };
            let x = env * (2.0 * std::f32::consts::PI * 2000.0 * i as f32 / SR).sin();
            let (l, r) = limiter.process(x, x);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(
            peak <= ceiling * 1.001,
            "click escaped the ceiling: {peak} > {ceiling}"
        );
    }

    #[test]
    fn limiter_is_stereo_linked() {
        let mut limiter = StereoLimiter::new(SR, -1.0);
        let n = 48_000;
        let mut out_l = Vec::with_capacity(n);
        let mut out_r = Vec::with_capacity(n);
        for i in 0..n {
            let l = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / SR).sin();
            let r = 0.316 * l; // -10 dB relative, same phase
            let (ol, or) = limiter.process(l, r);
            out_l.push(ol);
            out_r.push(or);
        }
        let amp_l = steady_amplitude(&out_l, 1000.0);
        let amp_r = steady_amplitude(&out_r, 1000.0);
        let gain_l = amp_l / 1.0;
        let gain_r = amp_r / 0.316;
        assert!(
            (gain_l - gain_r).abs() < 0.01 * gain_l,
            "gains differ: {gain_l} vs {gain_r}"
        );
        assert!(gain_l < 1.0, "limiter did not engage");
    }

    #[test]
    fn drive_adds_harmonics_and_stays_bounded() {
        let mut drive = Drive::new(0.1);
        let input = sine(200.0, 0.5, 48_000);
        let out: Vec<f32> = input.iter().map(|&x| drive.process(x)).collect();
        let fundamental = steady_amplitude(&out, 200.0);
        let third = steady_amplitude(&out, 600.0);
        assert!((fundamental - 0.5 * 0.1_f32.tanh().recip() * 0.1_f32).abs() < 0.05);
        assert!(third < 0.01, "low drive must stay clean, h3 {third}");

        let mut drive = Drive::new(5.0);
        let out: Vec<f32> = input.iter().map(|&x| drive.process(x)).collect();
        let third = steady_amplitude(&out, 600.0);
        let fifth = steady_amplitude(&out, 1000.0);
        assert!(third > 0.02, "high drive h3 {third}");
        assert!(fifth > 0.005, "high drive h5 {fifth}");
        let extreme = drive.process(50.0);
        assert!(extreme.abs() <= 1.001, "extreme input produced {extreme}");
    }

    #[test]
    fn crush_quantizes_to_discrete_steps() {
        let mut crush = Crush::new(3.0);
        let ramp: Vec<f32> = (0..2000).map(|i| -1.0 + 2.0 * i as f32 / 2000.0).collect();
        let out: Vec<f32> = ramp.iter().map(|&x| crush.process(x)).collect();
        for &sample in &out {
            let steps = sample * 4.0;
            assert!(
                (steps - steps.round()).abs() < 1.0e-5,
                "value {sample} off-grid"
            );
            assert!(
                (-1.0..=0.75).contains(&sample),
                "value {sample} out of range"
            );
        }
        let distinct = {
            let mut values: Vec<f32> = out.clone();
            values.dedup();
            values.len()
        };
        assert!(distinct <= 8, "3-bit crush produced {distinct} levels");

        // 16-bit crush on a quiet signal stays finite and near-identity.
        let mut crush = Crush::new(16.0);
        let y = crush.process(0.123_456);
        assert!((y - 0.123_456).abs() < 1.0 / 32_000.0);
    }

    #[test]
    fn tremolo_modulates_amplitude_at_rate() {
        let mut tremolo = Tremolo::new(5.0, 1.0, SR, &mut Qrng::new(15));
        let out: Vec<f32> = (0..48_000).map(|_| tremolo.process(1.0)).collect();
        let max = out.iter().fold(0.0f32, |m, &s| m.max(s));
        let min = out.iter().fold(1.0f32, |m, &s| m.min(s));
        assert!(max > 0.98, "tremolo max {max}");
        assert!(min < 0.02, "tremolo min {min}");
        // The envelope crosses half amplitude twice per cycle.
        let crossings = out
            .windows(2)
            .filter(|pair| (pair[0] - 0.5 >= 0.0) != (pair[1] - 0.5 >= 0.0))
            .count();
        assert!(
            (8..=12).contains(&crossings),
            "envelope crossings {crossings} (expected ~10)"
        );

        // Depth 0 is a bypass.
        let mut tremolo = Tremolo::new(5.0, 0.0, SR, &mut Qrng::new(15));
        let bypassed: Vec<f32> = (0..100).map(|_| tremolo.process(0.7)).collect();
        assert!(bypassed.iter().all(|&s| (s - 0.7).abs() < 1e-6));
    }
}
