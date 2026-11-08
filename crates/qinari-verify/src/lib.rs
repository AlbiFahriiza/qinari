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
//!   cross-band correlation matrix — the legacy STFT-band form of the
//!   texture model's **C** statistic (the 0.1.5 docs mislabeled it as
//!   the paper's C1; finding F13);
//! - [`texture_stats`]: the McDermott–Simoncelli 2011 texture statistics
//!   on a 32-channel ERB cochlear front-end — cochlear marginal moments
//!   M1–M4, **C** cross-band correlations, modulation power, **C1**
//!   cross-frequency and complex **C2** across-band correlations (1515
//!   statistics; every parameter frozen with paper citations in
//!   `docs/VERIFICATION-0.2.0.md`), plus **AC**, labeled a Qinari
//!   extension because it is not part of the 2011 model;
//! - [`events`]: onset detection by envelope derivative, event rate,
//!   inter-onset CV, burstiness;
//! - [`interaural`]: interaural correlation per band — the four named
//!   bands (the §7 acceptance metric: below 0.9 above 500 Hz) plus the
//!   24 log-spaced analysis bands;
//! - [`speech`]: F0 track (autocorrelation), F0 σ/range in semitones, and
//!   pause statistics;
//! - [`dnsmos`]: DNSMOS P.835/P.808 scoring through ONNX Runtime, ported
//!   from Microsoft's `dnsmos_local.py` (non-personalized mode);
//! - [`distances`]: log-spectral distance, multi-resolution STFT loss
//!   (Yamamoto-style), and the weighted texture-statistic distance;
//! - [`golden`]: golden-target files with per-feature tolerances and the
//!   [`golden::check_against`] gate.
//!
//! Stereo-aware analysis (P0-3): every channel is measured on its own.
//! The report carries per-channel sections suffixed `_l` (channel 0)
//! and `_r` (channel 1, `null` for mono); the pre-P0-3 section names
//! survive unchanged and carry the per-statistic median across
//! channels, so old consumers keep reading them. Aggregates span all
//! channels — the `_l`/`_r` sections cover the first two only. Two
//! families are not per-channel by definition: interaural correlation
//! is cross-channel (it exists only between channels), and speech (F0,
//! pauses) describes one voice, so it stays on the mono mixdown — the
//! only family that does.

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

pub(crate) mod cochlea;
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
pub use modulation::{BandModulation, ModulationReport, TextureStats};
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
/// Stereo-aware (P0-3): spectrum, envelopes, modulation, and events are
/// measured per channel and aggregated by the per-statistic median
/// across channels. The pre-P0-3 section names carry that aggregate;
/// the per-channel sections ride along as `_l` (channel 0) and `_r`
/// (channel 1, `None` for mono). Interaural correlation stays
/// cross-channel, and speech stays on the mono mixdown (F0 and pauses
/// are monophonic by definition — one voice).
///
/// Every value is deterministic for the same input; buffers with zero
/// frames are legal and produce degenerate (all-zero / null) statistics.
pub fn analyze_buffer(buffer: &AudioBuffer) -> FeatureReport {
    let sample_rate = buffer.sample_rate();
    // One analysis pass per channel; the four family vectors are
    // allocated up front, one slot per channel. A zero-channel buffer
    // analyzes one empty (silent) signal — exactly what the retired
    // premature mixdown measured for it, because a zero-channel buffer
    // always has zero frames.
    let channels = buffer.channels();
    let mut spectra = Vec::with_capacity(channels.len().max(1));
    let mut envelope_sections = Vec::with_capacity(spectra.capacity());
    let mut modulations = Vec::with_capacity(spectra.capacity());
    let mut event_sections = Vec::with_capacity(spectra.capacity());
    let mut analyze_one = |signal: &[f32]| {
        let (envelopes, data) = envelopes::analyze(signal, sample_rate);
        spectra.push(spectrum::analyze(signal, sample_rate));
        envelope_sections.push(envelopes);
        modulations.push(modulation::analyze(&data));
        event_sections.push(events::analyze(&data.gain_db, data.env_rate));
    };
    if channels.is_empty() {
        analyze_one(&[]);
    } else {
        for channel in channels {
            analyze_one(channel);
        }
    }
    // The pre-P0-3 section names: per-statistic median across channels.
    let spectrum = spectrum::aggregate(&spectra);
    let envelopes = envelopes::aggregate(&envelope_sections);
    let modulation = modulation::aggregate(&modulations);
    let events = events::aggregate(&event_sections);
    let (spectrum_l, spectrum_r) = first_two(spectra);
    let (envelopes_l, envelopes_r) = first_two(envelope_sections);
    let (modulation_l, modulation_r) = first_two(modulations);
    let (events_l, events_r) = first_two(event_sections);
    // Cross-channel by definition: interaural correlation is measured
    // between the leading channel pair only (`None` for mono).
    let interaural = interaural::interaural_correlation(buffer);
    // Monophonic by definition: one voice, so F0 and pauses stay on the
    // mixdown — the only family that does.
    let speech = speech::analyze(&mono_mixdown(buffer), sample_rate);
    FeatureReport {
        sample_rate,
        frames: buffer.frames(),
        channels: channels.len(),
        duration_s: buffer.duration(),
        spectrum,
        spectrum_l,
        spectrum_r,
        envelopes,
        envelopes_l,
        envelopes_r,
        modulation,
        modulation_l,
        modulation_r,
        events,
        events_l,
        events_r,
        interaural,
        speech,
    }
}

