//! Modulation spectrum, gain modulation summary, cross-band correlation.
//!
//! For every band envelope (in dB) and the full-band gain envelope the
//! mean-removed series is transformed with an FFT of its own length; the
//! modulation spectrum is `|F[k]|^2`. Reported per band:
//!
//! - `peak_mean_ratio`: max / mean of the spectrum over bins `k >= 1`
//!   (DC excluded). This is the §7 discriminator between an
//!   exactly-periodic LFO (a line spike; the design window measured 57.0
//!   for sinusoid AM against 9.5 for latent drift) and natural drift.
//!   The value scales with the analysis length, so targets must pin the
//!   duration they were authored for.
//! - `peak_hz`: the frequency of the peak bin, at bin resolution
//!   (`env_rate / frames`, i.e. `1 / duration`).
//!
//! The cross-band correlation matrix is the Pearson correlation of the
//! dB envelopes between every pair of bands (`McDermott` C1); the diagonal
//! is fixed at 1.0.

use crate::envelopes::EnvelopeData;
use crate::stft::real_fft;

/// Number of points of the reported 0–20 Hz gain-modulation spectrum grid.
const GAIN_GRID: usize = 64;
/// Top of the gain-modulation grid in hertz.
const GAIN_GRID_TOP_HZ: f64 = 20.0;

/// Per-band modulation summary.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct BandModulation {
    /// Max/mean of the band's modulation spectrum (bins above DC).
    pub peak_mean_ratio: f64,
    /// Frequency of the modulation peak, at bin resolution.
    pub peak_hz: f64,
}

/// Modulation section of the feature report.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ModulationReport {
    /// Per-band modulation summaries, low to high.
    pub per_band: Vec<BandModulation>,
    /// Peak/mean of the full-band gain modulation spectrum.
    pub gain_peak_mean_ratio: f64,
    /// Frequency of the gain modulation peak, at bin resolution.
    pub gain_peak_hz: f64,
    /// The gain modulation spectrum on a fixed 0–20 Hz grid
    /// (64 points), normalized to its own maximum.
    pub gain_spectrum_0_20hz: Vec<f64>,
    /// 24x24 Pearson correlations of the band dB envelopes.
    pub cross_band_correlation: Vec<Vec<f64>>,
}

/// One-sided modulation spectrum of a mean-removed dB series.
///
/// Returns `(|F[1]|^2 ..= |F[L/2]|^2)`, i.e. DC excluded, Nyquist
/// included once; an empty input yields an empty spectrum.
fn modulation_spectrum(series: &[f64]) -> Vec<f64> {
    if series.len() < 2 {
        return Vec::new();
    }
    let mean = series.iter().sum::<f64>() / series.len() as f64;
    let centered: Vec<f64> = series.iter().map(|v| v - mean).collect();
    let spectrum = real_fft(&centered);
    let bins = spectrum.len(); // L/2 + 1
    let mut out = Vec::with_capacity(bins.saturating_sub(1));
    for bin in spectrum.iter().take(bins).skip(1) {
        out.push(bin.re * bin.re + bin.im * bin.im);
    }
    out
}

/// Peak/mean ratio and peak frequency of a modulation spectrum.
fn peak_summary(spectrum: &[f64], env_rate: f64, frames: usize) -> (f64, f64) {
    if spectrum.is_empty() || frames < 2 {
        return (0.0, 0.0);
    }
    let mean: f64 = spectrum.iter().sum::<f64>() / spectrum.len() as f64;
    if mean <= 0.0 {
        return (0.0, 0.0);
    }
    let (peak_bin, peak) = spectrum
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(k, &v)| (k + 1, v)) // +1: DC is excluded
        .expect("nonempty spectrum");
    let ratio = peak / mean;
    let bin_hz = env_rate / frames as f64;
    (ratio, peak_bin as f64 * bin_hz)
}

/// The gain spectrum on the fixed 0–20 Hz grid, normalized to its max.
fn gain_grid(spectrum: &[f64], env_rate: f64, frames: usize) -> Vec<f64> {
    let mut grid = vec![0.0f64; GAIN_GRID];
    if spectrum.is_empty() || frames < 2 {
        return grid;
    }
    let bin_hz = env_rate / frames as f64;
    for (i, slot) in grid.iter_mut().enumerate() {
        let lo = GAIN_GRID_TOP_HZ * i as f64 / GAIN_GRID as f64;
        let hi = GAIN_GRID_TOP_HZ * (i + 1) as f64 / GAIN_GRID as f64;
        let mut sum = 0.0;
        for (k, &m) in spectrum.iter().enumerate() {
            let f = (k + 1) as f64 * bin_hz;
            if f >= lo && f < hi {
                sum += m;
            }
        }
        *slot = sum;
    }
    let max = grid.iter().copied().fold(0.0, f64::max);
    if max > 0.0 {
        for slot in &mut grid {
            *slot /= max;
        }
    }
    grid
}

