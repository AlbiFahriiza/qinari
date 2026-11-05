//! Output format selection: extension validation, bit depths, mp3 bitrates.

use std::path::Path;
use thiserror::Error;

/// Error returned for an output path that names no supported container.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FormatError {
    /// The path's extension was neither `.wav` nor `.mp3`.
    ///
    /// The message always contains the literal text `.wav and .mp3 only`,
    /// which callers surface to the user verbatim.
    #[error("unsupported output format {:?}: .wav and .mp3 only", extension)]
    UnsupportedExtension {
        /// The extension that was rejected (`""` when the path has none).
        extension: String,
    },
}

/// The container an output path selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    /// RIFF/WAVE, written by [`crate::wav::write_wav`].
    Wav,
    /// MPEG-1 audio layer III, written by [`crate::mp3::write_mp3`].
    Mp3,
}

/// The sample format of a written WAV file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitDepth {
    /// 16-bit signed integers with noise-shaped TPDF dither.
    Bits16,
    /// 24-bit signed integers with flat TPDF dither.
    Bits24,
    /// 32-bit IEEE floats, no requantization.
    Float32,
}

/// The constant bitrates the mp3 encoder supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp3Bitrate {
    /// 128 kbps.
    Kbps128,
    /// 192 kbps.
    Kbps192,
    /// 256 kbps.
    Kbps256,
    /// 320 kbps.
    Kbps320,
}

impl Mp3Bitrate {
    /// Returns the bitrate in kilobits per second.
    pub const fn kbps(self) -> u32 {
        match self {
            Mp3Bitrate::Kbps128 => 128,
            Mp3Bitrate::Kbps192 => 192,
            Mp3Bitrate::Kbps256 => 256,
            Mp3Bitrate::Kbps320 => 320,
        }
    }
}

/// Validates that `path` ends in `.wav` or `.mp3` (case-insensitive) and
/// returns the container it selects.
///
/// # Errors
///
/// Returns [`FormatError::UnsupportedExtension`] for any other extension
/// (including no extension at all); the message names `.wav and .mp3 only`.
pub fn validate_output_path(path: &Path) -> Result<AudioFormat, FormatError> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match extension.as_str() {
        "wav" => Ok(AudioFormat::Wav),
        "mp3" => Ok(AudioFormat::Mp3),
        _ => Err(FormatError::UnsupportedExtension { extension }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn accepts_wav_and_mp3_in_any_letter_case() {
        assert_eq!(
            validate_output_path(Path::new("out.wav")),
            Ok(AudioFormat::Wav)
        );
        assert_eq!(
            validate_output_path(Path::new("out.mp3")),
            Ok(AudioFormat::Mp3)
        );
        assert_eq!(
            validate_output_path(Path::new("/tmp/OUT.WAV")),
            Ok(AudioFormat::Wav)
        );
        assert_eq!(
            validate_output_path(Path::new("Song.MP3")),
            Ok(AudioFormat::Mp3)
        );
        assert_eq!(
            validate_output_path(Path::new("dir.with.dots/sound.wav")),
            Ok(AudioFormat::Wav)
        );
    }

    #[test]
    fn rejects_other_extensions_with_the_required_message() {
        for path in ["out.flac", "out.ogg", "out", "out.wav.bak", "out."] {
            let error = validate_output_path(Path::new(path));
            let error = error.expect_err("path must be rejected");
            assert!(
                error.to_string().contains(".wav and .mp3 only"),
                "message for {path:?} was {error}"
            );
        }
        let error = validate_output_path(Path::new("out.flac")).unwrap_err();
        assert_eq!(
            error,
            FormatError::UnsupportedExtension {
                extension: "flac".to_string()
            }
        );
    }

    #[test]
    fn bitrate_accessors_report_kbps() {
        assert_eq!(Mp3Bitrate::Kbps128.kbps(), 128);
        assert_eq!(Mp3Bitrate::Kbps320.kbps(), 320);
    }
}
