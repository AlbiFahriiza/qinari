//! MP3 encoding via `mp3lame-encoder` (bundled LAME 4.0), CBR, mono/stereo.

use crate::format::Mp3Bitrate;
use crate::AudioError;
use mp3lame_encoder::{Bitrate, DualPcm, FlushGap, Mode, Quality};
use qinari_dsp::buffer::AudioBuffer;
use std::path::Path;

/// Frames fed to LAME per encode call (~1 s at 48 kHz).
const ENCODE_CHUNK_FRAMES: usize = 48_000;
/// Extra spare capacity on top of the documented worst case per chunk.
///
/// LAME writes into the spare capacity of the output vector; a zero-capacity
/// buffer is undefined behaviour, so every `encode_to_vec`/`flush_to_vec`
/// call must be preceded by an explicit reserve (this is one of the two
/// historical segfaults this crate guards against).
const ENCODE_HEADROOM_BYTES: usize = 16 * 1024;

/// Writes an [`AudioBuffer`] to a constant-bitrate MP3 file.
///
/// Mono buffers are encoded with the encoder set to mono mode and the single
/// channel duplicated into both LAME input buffers (passing a null right
/// channel to LAME segfaults). Samples are clamped to `[-1, 1]` and fed as
/// floats, so a dithered 16/24-bit buffer keeps its full depth.
///
/// # Errors
///
/// Returns [`AudioError::EmptyBuffer`] for zero-channel or zero-frame
/// buffers, [`AudioError::UnsupportedChannels`] above 2 channels,
/// [`AudioError::Mp3Encode`] when LAME rejects the parameters or fails
/// mid-stream, and [`AudioError::Io`] when the file cannot be written.
pub fn write_mp3<P: AsRef<Path>>(
    path: P,
    buffer: &AudioBuffer,
    bitrate: Mp3Bitrate,
) -> Result<(), AudioError> {
    let channels = buffer.num_channels();
    let frames = buffer.frames();
    if channels == 0 || frames == 0 {
        return Err(AudioError::EmptyBuffer);
    }
    if channels > 2 {
        return Err(AudioError::UnsupportedChannels { channels, max: 2 });
    }
    let mut encoder = build_encoder(buffer.sample_rate(), channels, bitrate)?;
    let mut mp3_bytes: Vec<u8> = Vec::new();
    let mut left: Vec<f32> = Vec::with_capacity(ENCODE_CHUNK_FRAMES);
    let mut right: Vec<f32> = Vec::with_capacity(ENCODE_CHUNK_FRAMES);
    for start in (0..frames).step_by(ENCODE_CHUNK_FRAMES) {
        let end = (start + ENCODE_CHUNK_FRAMES).min(frames);
        left.clear();
        right.clear();
        for frame in start..end {
            let l = buffer.get(0, frame).clamp(-1.0, 1.0);
            // Mono: duplicate the channel into both inputs; the right buffer
            // must never be empty when the encoder runs in mono mode.
            let r = if channels == 1 {
                l
            } else {
                buffer.get(1, frame).clamp(-1.0, 1.0)
            };
            left.push(l);
            right.push(r);
        }
        mp3_bytes
            .reserve(mp3lame_encoder::max_required_buffer_size(left.len()) + ENCODE_HEADROOM_BYTES);
        encoder
            .encode_to_vec(
                DualPcm {
                    left: &left,
                    right: &right,
                },
                &mut mp3_bytes,
            )
            .map_err(mp3_error)?;
    }
    mp3_bytes.reserve(mp3lame_encoder::max_required_buffer_size(0) + ENCODE_HEADROOM_BYTES);
    encoder
        .flush_to_vec::<FlushGap>(&mut mp3_bytes)
        .map_err(mp3_error)?;
    std::fs::write(path, mp3_bytes)?;
    Ok(())
}

