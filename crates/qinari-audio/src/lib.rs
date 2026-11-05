//! Qinari audio crate: file I/O, resampling, decoding, and mastering.
//!
//! This crate is the I/O and delivery layer of Qinari. It renders nothing
//! itself; it takes finished [`qinari_dsp::buffer::AudioBuffer`]s and moves
//! them between memory and disk:
//!
//! - [`mod@format`] validates output paths and selects the container/bitrate;
//! - [`wav`] and [`mp3`] encode buffers to `.wav` (16/24-bit int, 32-bit
//!   float) and `.mp3` (CBR 128..320 kbps) files;
//! - [`decode`] decodes `.wav` and `.mp3` files back into buffers;
//! - [`mod@resample`] converts between sample rates with an FFT resampler;
//! - [`mod@master`] loudness-targets a buffer through a character chain with a
//!   verification loop, and [`master::finalize_for_depth`] dithers the result
//!   for the target bit depth.
//!
//! All randomness in the pipeline flows through the seeded
//! [`qinari_dsp::rng::Qrng`], so identical inputs produce identical files.

pub mod decode;
pub mod format;
pub mod master;
pub mod mp3;
pub mod resample;
pub mod wav;

pub use decode::decode_audio;
pub use format::{validate_output_path, AudioFormat, BitDepth, FormatError, Mp3Bitrate};
pub use master::{finalize_for_depth, master, Character, MasteredReport, MasteringSettings};
pub use mp3::write_mp3;
pub use resample::resample;
pub use wav::{read_wav, write_wav};

use thiserror::Error;

/// Errors raised while reading, writing, or processing audio files.
#[derive(Debug, Error)]
pub enum AudioError {
    /// A file could not be opened, read, or written.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// `hound` failed to read or write a WAV file.
    #[error("wav error: {0}")]
    Wav(#[from] hound::Error),
    /// The LAME mp3 encoder rejected its parameters or failed mid-stream.
    #[error("mp3 encoding failed: {0}")]
    Mp3Encode(String),
    /// The resampler rejected a rate pair or failed mid-stream.
    #[error("resampling failed: {0}")]
    Resample(String),
    /// A file could not be probed or decoded.
    #[error("decoding failed: {0}")]
    Decode(String),
    /// A buffer with zero channels or zero frames cannot be encoded.
    #[error("cannot encode an empty buffer (zero channels or zero frames)")]
    EmptyBuffer,
    /// The buffer has more channels than the format supports.
    #[error("unsupported channel count {channels} (at most {max} supported)")]
    UnsupportedChannels {
        /// The offending channel count.
        channels: usize,
        /// The maximum this format supports.
        max: usize,
    },
    /// The WAV file uses a bit depth outside 16/24-bit int and 32-bit float.
    #[error("unsupported wav sample depth: {bits}-bit {sample_format}")]
    UnsupportedDepth {
        /// The offending bit depth.
        bits: u16,
        /// Whether the samples are stored as `int` or `float`.
        sample_format: &'static str,
    },
}

/// Splits interleaved sample frames into planar channels.
pub(crate) fn deinterleave(
    samples: &[f32],
    channels: usize,
    sample_rate: u32,
) -> Result<qinari_dsp::buffer::AudioBuffer, AudioError> {
    if channels == 0 || samples.len() % channels != 0 {
        return Err(AudioError::Decode(format!(
            "sample count {} is not a whole number of {}-channel frames",
            samples.len(),
            channels
        )));
    }
    let frames = samples.len() / channels;
    let mut planes = vec![Vec::with_capacity(frames); channels];
    for chunk in samples.chunks_exact(channels) {
        for (channel, &sample) in chunk.iter().enumerate() {
            planes[channel].push(sample);
        }
    }
    qinari_dsp::buffer::AudioBuffer::from_channels(planes, sample_rate)
        .map_err(|error| AudioError::Decode(error.to_string()))
}

#[cfg(test)]
pub(crate) mod test_support {
    use qinari_dsp::buffer::AudioBuffer;
    use std::path::PathBuf;

    /// Returns a unique temporary file path for a test artifact.
    ///
    /// Files are written into the system temp directory and named per test;
    /// they are intentionally not cleaned up (they are tiny and the OS
    /// handles its own temp hygiene).
    pub(crate) fn temp_file(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("qinari-audio-{}-{name}", std::process::id()));
        path
    }

    /// Renders an identical sine tone on every channel.
    pub(crate) fn tone(
        freq: f32,
        amplitude: f32,
        secs: f32,
        channels: usize,
        sample_rate: u32,
    ) -> AudioBuffer {
        let frames = (secs * sample_rate as f32).round() as usize;
        let mut planes = Vec::with_capacity(channels);
        for _ in 0..channels {
            planes.push(
                (0..frames)
                    .map(|n| {
                        amplitude
                            * (2.0 * std::f32::consts::PI * freq * n as f32 / sample_rate as f32)
                                .sin()
                    })
                    .collect(),
            );
        }
        AudioBuffer::from_channels(planes, sample_rate).expect("test buffer is well formed")
    }
}
