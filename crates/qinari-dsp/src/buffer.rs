//! Planar `f32` audio buffers and dB/linear gain helpers.

use thiserror::Error;

/// Errors that can occur when building an [`AudioBuffer`] from raw channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum BufferError {
    /// A channel did not have the same frame count as the first channel.
    #[error("channel {index} has {length} frames, expected {expected}")]
    RaggedChannels {
        /// Index of the offending channel.
        index: usize,
        /// Frame count of the offending channel.
        length: usize,
        /// Frame count of the first channel.
        expected: usize,
    },
    /// The sample rate was zero.
    #[error("sample rate must be greater than zero, got {rate}")]
    InvalidSampleRate {
        /// The offending sample rate.
        rate: u32,
    },
}

/// Converts a decibel value to a linear amplitude factor.
pub fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Converts a linear amplitude factor to decibels.
///
/// Returns `-inf` for zero and `NaN` for negative inputs.
pub fn linear_to_db(linear: f32) -> f32 {
    20.0 * linear.log10()
}

/// A planar audio buffer: one `Vec<f32>` of samples per channel.
///
/// Channels are stored separately (planar), samples are `f32`, and the
/// sample rate is an integer number of hertz. All Qinari engines render
/// into `AudioBuffer`s.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBuffer {
    channels: Vec<Vec<f32>>,
    sample_rate: u32,
}

impl AudioBuffer {
    /// Creates a buffer of `channels` channels and `frames` frames per
    /// channel, initialized to silence.
    ///
    /// # Panics
    ///
    /// Panics if `sample_rate` is zero.
    pub fn new(channels: usize, frames: usize, sample_rate: u32) -> Self {
        assert!(sample_rate > 0, "sample_rate must be greater than zero");
        Self {
            channels: vec![vec![0.0; frames]; channels],
            sample_rate,
        }
    }

    /// Creates a buffer from existing channel data.
    ///
    /// # Errors
    ///
    /// Returns [`BufferError::RaggedChannels`] if the channels have
    /// differing lengths, and [`BufferError::InvalidSampleRate`] if the
    /// sample rate is zero.
    pub fn from_channels(channels: Vec<Vec<f32>>, sample_rate: u32) -> Result<Self, BufferError> {
        if sample_rate == 0 {
            return Err(BufferError::InvalidSampleRate { rate: 0 });
        }
        let expected = channels.first().map_or(0, Vec::len);
        for (index, channel) in channels.iter().enumerate() {
            if channel.len() != expected {
                return Err(BufferError::RaggedChannels {
                    index,
                    length: channel.len(),
                    expected,
                });
            }
        }
        Ok(Self {
            channels,
            sample_rate,
        })
    }

    /// Returns the number of frames (samples per channel).
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    /// Returns the number of channels.
    pub fn num_channels(&self) -> usize {
        self.channels.len()
    }

    /// Returns the sample rate in hertz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Returns the duration in seconds as `f64`.
    pub fn duration(&self) -> f64 {
        f64::from(self.frames() as u32) / f64::from(self.sample_rate)
    }

    /// Returns the raw planar channel data.
    pub fn channels(&self) -> &[Vec<f32>] {
        &self.channels
    }

    /// Returns the sample on `channel` at `frame`, or `0.0` when out of
    /// range. Reading past the end yields silence so that mono sources can
    /// be read through a stereo view without special cases.
    pub fn get(&self, channel: usize, frame: usize) -> f32 {
        self.channels
            .get(channel)
            .and_then(|ch| ch.get(frame))
            .copied()
            .unwrap_or(0.0)
    }

    /// Writes `value` to `channel` at `frame`; out-of-range writes are
    /// ignored (mirroring [`AudioBuffer::get`]).
    pub fn set(&mut self, channel: usize, frame: usize, value: f32) {
        if let Some(ch) = self.channels.get_mut(channel) {
            if let Some(slot) = ch.get_mut(frame) {
                *slot = value;
            }
        }
    }

    /// Returns the maximum absolute sample value across all channels.
    pub fn peak(&self) -> f32 {
        let mut peak = 0.0f32;
        for channel in &self.channels {
            for &sample in channel {
                peak = peak.max(sample.abs());
            }
        }
        peak
    }

