//! WAV reading and writing via `hound` (16/24-bit int, 32-bit float).

use crate::format::BitDepth;
use crate::{deinterleave, AudioError};
use qinari_dsp::buffer::AudioBuffer;
use std::path::Path;

/// Full scale of a 16-bit sample.
const I16_SCALE: f32 = 32_768.0;
/// Full scale of a 24-bit sample.
const I24_SCALE: f32 = 8_388_608.0;

/// Reads a WAV file into an [`AudioBuffer`].
///
/// 16-bit and 24-bit integer files are normalized to `[-1, 1)` floats;
/// 32-bit float files pass through unchanged.
///
/// # Errors
///
/// Returns [`AudioError::Wav`] for malformed files, [`AudioError::Io`] when
/// the file cannot be opened, [`AudioError::UnsupportedDepth`] for any depth
/// other than 16/24-bit int or 32-bit float, and [`AudioError::Decode`] when
/// the sample data is not a whole number of frames.
pub fn read_wav<P: AsRef<Path>>(path: P) -> Result<AudioBuffer, AudioError> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels);
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|sample| sample.map(|value| f32::from(value) / I16_SCALE))
            .collect::<Result<Vec<f32>, _>>()?,
        (hound::SampleFormat::Int, 24) => reader
            .samples::<i32>()
            .map(|sample| sample.map(|value| value as f32 / I24_SCALE))
            .collect::<Result<Vec<f32>, _>>()?,
        (hound::SampleFormat::Float, 32) => {
            reader.samples::<f32>().collect::<Result<Vec<f32>, _>>()?
        }
        (sample_format, bits) => {
            return Err(AudioError::UnsupportedDepth {
                bits,
                sample_format: if sample_format == hound::SampleFormat::Float {
                    "float"
                } else {
                    "int"
                },
            })
        }
    };
    deinterleave(&samples, channels, spec.sample_rate)
}

/// Writes an [`AudioBuffer`] to a WAV file at the given [`BitDepth`].
///
/// Integer depths clamp samples to full scale and quantize to the nearest
/// step; `Float32` writes the samples unchanged. Callers that dither should
/// run [`crate::master::finalize_for_depth`] first so the dither survives
/// quantization exactly.
///
/// # Errors
///
/// Returns [`AudioError::EmptyBuffer`] for zero-channel or zero-frame
/// buffers, [`AudioError::UnsupportedChannels`] above `65_535` channels,
/// [`AudioError::Wav`] for writer failures, and [`AudioError::Io`] when the
/// file cannot be created.
pub fn write_wav<P: AsRef<Path>>(
    path: P,
    buffer: &AudioBuffer,
    depth: BitDepth,
) -> Result<(), AudioError> {
    let channels = buffer.num_channels();
    if channels == 0 || buffer.frames() == 0 {
        return Err(AudioError::EmptyBuffer);
    }
    if channels > usize::from(u16::MAX) {
        return Err(AudioError::UnsupportedChannels {
            channels,
            max: usize::from(u16::MAX),
        });
    }
    let spec = hound::WavSpec {
        channels: channels as u16,
        sample_rate: buffer.sample_rate(),
        bits_per_sample: match depth {
            BitDepth::Bits16 => 16,
            BitDepth::Bits24 => 24,
            BitDepth::Float32 => 32,
        },
        sample_format: match depth {
            BitDepth::Float32 => hound::SampleFormat::Float,
            BitDepth::Bits16 | BitDepth::Bits24 => hound::SampleFormat::Int,
        },
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for frame in 0..buffer.frames() {
        for channel in 0..channels {
            let sample = buffer.get(channel, frame);
            match depth {
                BitDepth::Bits16 => writer.write_sample(quantize_i16(sample))?,
                BitDepth::Bits24 => writer.write_sample(quantize_i24(sample))?,
                BitDepth::Float32 => writer.write_sample(sample)?,
            }
        }
    }
    writer.finalize()?;
    Ok(())
}

/// Clamps to full scale and quantizes to the 16-bit grid.
fn quantize_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * I16_SCALE)
        .round()
        .clamp(-I16_SCALE, I16_SCALE - 1.0) as i16
}

