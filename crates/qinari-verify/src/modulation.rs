//! Modulation spectrum, gain modulation summary, cross-band correlation,
//! and the McDermott–Simoncelli texture statistics.
//!
//! # Legacy STFT-band summaries
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
//! dB envelopes between every pair of bands; the diagonal is fixed at
//! 1.0. This is the **C** statistic of the texture model in its legacy
//! STFT-band form — the 0.1.5 docs mislabeled it as the paper's C1
//! (finding F13); the paper's C, C1, and C2 are computed on the cochlear
//! model by [`texture_stats`] below.
//!
//! # McDermott–Simoncelli texture statistics (P0-2)
//!
//! [`texture_stats`] computes the 2011 texture statistics on the
//! cochlear front-end ([`crate::cochlea`]): the **C** cochlear
//! cross-band envelope correlations, modulation power over the 20-filter
//! marginal bank, **C1** cross-frequency correlations within a
//! modulation band of the 7-filter octave bank, and the complex **C2**
//! across-band correlation with frequency doubling. Every parameter is
//! frozen with its primary-source citation in
//! `docs/VERIFICATION-0.2.0.md` §§B–D; the naming rules of §F bind: C =
//! cochlear cross-band correlation, C1 = cross-frequency within one
//! modulation band, C2 = across-modulation-band complex correlation,
//! and AC — which is *not* one of the 1515 statistics of the 2011 model —
//! is labeled a Qinari extension everywhere it appears.

use crate::cochlea::{Cochleagram, CHANNEL_COUNT, ENV_RATE};
use crate::envelopes::EnvelopeData;
use crate::stft::{analytic_signal, inverse_real_fft, real_fft};
use rustfft::num_complex::Complex64;

/// Number of points of the reported 0–20 Hz gain-modulation spectrum grid.
const GAIN_GRID: usize = 64;
/// Top of the gain-modulation grid in hertz.
const GAIN_GRID_TOP_HZ: f64 = 20.0;

// ---- Frozen texture-model parameters (VERIFICATION-0.2.0.md §C–§D) ----
/// Marginal (modulation-power) filter count — 20 filters spanning
/// 0.5–200 Hz ([S1] p.937, txt L1550–1551).
const MARGINAL_BANDS: usize = 20;
/// Marginal-bank quality factor, for 3 dB bandwidths — 2 ([S1] p.937,
/// txt L1551).
const MARGINAL_Q: f64 = 2.0;
/// Octave (C1/C2) filter count — seven filters, centers 1.5625–100 Hz
/// ([S1] p.937, txt L1563–1570).
const OCTAVE_BANDS: usize = 7;
/// Lowest octave-band center (Hz) — 1.5625 ([S1] p.937, txt L1569);
/// it exists because C2 needs the full octave chain.
const OCTAVE_LOW_HZ: f64 = 1.5625;
/// Modulation bands the C1 statistic is imposed on: octave bands 2–7,
/// i.e. 3.125–100 Hz — six bands, excluding 1.5625 Hz ([S1] p.938, txt
/// L1633 `n ∈ [2,7]` and L1646–1647; VERIFICATION §E).
const C1_BANDS: usize = 6;
/// C2 octave pairs: (1.5625, 3.125) … (50, 100) — `m ∈ [1,6]`, `n = m+1`
/// ([S1] p.938, txt L1626).
const C2_PAIRS: usize = 6;
/// Cochlear-neighbor offsets of the C statistic, `(k−j) ∈
/// {1,2,3,5,8,11,16,21}` — "a subset of eight of its neighbors" ([S1]
/// p.937, txt L1554–1562).
const C_OFFSETS: [usize; 8] = [1, 2, 3, 5, 8, 11, 16, 21];
/// C1 cochlear-neighbor offsets — each filter correlated with its two
/// nearest neighbors, `(k−j) ∈ {1,2}` ([S1] p.938, txt L1633, L1644–1647).
const C1_OFFSETS: [usize; 2] = [1, 2];
/// AC (Qinari extension) lags in milliseconds. NOT part of the 2011
/// model's 1515 statistics (VERIFICATION §D); frozen as the Qinari
/// extension's parameter set.
const AC_LAGS_MS: [f64; 6] = [25.0, 50.0, 100.0, 250.0, 500.0, 1000.0];

/// Lowest marginal-bank center (Hz) — 0.5, "20 filters spanning
/// 0.5–200 Hz" ([S1] p.937, txt L1550–1551).
const MARGINAL_LOW_HZ: f64 = 0.5;
/// Highest marginal-bank center (Hz) — 200, the envelope Nyquist; the
/// 200 Hz filter keeps only the lower half of its half-cosine because
/// the 400 Hz envelope grid cannot represent the upper half ([S1]
/// p.937, txt L1550–1551 and L1561–1562; VERIFICATION §C).
const MARGINAL_HIGH_HZ: f64 = 200.0;
/// Total C statistics: 189 = Σ(32 − d) over the frozen offsets — "189
/// cochlear cross-correlations" ([S1] p.938, txt L1642–1645).
const C_STAT_COUNT: usize = 189;
/// Total C1 statistics: 366 = 6 bands × (31 + 30) — "366 C1
/// correlations" ([S1] p.938, txt L1644).
const C1_STAT_COUNT: usize = 366;

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

