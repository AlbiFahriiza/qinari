//! The feature report: every measured statistic, serializable.
//!
//! [`FeatureReport::flatten`] turns the report into a flat list of
//! `(dotted path, value)` pairs — the address space the golden-target
//! machinery gates. Optional statistics (no events, no voiced frames,
//! mono input) flatten to `None`.

use crate::events::EventReport;
use crate::interaural::InterauralReport;
use crate::modulation::ModulationReport;
use crate::spectrum::SpectrumReport;
use crate::{envelopes::EnvelopeReport, speech::SpeechReport};

/// The full analysis of one buffer.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FeatureReport {
    /// Sample rate in hertz.
    pub sample_rate: u32,
    /// Frames per channel.
    pub frames: usize,
    /// Channel count.
    pub channels: usize,
    /// Duration in seconds.
    pub duration_s: f64,
    /// Spectral features (Welch, `rolloff95`, band energies) — the
    /// per-statistic median across channels.
    pub spectrum: SpectrumReport,
    /// Channel 0 ("L") spectral features.
    pub spectrum_l: SpectrumReport,
    /// Channel 1 ("R") spectral features (`None` for mono).
    pub spectrum_r: Option<SpectrumReport>,
    /// Band envelope moments — the per-statistic median across channels.
    pub envelopes: EnvelopeReport,
    /// Channel 0 ("L") band envelope moments.
    pub envelopes_l: EnvelopeReport,
    /// Channel 1 ("R") band envelope moments (`None` for mono).
    pub envelopes_r: Option<EnvelopeReport>,
    /// Modulation and cross-band statistics — the per-statistic median
    /// across channels.
    pub modulation: ModulationReport,
    /// Channel 0 ("L") modulation and cross-band statistics.
    pub modulation_l: ModulationReport,
    /// Channel 1 ("R") modulation and cross-band statistics (`None`
    /// for mono).
    pub modulation_r: Option<ModulationReport>,
    /// Event statistics — the per-statistic median across channels.
    pub events: EventReport,
    /// Channel 0 ("L") event statistics.
    pub events_l: EventReport,
    /// Channel 1 ("R") event statistics (`None` for mono).
    pub events_r: Option<EventReport>,
    /// Interaural correlations per band (`None` for mono buffers) —
    /// cross-channel by definition.
    pub interaural: Option<InterauralReport>,
    /// Speech features (F0, pauses) — on the mono mixdown:
    /// monophonic by definition (one voice).
    pub speech: SpeechReport,
}

/// One flattened report value.
type FlatValue = (String, Option<f64>);

/// Pushes a plain value.
fn push(out: &mut Vec<FlatValue>, path: &str, value: Option<f64>) {
    out.push((path.to_string(), value));
}

/// Pushes every element of a slice as `<prefix>.<index>`.
fn push_slice(out: &mut Vec<FlatValue>, prefix: &str, values: &[f64]) {
    for (i, v) in values.iter().enumerate() {
        push(out, &format!("{prefix}.{i}"), Some(*v));
    }
}

/// Pushes a fixed-length vector section, or `len` nulls for an absent
/// section — keeps the golden address space present for mono buffers
/// (the interaural convention).
fn push_fixed(out: &mut Vec<FlatValue>, prefix: &str, len: usize, values: Option<&[f64]>) {
    match values {
        Some(values) if values.len() == len => push_slice(out, prefix, values),
        _ => {
            for i in 0..len {
                push(out, &format!("{prefix}.{i}"), None);
            }
        }
    }
}

/// Pushes a fixed `rows` x `cols` matrix section, or nulls when absent
/// (see [`push_fixed`]).
fn push_matrix(
    out: &mut Vec<FlatValue>,
    prefix: &str,
    rows: usize,
    cols: usize,
    matrix: Option<&[Vec<f64>]>,
) {
    let well_shaped = |m: &[Vec<f64>]| m.len() == rows && m.iter().all(|r| r.len() == cols);
    match matrix {
        Some(m) if well_shaped(m) => {
            for (i, row) in m.iter().enumerate() {
                push_slice(out, &format!("{prefix}.{i}"), row);
            }
        }
        _ => {
            for i in 0..rows {
                for j in 0..cols {
                    push(out, &format!("{prefix}.{i}.{j}"), None);
                }
            }
        }
    }
}

