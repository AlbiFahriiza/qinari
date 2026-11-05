//! Sample-rate conversion via `rubato`'s synchronous FFT resampler.

use crate::{deinterleave, AudioError};
use qinari_dsp::buffer::AudioBuffer;
use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler};

/// Processing chunk for the resampler, independent of clip length.
const CHUNK_FRAMES: usize = 1024;

/// Resamples `buffer` to `target_rate` hertz.
///
/// Same-rate inputs return a clone. The FFT resampler runs on fixed chunks
/// and its startup delay is trimmed, so the output length is
/// `round(frames * target_rate / source_rate)` within one frame.
///
/// # Errors
///
/// Returns [`AudioError::Resample`] when a rate is zero or otherwise
/// rejected by the resampler, or when the internal buffers disagree on
/// shape (which cannot happen for well-formed buffers).
///
/// # Panics
///
/// Never for a well-formed buffer: the only `expect` guards the empty-shape
/// early return, which is always constructible.
pub fn resample(buffer: &AudioBuffer, target_rate: u32) -> Result<AudioBuffer, AudioError> {
    if target_rate == 0 {
        return Err(AudioError::Resample(
            "target sample rate must be greater than zero".into(),
        ));
    }
    let source_rate = buffer.sample_rate();
    if target_rate == source_rate {
        return Ok(buffer.clone());
    }
    let channels = buffer.num_channels();
    let frames = buffer.frames();
    if channels == 0 || frames == 0 {
        return Ok(
            AudioBuffer::from_channels(vec![Vec::new(); channels], target_rate)
                .expect("empty shape is always constructible"),
        );
    }
    let mut resampler = Fft::<f32>::new(
        source_rate as usize,
        target_rate as usize,
        CHUNK_FRAMES,
        channels,
        FixedSync::Both,
    )
    .map_err(|error| AudioError::Resample(error.to_string()))?;
    let input = SequentialSliceOfVecs::new(buffer.channels(), channels, frames)
        .map_err(|error| AudioError::Resample(error.to_string()))?;
    let output = resampler
        .process_all(&input, frames, None)
        .map_err(|error| AudioError::Resample(error.to_string()))?;
    let interleaved = output.take_data();
    deinterleave(&interleaved, channels, target_rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::tone;
    use qinari_dsp::analysis::goertzel_magnitude;

    #[test]
    fn downsampling_48k_to_44100_preserves_a_1khz_tone_and_duration() {
        let original = tone(1000.0, 0.5, 1.0, 2, 48_000);
        let resampled = resample(&original, 44_100).expect("resample must succeed");
        assert_eq!(resampled.sample_rate(), 44_100);
        assert_eq!(resampled.num_channels(), 2);
        let expected = 44_100.0;
        assert!(
            (resampled.frames() as f64 - expected).abs() < expected * 0.01,
            "frames {}, expected ~{expected}",
            resampled.frames()
        );
        for channel in 0..2 {
            let measured = goertzel_magnitude(&resampled.channels()[channel], 1000.0, 44_100.0);
            assert!(
                (measured - 0.5).abs() < 0.05,
                "channel {channel} tone {measured}, expected ~0.5"
            );
        }
        assert!(!resampled.has_non_finite());
    }

    #[test]
    fn upsampling_44100_to_48000_preserves_a_1khz_tone_and_duration() {
        let original = tone(1000.0, 0.5, 1.0, 2, 44_100);
        let resampled = resample(&original, 48_000).expect("resample must succeed");
        assert_eq!(resampled.sample_rate(), 48_000);
        let expected = 48_000.0;
        assert!(
            (resampled.frames() as f64 - expected).abs() < expected * 0.01,
            "frames {}, expected ~{expected}",
            resampled.frames()
        );
        let measured = goertzel_magnitude(&resampled.channels()[0], 1000.0, 48_000.0);
        assert!(
            (measured - 0.5).abs() < 0.05,
            "tone {measured}, expected ~0.5"
        );
    }

    #[test]
    fn same_rate_resample_returns_an_equal_clone() {
        let original = tone(500.0, 0.3, 0.5, 2, 48_000);
        let cloned = resample(&original, 48_000).expect("same rate must succeed");
        assert_eq!(cloned, original);
    }

    #[test]
    fn empty_buffers_pass_through_with_the_new_rate() {
        let empty = AudioBuffer::new(0, 0, 48_000);
        let resampled = resample(&empty, 44_100).expect("empty must pass through");
        assert_eq!(resampled.sample_rate(), 44_100);
        assert_eq!(resampled.frames(), 0);
        let silent = AudioBuffer::new(2, 0, 48_000);
        let resampled = resample(&silent, 44_100).expect("zero frames must pass through");
        assert_eq!(resampled.sample_rate(), 44_100);
        assert_eq!(resampled.num_channels(), 2);
        assert_eq!(resampled.frames(), 0);
    }

    #[test]
    fn zero_target_rate_is_rejected() {
        let original = tone(500.0, 0.3, 0.1, 1, 48_000);
        let error = resample(&original, 0).expect_err("zero rate must be rejected");
        assert!(matches!(error, AudioError::Resample(_)), "got {error}");
    }
}