// ---- The 2011 texture statistics (VERIFICATION-0.2.0.md §§C–D) ----

/// Center frequencies of the marginal bank: 20 filters equally spaced
/// on a log scale, 0.5–200 Hz inclusive ([S1] p.937, txt L1550–1551;
/// [S1-impl] `make_constQ_cos_filters.m` `Cfs = 2.^(log2(low):step:log2(hi))`;
/// VERIFICATION §C).
fn marginal_centers() -> Vec<f64> {
    let steps = MARGINAL_BANDS - 1;
    (0..MARGINAL_BANDS)
        .map(|n| {
            MARGINAL_LOW_HZ * (MARGINAL_HIGH_HZ / MARGINAL_LOW_HZ).powf(n as f64 / steps as f64)
        })
        .collect()
}

/// Response of one marginal filter: half-cosine on the LINEAR frequency
/// axis, support `(c − c/Q, c + c/Q)`, zero outside — the bank's "log
/// scale" attaches to the center spacing, not the shape ([S1] p.937,
/// txt L1549–1551; [S1-impl] `make_constQ_cos_filters.m`, "filters are
/// symmetric on linear scale"; VERIFICATION §C). Q = 2 for 3 dB
/// bandwidths puts the half-power points at `0.75·c` and `1.25·c`
/// ([S1] txt L1551).
fn marginal_response(center: f64, hz: f64) -> f64 {
    let bw = center / MARGINAL_Q;
    if hz > center - bw && hz < center + bw {
        (std::f64::consts::PI * (hz - center) / (2.0 * bw)).cos()
    } else {
        0.0
    }
}

/// Center frequencies of the C1/C2 bank: seven filters, 1.5625 Hz up
/// in octave steps to 100 Hz ([S1] p.937, txt L1563–1570;
/// VERIFICATION §C/§E).
fn octave_centers() -> Vec<f64> {
    (0..OCTAVE_BANDS)
        .map(|i| OCTAVE_LOW_HZ * 2f64.powi(i as i32))
        .collect()
}

/// Response of one octave filter: half-cosine on the log2 axis, support
/// `(c/2, 2c)`, zero outside; the 3 dB points sit at `c·2^(±1/2)`, so
/// the printed parenthetical is `Q_3dB` = √2 — "more broadly tuned" than
/// the marginal bank's Q = 2 ([S1] p.937, txt L1566–1569; [S1-impl]
/// `make_octave_cos_filters2.m`; VERIFICATION §C).
fn octave_response(center: f64, hz: f64) -> f64 {
    if hz > center / 2.0 && hz < 2.0 * center {
        (std::f64::consts::FRAC_PI_2 * (hz / center).log2()).cos()
    } else {
        0.0
    }
}

/// Weighted power of the marginal-bank bands of one channel, each
/// normalized by the channel's envelope variance — `M_k,n` ([S1] p.937,
/// txt L1589–1603). `masked` is caller-owned scratch of one spectrum's
/// length, reused across bands and channels.
fn marginal_powers(
    spec: &[Complex64],
    masked: &mut [Complex64],
    w: &[f64],
    bin_hz: f64,
    centers: &[f64],
    var: f64,
) -> Vec<f64> {
    let frames = w.len();
    let scale = 1.0 / frames as f64;
    let mut powers = Vec::with_capacity(centers.len());
    for &c in centers {
        for (j, slot) in masked.iter_mut().enumerate() {
            *slot = spec[j] * marginal_response(c, j as f64 * bin_hz);
        }
        let band = inverse_real_fft(masked, frames);
        let mut power = 0.0;
        for (&wi, &b) in w.iter().zip(band.iter()) {
            let b = b * scale;
            power += wi * b * b;
        }
        powers.push(if var > 0.0 { power / var } else { 0.0 });
    }
    powers
}

