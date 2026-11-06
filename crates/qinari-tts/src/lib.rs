//! Qinari tts crate: the bundled Supertonic engine, OpenAI-compatible
//! cloud synthesis, and shared text machinery.
//!
//! Engines implement [`TtsEngine`] over the same
//! [`SynthRequest`]/[`Synthesis`] contract:
//!
//! - [`supertonic::SupertonicEngine`] is the bundled local engine: Supertonic 3
//!   driven through four ONNX Runtime (`ort`) sessions, seeded and
//!   reproducible, producing 44 100 Hz mono audio in ten voices
//!   (`supertonic-M1…M5, F1…F5`, all multilingual).
//! - [`cloud::OpenAiCompatibleEngine`] speaks the `OpenAI`
//!   `POST /audio/speech` protocol, so any compatible service works.
//!
//! Both engines expand numbers first ([`textnorm::expand_numbers`]) and
//! bound each synthesis call with [`textnorm::chunk_text`]. Qinari markup
//! (`[laugh]` `[breath]` `[sigh]` `[pause:<ms>]`, see [`markup`]) is parsed
//! before synthesis; unknown tags are hard errors.
//!
//! All local randomness flows through the seeded [`qinari_dsp::rng::Qrng`]:
//! the same [`SynthRequest::seed`] renders the same audio (up to ONNX
//! Runtime float-order nondeterminism). Model files are downloaded once by
//! the CLI and verified against the pinned digests in [`voices`] both at
//! install time and at engine load.

pub mod cloud;
pub mod engine;
pub mod markup;
pub mod prosody;
pub mod supertonic;
pub mod textnorm;
pub mod voices;

pub use engine::{
    SynthRequest, Synthesis, SynthesisStats, TtsEngine, DEFAULT_STEPS, MAX_STEPS, MIN_STEPS,
};
pub use textnorm::{chunk_text, expand_numbers, Language};
pub use voices::{default_voice, find_voice, voices, EngineKind, LicenseClass, VoiceEntry};

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
    /// The denoising-step count was outside `1..=100`.
    #[error("steps {value} is out of range ({min}..={max})")]
    StepsOutOfRange {
        /// The offending step count.
        value: u8,
        /// The lowest accepted step count.
        min: u8,
        /// The highest accepted step count.
        max: u8,
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
    /// The model directory is missing files, unreadable, or fails its
    /// pinned digests.
    #[error("invalid model directory: {detail}")]
    ModelDir {
        /// What is wrong with the directory.
        detail: String,
    },
    /// A voice-style file is missing, unreadable, or malformed.
    #[error("invalid voice style: {detail}")]
    Style {
        /// What is wrong with the style.
        detail: String,
    },
    /// The text contains markup that does not parse.
    #[error("invalid markup: {detail}")]
    Markup {
        /// What is wrong with the markup.
        detail: String,
    },
    /// The engine could not run its graphs or use their output (bad tensor
    /// shape, resampling failure).
    #[error("inference failed: {detail}")]
    Inference {
        /// What failed.
        detail: String,
    },
    /// The ONNX Runtime environment could not be initialized.
    #[error("failed to initialize the ONNX Runtime")]
    OnnxInit,
    /// ONNX Runtime rejected the model or failed during inference.
    #[error("ONNX Runtime error: {0}")]
    Onnx(#[from] ort::Error),
    /// A file could not be read or written.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}
