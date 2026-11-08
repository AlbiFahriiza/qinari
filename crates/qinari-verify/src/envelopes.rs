//! Band envelopes and their moments.
//!
//! The 24 log-spaced bands of [`crate::spectrum::band24_edges`] are
//! followed through a 1024-point STFT (hop 256): for every frame the
//! in-band power is summed, giving one envelope time series per band plus
//! a full-band ("gain") envelope.
//!
//! Reported moments, all deterministic:
//!
//! - `mean_db`, `variance`, `kurtosis_excess`: mean, population variance,
//!   and excess kurtosis of the band's envelope in dB
//!   (`10*log10(power + 1e-12)`); the compressive step keeps transients
//!   from dominating the way they would in the linear domain;
//! - `sparsity`: Hoyer sparsity `(sqrt(N) - L1/L2) / (sqrt(N) - 1)` of the
//!   *linear* power envelope — 0 for a flat envelope, near 1 for a
//!   sparse click train (noise sits around 0.3).

use crate::spectrum::band24_edges;
use crate::stft::{power_frames, ANALYSIS_FFT, ANALYSIS_HOP};

/// Power floor added before the log (`-120 dB` for digital silence).
const DB_FLOOR: f64 = 1e-12;

/// Per-band envelope moments.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct BandMoments {
    /// Lower band edge in hertz.
    pub edge_lo_hz: f64,
    /// Upper band edge in hertz.
    pub edge_hi_hz: f64,
    /// Mean of the band's dB envelope.
    pub mean_db: f64,
    /// Population variance of the band's dB envelope.
    pub variance: f64,
    /// Hoyer sparsity of the band's linear power envelope.
    pub sparsity: f64,
    /// Excess kurtosis of the band's dB envelope.
    pub kurtosis_excess: f64,
}

/// Envelope section of the feature report.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct EnvelopeReport {
    /// The 25 band edges in hertz (24 bands).
    pub band_edge_hz: Vec<f64>,
    /// Moments per band, low to high.
    pub bands: Vec<BandMoments>,
}

/// Internal envelope time series shared with the modulation and event
/// extractors.
pub(crate) struct EnvelopeData {
    /// Envelope frame rate in hertz (`sample_rate / 256`).
    pub env_rate: f64,
    /// Per-band dB envelopes: 24 bands x frames, band-major (one row per
    /// band) so the modulation extractor reads the series of each band
    /// directly. The 0.1.x layout stored frame-major rows, which made
    /// `per_band` and the cross-band matrix grow with buffer length
    /// instead of following the fixed 24-band set.
    pub band_db: Vec<Vec<f64>>,
    /// Full-band dB envelope (frames).
    pub gain_db: Vec<f64>,
}

/// `10*log10(power + floor)`.
fn to_db(power: f64) -> f64 {
    10.0 * (power + DB_FLOOR).log10()
}

/// Maps each of the 513 analysis bins to one of the 24 bands.
fn bin_to_band(edges: &[f64], sample_rate: u32) -> Vec<usize> {
    let bin_hz = f64::from(sample_rate) / ANALYSIS_FFT as f64;
    (0..=ANALYSIS_FFT / 2)
        .map(|k| {
            // Bins below the first edge (DC, sub-40 Hz) join the first band;
            // the Nyquist bin sits exactly ON the last edge, where the
            // half-open windows miss it, so anything at or above the last
            // edge belongs to the top band.
            let f = k as f64 * bin_hz;
            if f >= edges[edges.len() - 1] {
                return edges.len() - 2;
            }
            let mut band = 0;
            for (i, pair) in edges.windows(2).enumerate() {
                if f >= pair[0] && f < pair[1] {
                    band = i;
                    break;
                }
            }
            band
        })
        .collect()
}

/// Population mean and variance of a series.
fn mean_variance(series: &[f64]) -> (f64, f64) {
    if series.is_empty() {
        return (0.0, 0.0);
    }
    let n = series.len() as f64;
    let mean = series.iter().sum::<f64>() / n;
    let var = series
        .iter()
        .map(|v| {
            let d = v - mean;
            d * d
        })
        .sum::<f64>()
        / n;
    (mean, var)
}

/// Excess kurtosis given mean and variance (0 for degenerate spread).
fn kurtosis_excess(series: &[f64], mean: f64, variance: f64) -> f64 {
    if variance <= 0.0 {
        return 0.0;
    }
    let n = series.len().max(1) as f64;
    let m4 = series
        .iter()
        .map(|v| {
            let d = v - mean;
            d * d * d * d
        })
        .sum::<f64>()
        / n;
    m4 / (variance * variance) - 3.0
}