/// Octave-bank analysis of one channel: the weighted variance of every
/// band (`band_var`, the C1/C2 normalizer, [S1] p.938, txt L1637–1641),
/// the real bands 3.125–100 Hz that C1 correlates across channels, and
/// the six C2 octave-pair correlations — the lower band frequency-doubled
/// by squaring its analytic signal and dividing the magnitude back out,
/// `d = a²/|a|`, which preserves the magnitude and doubles the phase
/// ([S1] p.938, txt L1615–1635; Suppl. txt L2418–2419); real and
/// imaginary parts are measured independently.
fn octave_band_stats(
    spec: &[Complex64],
    masked: &mut [Complex64],
    w: &[f64],
    bin_hz: f64,
    centers: &[f64],
) -> (Vec<f64>, Vec<Vec<f64>>, Vec<[f64; 2]>) {
    let frames = w.len();
    let mut band_var = Vec::with_capacity(centers.len());
    let mut c1_bands = Vec::with_capacity(centers.len() - 1);
    let mut c2 = Vec::with_capacity(centers.len() - 1);
    let mut analytic: Vec<Vec<Complex64>> = Vec::with_capacity(centers.len());
    for (i, &c) in centers.iter().enumerate() {
        for (j, slot) in masked.iter_mut().enumerate() {
            *slot = spec[j] * octave_response(c, j as f64 * bin_hz);
        }
        // The analytic signal carries the real band (Re) and its Hilbert
        // transform (Im) in one pass ([S1] p.938, txt L1611–1613).
        let a = analytic_signal(masked, frames);
        let real: Vec<f64> = a.iter().map(|z| z.re).collect();
        band_var.push(dot_weighted(w, &real, &real));
        if i >= 1 {
            c1_bands.push(real);
        }
        analytic.push(a);
    }
    for m in 0..centers.len() - 1 {
        let (low, high) = (&analytic[m], &analytic[m + 1]);
        let den = (band_var[m] * band_var[m + 1]).sqrt();
        let mut acc = Complex64::new(0.0, 0.0);
        for t in 0..frames {
            let mag = low[t].norm();
            let d = if mag > 0.0 {
                low[t] * low[t] / mag
            } else {
                Complex64::new(0.0, 0.0)
            };
            acc += (d.conj() * high[t]) * w[t];
        }
        c2.push(if den > 0.0 {
            [acc.re / den, acc.im / den]
        } else {
            [0.0, 0.0]
        });
    }
    (band_var, c1_bands, c2)
}

/// Weighted inner product `Σ w·x·y` — the paper's weighted time average
/// with `Σ w = 1` ([S1] p.937, txt L1572–1587).
fn dot_weighted(w: &[f64], x: &[f64], y: &[f64]) -> f64 {
    w.iter()
        .zip(x.iter())
        .zip(y.iter())
        .map(|((&wi, &xi), &yi)| wi * xi * yi)
        .sum()
}

/// Weighted M1–M4 marginal moments of one compressed envelope, plus the
/// intermediates the other statistics reuse: the weighted variance and
/// the mean-removed envelope ([S1] p.937, txt L1594–1605). M2 normalizes
/// the variance by the squared mean, M3/M4 by σ³/σ⁴; degenerate channels
/// (zero mean or variance) measure 0.
fn channel_marginals(w: &[f64], s: &[f64]) -> ([f64; 4], f64, Vec<f64>) {
    let mu: f64 = w.iter().zip(s.iter()).map(|(&wi, &v)| wi * v).sum();
    let dev: Vec<f64> = s.iter().map(|&v| v - mu).collect();
    let mut third = 0.0;
    let mut fourth = 0.0;
    for (&wi, &d) in w.iter().zip(dev.iter()) {
        third += wi * d * d * d;
        fourth += wi * d * d * d * d;
    }
    let var = dot_weighted(w, &dev, &dev);
    let m2 = if mu > 0.0 { var / (mu * mu) } else { 0.0 };
    let m3 = if var > 0.0 {
        third / var.powf(1.5)
    } else {
        0.0
    };
    let m4 = if var > 0.0 { fourth / (var * var) } else { 0.0 };
    ([mu, m2, m3, m4], var, dev)
}

/// The 189 C statistics: weighted Pearson correlations of the compressed
/// envelopes, ordered by neighbor offset {1,2,3,5,8,11,16,21}, then by
/// the lower channel of the pair ([S1] p.937, txt L1554–1562).
fn c_correlations(w: &[f64], centered: &[Vec<f64>], vars: &[f64]) -> Vec<f64> {
    let mut c = vec![0.0; C_STAT_COUNT];
    let mut ci = 0;
    for &d in &C_OFFSETS {
        for j in 0..CHANNEL_COUNT - d {
            let k = j + d;
            let den = (vars[j] * vars[k]).sqrt();
            c[ci] = if den > 0.0 {
                dot_weighted(w, &centered[j], &centered[k]) / den
            } else {
                0.0
            };
            ci += 1;
        }
    }
    debug_assert_eq!(ci, C_STAT_COUNT);
    c
}