/// Pushes a spectrum section (aggregate or per-channel).
fn push_spectrum(out: &mut Vec<FlatValue>, prefix: &str, section: Option<&SpectrumReport>) {
    push(
        out,
        &format!("{prefix}.rolloff95_hz"),
        section.map(|s| s.rolloff95_hz),
    );
    push(
        out,
        &format!("{prefix}.band_energy_fraction.b0_500"),
        section.map(|s| s.band_energy_fraction.b0_500),
    );
    push(
        out,
        &format!("{prefix}.band_energy_fraction.b500_2000"),
        section.map(|s| s.band_energy_fraction.b500_2000),
    );
    push(
        out,
        &format!("{prefix}.band_energy_fraction.b2000_8000"),
        section.map(|s| s.band_energy_fraction.b2000_8000),
    );
    push(
        out,
        &format!("{prefix}.band_energy_fraction.b8000_nyquist"),
        section.map(|s| s.band_energy_fraction.b8000_nyquist),
    );
    push_fixed(
        out,
        &format!("{prefix}.bands24_energy_fraction"),
        24,
        section.map(|s| s.bands24_energy_fraction.as_slice()),
    );
}

/// Pushes the per-band moments of an envelope section. Band edges are
/// sample-rate constants (identical across channels), so only the
/// aggregate section carries them — the per-channel sections skip the
/// redundant copy.
fn push_envelope_bands(out: &mut Vec<FlatValue>, prefix: &str, section: Option<&EnvelopeReport>) {
    for i in 0..24 {
        let band = section.and_then(|s| s.bands.get(i));
        push(
            out,
            &format!("{prefix}.bands.{i}.mean_db"),
            band.map(|b| b.mean_db),
        );
        push(
            out,
            &format!("{prefix}.bands.{i}.variance"),
            band.map(|b| b.variance),
        );
        push(
            out,
            &format!("{prefix}.bands.{i}.sparsity"),
            band.map(|b| b.sparsity),
        );
        push(
            out,
            &format!("{prefix}.bands.{i}.kurtosis_excess"),
            band.map(|b| b.kurtosis_excess),
        );
    }
}

/// Pushes a modulation section (aggregate or per-channel).
fn push_modulation(out: &mut Vec<FlatValue>, prefix: &str, section: Option<&ModulationReport>) {
    for i in 0..24 {
        let band = section.and_then(|s| s.per_band.get(i));
        push(
            out,
            &format!("{prefix}.per_band.{i}.peak_mean_ratio"),
            band.map(|b| b.peak_mean_ratio),
        );
        push(
            out,
            &format!("{prefix}.per_band.{i}.peak_hz"),
            band.map(|b| b.peak_hz),
        );
    }
    push(
        out,
        &format!("{prefix}.gain_peak_mean_ratio"),
        section.map(|s| s.gain_peak_mean_ratio),
    );
    push(
        out,
        &format!("{prefix}.gain_peak_hz"),
        section.map(|s| s.gain_peak_hz),
    );
    push_fixed(
        out,
        &format!("{prefix}.gain_spectrum_0_20hz"),
        64,
        section.map(|s| s.gain_spectrum_0_20hz.as_slice()),
    );
    push_matrix(
        out,
        &format!("{prefix}.cross_band_correlation"),
        24,
        24,
        section.map(|s| s.cross_band_correlation.as_slice()),
    );
}

/// Pushes an event section (aggregate or per-channel).
fn push_events(out: &mut Vec<FlatValue>, prefix: &str, section: Option<&EventReport>) {
    push(
        out,
        &format!("{prefix}.count"),
        section.map(|s| s.count as f64),
    );
    push(
        out,
        &format!("{prefix}.rate_hz"),
        section.map(|s| s.rate_hz),
    );
    push(
        out,
        &format!("{prefix}.mean_interval_s"),
        section.and_then(|s| s.mean_interval_s),
    );
    push(
        out,
        &format!("{prefix}.interval_cv"),
        section.and_then(|s| s.interval_cv),
    );
    push(
        out,
        &format!("{prefix}.burstiness"),
        section.and_then(|s| s.burstiness),
    );
}