/// Configures LAME for one encode run.
fn build_encoder(
    sample_rate: u32,
    channels: usize,
    bitrate: Mp3Bitrate,
) -> Result<mp3lame_encoder::Encoder, AudioError> {
    let lame_bitrate = match bitrate {
        Mp3Bitrate::Kbps128 => Bitrate::Kbps128,
        Mp3Bitrate::Kbps192 => Bitrate::Kbps192,
        Mp3Bitrate::Kbps256 => Bitrate::Kbps256,
        Mp3Bitrate::Kbps320 => Bitrate::Kbps320,
    };
    let mut builder = mp3lame_encoder::Builder::new()
        .ok_or_else(|| AudioError::Mp3Encode("LAME failed to allocate its encoder".into()))?;
    builder.set_sample_rate(sample_rate).map_err(mp3_error)?;
    builder
        .set_num_channels(channels as u8)
        .map_err(mp3_error)?;
    builder.set_brate(lame_bitrate).map_err(mp3_error)?;
    builder.set_quality(Quality::Best).map_err(mp3_error)?;
    // Plain CBR stream: no LAME/Xing tag needs to be stitched back in.
    builder.set_to_write_vbr_tag(false).map_err(mp3_error)?;
    if channels == 1 {
        builder.set_mode(Mode::Mono).map_err(mp3_error)?;
    }
    builder.build().map_err(mp3_error)
}

/// Wraps a LAME build/encode error for the [`AudioError::Mp3Encode`] variant.
fn mp3_error(error: impl std::fmt::Display) -> AudioError {
    AudioError::Mp3Encode(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode_audio;
    use crate::test_support::{temp_file, tone};
    use qinari_dsp::analysis::goertzel_magnitude;

    const SR: u32 = 44_100;

    #[test]
    fn mp3_stereo_roundtrip_preserves_tone_and_shape() {
        let original = tone(997.0, 0.5, 1.0, 2, SR);
        let path = temp_file("stereo.mp3");
        write_mp3(&path, &original, Mp3Bitrate::Kbps320).expect("encode must succeed");
        let decoded = decode_audio(&path).expect("decode must succeed");
        assert_eq!(decoded.num_channels(), 2);
        assert_eq!(decoded.sample_rate(), SR);
        let expected_frames = SR as usize;
        let deviation =
            (decoded.frames() as f64 - expected_frames as f64).abs() / expected_frames as f64;
        assert!(
            deviation < 0.05,
            "decoded {} frames, expected ~{expected_frames} (deviation {deviation})",
            decoded.frames()
        );
        for channel in 0..2 {
            let measured = goertzel_magnitude(&decoded.channels()[channel], 997.0, SR as f32);
            assert!(
                (measured - 0.5).abs() < 0.1,
                "channel {channel} tone {measured}, expected ~0.5"
            );
        }
        assert!(!decoded.has_non_finite());
    }

    #[test]
    fn mp3_mono_roundtrip_does_not_crash_lame() {
        // Regression: the mono path must duplicate the channel into both
        // LAME input buffers (a null right channel segfaults LAME), and every
        // encode call must reserve output capacity first.
        let original = tone(440.0, 0.5, 0.5, 1, SR);
        let path = temp_file("mono.mp3");
        write_mp3(&path, &original, Mp3Bitrate::Kbps192).expect("mono encode must succeed");
        let decoded = decode_audio(&path).expect("decode must succeed");
        assert_eq!(decoded.num_channels(), 1, "mono must stay mono");
        assert_eq!(decoded.sample_rate(), SR);
        let expected_frames = (f64::from(SR) * 0.5) as usize;
        assert!(
            (decoded.frames() as i64 - expected_frames as i64).abs() < expected_frames as i64 / 5,
            "decoded {} frames, expected ~{expected_frames}",
            decoded.frames()
        );
        let measured = goertzel_magnitude(&decoded.channels()[0], 440.0, SR as f32);
        assert!(
            (measured - 0.5).abs() < 0.1,
            "tone magnitude {measured}, expected ~0.5"
        );
    }

    #[test]
    fn mp3_write_rejects_empty_buffers() {
        let empty = AudioBuffer::new(0, 0, SR);
        let path = temp_file("empty.mp3");
        let error = write_mp3(&path, &empty, Mp3Bitrate::Kbps128).expect_err("must be rejected");
        assert!(matches!(error, AudioError::EmptyBuffer), "got {error}");
    }

    #[test]
    fn mp3_write_rejects_more_than_two_channels() {
        let three_channel = AudioBuffer::new(3, 8, SR);
        let path = temp_file("three.mp3");
        let error =
            write_mp3(&path, &three_channel, Mp3Bitrate::Kbps128).expect_err("must be rejected");
        assert!(
            matches!(
                error,
                AudioError::UnsupportedChannels {
                    channels: 3,
                    max: 2
                }
            ),
            "got {error}"
        );
    }
}