/// The AC (Qinari extension): circular weighted autocorrelation of each
/// compressed envelope at the frozen lags, normalized by the geometric
/// mean of the weighted energies of the two lag-aligned copies — the
/// symmetric form of the C/C1 normalizers, so weighted Cauchy–Schwarz
/// bounds |AC| ≤ 1 exactly (a one-sided variance normalizer would be
/// unbounded, because the measurement window is not shift-invariant).
/// Not among the 1515 statistics of the 2011 model (VERIFICATION §D).
fn ac_correlations(w: &[f64], centered: &[Vec<f64>], vars: &[f64]) -> Vec<f64> {
    let frames = w.len();
    let mut ac = vec![0.0; CHANNEL_COUNT * AC_LAGS_MS.len()];
    for k in 0..CHANNEL_COUNT {
        let c = &centered[k];
        for (lag, &ms) in AC_LAGS_MS.iter().enumerate() {
            let shift = (ms * ENV_RATE / 1000.0).round() as usize % frames;
            let mut num = 0.0;
            let mut shifted_energy = 0.0;
            for t in 0..frames {
                let s = c[(t + shift) % frames];
                num += w[t] * c[t] * s;
                shifted_energy += w[t] * s * s;
            }
            let den = (vars[k] * shifted_energy).sqrt();
            ac[k * AC_LAGS_MS.len() + lag] = if den > 0.0 { num / den } else { 0.0 };
        }
    }
    ac
}

/// The 2011 texture statistics of one cochleagram, plus the AC (Qinari
/// extension): 1515 + 192 numbers, all deterministic.
///
/// Indexing (VERIFICATION-0.2.0.md §D):
///
/// - `marginal[k]` = `[M1, M2, M3, M4]` of channel `k`;
/// - `c` is ordered by neighbor offset {1,2,3,5,8,11,16,21}, then by
///   the lower channel of the pair;
/// - `mod_power[k·20 + n]`: power of marginal band `n` (0.5–200 Hz,
///   log-spaced) in channel `k`, normalized by the channel's envelope
///   variance;
/// - `c1` is ordered by modulation band (3.125–100 Hz, six bands), then
///   neighbor offset {1,2}, then the lower channel;
/// - `c2[k·6 + m]` = `[real, imaginary]` of the pair (octave band `m`,
///   band `m+1`) in channel `k` — both parts are separate statistics;
/// - `ac[k·6 + lag]` (Qinari extension): envelope autocorrelation of
///   channel `k` at 25/50/100/250/500/1000 ms.
///
/// Degenerate grams (empty buffers, rates without cochlear resolution,
/// silence) measure all zeros; no statistic ever panics or returns NaN.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TextureStats {
    /// Cochlear channel labels in hertz (32 entries) — the channel axis
    /// of every other field.
    pub center_hz: Vec<f64>,
    /// M1–M4 cochlear marginals per channel (128 statistics; [S1]
    /// p.937, txt L1594–1605).
    pub marginal: Vec<[f64; 4]>,
    /// C cross-band envelope correlations (189 statistics; [S1]
    /// p.937, txt L1554–1562).
    pub c: Vec<f64>,
    /// Marginal-bank modulation power (640 statistics; [S1] p.937,
    /// txt L1589–1603).
    pub mod_power: Vec<f64>,
    /// C1 cross-frequency correlations within a modulation band (366
    /// statistics; [S1] p.938, txt L1629–1647).
    pub c1: Vec<f64>,
    /// C2 complex across-band correlations as `[real, imaginary]`
    /// (192 statistics; [S1] p.938, txt L1615–1635).
    pub c2: Vec<[f64; 2]>,
    /// AC (Qinari extension) envelope autocorrelations (192 values) —
    /// not among the 1515 statistics of the 2011 model (VERIFICATION
    /// §D).
    pub ac: Vec<f64>,
}

impl TextureStats {
    /// All-zero statistics of the frozen shape for `center_hz` — what
    /// every degenerate gram measures; the totals stay addressable.
    fn zeroed(center_hz: Vec<f64>) -> Self {
        Self {
            center_hz,
            marginal: vec![[0.0, 0.0, 0.0, 0.0]; CHANNEL_COUNT],
            c: vec![0.0; C_STAT_COUNT],
            mod_power: vec![0.0; CHANNEL_COUNT * MARGINAL_BANDS],
            c1: vec![0.0; C1_STAT_COUNT],
            c2: vec![[0.0, 0.0]; CHANNEL_COUNT * C2_PAIRS],
            ac: vec![0.0; CHANNEL_COUNT * AC_LAGS_MS.len()],
        }
    }
}

/// Per-channel weighted moments (M1–M4, [S1] p.937, txt L1594–1605) and
/// the mean-removed envelopes every other statistic reuses, with their
/// weighted variances.
fn channel_moment_table(
    w: &[f64],
    channels: &[Vec<f64>],
) -> (Vec<[f64; 4]>, Vec<f64>, Vec<Vec<f64>>) {
    let mut marginal = vec![[0.0, 0.0, 0.0, 0.0]; channels.len()];
    let mut vars = vec![0.0f64; channels.len()];
    let mut centered = Vec::with_capacity(channels.len());
    for (k, s) in channels.iter().enumerate() {
        let (moments, var, dev) = channel_marginals(w, s);
        marginal[k] = moments;
        vars[k] = var;
        centered.push(dev);
    }
    (marginal, vars, centered)
}

