//! Spectral features: Welch power spectrum, `rolloff95`, band energies.
//!
//! Conventions (all deterministic, all in `f64`):
//!
//! - Welch: 1024-point segments, periodic Hann window, 50% overlap
//!   (512-sample hop). Partial trailing segments are dropped; buffers
//!   shorter than one segment are zero-padded to a single segment.
//! - Power convention: an on-bin sine of amplitude `a` reads `a^2 / 2`
//!   at its bin (the mean square of the sine), via
//!   `P[k] = 2/(\sum w)^2 * mean_segments |X_seg[k]|^2`.
//! - `rolloff95`: the frequency at which the cumulative spectrum reaches
//!   95% of the total, with linear interpolation between bin centers.
//! - Band energies: fractions of the total power in `[0, 500)`,
//!   `[500, 2000)`, `[2000, 8000)`, and `[8000, nyquist]` Hz, plus the
//!   same fractions over the 24 log-spaced bands of
//!   [`band24_edges`].

use crate::stft::{hann_window, power_frames, ANALYSIS_FFT};

/// Welch segment hop: 50% overlap.
const WELCH_HOP: usize = ANALYSIS_FFT / 2;

/// Frequency below which 95% of spectral energy sits, in hertz.
///
/// The crossing between the two bracketing bins is linearly interpolated
/// for sub-bin resolution. A spectrum with no energy reports `0.0`.
pub fn rolloff95(power: &[f64], sample_rate: u32) -> f64 {
    let bin_hz = f64::from(sample_rate) / ANALYSIS_FFT as f64;
    let total: f64 = power.iter().sum();
    if total <= 0.0 {
        return 0.0;
    }
    let target = 0.95 * total;
    let mut cumulative = 0.0;
    for (k, &p) in power.iter().enumerate() {
        let next = cumulative + p;
        if next >= target {
            let inside = if p > 0.0 {
                (target - cumulative) / p
            } else {
                0.0
            };
            return (k as f64 + inside) * bin_hz;
        }
        cumulative = next;
    }
    // Only reachable when rounding left the last bin short of the target.
    (power.len() - 1) as f64 * bin_hz
}

/// Welch power spectrum: one value per bin (`1024/2 + 1` bins).
///
/// See the module documentation for the normalization; deterministically
/// reproducible for the same input.
pub fn welch_power_spectrum(samples: &[f32]) -> Vec<f64> {
    let frames = power_frames(samples, ANALYSIS_FFT, WELCH_HOP);
    let win = hann_window(ANALYSIS_FFT);
    let window_sum: f64 = win.iter().sum();
    let scale = 2.0 / (window_sum * window_sum);
    let bins = frames.first().map_or(0, Vec::len);
    let mut averaged = vec![0.0f64; bins];
    for frame in &frames {
        for (slot, &p) in averaged.iter_mut().zip(frame.iter()) {
            *slot += p;
        }
    }
    let count = frames.len() as f64;
    for slot in &mut averaged {
        *slot *= scale / count;
    }
    averaged
}

/// The 25 edges of the 24 log-spaced analysis bands, in hertz.
///
/// The bands tile `[40, nyquist]` logarithmically (`edges[i] =
/// 40 * (nyquist/40)^(i/24)`). Rates whose Nyquist is at or below 80 Hz
/// collapse all edges onto the Nyquist (degenerate but well-defined).
pub fn band24_edges(sample_rate: u32) -> Vec<f64> {
    let nyquist = f64::from(sample_rate) / 2.0;
    let f_min = 40.0;
    let f_max = nyquist.max(f_min * 2.0);
    (0..=24)
        .map(|i| {
            let t = f64::from(i) / 24.0;
            f_min * (f_max / f_min).powf(t)
        })
        .collect()
}

/// Fractions of total power in the four named bands.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct BandFractions {
    /// Fraction of spectral power in `[0, 500)` Hz.
    pub b0_500: f64,
    /// Fraction of spectral power in `[500, 2000)` Hz.
    pub b500_2000: f64,
    /// Fraction of spectral power in `[2000, 8000)` Hz.
    pub b2000_8000: f64,
    /// Fraction of spectral power in `[8000, nyquist]` Hz.
    pub b8000_nyquist: f64,
}

/// Spectral section of the feature report.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SpectrumReport {
    /// Welch bin spacing in hertz (`sample_rate / 1024`).
    pub bin_hz: f64,
    /// The `rolloff95` frequency in hertz.
    pub rolloff95_hz: f64,
    /// Power fractions of the four named bands.
    pub band_energy_fraction: BandFractions,
    /// Power fractions of the 24 log-spaced bands (sums to ~1 minus DC).
    pub bands24_energy_fraction: Vec<f64>,
}

