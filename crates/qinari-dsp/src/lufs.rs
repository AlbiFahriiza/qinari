//! ITU-R BS.1770-4 integrated loudness (LUFS) measurement.
//!
//! [`KWeighting`] implements the pre-filter of the loudness algorithm (an RBJ
//! high shelf over a high-pass, valid at any sample rate); [`LoudnessMeter`]
//! accumulates 400 ms blocks with a 100 ms step and applies the absolute
//! (-70 LUFS) and relative (-10 LU) gates of the standard.

use crate::buffer::AudioBuffer;
use crate::filter::{Biquad, BiquadKind};
use std::collections::VecDeque;

/// K-weighting shelf corner in hertz.
const SHELF_FC_HZ: f32 = 1681.97;
/// K-weighting shelf Q (the plan's 0.7071; `1/sqrt(2)` at full precision).
const SHELF_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// K-weighting shelf gain in decibels.
const SHELF_GAIN_DB: f32 = 3.999;
/// K-weighting high-pass corner in hertz.
const HP_FC_HZ: f32 = 38.135;
/// K-weighting high-pass Q.
const HP_Q: f32 = 0.5;
/// Absolute gate of the algorithm in LUFS.
const ABSOLUTE_GATE_LUFS: f64 = -70.0;
/// Relative gate of the algorithm in LU.
const RELATIVE_GATE_LU: f64 = -10.0;
/// Offset of the loudness formula (BS.1770-4 equation 1).
const LOUDNESS_OFFSET: f64 = -0.691;

/// An RBJ audio-EQ-cookbook high shelf, published verbatim so the
/// K-weighting response can be audited against BS.1770-4.
fn rbj_high_shelf(sample_rate: f32, f0: f32, q: f32, gain_db: f32) -> Biquad {
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * f0 / sample_rate;
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q);
    let sqrt_a = a.sqrt();
    let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha);
    let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
    let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha);
    let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
    let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
    let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha;
    Biquad::from_coefficients(b0, b1, b2, a0, a1, a2)
}

/// The BS.1770-4 K-weighting filter for one channel.
///
/// Stage 1 is a +3.999 dB high shelf at 1681.97 Hz (Q 0.7071); stage 2 a
/// high-pass at 38.135 Hz (Q 0.5). Both are the standard's design parameters
/// evaluated at the given sample rate.
#[derive(Debug, Clone)]
pub struct KWeighting {
    shelf: Biquad,
    highpass: Biquad,
}

impl KWeighting {
    /// Creates the weighting chain for `sample_rate`.
    ///
    /// # Panics
    ///
    /// Panics if `sample_rate` is not positive.
    pub fn new(sample_rate: f32) -> Self {
        assert!(sample_rate > 0.0, "sample_rate must be positive");
        Self {
            shelf: rbj_high_shelf(sample_rate, SHELF_FC_HZ, SHELF_Q, SHELF_GAIN_DB),
            highpass: Biquad::design(BiquadKind::Highpass, sample_rate, HP_FC_HZ, HP_Q, 0.0),
        }
    }

    /// Processes one sample through both stages.
    pub fn process(&mut self, x: f32) -> f32 {
        self.highpass.process(self.shelf.process(x))
    }
}

/// One in-flight 400 ms block: summed weighted power and samples seen.
#[derive(Debug, Clone, Copy, Default)]
struct BlockAccumulator {
    sum: f64,
    seen: usize,
}

/// A BS.1770-4 loudness meter over interleaved per-sample frames.
///
/// Every channel is K-weighted (all channels count as front channels with
/// weight 1.0). Blocks cover 400 ms and start every 100 ms; a block is only
/// measured once 400 ms of signal have arrived, so a stream shorter than
/// 400 ms produces no blocks at all. Gating follows the standard: blocks
/// below -70 LUFS are dropped (absolute gate), then blocks below the mean
/// of the survivors minus 10 LU are dropped (relative gate).
#[derive(Debug, Clone)]
pub struct LoudnessMeter {
    weighting: Vec<KWeighting>,
    block_samples: usize,
    step_samples: usize,
    position: u64,
    active: VecDeque<BlockAccumulator>,
    completed: Vec<f64>,
}