    /// Returns the root-mean-square level over all channels.
    pub fn rms(&self) -> f32 {
        let mut sum = 0.0f64;
        let mut count = 0usize;
        for channel in &self.channels {
            for &sample in channel {
                sum += f64::from(sample) * f64::from(sample);
                count += 1;
            }
        }
        if count == 0 {
            0.0
        } else {
            (sum / count as f64).sqrt() as f32
        }
    }

    /// Returns `true` if any sample is NaN or infinite.
    pub fn has_non_finite(&self) -> bool {
        self.channels
            .iter()
            .flatten()
            .any(|sample| !sample.is_finite())
    }

    /// Applies a gain in decibels to every sample.
    pub fn apply_gain_db(&mut self, db: f32) {
        let gain = db_to_linear(db);
        for channel in &mut self.channels {
            for sample in channel.iter_mut() {
                *sample *= gain;
            }
        }
    }

    /// Applies `f` to each `(left, right)` pair of the first two channels.
    ///
    /// Buffers with fewer than two channels are left unchanged.
    pub fn map_stereo(&mut self, mut f: impl FnMut(f32, f32) -> (f32, f32)) {
        if self.channels.len() < 2 {
            return;
        }
        let (left, right) = self.channels.split_at_mut(1);
        let left = &mut left[0];
        let right = &mut right[0];
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (nl, nr) = f(*l, *r);
            *l = nl;
            *r = nr;
        }
    }

    /// Mixes `other` into this buffer, scaled by `gain`.
    ///
    /// Frames beyond either buffer's end contribute silence; `other`'s
    /// channels are wrapped (`self` channel `c` mixes from `other` channel
    /// `c % other.num_channels()`), so a mono source mixes into both sides
    /// of a stereo buffer.
    pub fn mix(&mut self, other: &AudioBuffer, gain: f32) {
        if other.channels.is_empty() {
            return;
        }
        let frames = self.frames().min(other.frames());
        for (c, channel) in self.channels.iter_mut().enumerate() {
            let source = &other.channels[c % other.channels.len()];
            for (dst, &src) in channel.iter_mut().zip(source.iter()).take(frames) {
                *dst += gain * src;
            }
        }
    }

    /// Adds `other` to this buffer sample-by-sample, silence-padding any
    /// channel or frame that either buffer is missing.
    ///
    /// The buffer grows to the larger channel count and frame count of the
    /// two inputs; new samples start as silence.
    pub fn add(&mut self, other: &AudioBuffer) {
        self.resize_shape(other.num_channels().max(self.num_channels()));
        let frames = other.frames();
        for channel in &mut self.channels {
            channel.resize(frames.max(channel.len()), 0.0);
        }
        for (c, source) in other.channels.iter().enumerate() {
            let dst = &mut self.channels[c];
            for (dst_sample, &src) in dst.iter_mut().zip(source.iter()) {
                *dst_sample += src;
            }
        }
    }

    /// Appends `other`'s frames after this buffer's frames.
    ///
    /// The result has the larger channel count of the two inputs; channels
    /// missing on either side are silence-padded.
    pub fn concat(&mut self, other: &AudioBuffer) {
        self.resize_shape(other.num_channels().max(self.num_channels()));
        for (c, source) in other.channels.iter().enumerate() {
            self.channels[c].extend_from_slice(source);
        }
    }

    /// Applies a linear fade-in over the first `frames` frames (clamped to
    /// the buffer length); the very first faded sample is exactly zero.
    pub fn fade_in(&mut self, frames: usize) {
        let frames = frames.min(self.frames());
        if frames == 0 {
            return;
        }
        for channel in &mut self.channels {
            for (i, sample) in channel.iter_mut().take(frames).enumerate() {
                *sample *= i as f32 / frames as f32;
            }
        }
    }

    /// Applies a linear fade-out over the last `frames` frames (clamped to
    /// the buffer length); the very last sample is exactly zero.
    pub fn fade_out(&mut self, frames: usize) {
        let frames = frames.min(self.frames());
        if frames == 0 {
            return;
        }
        let start = self.frames() - frames;
        for channel in &mut self.channels {
            for (i, sample) in channel.iter_mut().skip(start).enumerate() {
                *sample *= (frames - 1 - i) as f32 / frames as f32;
            }
        }
    }

    /// Grows the channel count to `channels`, zero-filling new channels to
    /// the current frame count.
    fn resize_shape(&mut self, channels: usize) {
        while self.channels.len() < channels {
            self.channels.push(vec![0.0; self.frames()]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    #[test]
    fn db_helpers_round_trip() {
        assert!((db_to_linear(-6.020_6) - 0.5).abs() < 1e-4);
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_linear(6.020_6) - 2.0).abs() < 1e-4);
        for db in [-40.0, -12.0, -3.01, 0.0, 3.5, 12.0] {
            let round = linear_to_db(db_to_linear(db));
            assert!((round - db).abs() < 1e-3, "{db} -> {round}");
        }
        assert_eq!(db_to_linear(-120.0), 1e-6);
    }

    #[test]
    fn from_channels_rejects_ragged_input_and_zero_rate() {
        let ok = AudioBuffer::from_channels(vec![vec![0.0; 10], vec![0.0; 10]], SR);
        assert!(ok.is_ok());
        assert_eq!(ok.unwrap().frames(), 10);

        let ragged = AudioBuffer::from_channels(vec![vec![0.0; 10], vec![0.0; 9]], SR);
        assert_eq!(
            ragged.unwrap_err(),
            BufferError::RaggedChannels {
                index: 1,
                length: 9,
                expected: 10
            }
        );
        let bad_rate = AudioBuffer::from_channels(vec![vec![0.0; 4]], 0);
        assert_eq!(
            bad_rate.unwrap_err(),
            BufferError::InvalidSampleRate { rate: 0 }
        );
    }

    #[test]
    fn accessors_report_shape_and_duration() {
        let buffer = AudioBuffer::new(2, 96_000, SR);
        assert_eq!(buffer.frames(), 96_000);
        assert_eq!(buffer.num_channels(), 2);
        assert_eq!(buffer.sample_rate(), SR);
        assert!((buffer.duration() - 2.0).abs() < 1e-12);
        assert_eq!(buffer.get(0, 0), 0.0);
        assert_eq!(buffer.get(99, 0), 0.0);

        let mut buffer = AudioBuffer::new(1, 4, SR);
        buffer.set(0, 2, 0.5);
        assert_eq!(buffer.get(0, 2), 0.5);
        buffer.set(5, 0, 1.0); // out of range: ignored
        buffer.set(0, 99, 1.0); // out of range: ignored
        assert_eq!(buffer.get(0, 99), 0.0);
    }

    #[test]
    fn peak_and_rms_of_known_signals() {
        // Unit sine: peak 1.0, rms 1/sqrt(2).
        let sine: Vec<f32> = (0..4800)
            .map(|n| (2.0 * std::f32::consts::PI * n as f32 / 48.0).sin())
            .collect();
        let buffer = AudioBuffer::from_channels(vec![sine], SR).unwrap();
        assert!((buffer.peak() - 1.0).abs() < 0.01);
        assert!((buffer.rms() - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);

        let buffer = AudioBuffer::new(2, 100, SR);
        assert_eq!(buffer.peak(), 0.0);
        assert_eq!(buffer.rms(), 0.0);
        assert!(!buffer.has_non_finite());

        let mut buffer = AudioBuffer::new(1, 3, SR);
        buffer.set(0, 1, f32::NAN);
        assert!(buffer.has_non_finite());
    }

    #[test]
    fn apply_gain_db_scales_amplitude() {
        let mut buffer = AudioBuffer::new(1, 8, SR);
        buffer.set(0, 0, 0.25);
        buffer.apply_gain_db(12.041_2); // ~4x
        assert!((buffer.get(0, 0) - 1.0).abs() < 1e-3);
        buffer.apply_gain_db(-12.041_2);
        assert!((buffer.get(0, 0) - 0.25).abs() < 1e-3);
    }

    #[test]
    fn fades_reach_exactly_zero_at_edges() {
        let mut buffer = AudioBuffer::new(2, 100, SR);
        for c in 0..2 {
            for f in 0..100 {
                buffer.set(c, f, 0.7);
            }
        }
        buffer.fade_in(50);
        assert_eq!(buffer.get(0, 0), 0.0);
        assert_eq!(buffer.get(1, 0), 0.0);
        assert!((buffer.get(0, 49) - 0.7 * 49.0 / 50.0).abs() < 1e-6);
        assert!(
            (buffer.get(0, 50) - 0.7).abs() < 1e-6,
            "beyond fade untouched"
        );
        assert!((buffer.get(0, 99) - 0.7).abs() < 1e-6);

        let mut buffer = AudioBuffer::new(1, 100, SR);
        for f in 0..100 {
            buffer.set(0, f, -0.5);
        }
        buffer.fade_out(25);
        assert_eq!(buffer.get(0, 99), 0.0);
        assert_eq!(buffer.get(0, 74), -0.5, "before fade untouched");
        assert!((buffer.get(0, 75) - (-0.5) * 24.0 / 25.0).abs() < 1e-6);

        // Fade longer than the buffer clamps to the whole buffer.
        let mut buffer = AudioBuffer::new(1, 10, SR);
        for f in 0..10 {
            buffer.set(0, f, 1.0);
        }
        buffer.fade_in(500);
        assert_eq!(buffer.get(0, 0), 0.0);
        assert!(buffer.get(0, 9) > 0.8);
        buffer.fade_out(500);
        assert_eq!(buffer.get(0, 9), 0.0);

        // Zero-frame fade is a no-op.
        let mut buffer = AudioBuffer::new(1, 4, SR);
        buffer.set(0, 0, 1.0);
        buffer.fade_in(0);
        buffer.fade_out(0);
        assert_eq!(buffer.get(0, 0), 1.0);
    }

    #[test]
    fn map_stereo_pairs_channels() {
        let mut buffer =
            AudioBuffer::from_channels(vec![vec![0.1; 8], vec![-0.2; 8], vec![0.5; 8]], SR)
                .unwrap();
        buffer.map_stereo(|l, r| (r, l));
        for f in 0..8 {
            assert_eq!(buffer.get(0, f), -0.2);
            assert_eq!(buffer.get(1, f), 0.1);
            assert_eq!(buffer.get(2, f), 0.5, "third channel untouched");
        }

        let mut mono = AudioBuffer::from_channels(vec![vec![0.3; 4]], SR).unwrap();
        mono.map_stereo(|l, r| (l + r, l - r));
        assert_eq!(mono.get(0, 0), 0.3, "mono buffer unchanged");
    }

    #[test]
    fn mix_wraps_channels_and_pads_frames() {
        let mut master = AudioBuffer::from_channels(vec![vec![1.0; 6], vec![-1.0; 6]], SR).unwrap();
        let mono = AudioBuffer::from_channels(vec![vec![0.5, 0.25, 0.125]], SR).unwrap();
        master.mix(&mono, 2.0);
        assert_eq!(master.frames(), 6, "mix does not change length");
        assert!((master.get(0, 0) - 2.0).abs() < 1e-6);
        assert!(
            (master.get(1, 0) - (-1.0 + 1.0)).abs() < 1e-6,
            "wrapped channel"
        );
        assert!((master.get(0, 1) - (1.0 + 0.5)).abs() < 1e-6);
        assert!((master.get(0, 3) - 1.0).abs() < 1e-6, "padded with silence");

        // Stereo into stereo mixes in place.
        let mut master = AudioBuffer::from_channels(vec![vec![1.0; 2], vec![1.0; 2]], SR).unwrap();
        let stereo = AudioBuffer::from_channels(vec![vec![0.1; 2], vec![0.2; 2]], SR).unwrap();
        master.mix(&stereo, 1.0);
        assert!((master.get(0, 0) - 1.1).abs() < 1e-6);
        assert!((master.get(1, 0) - 1.2).abs() < 1e-6);
    }

    #[test]
    fn add_silence_pads_shape() {
        let mut base = AudioBuffer::from_channels(vec![vec![0.5; 4]], SR).unwrap();
        let upper = AudioBuffer::from_channels(vec![vec![-0.5; 4], vec![0.25; 4]], SR).unwrap();
        base.add(&upper);
        assert_eq!(base.num_channels(), 2);
        assert_eq!(base.frames(), 4);
        assert!((base.get(0, 0) - 0.0).abs() < 1e-6);
        assert!((base.get(1, 0) - 0.25).abs() < 1e-6);

        // Longer other extends self with silence.
        let mut base = AudioBuffer::from_channels(vec![vec![1.0; 2]], SR).unwrap();
        let longer = AudioBuffer::from_channels(vec![vec![0.0; 5]], SR).unwrap();
        base.add(&longer);
        assert_eq!(base.frames(), 5);
        assert!((base.get(0, 4) - 0.0).abs() < 1e-6);
        assert!((base.get(0, 0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn db_helpers_handle_zero_and_extremes() {
        assert_eq!(linear_to_db(0.0), f32::NEG_INFINITY);
        assert!(linear_to_db(1.0).abs() < 1.0e-6);
        assert_eq!(db_to_linear(f32::NEG_INFINITY), 0.0);
        assert!((db_to_linear(60.0) - 1000.0).abs() < 0.01);
        assert!(db_to_linear(f32::INFINITY).is_infinite());
    }

    #[test]
    fn empty_and_zero_channel_buffers_are_well_formed() {
        let empty = AudioBuffer::new(0, 0, SR);
        assert_eq!(empty.frames(), 0);
        assert_eq!(empty.num_channels(), 0);
        assert_eq!(empty.peak(), 0.0);
        assert_eq!(empty.rms(), 0.0);
        assert!((empty.duration() - 0.0).abs() < 1.0e-12);
        assert!(!empty.has_non_finite());
        assert_eq!(empty.get(0, 0), 0.0);

        let no_channels = AudioBuffer::from_channels(vec![], SR).unwrap();
        assert_eq!(no_channels.num_channels(), 0);
        assert_eq!(no_channels.frames(), 0);

        // Fades on an empty buffer must not panic or divide by zero.
        let mut empty = AudioBuffer::new(0, 0, SR);
        empty.fade_in(100);
        empty.fade_out(100);
        assert_eq!(empty.frames(), 0);
    }

    #[test]
    fn mix_with_zero_gain_changes_nothing_and_negative_gain_subtracts() {
        let mut base = AudioBuffer::from_channels(vec![vec![0.5; 4], vec![-0.25; 4]], SR).unwrap();
        let layer = AudioBuffer::from_channels(vec![vec![0.125; 4]], SR).unwrap();
        base.mix(&layer, 0.0);
        for frame in 0..4 {
            assert!((base.get(0, frame) - 0.5).abs() < 1.0e-7);
            assert!((base.get(1, frame) - (-0.25)).abs() < 1.0e-7);
        }
        base.mix(&layer, -1.0);
        for frame in 0..4 {
            assert!((base.get(0, frame) - (0.5 - 0.125)).abs() < 1.0e-6);
            assert!((base.get(1, frame) - (-0.25 - 0.125)).abs() < 1.0e-6);
        }
    }

    #[test]
    fn apply_gain_db_minus_infinity_silences_the_buffer() {
        let mut buffer = AudioBuffer::from_channels(vec![vec![0.3; 8], vec![-0.9; 8]], SR).unwrap();
        buffer.apply_gain_db(f32::NEG_INFINITY);
        assert_eq!(buffer.peak(), 0.0);
    }

    #[test]
    fn channels_accessor_exposes_the_planar_data() {
        let buffer = AudioBuffer::from_channels(vec![vec![1.0, 2.0], vec![3.0, 4.0]], SR).unwrap();
        assert_eq!(buffer.channels().len(), 2);
        assert_eq!(buffer.channels()[0], vec![1.0, 2.0]);
        assert_eq!(buffer.channels()[1], vec![3.0, 4.0]);
    }

    #[test]
    fn concat_preserves_order_and_pads_channels() {
        let mut a = AudioBuffer::from_channels(vec![vec![1.0; 3], vec![0.1; 3]], SR).unwrap();
        let b = AudioBuffer::from_channels(vec![vec![2.0; 2]], SR).unwrap();
        a.concat(&b);
        assert_eq!(a.frames(), 5);
        assert_eq!(a.num_channels(), 2);
        assert!((a.get(0, 0) - 1.0).abs() < 1e-6);
        assert!((a.get(0, 3) - 2.0).abs() < 1e-6);
        assert!((a.get(0, 4) - 2.0).abs() < 1e-6);
        assert_eq!(a.get(1, 3), 0.0, "second channel silence-padded");
        assert!((a.get(1, 0) - 0.1).abs() < 1e-6);

        let mut empty = AudioBuffer::new(0, 0, SR);
        let b = AudioBuffer::from_channels(vec![vec![7.0; 2]], SR).unwrap();
        empty.concat(&b);
        assert_eq!(empty.num_channels(), 1);
        assert_eq!(empty.frames(), 2);
        assert!((empty.get(0, 1) - 7.0).abs() < 1e-6);
    }
}