/// Hoyer sparsity of a non-negative series.
fn hoyer_sparsity(series: &[f64]) -> f64 {
    let n = series.len();
    if n < 2 {
        return 0.0;
    }
    let l1: f64 = series.iter().sum();
    let l2 = series.iter().map(|v| v * v).sum::<f64>().sqrt();
    if l1 <= 0.0 || l2 <= 0.0 {
        return 0.0;
    }
    let sqrt_n = (n as f64).sqrt();
    let ratio = l1 / l2;
    if ratio >= sqrt_n {
        return 0.0;
    }
    (sqrt_n - ratio) / (sqrt_n - 1.0)
}

/// Builds the envelope report and time series for a mono signal.
pub(crate) fn analyze(mono: &[f32], sample_rate: u32) -> (EnvelopeReport, EnvelopeData) {
    let edges = band24_edges(sample_rate);
    let mapping = bin_to_band(&edges, sample_rate);
    let frames = power_frames(mono, ANALYSIS_FFT, ANALYSIS_HOP);
    let frame_total = frames.len();
    let band_count = edges.len() - 1;

    let mut band_power = vec![vec![0.0f64; band_count]; frame_total];
    let mut gain_power = vec![0.0f64; frame_total];
    for (t, frame) in frames.iter().enumerate() {
        let mut total = 0.0;
        for (k, &p) in frame.iter().enumerate() {
            total += p;
            band_power[t][mapping[k]] += p;
        }
        gain_power[t] = total;
    }

    let band_db: Vec<Vec<f64>> = (0..band_count)
        .map(|b| (0..frame_total).map(|t| to_db(band_power[t][b])).collect())
        .collect();
    let gain_db: Vec<f64> = gain_power.iter().map(|&p| to_db(p)).collect();

    let mut report_bands = Vec::with_capacity(band_count);
    for b in 0..band_count {
        let db_series = &band_db[b];
        let (mean, var) = mean_variance(db_series);
        let linear: Vec<f64> = (0..frame_total).map(|t| band_power[t][b]).collect();
        report_bands.push(BandMoments {
            edge_lo_hz: edges[b],
            edge_hi_hz: edges[b + 1],
            mean_db: mean,
            variance: var,
            sparsity: hoyer_sparsity(&linear),
            kurtosis_excess: kurtosis_excess(db_series, mean, var),
        });
    }

    let report = EnvelopeReport {
        band_edge_hz: edges.clone(),
        bands: report_bands,
    };
    let data = EnvelopeData {
        env_rate: f64::from(sample_rate) / ANALYSIS_HOP as f64,
        band_db,
        gain_db,
    };
    (report, data)
}