/// Measures the 2011 McDermott–Simoncelli texture statistics of a
/// buffer: the full 1515-number set (cochlear marginals M1–M4, C
/// cross-band correlations, marginal-bank modulation power, C1
/// cross-frequency and complex C2 across-band correlations) on the
/// 32-channel ERB cochlear front-end, plus the AC (Qinari extension)
/// envelope autocorrelations.
///
/// The cochlear front-end analyzes at the buffer's native sample rate
/// (clipped to the 2011 span of 20–10000 Hz); every parameter is frozen
/// with its primary-source citation in `docs/VERIFICATION-0.2.0.md`.
/// Deterministic: the same buffer always produces the same numbers.
/// Degenerate inputs (empty buffers, rates without cochlear resolution,
/// silence) measure all zeros, never NaN.
///
/// Stereo-aware (P0-3): the 2011 statistics are within-channel by
/// construction ([S1] analyzes one waveform), so every channel measures
/// its own gram ([`texture_stats_channels`]) and this function returns
/// the per-statistic median across channels — the same treatment as the
/// report families. For mono buffers the value is exactly what the
/// retired premature mixdown measured.
pub fn texture_stats(buffer: &AudioBuffer) -> TextureStats {
    modulation::aggregate_texture(&texture_stats_channels(buffer))
}

/// The per-channel texture statistics: one entry per channel, in channel
/// order (channel 0 = "L", channel 1 = "R").
///
/// Zero-channel buffers measure the degenerate gram of one empty
/// (silent) signal, matching what the retired premature mixdown
/// measured for them.
pub fn texture_stats_channels(buffer: &AudioBuffer) -> Vec<TextureStats> {
    let sample_rate = buffer.sample_rate();
    if buffer.channels().is_empty() {
        return vec![modulation::texture_stats(&cochlea::analyze(
            &[],
            sample_rate,
        ))];
    }
    buffer
        .channels()
        .iter()
        .map(|channel| modulation::texture_stats(&cochlea::analyze(channel, sample_rate)))
        .collect()
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
/// P0-3 retired the premature mixdown from the feature pipeline; the
/// mixdown remains where the signal is monophonic by definition: the
/// speech analysis (one voice) and the DNSMOS model input (the model's
/// waveform contract), plus the two waveform-comparison distances
/// (log-spectral and multi-resolution STFT), whose comparison domain
/// is the mono waveform and is unchanged. The texture distance reads
/// the report's aggregate sections, so it follows the per-channel
/// aggregate like every old field. Mono buffers are cloned; buffers
/// with zero channels yield an empty signal.
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

/// Median across channels: the middle value after an in-place sort for
/// odd counts, `f64::midpoint` of the two middle values for even counts
/// (the overflow-safe mean).
///
/// Equal values stay bit-exact — `midpoint(a, a)` is `a` — so a
/// symmetric stereo buffer (L == R) reproduces the numbers the retired
/// premature mixdown measured for it. Empty input measures 0.0;
/// callers always pass at least one channel value.
pub(crate) fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        f64::midpoint(values[n / 2 - 1], values[n / 2])
    }
}

