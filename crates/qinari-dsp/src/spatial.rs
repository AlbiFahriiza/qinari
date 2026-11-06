//! Spatial realism: bed decorrelation, distance, early reflections.
//!
//! Three structural tells this module removes
//! (`RESEARCH-AUDIO-REALISM.md` §R 1.2-5, §R 2.6):
//!
//! - mono-duplicated beds (interaural correlation 1.0 — listeners
//!   discriminate departures from it, Gabriel & Colburn 1981):
//!   [`decorrelate`] keeps the shared low band and replaces the high
//!   band with level-matched independent noise per channel;
//! - anechoic one-shots: [`DistanceFilter`] applies the propagation
//!   laws (delay ∝ d, gain ∝ 1/d, HF rolloff growing with d) and
//!   [`EarlyReflections`] adds up to a few feed-forward taps through
//!   the existing [`crate::fx::Delay`] line.

use crate::buffer::AudioBuffer;
use crate::filter::OnePole;
use crate::fx::Delay;
use crate::noise::{NoiseGen, NoiseKind};
use crate::rng::Qrng;

/// Per-channel state of a [`StereoDecorrelator`].
#[derive(Debug, Clone)]
struct ChannelState {
    lowpass: OnePole,
    noise: NoiseGen,
    /// Two cascaded highpasses: the replacement noise must not leak
    /// back below the corner (a one-pole transition would keep 0.5
    /// amplitude an octave down and erode the correlated low band).
    noise_highpass: [OnePole; 2],
    input_ms: f32,
    noise_ms: f32,
}

impl ChannelState {
    fn new(corner_hz: f32, sample_rate: f32, rng: &mut Qrng) -> Self {
        Self {
            lowpass: OnePole::new(sample_rate, corner_hz),
            noise: NoiseGen::new(NoiseKind::Pink, rng.fork()),
            noise_highpass: [
                OnePole::new_highpass(sample_rate, corner_hz),
                OnePole::new_highpass(sample_rate, corner_hz),
            ],
            input_ms: 0.0,
            noise_ms: 0.0,
        }
    }
}

/// Replaces a stereo bed's high band with independent, level-matched
/// noise per channel.
///
/// Below `corner_hz` both channels keep their shared signal (a bed's
/// low frequencies are physically correlated at the two ears); above it
/// each channel carries its own pink noise whose slowly-tracked power
/// matches the input's high band, so the timbre and level survive while
/// the interaural correlation above the corner collapses toward zero.
/// Silence in produces silence out.
///
/// The corner should sit near 500 Hz (the design's acceptance band);
/// higher corners leave more of the tell audible.
#[derive(Debug, Clone)]
pub struct StereoDecorrelator {
    left: ChannelState,
    right: ChannelState,
    smooth: f32,
}

impl StereoDecorrelator {
    /// Creates the decorrelator; the two noise streams fork from `rng`.
    pub fn new(corner_hz: f32, sample_rate: f32, rng: &mut Qrng) -> Self {
        // ~40 ms power tracking: fast enough to follow a bed's dynamics,
        // slow enough not to track the carrier itself.
        let smooth = 1.0 - (-1.0 / (0.040 * sample_rate)).exp();
        Self {
            left: ChannelState::new(corner_hz, sample_rate, rng),
            right: ChannelState::new(corner_hz, sample_rate, rng),
            smooth,
        }
    }

    /// Processes one stereo frame.
    pub fn process(&mut self, left_in: f32, right_in: f32) -> (f32, f32) {
        (
            Self::channel(&mut self.left, left_in, self.smooth),
            Self::channel(&mut self.right, right_in, self.smooth),
        )
    }

    /// One channel of the split-and-replace operation.
    fn channel(state: &mut ChannelState, x: f32, smooth: f32) -> f32 {
        let low = state.lowpass.process(x);
        let high = x - low;
        let raw = state.noise.process();
        let first = state.noise_highpass[0].process(raw);
        let noise = state.noise_highpass[1].process(first);
        state.input_ms += (high * high - state.input_ms) * smooth;
        state.noise_ms += (noise * noise - state.noise_ms) * smooth;
        let gain = if state.noise_ms > 1.0e-9 {
            (state.input_ms / state.noise_ms).sqrt()
        } else {
            0.0
        };
        low + noise * gain
    }
}