/// Pearson correlation of two series; 0.0 for degenerate spread.
fn correlation(left: &[f64], right: &[f64]) -> f64 {
    let n = left.len().min(right.len());
    if n == 0 {
        return 0.0;
    }
    let count = n as f64;
    let mean_left = left.iter().sum::<f64>() / count;
    let mean_right = right.iter().sum::<f64>() / count;
    let mut num = 0.0;
    let mut var_left = 0.0;
    let mut var_right = 0.0;
    for (l, r) in left.iter().zip(right.iter()) {
        let dl = l - mean_left;
        let dr = r - mean_right;
        num += dl * dr;
        var_left += dl * dl;
        var_right += dr * dr;
    }
    let den = (var_left * var_right).sqrt();
    if den <= 0.0 {
        return 0.0;
    }
    (num / den).clamp(-1.0, 1.0)
}

/// Builds the modulation report from the envelope data.
pub(crate) fn analyze(data: &EnvelopeData) -> ModulationReport {
    let frames = data.gain_db.len();
    let band_count = data.band_db.len();

    let mut per_band = Vec::with_capacity(band_count);
    for series in &data.band_db {
        let spectrum = modulation_spectrum(series);
        let (ratio, peak_hz) = peak_summary(&spectrum, data.env_rate, frames);
        per_band.push(BandModulation {
            peak_mean_ratio: ratio,
            peak_hz,
        });
    }

    let gain_spectrum = modulation_spectrum(&data.gain_db);
    let (gain_ratio, gain_peak_hz) = peak_summary(&gain_spectrum, data.env_rate, frames);
    let grid = gain_grid(&gain_spectrum, data.env_rate, frames);

    let mut cross = vec![vec![0.0f64; band_count]; band_count];
    for (i, row) in cross.iter_mut().enumerate() {
        row[i] = 1.0;
        for (j, slot) in row.iter_mut().enumerate().skip(i + 1) {
            *slot = correlation(&data.band_db[i], &data.band_db[j]);
        }
    }
    // Mirror the upper triangle into the lower one (reading `cross`
    // immutably while writing needs its own pass over a clone).
    let mut mirrored = cross.clone();
    for (i, row) in cross.iter().enumerate() {
        for (j, value) in row.iter().enumerate().skip(i + 1) {
            mirrored[j][i] = *value;
        }
    }
    cross = mirrored;

    ModulationReport {
        per_band,
        gain_peak_mean_ratio: gain_ratio,
        gain_peak_hz,
        gain_spectrum_0_20hz: grid,
        cross_band_correlation: cross,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENV_RATE: f64 = 200.0; // 5 ms envelope frames

    fn db_envelope_with_gain(frames: usize, gain: impl Fn(usize) -> f64) -> Vec<f64> {
        (0..frames)
            .map(|t| {
                let g = gain(t);
                -20.0 + 20.0 * g.log10() // dB of a power envelope scaled by g^2
            })
            .collect()
    }

    #[test]
    fn empty_and_short_series_are_degenerate() {
        assert_eq!(modulation_spectrum(&[]), [] as [f64; 0]);
        assert_eq!(modulation_spectrum(&[3.0]), [] as [f64; 0]);
        assert_eq!(peak_summary(&[], ENV_RATE, 10), (0.0, 0.0));
        assert_eq!(peak_summary(&[1.0; 8], ENV_RATE, 1), (0.0, 0.0));
        let grid = gain_grid(&[], ENV_RATE, 100);
        assert_eq!(grid.len(), GAIN_GRID);
        assert!(grid.iter().all(|g| g.abs() < f64::MIN_POSITIVE));
    }

    #[test]
    fn sinusoidal_am_peaks_at_its_frequency_with_a_high_ratio() {
        // 10 s at 200 frames/s = 2000 frames; 0.5 Hz = bin 10 exactly.
        let frames = 2000;
        let series = db_envelope_with_gain(frames, |t| {
            0.7 + 0.3 * (2.0 * std::f64::consts::PI * 0.5 * t as f64 / ENV_RATE).sin()
        });
        let spectrum = modulation_spectrum(&series);
        let (ratio, peak_hz) = peak_summary(&spectrum, ENV_RATE, frames);
        assert!(
            (peak_hz - 0.5).abs() < 0.11,
            "peak {peak_hz} should be 0.5 Hz (bin resolution 0.1 Hz)"
        );
        assert!(ratio > 100.0, "sinusoid AM ratio {ratio} should be large");
        // A DC-only series has no AC modulation.
        let flat = vec![-10.0f64; 500];
        let spectrum = modulation_spectrum(&flat);
        assert!(spectrum.iter().all(|m| m < &1e-9));
        let (r, hz) = peak_summary(&spectrum, ENV_RATE, 500);
        assert_eq!((r, hz), (0.0, 0.0));
    }

    #[test]
    fn slow_noise_modulation_scores_far_below_a_sinusoid() {
        // Equal-depth comparison: both envelopes carry the same dB std, so
        // the ratio difference reflects shape (line vs broad), not level.
        // Deterministic "latent-like" gain: an 8 s boxcar moving average of
        // a fixed pseudo-noise sequence, normalized to unit std.
        let frames = 4000; // 20 s
        let noise: Vec<f64> = (0..frames)
            .map(|t| ((t * 7919 + 13) % 101) as f64 / 101.0 - 0.5)
            .collect();
        let window = 1600; // 8 s
        let mut latent = vec![0.0f64; frames];
        let mut acc = 0.0;
        for t in 0..frames {
            acc += noise[t];
            if t >= window {
                acc -= noise[t - window];
            }
            let count = (t + 1).min(window) as f64;
            latent[t] = acc / count;
        }
        let var = latent.iter().map(|v| v * v).sum::<f64>() / frames as f64;
        let sigma = var.sqrt();
        for slot in &mut latent {
            *slot /= sigma;
        }
        // Sinusoid 0.7 +/- 0.3 has dg std 0.3*0.707 = 0.212; match it. The
        // normalized latent can swing past ±4.7σ, which would drive the
        // gain negative (and log10 undefined), so it is clamped the way a
        // real envelope's depth bound clamps it.
        let series = db_envelope_with_gain(frames, |t| 1.0 + 0.212 * latent[t].clamp(-2.0, 2.0));
        let (ratio, _) = peak_summary(&modulation_spectrum(&series), ENV_RATE, frames);

        let sinus = db_envelope_with_gain(frames, |t| {
            0.7 + 0.3 * (2.0 * std::f64::consts::PI * 0.05 * t as f64 / ENV_RATE).sin()
        });
        let (sin_ratio, sin_hz) = peak_summary(&modulation_spectrum(&sinus), ENV_RATE, frames);
        eprintln!("modulation ratios: sinusoid {sin_ratio}, latent-like {ratio}");
        assert!((sin_hz - 0.05).abs() < 0.06);
        assert!(
            sin_ratio > 5.0 * ratio,
            "sinusoid {sin_ratio} vs latent {ratio} must separate by >5x"
        );
        assert!(sin_ratio > 100.0, "the sinusoid side must be a clear spike");
    }

    #[test]
    fn gain_grid_buckets_the_spectrum_and_normalizes() {
        // Two lines at 5 Hz and 15 Hz (5 ms frames -> 0.5 Hz bins).
        let frames = 2000;
        let series = db_envelope_with_gain(frames, |t| {
            1.0 + 0.02 * (2.0 * std::f64::consts::PI * 5.0 * t as f64 / ENV_RATE).sin()
                + 0.01 * (2.0 * std::f64::consts::PI * 15.0 * t as f64 / ENV_RATE).sin()
        });
        let spectrum = modulation_spectrum(&series);
        let grid = gain_grid(&spectrum, ENV_RATE, frames);
        let bucket = |hz: f64| {
            let index = (hz / GAIN_GRID_TOP_HZ * GAIN_GRID as f64) as usize;
            grid[index]
        };
        let peak_bin_5hz = bucket(5.0);
        let harmonic_bin_15hz = bucket(15.0);
        assert!((peak_bin_5hz - 1.0).abs() < 1e-9, "5 Hz line is the max");
        assert!(
            harmonic_bin_15hz > 0.2,
            "15 Hz line visible: {harmonic_bin_15hz}"
        );
        // Grid length and zero floor bins.
        assert_eq!(grid.len(), GAIN_GRID);
        assert!(grid[0] < 1e-6);
    }

    #[test]
    fn cross_band_correlation_of_identical_series_is_one() {
        let a = vec![1.0, 2.0, 3.0, 4.0, 3.0, 2.0];
        assert!((correlation(&a, &a) - 1.0).abs() < 1e-12);
        let b: Vec<f64> = a.iter().map(|v| -2.0 * v).collect();
        assert!((correlation(&a, &b) + 1.0).abs() < 1e-12);
        let flat = vec![2.0; 6];
        assert_eq!(correlation(&flat, &a), 0.0);
        assert_eq!(correlation(&[], &a), 0.0);
    }
}