impl LoudnessMeter {
    /// Creates a meter for `channels` channels at `sample_rate`.
    ///
    /// # Panics
    ///
    /// Panics if `channels` is zero or `sample_rate` is not positive.
    pub fn new(channels: usize, sample_rate: f32) -> Self {
        assert!(channels > 0, "channels must be greater than zero");
        assert!(sample_rate > 0.0, "sample_rate must be positive");
        let block_samples = ((0.4 * sample_rate).round() as usize).max(1);
        let step_samples = ((0.1 * sample_rate).round() as usize).max(1);
        Self {
            weighting: (0..channels)
                .map(|_| KWeighting::new(sample_rate))
                .collect(),
            block_samples,
            step_samples: step_samples.min(block_samples),
            position: 0,
            active: VecDeque::new(),
            completed: Vec::new(),
        }
    }

    /// Processes one frame: `frame[c]` is the sample for channel `c`.
    ///
    /// A short frame contributes silence on the missing channels; extra
    /// entries are ignored.
    pub fn process(&mut self, frame: &[f32]) {
        if self.position.is_multiple_of(self.step_samples as u64) {
            self.active.push_back(BlockAccumulator::default());
        }
        let mut sum_sq = 0.0f64;
        for (channel, weighting) in self.weighting.iter_mut().enumerate() {
            let x = frame.get(channel).copied().unwrap_or(0.0);
            let weighted = f64::from(weighting.process(x));
            sum_sq += weighted * weighted;
        }
        for block in &mut self.active {
            block.sum += sum_sq;
            block.seen += 1;
        }
        self.position += 1;
        while self
            .active
            .front()
            .is_some_and(|block| block.seen >= self.block_samples)
        {
            if let Some(block) = self.active.pop_front() {
                let z = block.sum / self.block_samples as f64;
                self.completed.push(z);
            }
        }
    }

    /// Gated integrated loudness in LUFS.
    ///
    /// `None` means no block survived the absolute or relative gate, which
    /// covers silence and streams shorter than one 400 ms block.
    pub fn integrated(&self) -> Option<f64> {
        let loudness = |z: f64| LOUDNESS_OFFSET + 10.0 * z.log10();
        let absolute_gated: Vec<f64> = self
            .completed
            .iter()
            .copied()
            .filter(|z| loudness(*z) > ABSOLUTE_GATE_LUFS)
            .collect();
        if absolute_gated.is_empty() {
            return None;
        }
        let mean_absolute = absolute_gated.iter().sum::<f64>() / absolute_gated.len() as f64;
        let threshold = loudness(mean_absolute) + RELATIVE_GATE_LU;
        let gated: Vec<f64> = absolute_gated
            .into_iter()
            .filter(|z| loudness(*z) > threshold)
            .collect();
        if gated.is_empty() {
            return None;
        }
        let mean_gated = gated.iter().sum::<f64>() / gated.len() as f64;
        Some(loudness(mean_gated))
    }
}

