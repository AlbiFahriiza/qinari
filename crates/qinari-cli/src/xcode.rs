//! Exit codes and the CLI-wide error type.
//!
//! Every failure the binary reports to the user is a [`QinariError`]: a
//! prepared message plus the [`ExitCode`] the process exits with. The
//! full detail of every failure goes to the log file; the terminal only
//! ever sees these prepared messages.

use qinari_audio::format::FormatError;
use std::fmt;

/// Process exit codes used by the `qinari` binary.
///
/// They are a stable contract for scripts; `qinari doctor` uses
/// [`ExitCode::Unhealthy`] so health checks can be wired into cron and CI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// Success.
    Ok = 0,
    /// Unexpected internal error.
    Internal = 1,
    /// CLI usage error (bad arguments or unusable input file).
    Usage = 2,
    /// Configuration / setup error.
    Setup = 3,
    /// Network / download error.
    Network = 4,
    /// Render / DSP error.
    Render = 5,
    /// Encode / format error, including unsupported output format.
    Encode = 6,
    /// TTS engine error.
    Tts = 7,
    /// `qinari doctor` found problems.
    Unhealthy = 8,
}

impl ExitCode {
    /// The numeric value the process exits with.
    pub fn value(self) -> i32 {
        self as i32
    }
}

/// A failure to report to the user: what went wrong and how to exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QinariError {
    /// Prepared user-facing message.
    pub message: String,
    /// Exit code the process should terminate with.
    pub code: ExitCode,
}

impl QinariError {
    /// A CLI usage error (exit 2).
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: ExitCode::Usage,
        }
    }

    /// An unexpected internal error (exit 1).
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: ExitCode::Internal,
        }
    }

    /// A configuration / setup error (exit 3).
    pub fn setup(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: ExitCode::Setup,
        }
    }

    /// A network / download error (exit 4).
    pub fn download(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: ExitCode::Network,
        }
    }

    /// A render / DSP error (exit 5).
    pub fn render(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: ExitCode::Render,
        }
    }

    /// An encode / format error (exit 6).
    pub fn encode(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: ExitCode::Encode,
        }
    }

    /// A TTS engine error (exit 7).
    pub fn tts(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: ExitCode::Tts,
        }
    }
}

impl fmt::Display for QinariError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for QinariError {}

/// An output path that names no supported container is an encode failure:
/// `.wav and .mp3 only`.
impl From<FormatError> for QinariError {
    fn from(error: FormatError) -> Self {
        QinariError::encode(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_the_documented_table() {
        assert_eq!(ExitCode::Ok.value(), 0);
        assert_eq!(ExitCode::Internal.value(), 1);
        assert_eq!(ExitCode::Usage.value(), 2);
        assert_eq!(ExitCode::Setup.value(), 3);
        assert_eq!(ExitCode::Network.value(), 4);
        assert_eq!(ExitCode::Render.value(), 5);
        assert_eq!(ExitCode::Encode.value(), 6);
        assert_eq!(ExitCode::Tts.value(), 7);
        assert_eq!(ExitCode::Unhealthy.value(), 8);
    }

    #[test]
    fn format_errors_are_encode_failures() {
        let error = QinariError::from(FormatError::UnsupportedExtension {
            extension: "flac".to_string(),
        });
        assert_eq!(error.code, ExitCode::Encode);
        assert!(error.to_string().contains(".wav and .mp3 only"), "{error}");
    }

    #[test]
    fn constructors_carry_their_codes() {
        let cases = [
            (QinariError::usage("x"), ExitCode::Usage),
            (QinariError::internal("x"), ExitCode::Internal),
            (QinariError::setup("x"), ExitCode::Setup),
            (QinariError::download("x"), ExitCode::Network),
            (QinariError::render("x"), ExitCode::Render),
            (QinariError::encode("x"), ExitCode::Encode),
            (QinariError::tts("x"), ExitCode::Tts),
        ];
        for (error, code) in cases {
            assert_eq!(error.code, code);
            assert_eq!(error.to_string(), "x");
        }
    }
}