/// Sums the bins of `power` whose center frequency is in `[lo, hi)`.
fn band_power(power: &[f64], sample_rate: u32, lo: f64, hi: f64, top_inclusive: bool) -> f64 {
    let bin_hz = f64::from(sample_rate) / ANALYSIS_FFT as f64;
    let mut sum = 0.0;
    for (k, &p) in power.iter().enumerate() {
        let f = k as f64 * bin_hz;
        let inside = if top_inclusive { f <= hi } else { f < hi };
        if f >= lo && inside {
            sum += p;
        }
    }
    sum
}

/// Splits `total` into the four named band fractions.
fn named_fractions(power: &[f64], sample_rate: u32, total: f64) -> BandFractions {
    let share = |lo: f64, hi: f64, top: bool| {
        if total <= 0.0 {
            0.0
        } else {
            band_power(power, sample_rate, lo, hi, top) / total
        }
    };
    BandFractions {
        b0_500: share(0.0, 500.0, false),
        b500_2000: share(500.0, 2000.0, false),
        b2000_8000: share(2000.0, 8000.0, false),
        b8000_nyquist: share(8000.0, f64::from(sample_rate) / 2.0, true),
    }
}

/// Splits `total` over the 24 log-spaced bands.
fn bands24_fractions(power: &[f64], sample_rate: u32, total: f64) -> Vec<f64> {
    let edges = band24_edges(sample_rate);
    let mut out = Vec::with_capacity(24);
    for i in 0..24 {
        let lo = edges[i];
        let hi = edges[i + 1];
        let top = i == 23;
        if total <= 0.0 {
            out.push(0.0);
        } else {
            out.push(band_power(power, sample_rate, lo, hi, top) / total);
        }
    }
    out
}

/// Builds the spectral report from a mono signal.
pub(crate) fn analyze(mono: &[f32], sample_rate: u32) -> SpectrumReport {
    let power = welch_power_spectrum(mono);
    let total: f64 = power.iter().sum();
    SpectrumReport {
        bin_hz: f64::from(sample_rate) / ANALYSIS_FFT as f64,
        rolloff95_hz: rolloff95(&power, sample_rate),
        band_energy_fraction: named_fractions(&power, sample_rate, total),
        bands24_energy_fraction: bands24_fractions(&power, sample_rate, total),
    }
}