/// Per-channel modulation-band measurements of the whole gram: the 640
/// marginal-bank powers, the 192 C2 octave pairs, and the octave-band
/// tables C1 reads — `band_var[k][i]` is Σw·b̃² of octave band `i` in
/// channel `k`, the C1/C2 normalizer ([S1] p.938, txt L1637–1641), and
/// `c1_bands[b][k]` keeps the real bands 3.125–100 Hz (filter 0 exists
/// only for C2's lowest pair, so it is not stored).
struct BandWork {
    mod_power: Vec<f64>,
    c2: Vec<[f64; 2]>,
    band_var: Vec<Vec<f64>>,
    c1_bands: Vec<Vec<Vec<f64>>>,
}

fn band_work(gram: &Cochleagram, w: &[f64], vars: &[f64]) -> BandWork {
    let frames = w.len();
    let bin_hz = ENV_RATE / frames as f64;
    let bins = frames / 2 + 1;
    let marginal = marginal_centers();
    let octave = octave_centers();
    let mut work = BandWork {
        mod_power: vec![0.0; CHANNEL_COUNT * MARGINAL_BANDS],
        c2: vec![[0.0, 0.0]; CHANNEL_COUNT * C2_PAIRS],
        band_var: vec![vec![0.0; OCTAVE_BANDS]; CHANNEL_COUNT],
        c1_bands: vec![vec![Vec::new(); CHANNEL_COUNT]; C1_BANDS],
    };
    let mut masked: Vec<Complex64> = vec![Complex64::new(0.0, 0.0); bins];
    for (k, channel) in gram.channels.iter().enumerate() {
        let spec = real_fft(channel);
        let powers = marginal_powers(&spec, &mut masked, w, bin_hz, &marginal, vars[k]);
        work.mod_power[k * MARGINAL_BANDS..(k + 1) * MARGINAL_BANDS].copy_from_slice(&powers);
        let (variances, c1_bands, c2_row) =
            octave_band_stats(&spec, &mut masked, w, bin_hz, &octave);
        work.band_var[k] = variances;
        for (band, real) in c1_bands.into_iter().enumerate() {
            work.c1_bands[band][k] = real;
        }
        work.c2[k * C2_PAIRS..(k + 1) * C2_PAIRS].copy_from_slice(&c2_row);
    }
    work
}

/// The 366 C1 statistics: cross-frequency correlations within one
/// modulation band, for the six bands 3.125–100 Hz and cochlear offsets
/// {1,2}, ordered by band, then offset, then the lower channel of the
/// pair ([S1] p.938, txt L1629–1647). `c1_bands[b]` is octave band b+1
/// (the 1.5625 Hz filter is C2-only, VERIFICATION §E), whose normalizer
/// sits in `band_var[.][b+1]`.
fn c1_correlations(w: &[f64], work: &BandWork) -> Vec<f64> {
    let mut c1 = vec![0.0; C1_STAT_COUNT];
    let mut c1i = 0;
    for band in 1..=C1_BANDS {
        for &d in &C1_OFFSETS {
            for j in 0..CHANNEL_COUNT - d {
                let k = j + d;
                let den = (work.band_var[j][band] * work.band_var[k][band]).sqrt();
                c1[c1i] = if den > 0.0 {
                    dot_weighted(w, &work.c1_bands[band - 1][j], &work.c1_bands[band - 1][k]) / den
                } else {
                    0.0
                };
                c1i += 1;
            }
        }
    }
    debug_assert_eq!(c1i, C1_STAT_COUNT);
    c1
}

