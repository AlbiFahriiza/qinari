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
    /// Spectral features (Welch, `rolloff95`, band energies).
    pub spectrum: SpectrumReport,
    /// Band envelope moments.
    pub envelopes: EnvelopeReport,
    /// Modulation and cross-band statistics.
    pub modulation: ModulationReport,
    /// Event statistics.
    pub events: EventReport,
    /// Interaural correlations (`None` for mono buffers).
    pub interaural: Option<InterauralReport>,
    /// Speech features (F0, pauses).
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

impl FeatureReport {
    /// Flattens the report into `(dotted path, value)` pairs.
    ///
    /// Paths follow the JSON field names, e.g.
    /// `spectrum.rolloff95_hz`, `envelopes.bands.7.sparsity`,
    /// `modulation.cross_band_correlation.3.12`, `events.interval_cv`,
    /// `interaural.b500_2000`, `speech.f0.f0_sigma_st`.
    // A flat serialization table: one push per named field, in report
    // order, exactly like a hand-written `Serialize` impl. Splitting it
    // per section would scatter the path namespace across helpers without
    // making any single piece simpler (documented allow per
    // CHECKPOINT_RULES gate 2).
    #[allow(clippy::too_many_lines)]
    pub fn flatten(&self) -> Vec<FlatValue> {
        let mut out = Vec::with_capacity(900);
        push(&mut out, "sample_rate", Some(f64::from(self.sample_rate)));
        push(&mut out, "frames", Some(self.frames as f64));
        push(&mut out, "channels", Some(self.channels as f64));
        push(&mut out, "duration_s", Some(self.duration_s));
        push(
            &mut out,
            "spectrum.rolloff95_hz",
            Some(self.spectrum.rolloff95_hz),
        );
        push(
            &mut out,
            "spectrum.band_energy_fraction.b0_500",
            Some(self.spectrum.band_energy_fraction.b0_500),
        );
        push(
            &mut out,
            "spectrum.band_energy_fraction.b500_2000",
            Some(self.spectrum.band_energy_fraction.b500_2000),
        );
        push(
            &mut out,
            "spectrum.band_energy_fraction.b2000_8000",
            Some(self.spectrum.band_energy_fraction.b2000_8000),
        );
        push(
            &mut out,
            "spectrum.band_energy_fraction.b8000_nyquist",
            Some(self.spectrum.band_energy_fraction.b8000_nyquist),
        );
        push_slice(
            &mut out,
            "spectrum.bands24_energy_fraction",
            &self.spectrum.bands24_energy_fraction,
        );
        push_slice(
            &mut out,
            "envelopes.band_edge_hz",
            &self.envelopes.band_edge_hz,
        );
        for (i, band) in self.envelopes.bands.iter().enumerate() {
            push(
                &mut out,
                &format!("envelopes.bands.{i}.mean_db"),
                Some(band.mean_db),
            );
            push(
                &mut out,
                &format!("envelopes.bands.{i}.variance"),
                Some(band.variance),
            );
            push(
                &mut out,
                &format!("envelopes.bands.{i}.sparsity"),
                Some(band.sparsity),
            );
            push(
                &mut out,
                &format!("envelopes.bands.{i}.kurtosis_excess"),
                Some(band.kurtosis_excess),
            );
        }
        for (i, band) in self.modulation.per_band.iter().enumerate() {
            push(
                &mut out,
                &format!("modulation.per_band.{i}.peak_mean_ratio"),
                Some(band.peak_mean_ratio),
            );
            push(
                &mut out,
                &format!("modulation.per_band.{i}.peak_hz"),
                Some(band.peak_hz),
            );
        }
        push(
            &mut out,
            "modulation.gain_peak_mean_ratio",
            Some(self.modulation.gain_peak_mean_ratio),
        );
        push(
            &mut out,
            "modulation.gain_peak_hz",
            Some(self.modulation.gain_peak_hz),
        );
        push_slice(
            &mut out,
            "modulation.gain_spectrum_0_20hz",
            &self.modulation.gain_spectrum_0_20hz,
        );
        for (i, row) in self.modulation.cross_band_correlation.iter().enumerate() {
            push_slice(
                &mut out,
                &format!("modulation.cross_band_correlation.{i}"),
                row,
            );
        }
        push(&mut out, "events.count", Some(self.events.count as f64));
        push(&mut out, "events.rate_hz", Some(self.events.rate_hz));
        push(
            &mut out,
            "events.mean_interval_s",
            self.events.mean_interval_s,
        );
        push(&mut out, "events.interval_cv", self.events.interval_cv);
        push(&mut out, "events.burstiness", self.events.burstiness);
        let interaural = match &self.interaural {
            Some(report) => (
                Some(report.b0_500),
                Some(report.b500_2000),
                Some(report.b2000_8000),
                Some(report.b8000_nyquist),
            ),
            None => (None, None, None, None),
        };
        push(&mut out, "interaural.b0_500", interaural.0);
        push(&mut out, "interaural.b500_2000", interaural.1);
        push(&mut out, "interaural.b2000_8000", interaural.2);
        push(&mut out, "interaural.b8000_nyquist", interaural.3);
        push(
            &mut out,
            "speech.f0.voiced_frames",
            Some(self.speech.f0.voiced_frames as f64),
        );
        push(
            &mut out,
            "speech.f0.voiced_fraction",
            Some(self.speech.f0.voiced_fraction),
        );
        push(&mut out, "speech.f0.f0_mean_hz", self.speech.f0.f0_mean_hz);
        push(
            &mut out,
            "speech.f0.f0_sigma_st",
            self.speech.f0.f0_sigma_st,
        );
        push(
            &mut out,
            "speech.f0.f0_range_st",
            self.speech.f0.f0_range_st,
        );
        push(
            &mut out,
            "speech.pauses.count",
            Some(self.speech.pauses.count as f64),
        );
        push(
            &mut out,
            "speech.pauses.total_s",
            Some(self.speech.pauses.total_s),
        );
        push(
            &mut out,
            "speech.pauses.rate_per_min",
            Some(self.speech.pauses.rate_per_min),
        );
        push(
            &mut out,
            "speech.pauses.mean_duration_s",
            self.speech.pauses.mean_duration_s,
        );
        out
    }
}