/// Median-across-channels spectrum: the aggregate of the per-channel
/// spectra (the stereo-aware contract of `crate::analyze_buffer`).
///
/// # Panics
///
/// Panics if `reports` is empty — a programming error: the zero-channel
/// case synthesizes one silent channel, so callers always pass at least
/// one report.
pub(crate) fn aggregate(reports: &[SpectrumReport]) -> SpectrumReport {
    let first = reports.first().expect("at least one channel report");
    let mut scratch = Vec::with_capacity(reports.len());
    let rolloff95_hz = crate::median_stat(&mut scratch, reports, |r| r.rolloff95_hz);
    let band_energy_fraction = BandFractions {
        b0_500: crate::median_stat(&mut scratch, reports, |r| r.band_energy_fraction.b0_500),
        b500_2000: crate::median_stat(&mut scratch, reports, |r| r.band_energy_fraction.b500_2000),
        b2000_8000: crate::median_stat(&mut scratch, reports, |r| {
            r.band_energy_fraction.b2000_8000
        }),
        b8000_nyquist: crate::median_stat(&mut scratch, reports, |r| {
            r.band_energy_fraction.b8000_nyquist
        }),
    };
    // Same sample rate -> the same band set in every channel, so every
    // per-channel vector matches the first report in length.
    let bands24_energy_fraction = (0..first.bands24_energy_fraction.len())
        .map(|b| crate::median_stat(&mut scratch, reports, |r| r.bands24_energy_fraction[b]))
        .collect();
    SpectrumReport {
        // A sample-rate constant: identical in every channel.
        bin_hz: first.bin_hz,
        rolloff95_hz,
        band_energy_fraction,
        bands24_energy_fraction,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    fn sine(freq: f64, amp: f64, secs: f64) -> Vec<f32> {
        let n = (secs * f64::from(SR)).round() as usize;
        (0..n)
            .map(|i| {
                (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(SR)).sin()) as f32
            })
            .collect()
    }

    #[test]
    fn welch_reads_half_squared_amplitude_of_an_on_bin_sine() {
        // 43.066 Hz bin spacing; 43.066*7 = 301.46 Hz is nearly on-bin.
        // 176.4 samples: 440 Hz lands on a bin at 44.1k for 1 s windows?
        // Use 301.462 Hz = bin 7 with 0.05% offset.
        let samples = sine(301.462, 0.5, 4.0);
        let power = welch_power_spectrum(&samples);
        // Expected peak: a^2/2 = 0.125 (power of the 0.5-amplitude sine).
        let peak_bin = power
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(k, _)| k)
            .expect("nonempty");
        assert_eq!(peak_bin, 7, "tone at 301.46 Hz -> bin 7 (301.47 Hz)");
        assert!(
            (power[7] - 0.125).abs() < 0.02,
            "bin 7 power {} (expected ~0.125)",
            power[7]
        );
    }

    #[test]
    fn rolloff95_of_a_tone_sits_a_few_bins_above_the_tone() {
        let samples = sine(300.0, 0.5, 4.0);
        let power = welch_power_spectrum(&samples);
        let rolloff = rolloff95(&power, SR);
        assert!(
            (300.0..800.0).contains(&rolloff),
            "rolloff95 {rolloff} should sit just above 300 Hz"
        );
        let quiet = welch_power_spectrum(&vec![0.0f32; 44_100]);
        assert!(rolloff95(&quiet, SR) < f64::from(SR) / 2.0);
        assert_eq!(rolloff95(&[0.0; 513], SR), 0.0);
    }

    #[test]
    fn band_edges_tile_logarithmically() {
        let edges = band24_edges(SR);
        assert_eq!(edges.len(), 25);
        assert!((edges[0] - 40.0).abs() < 1e-9);
        assert!((edges[24] - 22_050.0).abs() < 1e-6);
        for pair in edges.windows(2) {
            assert!(pair[0] < pair[1]);
        }
        // Log-uniform: each ratio equal.
        let ratio = edges[1] / edges[0];
        assert!((edges[12] / edges[11] - ratio).abs() < 1e-6);
        let tiny = band24_edges(100); // nyquist 50 < 80: the tiling widens
                                      // to 2× f_min so the bands stay logarithmic above Nyquist.
        assert!((tiny[0] - 40.0).abs() < 1e-9);
        assert!((tiny[24] - 80.0).abs() < 1e-9);
    }

    #[test]
    fn tone_energy_lands_in_its_named_band() {
        let samples = sine(300.0, 0.5, 4.0);
        let report = analyze(&samples, SR);
        assert!(report.band_energy_fraction.b0_500 > 0.95);
        assert!(report.band_energy_fraction.b500_2000 < 0.02);
        let samples = sine(5_000.0, 0.5, 4.0);
        let report = analyze(&samples, SR);
        assert!(report.band_energy_fraction.b2000_8000 > 0.95);
        assert!(report.band_energy_fraction.b0_500 < 0.02);
        let samples = sine(15_000.0, 0.5, 4.0);
        let report = analyze(&samples, SR);
        assert!(report.band_energy_fraction.b8000_nyquist > 0.95);
    }

    #[test]
    fn white_noise_is_flat_and_rolls_off_near_95_percent_of_nyquist() {
        let mut rng = qinari_dsp::rng::Qrng::new(2024);
        let secs = 6.0;
        let n = (secs * f64::from(SR)).round() as usize;
        let noise: Vec<f32> = (0..n).map(|_| rng.next_f32() * 2.0 - 1.0).collect();
        let report = analyze(&noise, SR);
        let power = welch_power_spectrum(&noise);
        let interior = &power[5..509];
        let max = interior.iter().copied().fold(f64::MIN, f64::max);
        let min = interior.iter().copied().fold(f64::MAX, f64::min);
        assert!(max / min < 1.6, "flatness max/min {max}/{min}");
        let nyquist = f64::from(SR) / 2.0;
        assert!(
            (report.rolloff95_hz / nyquist - 0.95).abs() < 0.02,
            "rolloff95 {} vs nyquist {nyquist}",
            report.rolloff95_hz
        );
        let expected = (nyquist - 8000.0) / nyquist;
        assert!(
            (report.band_energy_fraction.b8000_nyquist - expected).abs() < 0.05,
            "8000+ fraction {} vs expected {expected}",
            report.band_energy_fraction.b8000_nyquist
        );
        let sum: f64 = report.bands24_energy_fraction.iter().sum();
        assert!((0.97..=1.0).contains(&sum), "24-band sum {sum}");
    }

    #[test]
    fn spectrum_of_a_440_hz_sine_peaks_at_the_nearest_bin() {
        let samples = sine(440.0, 0.4, 3.0);
        let report = analyze(&samples, SR);
        let power = welch_power_spectrum(&samples);
        let peak_bin = power
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(k, _)| k)
            .expect("nonempty");
        assert_eq!(peak_bin, 10, "440 Hz -> bin 10 (430.7 Hz)");
        assert!(report.rolloff95_hz > 400.0 && report.rolloff95_hz < 1200.0);
        assert!(report.band_energy_fraction.b0_500 > 0.8);
    }

    #[test]
    fn empty_and_silent_signals_report_zero_energy_fractions() {
        let report = analyze(&[], SR);
        assert_eq!(report.rolloff95_hz, 0.0);
        assert_eq!(report.band_energy_fraction.b0_500, 0.0);
        let sum: f64 = report.bands24_energy_fraction.iter().sum();
        assert_eq!(sum, 0.0);
        let silence = analyze(&vec![0.0f32; 44_100], SR);
        assert_eq!(silence.rolloff95_hz, 0.0);
    }
}