/// Gathers one per-channel statistic through `scratch` (cleared) and
/// returns its [`median`] across channels.
pub(crate) fn median_stat<T>(
    scratch: &mut Vec<f64>,
    reports: &[T],
    get: impl Fn(&T) -> f64,
) -> f64 {
    scratch.clear();
    scratch.extend(reports.iter().map(get));
    median(scratch)
}

/// Median of an optional per-channel statistic: quoted only when every
/// channel quotes it. One channel withholding the statistic (an event
/// section below its two-event floor, say) withholds the aggregate too
/// — the aggregate cannot claim support the channels lack.
pub(crate) fn median_option<T>(
    scratch: &mut Vec<f64>,
    reports: &[T],
    get: impl Fn(&T) -> Option<f64>,
) -> Option<f64> {
    scratch.clear();
    let mut quoted_by_all = true;
    for report in reports {
        match get(report) {
            Some(value) => scratch.push(value),
            None => quoted_by_all = false,
        }
    }
    if quoted_by_all {
        Some(median(scratch))
    } else {
        None
    }
}

/// Splits a per-channel family vector into the L (channel 0) and R
/// (channel 1, `None` for mono) sections.
///
/// # Panics
///
/// Panics on an empty vector — a programming error: the zero-channel
/// case synthesizes one silent channel, so every family vector holds
/// at least one report.
fn first_two<T>(family: Vec<T>) -> (T, Option<T>) {
    let mut sections = family.into_iter();
    (
        sections
            .next()
            .expect("nonempty: zero channels synthesize one"),
        sections.next(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::f64::consts::PI;

    const SR: u32 = 44_100;

    fn stereo(l: Vec<f32>, r: Vec<f32>) -> AudioBuffer {
        AudioBuffer::from_channels(vec![l, r], SR).expect("well formed")
    }

    fn mono(signal: Vec<f32>) -> AudioBuffer {
        AudioBuffer::from_channels(vec![signal], SR).expect("well formed")
    }

    /// A 2 Hz raised-cosine burst train on a carrier: voiced for the F0
    /// track, onsets for the event detector, deterministic.
    fn bursts(carrier_hz: f64, amp: f32, secs: f64) -> Vec<f32> {
        let n = (secs * f64::from(SR)).round() as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(SR);
                let ph = (t * 2.0).fract();
                let gate = if ph < 0.5 {
                    let rise = (2.0 * PI * ph).sin();
                    rise * rise
                } else {
                    0.0
                };
                (f64::from(amp) * gate * (2.0 * PI * carrier_hz * t).sin()) as f32
            })
            .collect()
    }

    fn steady_tone(hz: f64, amp: f32, secs: f64) -> Vec<f32> {
        let n = (secs * f64::from(SR)).round() as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(SR);
                (f64::from(amp) * (2.0 * PI * hz * t).sin()) as f32
            })
            .collect()
    }

    /// Index of the 24-band set holding `hz`.
    fn band_of(hz: f64) -> usize {
        let edges = spectrum::band24_edges(SR);
        edges
            .windows(2)
            .position(|pair| hz >= pair[0] && hz < pair[1])
            .unwrap_or(edges.len() - 2)
    }

    #[test]
    fn asymmetric_lr_is_detected_in_the_per_channel_sections() {
        // L: 220 Hz bursts (low band, eventful); R: a steady 3 kHz tone
        // (high band, still). The retired mixdown averaged the difference
        // away; the per-channel sections must now read it.
        let l = bursts(220.0, 0.3, 2.0);
        let r = steady_tone(3000.0, 0.3, 2.0);
        let report = analyze_buffer(&stereo(l, r));

        // Spectrum: L puts its energy below 500 Hz, R above 2 kHz.
        assert!(
            report.spectrum_l.band_energy_fraction.b0_500 > 0.8,
            "L: {:?}",
            report.spectrum_l.band_energy_fraction
        );
        let spectrum_r = report.spectrum_r.as_ref().expect("stereo");
        assert!(spectrum_r.band_energy_fraction.b0_500 < 0.05);
        assert!(
            spectrum_r.band_energy_fraction.b2000_8000
                + spectrum_r.band_energy_fraction.b8000_nyquist
                > 0.9
        );

        // Envelopes: the L carrier band swings with the gate; the R
        // band is still.
        let low_band = band_of(220.0);
        let high_band = band_of(3000.0);
        assert!(
            report.envelopes_l.bands[low_band].variance > 10.0,
            "L carrier band: {:?}",
            report.envelopes_l.bands[low_band]
        );
        let envelopes_r = report.envelopes_r.as_ref().expect("stereo");
        // The R carrier band carries only the 3 kHz tone window-leakage
        // floor — a still, near-silent band whose dB floor jitters some
        // 18 dB². That is two-plus orders of magnitude below the
        // gate-swinging L band, and the R (tone) band is essentially
        // frozen.
        assert!(
            envelopes_r.bands[low_band].variance * 100.0
                < report.envelopes_l.bands[low_band].variance,
            "R low-band floor {} vs L {}",
            envelopes_r.bands[low_band].variance,
            report.envelopes_l.bands[low_band].variance
        );
        assert!(envelopes_r.bands[high_band].variance < 0.001);

        // Modulation: the L carrier band peaks at the 2 Hz gate.
        let l_peak = report.modulation_l.per_band[low_band];
        assert!(
            (l_peak.peak_hz - 2.0).abs() < 0.5,
            "L carrier-band modulation: {l_peak:?}"
        );
        assert!(l_peak.peak_mean_ratio > 3.0, "L ratio: {l_peak:?}");

        // Events: the bursts onset on L only; R is still. The aggregate
        // quotes only what every channel supports: R withholds the
        // interval statistics, and the count medians between the two.
        assert!(
            report.events_l.count >= 3,
            "L events: {}",
            report.events_l.count
        );
        let events_r = report.events_r.as_ref().expect("stereo");
        assert_eq!(events_r.count, 0);
        assert!(report.events_l.interval_cv.is_some());
        assert!(
            report.events.count >= 1 && report.events.count < report.events_l.count,
            "aggregate count {} vs L {}",
            report.events.count,
            report.events_l.count
        );
        assert!(report.events.mean_interval_s.is_none());
        assert!(report.events.interval_cv.is_none());
        assert!(report.events.burstiness.is_none());
    }

    #[test]
    fn symmetric_control_l_equals_r_and_the_aggregate_is_bit_exact() {
        let signal = bursts(220.0, 0.3, 2.0);
        let buffer = stereo(signal.clone(), signal.clone());
        // The mixdown of L == R is L itself, so the aggregates below are
        // exactly the numbers the pre-P0-3 mixdown pipeline measured.
        assert_eq!(mono_mixdown(&buffer), signal);
        let report = analyze_buffer(&buffer);
        let spectrum_r = report.spectrum_r.as_ref().expect("stereo");
        let envelopes_r = report.envelopes_r.as_ref().expect("stereo");
        let modulation_r = report.modulation_r.as_ref().expect("stereo");
        let events_r = report.events_r.as_ref().expect("stereo");
        assert_eq!(report.spectrum_l, *spectrum_r);
        assert_eq!(report.envelopes_l, *envelopes_r);
        assert_eq!(report.modulation_l, *modulation_r);
        assert_eq!(report.events_l, *events_r);
        // The median of equal values is the value, bit-exact.
        assert_eq!(report.spectrum, report.spectrum_l);
        assert_eq!(report.envelopes, report.envelopes_l);
        assert_eq!(report.modulation, report.modulation_l);
        assert_eq!(report.events, report.events_l);
    }

    #[test]
    fn every_pre_p0_3_flatten_path_stays_present_and_populated() {
        // A stereo buffer that voices, bursts, and runs long enough that
        // every optional statistic (F0 moments, event intervals) is
        // populated, so each pre-P0-3 path must flatten Some.
        let signal = bursts(220.0, 0.3, 2.0);
        let report = analyze_buffer(&stereo(signal.clone(), signal));
        let raw = report.flatten();
        // Duplicate paths would silently shadow in the golden map.
        let values: HashMap<String, Option<f64>> = raw.iter().cloned().collect::<HashMap<_, _>>();
        assert_eq!(values.len(), raw.len(), "flatten paths must be unique");
        for path in pre_p0_3_paths() {
            match values.get(&path) {
                None => panic!("pre-P0-3 path missing: {path}"),
                Some(None) => panic!("pre-P0-3 path present but null: {path}"),
                Some(Some(_)) => {}
            }
        }
    }

    /// The pre-P0-3 flatten address space. The 0.1.x flatten walked the
    /// actual vectors (dynamic lengths), so this list is every path it
    /// could emit for a buffer of at least 25 envelope frames — the set
    /// the golden targets and baseline captures could ever reference.
    fn pre_p0_3_paths() -> Vec<String> {
        let mut out = vec![
            "sample_rate".to_string(),
            "frames".to_string(),
            "channels".to_string(),
            "duration_s".to_string(),
            "spectrum.rolloff95_hz".to_string(),
            "spectrum.band_energy_fraction.b0_500".to_string(),
            "spectrum.band_energy_fraction.b500_2000".to_string(),
            "spectrum.band_energy_fraction.b2000_8000".to_string(),
            "spectrum.band_energy_fraction.b8000_nyquist".to_string(),
            "modulation.gain_peak_mean_ratio".to_string(),
            "modulation.gain_peak_hz".to_string(),
            "events.count".to_string(),
            "events.rate_hz".to_string(),
            "events.mean_interval_s".to_string(),
            "events.interval_cv".to_string(),
            "events.burstiness".to_string(),
            "interaural.b0_500".to_string(),
            "interaural.b500_2000".to_string(),
            "interaural.b2000_8000".to_string(),
            "interaural.b8000_nyquist".to_string(),
            "speech.f0.voiced_frames".to_string(),
            "speech.f0.voiced_fraction".to_string(),
            "speech.f0.f0_mean_hz".to_string(),
            "speech.f0.f0_sigma_st".to_string(),
            "speech.f0.f0_range_st".to_string(),
            "speech.pauses.count".to_string(),
            "speech.pauses.total_s".to_string(),
            "speech.pauses.rate_per_min".to_string(),
            "speech.pauses.mean_duration_s".to_string(),
        ];
        for i in 0..24 {
            out.push(format!("spectrum.bands24_energy_fraction.{i}"));
            out.push(format!("envelopes.bands.{i}.mean_db"));
            out.push(format!("envelopes.bands.{i}.variance"));
            out.push(format!("envelopes.bands.{i}.sparsity"));
            out.push(format!("envelopes.bands.{i}.kurtosis_excess"));
            out.push(format!("modulation.per_band.{i}.peak_mean_ratio"));
            out.push(format!("modulation.per_band.{i}.peak_hz"));
        }
        for i in 0..25 {
            out.push(format!("envelopes.band_edge_hz.{i}"));
        }
        for i in 0..64 {
            out.push(format!("modulation.gain_spectrum_0_20hz.{i}"));
        }
        for i in 0..24 {
            for j in 0..24 {
                out.push(format!("modulation.cross_band_correlation.{i}.{j}"));
            }
        }
        out
    }

    #[test]
    fn mono_buffers_have_no_r_sections_and_keep_interaural_none() {
        let report = analyze_buffer(&mono(bursts(220.0, 0.3, 2.0)));
        assert!(report.spectrum_r.is_none());
        assert!(report.envelopes_r.is_none());
        assert!(report.modulation_r.is_none());
        assert!(report.events_r.is_none());
        assert!(report.interaural.is_none());
        // The median of one channel is that channel: aggregate == L.
        assert_eq!(report.spectrum, report.spectrum_l);
        assert_eq!(report.events, report.events_l);
    }

    #[test]
    fn json_carries_l_r_sections_and_nulls_r_for_mono() {
        let signal = bursts(220.0, 0.3, 1.0);
        let stereo_json =
            serde_json::to_value(analyze_buffer(&stereo(signal.clone(), signal.clone())))
                .expect("serializes");
        for key in [
            "spectrum_l",
            "spectrum_r",
            "envelopes_l",
            "envelopes_r",
            "modulation_l",
            "modulation_r",
            "events_l",
            "events_r",
        ] {
            assert!(
                stereo_json.get(key).is_some_and(|v| !v.is_null()),
                "missing key {key}"
            );
        }
        assert!(
            stereo_json["interaural"]["per_band"]
                .as_array()
                .is_some_and(|bands| bands.len() == 24),
            "per-band interaural must ride the section"
        );
        let mono_json = serde_json::to_value(analyze_buffer(&mono(signal))).expect("serializes");
        for key in ["spectrum_r", "envelopes_r", "modulation_r", "events_r"] {
            assert!(
                mono_json.get(key).is_some_and(serde_json::Value::is_null),
                "mono must null {key}"
            );
        }
        assert!(mono_json.get("spectrum_l").is_some_and(|v| !v.is_null()));
        assert!(mono_json
            .get("interaural")
            .is_some_and(serde_json::Value::is_null));
    }

    #[test]
    fn zero_channel_buffers_measure_the_degenerate_report() {
        // Constructible and legal: frames() == 0. The analysis measures
        // one silent empty signal — the same degenerate case the retired
        // mixdown measured — and must not panic.
        let buffer = AudioBuffer::from_channels(Vec::new(), SR).expect("well formed");
        let report = analyze_buffer(&buffer);
        assert_eq!(report.spectrum_l.rolloff95_hz, 0.0);
        assert_eq!(report.events_l.count, 0);
        assert_eq!(report.speech.f0.voiced_frames, 0);
        let stats = texture_stats(&buffer);
        assert!(stats.c.iter().all(|v| *v == 0.0));
        assert_eq!(texture_stats_channels(&buffer).len(), 1);
    }

    #[test]
    fn texture_stats_measure_channels_then_aggregate_the_median() {
        // Asymmetric: L low bursts, R a high steady tone — the per-channel
        // statistics differ where the channels differ.
        let l = bursts(220.0, 0.3, 2.0);
        let r = steady_tone(3000.0, 0.3, 2.0);
        let per = texture_stats_channels(&stereo(l, r));
        assert_eq!(per.len(), 2);
        let low_band = band_of(220.0);
        assert!(
            per[0].marginal[low_band][1] > per[1].marginal[low_band][1],
            "L carrier-band marginal {:?} vs R {:?}",
            per[0].marginal[low_band],
            per[1].marginal[low_band]
        );
        // Symmetric control: the elementwise median of equal values is
        // bit-exact the value.
        let signal = bursts(220.0, 0.3, 2.0);
        let symmetric = stereo(signal.clone(), signal);
        assert_eq!(
            texture_stats(&symmetric),
            texture_stats_channels(&symmetric)[0]
        );
    }
}
