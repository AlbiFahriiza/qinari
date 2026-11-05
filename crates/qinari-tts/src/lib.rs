//! Qinari tts crate: offline VITS synthesis, OpenAI-compatible cloud
//! synthesis, and shared text normalization.
//!
//! Two engines implement [`TtsEngine`] over the same
//! [`SynthRequest`]/[`Synthesis`] contract:
//!
//! - [`local::LocalVitsEngine`] runs Piper-class VITS voices through ONNX
//!   Runtime (`ort`), phonemized offline by the pure-Rust eSpeak NG port;
//!   both bundled voices produce 22 050 Hz mono audio.
//! - [`cloud::OpenAiCompatibleEngine`] speaks the `OpenAI`
//!   `POST /audio/speech` protocol, so any compatible service works.
//!
//! Both engines expand numbers first ([`textnorm::expand_numbers`]) and
//! bound each synthesis call with [`textnorm::chunk_text`].
//!
//! All randomness-free, all offline for the local engine: model files and
//! eSpeak data are downloaded once by the CLI and verified against the
//! pinned digests in [`voices`].

pub mod cloud;
pub mod engine;
pub mod local;
pub mod textnorm;
pub mod voices;

pub use engine::{SynthRequest, Synthesis, SynthesisStats, TtsEngine};
pub use textnorm::{chunk_text, expand_numbers, Language};
pub use voices::{default_voice, find_voice, voices, VoiceEntry};

use thiserror::Error;

/// Errors raised while synthesizing speech.
#[derive(Debug, Error)]
pub enum TtsError {
    /// The request text was empty or only whitespace.
    #[error("text is empty")]
    EmptyText,
    /// The request text exceeded the character limit.
    #[error("text is too long ({len} characters; the limit is {max})")]
    TextTooLong {
        /// The offending length in Unicode characters.
        len: usize,
        /// The maximum accepted length.
        max: usize,
    },
    /// The speed factor was outside `0.5..=2.0`.
    #[error("speed {value} is out of range (0.5..=2.0)")]
    SpeedOutOfRange {
        /// The offending speed factor.
        value: f32,
    },
    /// The channel count was neither 1 nor 2.
    #[error("{channels} audio channels requested (only 1 or 2 supported)")]
    BadChannels {
        /// The offending channel count.
        channels: u16,
    },
    /// The sample rate was outside the accepted range.
    #[error("sample rate {rate} is out of range (8000..=192000)")]
    BadSampleRate {
        /// The offending sample rate in hertz.
        rate: u32,
    },
    /// The engine cannot produce audio at the requested rate; the caller
    /// should synthesize at the supported rate and resample afterwards.
    #[error("this voice produces {supported} Hz audio; {requested} Hz was requested (synthesize at {supported} Hz and resample the result instead)")]
    UnsupportedSampleRate {
        /// The requested sample rate in hertz.
        requested: u32,
        /// The voice's native sample rate in hertz.
        supported: u32,
    },
    /// No voice with the given catalog id exists.
    #[error("unknown voice '{id}'")]
    UnknownVoice {
        /// The requested voice id.
        id: String,
    },
    /// No API key was configured for the cloud engine.
    #[error("no API key configured (set QINARI_TTS_API_KEY or OPENAI_API_KEY)")]
    NoApiKey,
    /// The speech service answered with an error status after exhausting
    /// retries.
    #[error("the speech service returned HTTP {status}")]
    Http {
        /// The final HTTP status code.
        status: u16,
    },
    /// The speech service did not answer within the configured timeout.
    #[error("timed out waiting for the speech service")]
    Timeout,
    /// The request could not be delivered to the speech service.
    #[error("network error while contacting the speech service: {detail}")]
    Network {
        /// What went wrong on the wire.
        detail: String,
    },
    /// The speech service answered, but the response was not usable audio.
    #[error("invalid response from the speech service: {detail}")]
    InvalidResponse {
        /// What was wrong with the response.
        detail: String,
    },
    /// The voice configuration file (`.onnx.json`) was missing, unreadable,
    /// or did not match the expected format.
    #[error("invalid voice configuration: {detail}")]
    VoiceConfig {
        /// What was wrong with the configuration.
        detail: String,
    },
    /// The ONNX Runtime environment could not be initialized.
    #[error("failed to initialize the ONNX Runtime")]
    OnnxInit,
    /// ONNX Runtime rejected the model or failed during inference.
    #[error("ONNX Runtime error: {0}")]
    Onnx(#[from] ort::Error),
    /// eSpeak NG failed to load its data or phonemize the text.
    #[error("espeak-ng error: {0}")]
    Espeak(#[from] espeak_ng::Error),
    /// A file could not be read or written.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}