/// Measures the 1515 texture statistics of the 2011 model plus the AC
/// (Qinari extension) on one cochleagram (the output of
/// [`crate::cochlea::analyze`]).
///
/// Every parameter is frozen with its primary-source citation in
/// `docs/VERIFICATION-0.2.0.md` §§A–D; the naming rules of §F bind: C =
/// cochlear cross-band correlation, C1 = cross-frequency within one
/// modulation band, C2 = across-modulation-band complex correlation,
/// AC = Qinari extension. All filtering is circular and zero-phase in
/// the discrete frequency domain ([S1] p.937, txt L1572–1587); every
/// average is weighted by the measurement window (`Σ w = 1`).
pub(crate) fn texture_stats(gram: &Cochleagram) -> TextureStats {
    if gram.window.is_empty() {
        return TextureStats::zeroed(gram.center_hz.clone());
    }
    let w = &gram.window;
    let (marginal, vars, centered) = channel_moment_table(w, &gram.channels);
    let work = band_work(gram, w, &vars);
    let c1 = c1_correlations(w, &work);
    TextureStats {
        center_hz: gram.center_hz.clone(),
        marginal,
        c: c_correlations(w, &centered, &vars),
        mod_power: work.mod_power,
        c1,
        c2: work.c2,
        ac: ac_correlations(w, &centered, &vars),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cochlea;

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

    // ---- P0-2 texture statistics (VERIFICATION-0.2.0.md §§A–F) ----

    /// Test signal rate: 44.1 kHz native (hi cutoff clipped to 10 kHz,
    /// the 2011 span), 12 s → exactly 4800 envelope frames at 400 Hz.
    /// Every test modulation frequency (2, 6.25, 12.5 Hz) is an exact
    /// bin of the 4800-frame envelope grid.
    const TEX_SR: u32 = 44_100;
    const TEX_SECONDS: f64 = 12.0;

    /// Channel labels of the frozen bank — a degenerate analysis still
    /// labels the channels, and labels do not depend on the signal.
    fn tex_centers() -> Vec<f64> {
        cochlea::analyze(&[], TEX_SR).center_hz
    }

    /// One AM tone at `carrier` whose amplitude is pre-compensated for
    /// the 0.3 compression, so the measured compressed envelope of the
    /// carrier's channel is exactly `1 + env(t)` ([S1] p.937, txt
    /// L1536–1540).
    fn tex_am_tone(carrier: f64, env: &dyn Fn(f64) -> f64) -> Vec<f32> {
        let samples = (TEX_SECONDS * f64::from(TEX_SR)).round() as usize;
        (0..samples)
            .map(|i| {
                let t = i as f64 / f64::from(TEX_SR);
                let m = (1.0 + env(t)).powf(1.0 / 0.3);
                (m * (2.0 * std::f64::consts::PI * carrier * t).cos()) as f32
            })
            .collect()
    }

    fn tex_add(mono: &mut [f32], tone: &[f32]) {
        for (s, v) in mono.iter_mut().zip(tone) {
            *s += v;
        }
    }

    /// Sum of AM tones at the centers of `channels`, each pre-compensated
    /// for the 0.3 compression so its channel's measured envelope is
    /// `1 + sign·env(t)` ([S1] p.937, txt L1536–1540).
    fn tex_tone_sum(
        centers: &[f64],
        channels: &[(usize, f64)],
        env: &dyn Fn(f64) -> f64,
    ) -> Vec<f32> {
        let mut mono = vec![0.0f32; (TEX_SECONDS * f64::from(TEX_SR)).round() as usize];
        for &(ch, sign) in channels {
            tex_add(&mut mono, &tex_am_tone(centers[ch], &|t| sign * env(t)));
        }
        mono
    }

    /// Linear index of `C(a, b)` in [`TextureStats::c`].
    fn tex_c_index(a: usize, b: usize) -> usize {
        let d = b - a;
        let off = C_OFFSETS
            .iter()
            .position(|&o| o == d)
            .expect("frozen offset");
        C_OFFSETS[..off]
            .iter()
            .map(|&o| CHANNEL_COUNT - o)
            .sum::<usize>()
            + a
    }

    #[test]
    fn texture_totals_match_the_2011_enumeration_and_degenerate_grams_measure_zero() {
        // The frozen index sets reproduce the paper's per-family totals:
        // 128 + 189 + 640 + 366 + 192 = 1515 (p.938, txt L1642–1645).
        assert_eq!(
            C_OFFSETS.iter().map(|&d| CHANNEL_COUNT - d).sum::<usize>(),
            C_STAT_COUNT
        );
        assert_eq!(
            C1_OFFSETS.iter().map(|&o| CHANNEL_COUNT - o).sum::<usize>() * C1_BANDS,
            C1_STAT_COUNT
        );
        let stats = texture_stats(&cochlea::analyze(&[], TEX_SR));
        assert_eq!(stats.center_hz.len(), CHANNEL_COUNT);
        assert_eq!(stats.marginal.len(), CHANNEL_COUNT); // 32 × 4 = 128
        assert_eq!(stats.c.len(), 189);
        assert_eq!(stats.mod_power.len(), 640);
        assert_eq!(stats.c1.len(), 366);
        assert_eq!(stats.c2.len(), 192);
        assert_eq!(stats.ac.len(), 192);
        for family in [&stats.c, &stats.mod_power, &stats.c1, &stats.ac] {
            assert!(family.iter().all(|v| *v == 0.0));
        }
        assert!(stats.marginal.iter().all(|m| *m == [0.0, 0.0, 0.0, 0.0]));
        assert!(stats.c2.iter().all(|v| *v == [0.0, 0.0]));
        // Silence and a one-sample buffer measure zero too (total
        // functions: no NaN, no panic).
        for mono in [vec![0.0f32; 8_000], vec![0.6f32]] {
            let stats = texture_stats(&cochlea::analyze(&mono, TEX_SR));
            assert!(stats.c.iter().all(|v| *v == 0.0));
            assert!(stats.c2.iter().all(|v| *v == [0.0, 0.0]));
            assert!(stats.ac.iter().all(|v| *v == 0.0));
        }
    }

    #[test]
    fn c_statistic_matches_analytic_values_on_synthetic_signals() {
        // Four AM tones at the exact centers of cochlear channels 20,
        // 21, 28, 29 — a pure tone at a channel center lands only in
        // that channel (the neighbors' half-cosine response is zero
        // there). Channels 20/28/29 share the envelope
        // +0.4·sin(2π·2t); channel 21 carries −0.4·sin(2π·2t). The
        // analytic C values are +1 for (20,28) offset 8 and (28,29)
        // offset 1, and −1 for (20,21) offset 1 and (21,29) offset 8 —
        // identical and sign-flipped envelopes correlate ±1 under any
        // weighting — within the 0.02 plan tolerance.
        let mono = tex_tone_sum(
            &tex_centers(),
            &[(20, 1.0), (21, -1.0), (28, 1.0), (29, 1.0)],
            &|t| 0.4 * (2.0 * std::f64::consts::PI * 2.0 * t).sin(),
        );
        let stats = texture_stats(&cochlea::analyze(&mono, TEX_SR));
        let tol = 0.02;
        for (a, b, expected) in [(20, 28, 1.0), (28, 29, 1.0), (20, 21, -1.0), (21, 29, -1.0)] {
            let got = stats.c[tex_c_index(a, b)];
            assert!(
                (got - expected).abs() < tol,
                "C({a},{b}) = {got}, expected {expected}"
            );
        }
    }

    #[test]
    fn c1_statistic_matches_analytic_values_on_synthetic_signals() {
        // Channels 20/21/22, all modulated at 6.25 Hz — an exact center
        // of the octave bank (its neighbors respond zero there) — with
        // signs +/−/−. The analytic C1 values in the 6.25 Hz band are
        // −1 for (20,21) offset 1 and (20,22) offset 2, and +1 for
        // (21,22) offset 1: every C1 offset of that band covered.
        let mono = tex_tone_sum(&tex_centers(), &[(20, 1.0), (21, -1.0), (22, -1.0)], &|t| {
            0.4 * (2.0 * std::f64::consts::PI * 6.25 * t).sin()
        });
        let stats = texture_stats(&cochlea::analyze(&mono, TEX_SR));
        // 6.25 Hz is octave filter 2 (0-based), i.e. C1 band 1 of the
        // six (3.125–100 Hz); layout: band, then offset {1,2}, then the
        // lower channel. The 6.25 Hz block starts after the first.
        let per_band: usize = C1_OFFSETS.iter().map(|&o| CHANNEL_COUNT - o).sum();
        let band_625_start = per_band;
        let at = |a: usize, b: usize| {
            if b - a == 1 {
                band_625_start + a
            } else {
                band_625_start + (CHANNEL_COUNT - 1) + a
            }
        };
        let tol = 0.02;
        for (a, b, expected) in [(20, 21, -1.0), (20, 22, -1.0), (21, 22, 1.0)] {
            let got = stats.c1[at(a, b)];
            assert!(
                (got - expected).abs() < tol,
                "C1({a},{b}) = {got}, expected {expected}"
            );
        }
    }

    #[test]
    fn c2_statistic_matches_analytic_values_on_synthetic_signals() {
        // One channel (20) with a two-line envelope: 0.3·sin at 6.25 Hz
        // (phase φ1) and 12.5 Hz (φ2) — an octave pair, so the lower
        // band's frequency-doubled phase is locked to the upper band's.
        // With Δ = φ2 − 2φ1 = π/3 the printed formula gives
        // C2 = 2i·e^{iΔ} = −√3 + i: magnitude 2, the Cauchy–Schwarz
        // equality of a perfectly phase-locked pair (VERIFICATION §D).
        let phi1 = 0.4;
        let phi2 = std::f64::consts::FRAC_PI_3 + 2.0 * phi1; // Δ = π/3
        let centers = tex_centers();
        let tone = tex_am_tone(centers[20], &|t| {
            0.3 * (2.0 * std::f64::consts::PI * 6.25 * t + phi1).sin()
                + 0.3 * (2.0 * std::f64::consts::PI * 12.5 * t + phi2).sin()
        });
        let stats = texture_stats(&cochlea::analyze(&tone, TEX_SR));
        // Pair m=2 (bands 3–4 = 6.25 and 12.5 Hz) of channel 20.
        let [re, im] = stats.c2[20 * C2_PAIRS + 2];
        assert!((re + 3f64.sqrt()).abs() < 0.02, "C2 re = {re}");
        assert!((im - 1.0).abs() < 0.02, "C2 im = {im}");
    }

    /// AC of a pure sine envelope of `f_m` Hz is `cos(2π·f_m·τ)` at every
    /// frozen lag (the Qinari extension's math, VERIFICATION §D).
    fn assert_ac_matches_a_sine_envelope(stats: &TextureStats, channel: usize, f_m: f64) {
        for (lag, &tau_ms) in AC_LAGS_MS.iter().enumerate() {
            let expected = (2.0 * std::f64::consts::PI * f_m * tau_ms / 1000.0).cos();
            let got = stats.ac[channel * AC_LAGS_MS.len() + lag];
            assert!(
                (got - expected).abs() < 0.02,
                "AC({tau_ms}ms) = {got}, expected {expected}"
            );
        }
    }

    #[test]
    fn marginal_moments_modulation_power_and_ac_match_analytic_values() {
        // Channel 20 with a single-line envelope u = 1 + 0.4·sin(2π·6.25t).
        // Textbook values: a sine's skew is 0 and raw kurtosis 1.5; the
        // envelope variance is 0.4²/2 = 0.08; all of it lands in the
        // marginal band whose center is 6.23 Hz (response 1 − 6e-5 at
        // 6.25 Hz) — the 0.5 Hz and 200 Hz filters hold at most a
        // percent of it; and AC(τ) = cos(2π·6.25·τ) at the frozen lags
        // (a Qinari extension, but its math is the same).
        let centers = tex_centers();
        let tone = tex_am_tone(centers[20], &|t| {
            0.4 * (2.0 * std::f64::consts::PI * 6.25 * t).sin()
        });
        let stats = texture_stats(&cochlea::analyze(&tone, TEX_SR));
        let [m1, m2, m3, m4] = stats.marginal[20];
        assert!((m1 - 1.0).abs() < 0.02, "M1 = {m1}");
        assert!((m2 - 0.08).abs() < 0.004, "M2 = {m2}"); // 0.4²/2
        assert!(m3.abs() < 0.02, "M3 = {m3}");
        assert!((m4 - 1.5).abs() < 0.02, "M4 = {m4}");
        let band9 = 20 * MARGINAL_BANDS + 8; // 1-based filter 9, center 6.23 Hz
        let power = stats.mod_power[band9];
        assert!((power - 1.0).abs() < 0.02, "mod power = {power}");
        // The 0.5 Hz and 200 Hz filters have structurally disjoint
        // support from a 6.25 Hz line, but the carrier is not an exact
        // bin of the native grid, so the measured envelope carries a
        // small ripple floor. The honest bound: those bands hold at
        // most 1% of the band that carries the signal.
        assert!(
            stats.mod_power[20 * MARGINAL_BANDS] < 0.01 * power,
            "0.5 Hz filter power {} vs band-9 {power}",
            stats.mod_power[20 * MARGINAL_BANDS]
        );
        assert!(
            stats.mod_power[20 * MARGINAL_BANDS + 19] < 0.01 * power,
            "200 Hz filter power {} vs band-9 {power}",
            stats.mod_power[20 * MARGINAL_BANDS + 19]
        );
        assert_ac_matches_a_sine_envelope(&stats, 20, 6.25);
    }

    #[test]
    fn odd_length_signals_measure_finite_bounded_statistics() {
        // 12 s + 551 samples → 4805 envelope frames (odd): every
        // odd-size path stays finite and inside sanity bounds (|C|,
        // |C1| ≤ 1 — weighted Cauchy–Schwarz at equal times; |AC| ≤ 1 —
        // same, via the symmetric lag normalizer, VERIFICATION §D;
        // |C2| ≤ 2.5 — the ideal phase-locked value is 2, plus window
        // and leakage margin; power ≥ 0). The far-from-carrier channels
        // of this truncated signal hold pure boundary leakage — exactly
        // where an unbounded normalizer would misbehave.
        let samples = (TEX_SECONDS * f64::from(TEX_SR)).round() as usize + 551;
        let centers = tex_centers();
        let mut tone = tex_am_tone(centers[20], &|t| {
            0.4 * (2.0 * std::f64::consts::PI * 6.25 * t).sin()
        });
        tone.resize(samples, 0.0);
        let stats = texture_stats(&cochlea::analyze(&tone, TEX_SR));
        assert!(stats
            .c
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 1.0 + 1e-9));
        assert!(stats
            .c1
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 1.0 + 1e-9));
        assert!(stats.c2.iter().all(|v| v[0].is_finite()
            && v[1].is_finite()
            && (v[0] * v[0] + v[1] * v[1]).sqrt() <= 2.5));
        let worst_ac = stats.ac.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
        assert!(worst_ac <= 1.0 + 1e-9, "max |AC| = {worst_ac}");
        assert!(stats.ac.iter().all(|v| v.is_finite()));
        assert!(stats.mod_power.iter().all(|v| v.is_finite() && *v >= 0.0));
        assert!(stats
            .marginal
            .iter()
            .all(|m| m.iter().all(|v| v.is_finite())));
    }
}