/// Decorrelates the first two channels of `buffer` in place.
///
/// Mono (or empty) buffers are returned unchanged — there is nothing to
/// decorrelate. All randomness forks from `rng`, so the operation is a
/// pure function of the seed.
pub fn decorrelate(buffer: &mut AudioBuffer, corner_hz: f32, sample_rate: f32, rng: &mut Qrng) {
    if buffer.num_channels() < 2 {
        return;
    }
    let mut decorrelator = StereoDecorrelator::new(corner_hz, sample_rate, rng);
    buffer.map_stereo(|l, r| decorrelator.process(l, r));
}

/// Speed of sound in air at room temperature, metres per second.
const SPEED_OF_SOUND: f32 = 343.0;

/// Reference distance of the 1/d gain law, metres.
const GAIN_REFERENCE_M: f32 = 5.0;

/// Free-field propagation model: delay, 1/d gain, and air absorption.
///
/// The three qualitative laws of `DESIGN-REALISM-ENGINE.md` §7/§8:
///
/// - delay `d / 343 s` (a look-back delay line, so the filter is
///   causal);
/// - gain `1 / (1 + d / 5 m)` — the 1/d law pinned to unity at the
///   source and -6 dB at 5 m;
/// - a two-pole HF rolloff whose corner falls superlinearly with
///   distance: `12000 / (1 + (d/40 m)^1.7) Hz`, clamped to
///   `[200, 12000]`. The exact ISO 9613-1 curve is unverified for this
///   design, so only the monotonic law is pinned.
///
/// Process with [`DistanceFilter::process`]; the per-sample cost is
/// O(1).
#[derive(Debug, Clone)]
pub struct DistanceFilter {
    delay: Delay,
    lowpass: [OnePole; 2],
    gain: f32,
}

impl DistanceFilter {
    /// Builds the model for a source `meters` away (clamped to >= 0).
    pub fn from_meters(sample_rate: f32, meters: f32) -> Self {
        let meters = meters.max(0.0);
        let delay_secs = meters / SPEED_OF_SOUND;
        let corner = (12_000.0 / (1.0 + (meters / 40.0).powf(1.7))).clamp(200.0, 12_000.0);
        let mut delay = Delay::new(sample_rate, delay_secs + 0.01, 0.0);
        delay.set_delay_secs(delay_secs);
        Self {
            delay,
            lowpass: [
                OnePole::new(sample_rate, corner),
                OnePole::new(sample_rate, corner),
            ],
            gain: 1.0 / (1.0 + meters / GAIN_REFERENCE_M),
        }
    }

    /// Builds the model from a normalized distance in `[0, 1]` (values
    /// outside are clamped), mapped to `d * 100 m`. This is the scale
    /// the recipes' `distance` parameters use.
    pub fn from_normalized(sample_rate: f32, distance: f32) -> Self {
        Self::from_meters(sample_rate, distance.clamp(0.0, 1.0) * 100.0)
    }

    /// Processes one sample.
    pub fn process(&mut self, x: f32) -> f32 {
        // Read the tap before writing: buffer[index] holds the sample
        // from `line_samples` process calls ago.
        let delayed = self.delay.tap();
        self.delay.process(x);
        let first = self.lowpass[0].process(delayed);
        let absorbed = self.lowpass[1].process(first);
        absorbed * self.gain
    }
}

/// A small bank of feed-forward reflection taps built on [`Delay`].
///
/// Each tap is a `(delay_secs, gain)` pair; the output is the input
/// plus every delayed copy at its gain. Ground bounces, first-order
/// room reflections, Mach-stem sums — anywhere 1–4 discrete arrivals
/// are needed.
#[derive(Debug, Clone)]
pub struct EarlyReflections {
    lines: Vec<Delay>,
    gains: Vec<f32>,
}

impl EarlyReflections {
    /// Builds the tap bank; taps are used in the order given and a
    /// maximum of 8 are kept (the rest are ignored — the design caps
    /// discrete arrivals at four).
    pub fn new(sample_rate: f32, taps: &[(f32, f32)]) -> Self {
        let mut lines = Vec::with_capacity(taps.len());
        let mut gains = Vec::with_capacity(taps.len());
        for &(delay_secs, gain) in taps.iter().take(8) {
            let mut line = Delay::new(sample_rate, delay_secs.max(0.001) + 0.01, 0.0);
            line.set_delay_secs(delay_secs.max(0.001));
            lines.push(line);
            gains.push(gain);
        }
        Self { lines, gains }
    }