/// Median-across-channels envelope report: the aggregate of the
/// per-channel envelope sections (the stereo-aware contract of
/// `crate::analyze_buffer`).
///
/// # Panics
///
/// Panics if `reports` is empty — a programming error: the zero-channel
/// case synthesizes one silent channel, so callers always pass at least
/// one report.
pub(crate) fn aggregate(reports: &[EnvelopeReport]) -> EnvelopeReport {
    let first = reports.first().expect("at least one channel report");
    let mut scratch = Vec::with_capacity(reports.len());
    let band_count = first.bands.len();
    let mut bands = Vec::with_capacity(band_count);
    for b in 0..band_count {
        // Same sample rate -> the same band set in every channel.
        bands.push(BandMoments {
            // Sample-rate constants: identical in every channel.
            edge_lo_hz: first.bands[b].edge_lo_hz,
            edge_hi_hz: first.bands[b].edge_hi_hz,
            mean_db: crate::median_stat(&mut scratch, reports, |r| r.bands[b].mean_db),
            variance: crate::median_stat(&mut scratch, reports, |r| r.bands[b].variance),
            sparsity: crate::median_stat(&mut scratch, reports, |r| r.bands[b].sparsity),
            kurtosis_excess: crate::median_stat(&mut scratch, reports, |r| {
                r.bands[b].kurtosis_excess
            }),
        });
    }
    EnvelopeReport {
        band_edge_hz: first.band_edge_hz.clone(),
        bands,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    #[test]
    fn db_floor_and_conversions() {
        assert!((to_db(0.0) + 120.0).abs() < 1e-9);
        assert!((to_db(1.0) - 0.0).abs() < 1e-9);
        assert!((to_db(0.1) + 10.0).abs() < 1e-9);
    }

    #[test]
    fn moments_helpers_on_known_series() {
        let (mean, var) = mean_variance(&[2.0, 4.0, 6.0, 8.0]);
        assert!((mean - 5.0).abs() < 1e-12);
        assert!((var - 5.0).abs() < 1e-12);
        // Uniform: excess kurtosis exactly -1.2? For a two-point {0,10} at
        // p=0.5: kurtosis = 1/(p(1-p)) - 3 = 1. Flat: -3 + 3 = 0 excess?
        let flat = vec![1.0; 100];
        let (m, v) = mean_variance(&flat);
        assert!(kurtosis_excess(&flat, m, v) < 1e-9);
        let spiky = vec![0.0; 99];
        let spiky = [spiky, vec![10.0]].concat();
        let (m2, v2) = mean_variance(&spiky);
        let k = kurtosis_excess(&spiky, m2, v2);
        // Bernoulli(0.01) excess kurtosis: (1-6p+6p^2)/(p(1-p)) = 95.01.
        assert!((k - 95.01).abs() < 0.05, "kurtosis {k}");
        assert!((mean_variance(&[])).0.abs() < 1e-12);
        assert_eq!(kurtosis_excess(&[], 0.0, 0.0), 0.0);
    }

    #[test]
    fn hoyer_sparsity_bounds() {
        assert_eq!(hoyer_sparsity(&[]), 0.0);
        assert_eq!(hoyer_sparsity(&[5.0]), 0.0);
        assert_eq!(hoyer_sparsity(&[0.0; 64]), 0.0);
        let mut spike = vec![0.0f64; 999];
        spike.push(1.0);
        assert!(hoyer_sparsity(&spike) > 0.95, "single spike ~ 1");
        let noise: Vec<f64> = (0..10_000)
            .map(|i| f64::from((i * 7919) % 101) / 101.0)
            .collect();
        let s = hoyer_sparsity(&noise);
        // Uniform values sit near 0.14 (chi^2-like envelopes near 0.29).
        assert!(s > 0.1 && s < 0.2, "uniform series ~0.14, got {s}");
    }

    #[test]
    fn bin_to_band_covers_all_bins_without_gaps() {
        let edges = band24_edges(SR);
        let mapping = bin_to_band(&edges, SR);
        assert_eq!(mapping.len(), 513);
        assert_eq!(mapping[0], 0); // DC joins the first band
                                   // The bands tile logarithmically from 40 Hz to Nyquist, so 15 kHz
                                   // sits in band 22 (band 23 starts near 16.8 kHz).
        let bin_15k = (15_000.0 / (f64::from(SR) / ANALYSIS_FFT as f64)) as usize;
        assert_eq!(mapping[bin_15k], 22);
        // Nyquist itself lands in the top band.
        assert_eq!(*mapping.last().expect("nonempty"), 23);
        // The mapping is non-decreasing: bins tile the bands in order
        // without gaps or overlaps (low bands can share a bin's span at
        // this FFT size, so individual bands may be empty).
        assert!(mapping.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn steady_tone_has_a_flat_envelope_and_clicks_are_sparse() {
        // 300 Hz sine: the band containing it should show low dB variance.
        let n = 44_100 * 4;
        let tone: Vec<f32> = (0..n)
            .map(|i| {
                0.5 * (2.0 * std::f64::consts::PI * 300.0 * f64::from(i) / 44_100.0).sin() as f32
            })
            .collect();
        let (report, _) = analyze(&tone, SR);
        let b300 = report
            .bands
            .iter()
            .position(|b| b.edge_lo_hz <= 300.0 && 300.0 < b.edge_hi_hz)
            .expect("a band covers 300 Hz");
        let band = &report.bands[b300];
        assert!(band.variance < 25.0, "tone band variance {}", band.variance);
        assert!(band.sparsity < 0.35, "tone band sparsity {}", band.sparsity);
        assert!(band.kurtosis_excess.is_finite());
        assert!(band.mean_db > -60.0, "tone band mean {}", band.mean_db);

        // Click train at 2 Hz on silence: every band is sparse.
        let mut clicks = vec![0.0f32; 44_100 * 10];
        for t in 0..20 {
            let start = 11_025 + t * 22_050; // first burst at 0.25 s
            for k in 0..441 {
                clicks[start + k] = 0.8 * (std::f64::consts::PI * k as f64 / 441.0).sin() as f32;
            }
        }
        let (report, _) = analyze(&clicks, SR);
        let b2k = report
            .bands
            .iter()
            .position(|b| b.edge_lo_hz <= 2000.0 && 2000.0 < b.edge_hi_hz)
            .expect("a band covers 2 kHz");
        assert!(report.bands[b2k].sparsity > 0.8, "clicks sparsity");
        assert!(report.bands[b2k].kurtosis_excess > 10.0, "clicks kurtosis");
    }

    #[test]
    fn envelope_frames_match_the_stft_count() {
        assert_eq!(crate::stft::frame_count(0, ANALYSIS_FFT, ANALYSIS_HOP), 1);
        assert_eq!(
            crate::stft::frame_count(1024, ANALYSIS_FFT, ANALYSIS_HOP),
            1
        );
        assert_eq!(
            crate::stft::frame_count(1024 + 256, ANALYSIS_FFT, ANALYSIS_HOP),
            2
        );
    }
}