/// Clamps to full scale and quantizes to the 24-bit grid.
fn quantize_i24(sample: f32) -> i32 {
    (sample.clamp(-1.0, 1.0) * I24_SCALE)
        .round()
        .clamp(-I24_SCALE, I24_SCALE - 1.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{temp_file, tone};
    use qinari_dsp::analysis::goertzel_magnitude;

    const SR: u32 = 48_000;

    fn write_read_roundtrip(depth: BitDepth, tolerance: f32) -> AudioBuffer {
        let original = tone(997.0, 0.6, 0.25, 2, SR);
        let path = temp_file(&format!("roundtrip-{depth:?}.wav"));
        write_wav(&path, &original, depth).expect("write must succeed");
        let roundtrip = read_wav(&path).expect("read must succeed");
        assert_eq!(roundtrip.num_channels(), 2);
        assert_eq!(roundtrip.sample_rate(), SR);
        assert_eq!(roundtrip.frames(), original.frames());
        for channel in 0..2 {
            for frame in 0..original.frames() {
                let error = (roundtrip.get(channel, frame) - original.get(channel, frame)).abs();
                assert!(
                    error <= tolerance,
                    "{depth:?} channel {channel} frame {frame}: error {error} > {tolerance}"
                );
            }
        }
        // The tone must survive at its original amplitude.
        for channel in 0..2 {
            let measured = goertzel_magnitude(&roundtrip.channels()[channel], 997.0, SR as f32);
            assert!(
                (measured - 0.6).abs() < 0.01,
                "{depth:?} channel {channel} tone {measured}"
            );
        }
        roundtrip
    }

    #[test]
    fn wav_16_bit_roundtrip_preserves_the_signal() {
        write_read_roundtrip(BitDepth::Bits16, 1.0 / I16_SCALE + 1.0e-6);
    }

    #[test]
    fn wav_24_bit_roundtrip_preserves_the_signal() {
        write_read_roundtrip(BitDepth::Bits24, 1.0 / I24_SCALE + 1.0e-7);
    }

    #[test]
    fn wav_float_roundtrip_is_bit_exact() {
        let roundtrip = write_read_roundtrip(BitDepth::Float32, 0.0);
        let original = tone(997.0, 0.6, 0.25, 2, SR);
        assert_eq!(roundtrip.channels(), original.channels());
    }

    #[test]
    fn wav_integer_depths_clamp_over_scale_input() {
        let hot = qinari_dsp::buffer::AudioBuffer::from_channels(
            vec![vec![1.5, -1.5, 0.25, f32::MAX]],
            SR,
        )
        .expect("well formed");
        for depth in [BitDepth::Bits16, BitDepth::Bits24] {
            let path = temp_file(&format!("clamp-{depth:?}.wav"));
            write_wav(&path, &hot, depth).expect("clamping must keep the write in range");
            let read = read_wav(&path).expect("read back");
            assert!(read.peak() <= 1.0, "{depth:?} peak {}", read.peak());
        }
        // Float depth preserves the over-scale values verbatim.
        let path = temp_file("clamp-float.wav");
        write_wav(&path, &hot, BitDepth::Float32).expect("float write");
        let read = read_wav(&path).expect("read back");
        assert_eq!(read.get(0, 0), 1.5);
    }

    #[test]
    fn wav_read_rejects_unsupported_depths() {
        let path = temp_file("depth8.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SR,
            bits_per_sample: 8,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).expect("create");
        for _ in 0..16 {
            writer.write_sample(0i8).expect("write");
        }
        writer.finalize().expect("finalize");
        let error = read_wav(&path).expect_err("8-bit must be rejected");
        assert!(
            matches!(error, AudioError::UnsupportedDepth { bits: 8, .. }),
            "got {error}"
        );
    }

    #[test]
    fn wav_write_rejects_empty_buffers() {
        let empty = AudioBuffer::new(0, 0, SR);
        let path = temp_file("empty.wav");
        let error = write_wav(&path, &empty, BitDepth::Bits16).expect_err("must be rejected");
        assert!(matches!(error, AudioError::EmptyBuffer), "got {error}");
        let silent = AudioBuffer::new(2, 0, SR);
        let error = write_wav(&path, &silent, BitDepth::Bits24).expect_err("must be rejected");
        assert!(matches!(error, AudioError::EmptyBuffer), "got {error}");
    }
}