/// Pushes the interaural section. Cross-channel by definition, so it
/// has no per-channel variants; an absent section (mono) flattens to
/// nulls, the interaural convention.
fn push_interaural(out: &mut Vec<FlatValue>, section: Option<&InterauralReport>) {
    push(out, "interaural.b0_500", section.map(|s| s.b0_500));
    push(out, "interaural.b500_2000", section.map(|s| s.b500_2000));
    push(out, "interaural.b2000_8000", section.map(|s| s.b2000_8000));
    push(
        out,
        "interaural.b8000_nyquist",
        section.map(|s| s.b8000_nyquist),
    );
    for i in 0..24 {
        let value = section.and_then(|s| s.per_band.get(i).copied());
        push(out, &format!("interaural.per_band.{i}"), value);
    }
}

/// Pushes the speech section. Monophonic by definition (one voice, on
/// the mixdown), so it has no per-channel variants.
fn push_speech(out: &mut Vec<FlatValue>, speech: &SpeechReport) {
    push(
        out,
        "speech.f0.voiced_frames",
        Some(speech.f0.voiced_frames as f64),
    );
    push(
        out,
        "speech.f0.voiced_fraction",
        Some(speech.f0.voiced_fraction),
    );
    push(out, "speech.f0.f0_mean_hz", speech.f0.f0_mean_hz);
    push(out, "speech.f0.f0_sigma_st", speech.f0.f0_sigma_st);
    push(out, "speech.f0.f0_range_st", speech.f0.f0_range_st);
    push(out, "speech.pauses.count", Some(speech.pauses.count as f64));
    push(out, "speech.pauses.total_s", Some(speech.pauses.total_s));
    push(
        out,
        "speech.pauses.rate_per_min",
        Some(speech.pauses.rate_per_min),
    );
    push(
        out,
        "speech.pauses.mean_duration_s",
        speech.pauses.mean_duration_s,
    );
}

impl FeatureReport {
    /// Flattens the report into `(dotted path, value)` pairs.
    ///
    /// Paths follow the JSON field names, e.g.
    /// `spectrum.rolloff95_hz`, `envelopes.bands.7.sparsity`,
    /// `modulation.cross_band_correlation.3.12`, `events.interval_cv`,
    /// `interaural.b500_2000`, `speech.f0.f0_sigma_st`. Per-channel
    /// sections use the same names suffixed `_l` / `_r`, e.g.
    /// `spectrum_r.rolloff95_hz` (`null` for mono, like `interaural`).
    // One push per named field, in report order; the section helpers
    // hold the shared path tables, so a field name exists in exactly
    // one place (CHECKPOINT_RULES gate 2: one table, no scattering).
    pub fn flatten(&self) -> Vec<FlatValue> {
        // Aggregate (~900) + two per-channel sections (~1660).
        let mut out = Vec::with_capacity(2560);
        push(&mut out, "sample_rate", Some(f64::from(self.sample_rate)));
        push(&mut out, "frames", Some(self.frames as f64));
        push(&mut out, "channels", Some(self.channels as f64));
        push(&mut out, "duration_s", Some(self.duration_s));
        push_spectrum(&mut out, "spectrum", Some(&self.spectrum));
        push_spectrum(&mut out, "spectrum_l", Some(&self.spectrum_l));
        push_spectrum(&mut out, "spectrum_r", self.spectrum_r.as_ref());
        push_slice(
            &mut out,
            "envelopes.band_edge_hz",
            &self.envelopes.band_edge_hz,
        );
        push_envelope_bands(&mut out, "envelopes", Some(&self.envelopes));
        push_envelope_bands(&mut out, "envelopes_l", Some(&self.envelopes_l));
        push_envelope_bands(&mut out, "envelopes_r", self.envelopes_r.as_ref());
        push_modulation(&mut out, "modulation", Some(&self.modulation));
        push_modulation(&mut out, "modulation_l", Some(&self.modulation_l));
        push_modulation(&mut out, "modulation_r", self.modulation_r.as_ref());
        push_events(&mut out, "events", Some(&self.events));
        push_events(&mut out, "events_l", Some(&self.events_l));
        push_events(&mut out, "events_r", self.events_r.as_ref());
        push_interaural(&mut out, self.interaural.as_ref());
        push_speech(&mut out, &self.speech);
        out
    }
}
