//! Measurement and verification for the Qinari audio engines.
//!
//! This crate is the "engine grows eyes" stage (VERIFY-1) of the realism
//! design: pure, deterministic feature extraction over
//! [`qinari_dsp::buffer::AudioBuffer`]s, plus the speech feature set
//! (F0 / pauses / DNSMOS) that gates the expressive TTS engines.
//!
//! Feature families, all pure functions of a buffer (design
//! `DESIGN-REALISM-ENGINE.md` §6, `DESIGN-TTS-EXPRESSIVE.md` §3.6):
//!
//! - [`spectrum`]: Welch power spectrum, `rolloff95`, named band energies,
//!   and a 24-band log-spaced band set;
//! - [`envelopes`]: per-band envelope time series from STFT magnitudes and
//!   their moments (mean, variance, sparsity, kurtosis);
//! - [`modulation`]: envelope spectra per band, the broadband gain
//!   modulation summary (the exactly-periodic-LFO discriminator), and the
//!   cross-band correlation matrix (`McDermott` C1 statistics);
//! - [`events`]: onset detection by envelope derivative, event rate,
//!   inter-onset CV, burstiness;
//! - [`interaural`]: interaural correlation per named band (the §7
//!   acceptance metric: below 0.9 above 500 Hz);
//! - [`speech`]: F0 track (autocorrelation), F0 σ/range in semitones, and
//!   pause statistics;
//! - [`dnsmos`]: DNSMOS P.835/P.808 scoring through ONNX Runtime, ported
//!   from Microsoft's `dnsmos_local.py` (non-personalized mode);
//! - [`distances`]: log-spectral distance, multi-resolution STFT loss
//!   (Yamamoto-style), and the weighted texture-statistic distance;
//! - [`golden`]: golden-target files with per-feature tolerances and the
//!   [`golden::check_against`] gate.
//!
//! [`analyze_buffer`] bundles everything except DNSMOS into a
//! [`report::FeatureReport`]; [`analyze_file`] decodes a `.wav`/`.mp3` file
//! through [`qinari_audio`] first. All computations are deterministic:
//! the same buffer always produces a byte-identical report.
//!
//! ```no_run
//! use qinari_verify::{analyze_file, golden};
//!
//! let report = analyze_file("renders/rain.wav").expect("decodes");
//! let json = serde_json::to_string(&report).expect("serializes");
//! # let _ = json;
//! ```

#![forbid(unsafe_code)]

pub mod distances;
pub mod dnsmos;
pub mod envelopes;
pub mod events;
pub mod golden;
pub mod interaural;
pub mod modulation;
pub mod report;
pub mod spectrum;
pub mod speech;
pub(crate) mod stft;

pub use dnsmos::{dnsmos, DnsmosAsset, DnsmosScorer, DnsmosScores, DNSMOS_ASSETS};
pub use envelopes::{BandMoments, EnvelopeReport};
pub use events::EventReport;
pub use golden::{check_against, load_target, TargetFile, Tolerance, Violation};
pub use interaural::InterauralReport;
pub use modulation::{BandModulation, ModulationReport};
pub use qinari_audio::decode_audio;
pub use report::FeatureReport;
pub use spectrum::{BandFractions, SpectrumReport};

use qinari_dsp::buffer::AudioBuffer;
use std::path::Path;
use thiserror::Error;

/// Errors raised while measuring or verifying audio.
#[derive(Debug, Error)]
pub enum VerifyError {
    /// The DNSMOS ONNX Runtime environment could not be created.
    ///
    /// `ort` allows one environment per process; a second crate that
    /// already initialized it (for example `qinari-tts` in the same
    /// process) makes this crate's initialization fail. Run each
    /// subcommand in its own process, as the CLI does.
    #[error("could not initialize ONNX Runtime (only one environment is allowed per process)")]
    OnnxInit,
    /// ONNX Runtime rejected a model or an inference call.
    #[error("onnx runtime error: {0}")]
    Onnx(#[from] ort::Error),
    /// A model inference returned something unexpected.
    #[error("inference failed: {detail}")]
    Inference {
        /// What the model returned instead of the expected shape.
        detail: String,
    },
    /// A DNSMOS model file is missing from the models directory.
    #[error("DNSMOS model missing: {path}")]
    ModelMissing {
        /// The path that was expected to hold the model.
        path: std::path::PathBuf,
    },
    /// The buffer has no frames, so it cannot be scored or compared.
    #[error("cannot measure an empty buffer (zero frames)")]
    EmptyBuffer,
    /// Two buffers were compared at different sample rates.
    #[error("sample rates differ: {a} Hz vs {b} Hz")]
    MismatchedSampleRate {
        /// Sample rate of the first buffer.
        a: u32,
        /// Sample rate of the second buffer.
        b: u32,
    },
    /// An audio file could not be decoded or resampled.
    #[error("audio error: {0}")]
    Audio(#[from] qinari_audio::AudioError),
    /// A golden target file could not be parsed.
    #[error("could not parse target: {0}")]
    TargetParse(#[from] serde_json::Error),
    /// A file could not be read or written.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// A golden target references a feature the report does not carry.
    #[error("unknown feature path: {path}")]
    UnknownFeature {
        /// The offending dotted path.
        path: String,
    },
}

/// Analyzes a decoded buffer into a full feature report.
///
/// DNSMOS is not part of the report (it needs the ONNX models); use
/// [`dnsmos`] or [`DnsmosScorer`] separately.
///
/// Every value is deterministic for the same input; buffers with zero
/// frames are legal and produce degenerate (all-zero / null) statistics.
pub fn analyze_buffer(buffer: &AudioBuffer) -> FeatureReport {
    let mono = mono_mixdown(buffer);
    let spectrum = spectrum::analyze(&mono, buffer.sample_rate());
    let (envelopes, band_envelopes) = envelopes::analyze(&mono, buffer.sample_rate());
    let modulation = modulation::analyze(&band_envelopes);
    let events = events::analyze(&band_envelopes.gain_db, band_envelopes.env_rate);
    let interaural = interaural::interaural_correlation(buffer);
    let speech = speech::analyze(&mono, buffer.sample_rate());
    FeatureReport {
        sample_rate: buffer.sample_rate(),
        frames: buffer.frames(),
        channels: buffer.channels().len(),
        duration_s: buffer.duration(),
        spectrum,
        envelopes,
        modulation,
        events,
        interaural,
        speech,
    }
}

/// Decodes a `.wav`/`.mp3` file and analyzes it.
///
/// # Errors
///
/// Returns [`VerifyError::Audio`] when the file cannot be opened or
/// decoded.
pub fn analyze_file<P: AsRef<Path>>(path: P) -> Result<FeatureReport, VerifyError> {
    let buffer = qinari_audio::decode_audio(path)?;
    Ok(analyze_buffer(&buffer))
}

/// Averages all channels into one `f32` mono signal.
///
/// Mono buffers are cloned; buffers with zero channels yield an empty
/// signal.
pub(crate) fn mono_mixdown(buffer: &AudioBuffer) -> Vec<f32> {
    let frames = buffer.frames();
    let channels = buffer.channels();
    let mut mono = vec![0.0f32; frames];
    if channels.is_empty() {
        return mono;
    }
    for channel in channels {
        for (slot, &sample) in mono.iter_mut().zip(channel.iter()) {
            *slot += sample;
        }
    }
    let scale = 1.0 / channels.len() as f32;
    for slot in &mut mono {
        *slot *= scale;
    }
    mono
}