    /// Processes one sample: dry input plus every tap.
    pub fn process(&mut self, x: f32) -> f32 {
        let mut out = x;
        for (line, &gain) in self.lines.iter_mut().zip(self.gains.iter()) {
            let tap = line.tap();
            line.process(x);
            out += tap * gain;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Qrng;

    const SR: f32 = 48_000.0;

    /// Pearson correlation of two slices.
    fn correlation(left: &[f32], right: &[f32]) -> f64 {
        let (mut cross, mut left_sq, mut right_sq) = (0.0f64, 0.0f64, 0.0f64);
        for (&l, &r) in left.iter().zip(right) {
            let lv = f64::from(l);
            let rv = f64::from(r);
            cross += lv * rv;
            left_sq += lv * lv;
            right_sq += rv * rv;
        }
        let den = (left_sq * right_sq).sqrt();
        if den <= 0.0 {
            0.0
        } else {
            (cross / den).clamp(-1.0, 1.0)
        }
    }

    fn rms(samples: &[f32]) -> f64 {
        (samples.iter().map(|&s| f64::from(s * s)).sum::<f64>() / samples.len() as f64).sqrt()
    }

    #[test]
    fn decorrelator_kills_hf_correlation_and_keeps_lf() {
        // Identical L/R pink noise bed, the interaural-1.0 tell.
        let frames = 96_000; // 2 s
        let mut bed = NoiseGen::new(NoiseKind::Pink, Qrng::new(61));
        let mono: Vec<f32> = (0..frames).map(|_| bed.process() * 0.3).collect();
        let buffer = AudioBuffer::from_channels(vec![mono.clone(), mono.clone()], SR as u32)
            .expect("well formed");

        let mut decorrelated = buffer.clone();
        decorrelate(&mut decorrelated, 500.0, SR, &mut Qrng::new(62));

        let left = decorrelated.channels()[0].clone();
        let right = decorrelated.channels()[1].clone();
        // Above the corner the channels must be uncorrelated.
        let hp = |samples: &[f32]| -> Vec<f32> {
            let mut filter = OnePole::new_highpass(SR, 500.0);
            samples.iter().map(|&s| filter.process(s)).collect()
        };
        let hf_corr = correlation(&hp(&left), &hp(&right));
        assert!(hf_corr < 0.2, "HF correlation {hf_corr}");
        // Below the corner they stay the shared bed (identical input).
        let lp = |samples: &[f32]| -> Vec<f32> {
            let mut filter = OnePole::new(SR, 300.0);
            samples.iter().map(|&s| filter.process(s)).collect()
        };
        let lf_corr = correlation(&lp(&left), &lp(&right));
        assert!(lf_corr > 0.95, "LF correlation {lf_corr}");
        // Level survives: output RMS within ~2 dB of the input.
        let in_rms = rms(&mono);
        let out_rms = rms(&left);
        assert!(
            out_rms > 0.6 * in_rms && out_rms < 1.6 * in_rms,
            "rms in {in_rms} vs out {out_rms}"
        );
        assert!(decorrelated
            .channels()
            .iter()
            .flatten()
            .all(|s| s.is_finite()));
    }

    #[test]
    fn decorrelator_passes_silence_and_ignores_mono() {
        let mut silent =
            AudioBuffer::from_channels(vec![vec![0.0; 48_000], vec![0.0; 48_000]], SR as u32)
                .expect("well formed");
        decorrelate(&mut silent, 500.0, SR, &mut Qrng::new(63));
        assert_eq!(silent.peak(), 0.0, "silence must stay silent");

        let mut mono =
            AudioBuffer::from_channels(vec![vec![0.5; 100]], SR as u32).expect("well formed");
        decorrelate(&mut mono, 500.0, SR, &mut Qrng::new(63));
        assert_eq!(mono.channels()[0], vec![0.5; 100], "mono untouched");
    }

    #[test]
    fn decorrelator_is_deterministic() {
        let render = |seed: u64| {
            let mut bed = NoiseGen::new(NoiseKind::Pink, Qrng::new(65));
            let mono: Vec<f32> = (0..24_000).map(|_| bed.process()).collect();
            let mut buffer =
                AudioBuffer::from_channels(vec![mono.clone(), mono], SR as u32).expect("ok");
            decorrelate(&mut buffer, 500.0, SR, &mut Qrng::new(seed));
            buffer
        };
        let a = render(66);
        let b = render(66);
        assert_eq!(a.channels()[0], b.channels()[0]);
        assert_eq!(a.channels()[1], b.channels()[1]);
        let c = render(67);
        assert_ne!(a.channels()[0], c.channels()[0]);
    }

    #[test]
    fn distance_filter_delays_attenuates_and_lowpasses() {
        // An impulse through 34.3 m of air: 0.1 s delay, gain 1/(1+34.3/5)
        // = 0.127, and the HF rolled off. Compare a near (1 m) and far
        // (80 m) render of the same noise burst.
        let burst: Vec<f32> = {
            let mut rng = Qrng::new(71);
            (0..4800).map(|_| rng.next_f32() * 2.0 - 1.0).collect()
        };

        let run = |meters: f32| -> Vec<f32> {
            let mut filter = DistanceFilter::from_meters(SR, meters);
            let mut out = Vec::with_capacity(4800 + 9600);
            for &sample in &burst {
                out.push(filter.process(sample));
            }
            for _ in 0..9600 {
                out.push(filter.process(0.0));
            }
            out
        };

        // Delay: the output starts (near) silent while the line fills.
        let far = run(34.3);
        let head = rms(&far[..4_500]); // first 93.75 ms < 100 ms delay
        let body = rms(&far[5_500..]);
        assert!(head < 0.05 * body, "head rms {head} vs body {body}");

        // Monotonic distance laws.
        let near = DistanceFilter::from_meters(SR, 1.0);
        let mid = DistanceFilter::from_meters(SR, 40.0);
        let far80 = DistanceFilter::from_meters(SR, 80.0);
        let gains = [near.gain, mid.gain, far80.gain];
        assert!(
            gains[0] > gains[1] && gains[1] > gains[2] && gains[2] > 0.0,
            "gain must fall with distance: {gains:?}"
        );

        // HF content falls monotonically with distance: run white noise
        // through each and compare the 5 kHz goertzel power.
        let hf_power = |meters: f32| {
            let mut filter = DistanceFilter::from_meters(SR, meters);
            let mut rng = Qrng::new(73);
            let samples: Vec<f32> = (0..24_000)
                .map(|_| filter.process(rng.next_f32() * 2.0 - 1.0))
                .collect();
            crate::analysis::goertzel_power(&samples[4_800..], 5000.0, SR)
        };
        let near_hf = hf_power(1.0);
        let mid_hf = hf_power(40.0);
        let far_hf = hf_power(80.0);
        assert!(
            near_hf > 2.0 * mid_hf && mid_hf > 2.0 * far_hf,
            "HF must roll off with distance: {near_hf} / {mid_hf} / {far_hf}"
        );
    }

    #[test]
    fn distance_filter_from_normalized_maps_to_hundred_meters() {
        let near = DistanceFilter::from_normalized(SR, 0.0);
        let far = DistanceFilter::from_normalized(SR, 1.0);
        assert!((near.gain - 1.0).abs() < 1e-6, "zero distance is unity");
        // 100 m: gain 1/21, corner ~12000/(1+ (2.5)^1.7) ~= 1843 -> clamped path
        assert!((far.gain - 1.0 / 21.0).abs() < 1e-6);
    }

    #[test]
    fn early_reflections_place_taps_at_their_delays_and_gains() {
        let mut reflections =
            EarlyReflections::new(SR, &[(0.010, 0.5), (0.0237, -0.25), (0.05, 0.125)]);
        // Impulse: dry at 0, taps at 480, 1137 (0.0237*48000 rounds to
        // 1138; Delay rounds to nearest), 2400.
        let mut out = vec![0.0f32; 3_000];
        out[0] = reflections.process(1.0);
        for slot in out.iter_mut().skip(1) {
            *slot = reflections.process(0.0);
        }
        assert!((out[0] - 1.0).abs() < 1e-6, "dry passthrough");
        assert!((out[480] - 0.5).abs() < 1e-5, "tap 1: {}", out[480]);
        let tap2 = (0.0237 * SR).round() as usize;
        assert!((out[tap2] - (-0.25)).abs() < 1e-5, "tap 2: {}", out[tap2]);
        assert!((out[2400] - 0.125).abs() < 1e-5, "tap 3: {}", out[2400]);
        // Everything else silent.
        let elsewhere: f32 = out
            .iter()
            .enumerate()
            .filter(|(i, _)| ![0usize, 480, tap2, 2400].contains(i))
            .map(|(_, v)| v.abs())
            .sum();
        assert!(elsewhere < 1e-6, "stray energy {elsewhere}");
    }

    #[test]
    fn early_reflections_empty_bank_is_passthrough() {
        let mut reflections = EarlyReflections::new(SR, &[]);
        for i in 0..100 {
            let x = (i as f32 * 0.1).sin();
            assert_eq!(reflections.process(x), x);
        }
    }
}