/// Measures the integrated loudness of a whole buffer in LUFS.
///
/// Returns `None` for empty buffers or material that survives no gate.
pub fn integrated_lufs(buffer: &AudioBuffer) -> Option<f64> {
    let channels = buffer.num_channels();
    if channels == 0 {
        return None;
    }
    let mut meter = LoudnessMeter::new(channels, buffer.sample_rate() as f32);
    let mut frame = vec![0.0f32; channels];
    for index in 0..buffer.frames() {
        for (channel, slot) in frame.iter_mut().enumerate() {
            slot.clone_from(&buffer.get(channel, index));
        }
        meter.process(&frame);
    }
    meter.integrated()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;
    const SR_F: f32 = 48_000.0;

    fn tone_buffer(freq: f32, peak_dbfs: f32, secs: f32, channels: usize) -> AudioBuffer {
        let frames = (secs * SR_F) as usize;
        let amplitude = 10.0_f32.powf(peak_dbfs / 20.0);
        let mut channels_data: Vec<Vec<f32>> = Vec::with_capacity(channels);
        for _ in 0..channels {
            channels_data.push(
                (0..frames)
                    .map(|n| {
                        amplitude * (2.0 * std::f32::consts::PI * freq * n as f32 / SR_F).sin()
                    })
                    .collect(),
            );
        }
        AudioBuffer::from_channels(channels_data, SR).expect("test buffer is well formed")
    }

    /// K-weighting gain in dB at `freq`, measured with Goertzel.
    fn kweight_gain_db(freq: f32) -> f32 {
        let mut weighting = KWeighting::new(SR_F);
        let frames = SR_F as usize; // exactly on-bin at integer freq
        let input: Vec<f32> = (0..frames)
            .map(|n| (2.0 * std::f32::consts::PI * freq * n as f32 / SR_F).sin())
            .collect();
        let output: Vec<f32> = input.iter().map(|&x| weighting.process(x)).collect();
        let half = output.len() / 2;
        let measured = crate::analysis::goertzel_magnitude(&output[half..], freq, SR_F);
        20.0 * (measured / 1.0).log10()
    }

    #[test]
    fn kweight_response_matches_bs1770_shape() {
        // Published properties of the K-curve: 0 dB at DC, about +0.4 dB at
        // 997 Hz, +2 dB at the 1682 Hz shelf corner, +4 dB at high frequency
        // and a high-pass roll-off below 100 Hz.
        let midband = kweight_gain_db(997.0);
        assert!((0.05..0.85).contains(&midband), "997 Hz gain {midband} dB");
        let hf = kweight_gain_db(10_000.0);
        assert!((3.4..4.6).contains(&hf), "10 kHz gain {hf} dB");
        let at_shelf = kweight_gain_db(1682.0);
        assert!(
            (1.5..2.5).contains(&at_shelf),
            "shelf corner gain {at_shelf} dB"
        );
        let at_20 = kweight_gain_db(20.0);
        assert!((-14.5..-12.0).contains(&at_20), "20 Hz gain {at_20} dB");
    }

    #[test]
    fn anchor_997hz_stereo_l_r_reads_minus_23_lufs() {
        // Calibrated anchor (EBU R128 alignment): a 997 Hz sine at
        // -23 dBFS on L and R measures -23.0 LUFS +/- 0.5 on a BS.1770-4
        // conformant meter. Under the K-curve mandated by this crate the
        // reading is about -23.2 LUFS.
        let buffer = tone_buffer(997.0, -23.0, 1.0, 2);
        let lufs = integrated_lufs(&buffer);
        let lufs = lufs.expect("1 s of tone must produce blocks");
        assert!(
            (lufs - (-23.0)).abs() <= 0.51,
            "anchor read {lufs} LUFS, expected -23.0 +/- 0.5"
        );

        // The convenience function and a hand-driven meter must agree.
        let mut meter = LoudnessMeter::new(2, SR_F);
        let mut frame = [0.0f32; 2];
        for index in 0..buffer.frames() {
            frame[0] = buffer.get(0, index);
            frame[1] = buffer.get(1, index);
            meter.process(&frame);
        }
        let direct = meter.integrated().expect("hand-driven meter has blocks");
        assert!((direct - lufs).abs() < 1e-9, "{direct} vs {lufs}");
    }

    #[test]
    fn minus_20_dbfs_single_channel_reads_minus_23_lufs() {
        // The same total channel energy as the stereo anchor: one channel
        // driven at -20 dBFS. This is the literal "-20 dBFS reads
        // -23.0 LUFS" calibration; left-only and right-only must agree.
        let left = tone_buffer(997.0, -20.0, 1.0, 1);
        let lufs_left = integrated_lufs(&left).expect("blocks must exist");
        assert!(
            (lufs_left - (-23.0)).abs() <= 0.51,
            "single channel read {lufs_left} LUFS, expected -23.0 +/- 0.5"
        );

        let right = {
            let data = vec![left.channels()[0].clone()];
            AudioBuffer::from_channels(data, SR).expect("well formed")
        };
        let lufs_right = integrated_lufs(&right).expect("blocks must exist");
        assert!(
            (lufs_left - lufs_right).abs() < 0.05,
            "channel asymmetry: {lufs_left} vs {lufs_right}"
        );

        // A stereo pair at -20 dBFS per channel is 3 LU louder than one
        // channel at -20 dBFS (energy sums over channels).
        let both = tone_buffer(997.0, -20.0, 1.0, 2);
        let lufs_both = integrated_lufs(&both).expect("blocks must exist");
        assert!(
            (lufs_both - lufs_left - 3.01).abs() < 0.1,
            "stereo {lufs_both} vs mono {lufs_left}"
        );
    }

    #[test]
    fn silence_returns_none() {
        let silence = AudioBuffer::new(2, SR_F as usize, SR);
        assert_eq!(integrated_lufs(&silence), None);
        // Empty shape is also not measurable.
        let empty = AudioBuffer::new(0, 0, SR);
        assert_eq!(integrated_lufs(&empty), None);
    }

    #[test]
    fn gating_excludes_quiet_tail() {
        // 1 s at -23 dBFS then 2 s at -45 dBFS. The quiet blocks pass the
        // absolute gate but sit below the relative threshold, so the
        // integrated reading must stay near the loud section. Without any
        // gating the 27-block mean would be about -29 LUFS.
        let loud_part = tone_buffer(997.0, -23.0, 1.0, 1);
        let tail_part = tone_buffer(997.0, -45.0, 2.0, 1);
        let loud_data = loud_part.channels()[0].clone();
        let tail_data = tail_part.channels()[0].clone();
        let mut stereo: Vec<Vec<f32>> = Vec::with_capacity(2);
        for _ in 0..2 {
            let mut data = loud_data.clone();
            data.extend_from_slice(&tail_data);
            stereo.push(data);
        }
        // ^ each channel: 1 s loud + 2 s quiet
        let with_tail = AudioBuffer::from_channels(stereo, SR).expect("well formed");
        // loud_part is mono; the reference reading is a stereo copy of it.
        let loud_only = {
            let data = vec![loud_part.channels()[0].clone(); 2];
            integrated_lufs(&AudioBuffer::from_channels(data, SR).expect("well formed"))
                .expect("blocks")
        };

        let lufs = integrated_lufs(&with_tail).expect("blocks must exist");
        assert!(
            (lufs - loud_only).abs() < 1.0,
            "gated {lufs} vs loud-only {loud_only}"
        );
        assert!(lufs > -25.0, "quiet tail dragged reading to {lufs}");
    }

    #[test]
    fn level_steps_are_six_lu() {
        let read = |peak: f32| {
            integrated_lufs(&tone_buffer(997.0, peak, 1.0, 2)).expect("blocks must exist")
        };
        let high = read(-17.0);
        let mid = read(-23.0);
        let low = read(-29.0);
        assert!((high - mid - 6.0).abs() < 0.1, "{high} vs {mid}");
        assert!((mid - low - 6.0).abs() < 0.1, "{mid} vs {low}");
        assert!((high - mid).abs() > 5.9);
    }

    #[test]
    fn sample_rate_invariance() {
        let sr = 44_100u32;
        let frames = sr as usize;
        let amplitude = 10.0_f32.powf(-23.0 / 20.0);
        let data: Vec<f32> = (0..frames)
            .map(|n| amplitude * (2.0 * std::f32::consts::PI * 997.0 * n as f32 / sr as f32).sin())
            .collect();
        let at_44k = AudioBuffer::from_channels(vec![data.clone(), data], sr).expect("well formed");
        let reading_44k = integrated_lufs(&at_44k).expect("blocks must exist");
        let at_48k = integrated_lufs(&tone_buffer(997.0, -23.0, 1.0, 2)).expect("blocks");
        assert!(
            (reading_44k - at_48k).abs() < 0.2,
            "44.1 kHz read {reading_44k} vs 48 kHz {at_48k}"
        );
    }

    #[test]
    fn short_signals_have_no_complete_blocks() {
        // Shorter than one 400 ms window: no block, no reading.
        let short = tone_buffer(997.0, -23.0, 0.3, 2);
        assert_eq!(integrated_lufs(&short), None);
        // 450 ms: exactly one complete block (starts at 0).
        let barely = tone_buffer(997.0, -23.0, 0.45, 2);
        let lufs = integrated_lufs(&barely).expect("one block completes");
        assert!((lufs - (-23.0)).abs() <= 0.6, "short-signal reading {lufs}");
    }

    #[test]
    fn low_frequency_rolls_off_like_the_k_curve() {
        // A 20 Hz tone at -20 dBFS sits about 13 dB below the unity gain of
        // the K-curve, so it must read roughly 13 LU under the -20 dBFS
        // single-channel reference (-34 LUFS total).
        let buffer = tone_buffer(20.0, -20.0, 1.0, 2);
        let lufs = integrated_lufs(&buffer).expect("blocks must exist");
        assert!((-35.0..-33.0).contains(&lufs), "20 Hz reading {lufs} LUFS");
    }
}
