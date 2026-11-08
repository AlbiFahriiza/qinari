//! Interaural correlation per band.
//!
//! For the first two channels the 1024-point STFT is computed per
//! channel; per band the weighted one-sided cross-sum
//! `sum Re(X_L * conj(X_R))` over the band's bins is divided by the
//! geometric mean of the channel powers. With the window applied to both
//! channels this equals the lag-zero Pearson correlation of the
//! band-filtered signals — exactly the quantity Gabriel & Colburn showed
//! listeners discriminate (see `RESEARCH-AUDIO-REALISM.md` §R 1.1).
//!
//! Bands: the four named bands plus the 24 log-spaced analysis bands
//! (`spectrum::band24_edges`) — the per-frequency-band view. The log
//! bands start at 40 Hz, so the bins below that (DC, sub-40 Hz) belong
//! to no log band; the named `b0_500` covers them.
//!
//! The design's acceptance metric: bed interaural correlation below 0.9
//! above 500 Hz (`DESIGN-REALISM-ENGINE.md` §7). Buffers with fewer than
//! two channels report `None`.

use crate::spectrum::band24_edges;
use crate::stft::{bin_weight, hann_window, ANALYSIS_FFT, ANALYSIS_HOP};
use qinari_dsp::buffer::AudioBuffer;
use realfft::num_complex::Complex64;
use realfft::RealFftPlanner;

/// Interaural section of the feature report.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct InterauralReport {
    /// Correlation in `[0, 500)` Hz.
    pub b0_500: f64,
    /// Correlation in `[500, 2000)` Hz.
    pub b500_2000: f64,
    /// Correlation in `[2000, 8000)` Hz.
    pub b2000_8000: f64,
    /// Correlation in `[8000, nyquist]` Hz.
    pub b8000_nyquist: f64,
    /// Per-band correlations on the 24 log-spaced analysis bands
    /// ([`crate::spectrum::band24_edges`]), low to high — the
    /// per-frequency-band extension of the four named bands above.
    pub per_band: Vec<f64>,
}

/// Per-band accumulators for one correlation.
#[derive(Default, Clone, Copy)]
struct BandSums {
    cross: f64,
    left: f64,
    right: f64,
}

impl BandSums {
    fn correlation(&self) -> f64 {
        let den = (self.left * self.right).sqrt();
        if den <= 0.0 {
            return 0.0;
        }
        (self.cross / den).clamp(-1.0, 1.0)
    }
}

/// Bin index range (inclusive start, exclusive end) for `[lo, hi)` Hz.
fn bin_range(sample_rate: u32, lo: f64, hi: f64, top_inclusive: bool) -> (usize, usize) {
    let bin_hz = f64::from(sample_rate) / ANALYSIS_FFT as f64;
    let nyquist = f64::from(sample_rate) / 2.0;
    let hi = if top_inclusive { hi.min(nyquist) } else { hi };
    let start = (lo / bin_hz).ceil().max(0.0) as usize;
    let end = {
        let e = if top_inclusive {
            (hi / bin_hz).floor() + 1.0
        } else {
            (hi / bin_hz).ceil()
        };
        (e.max(0.0) as usize).min(ANALYSIS_FFT / 2 + 1)
    };
    let (start, end) = (start.min(end), end);
    if end > start {
        return (start, end);
    }
    // A band narrower than one FFT bin (only the lowest log bands at
    // 44.1 kHz) would own an empty bin set — reading as a silent band
    // (correlation 0) even where the signal is loud. Collapse it to the
    // single bin holding its center.
    let center = f64::midpoint(lo, hi);
    let k = ((center / bin_hz).round() as usize).min(ANALYSIS_FFT / 2);
    (k, k + 1)
}

/// Computes the interaural correlations of the first two channels.
///
/// Returns `None` for mono buffers. Silent bands report 0.0 (the
/// correlation is undefined there).
///
/// # Panics
///
/// Panics if the FFT plan rejects the pre-sized scratch buffers — a
/// programming error, since the sizes come from the plan itself.
pub fn interaural_correlation(buffer: &AudioBuffer) -> Option<InterauralReport> {
    if buffer.num_channels() < 2 {
        return None;
    }
    let sample_rate = buffer.sample_rate();
    let left = buffer.channels()[0].clone();
    let right = buffer.channels()[1].clone();
    let frames = crate::stft::frame_count(left.len().max(right.len()), ANALYSIS_FFT, ANALYSIS_HOP);

    let nyquist = f64::from(sample_rate) / 2.0;
    // 4 named bands + the 24 log-spaced bands, both accumulated in one
    // pass. The top log band reaches the Nyquist bin, like b8000_nyquist.
    let log_edges = band24_edges(sample_rate);
    let mut bands: Vec<(f64, f64, bool)> = vec![
        (0.0, 500.0, false),
        (500.0, 2000.0, false),
        (2000.0, 8000.0, false),
        (8000.0, nyquist, true),
    ];
    bands.extend(log_edges.windows(2).map(|pair| (pair[0], pair[1], false)));
    if let Some(top) = bands.last_mut() {
        top.1 = nyquist;
        top.2 = true;
    }
    let ranges: Vec<(usize, usize)> = bands
        .iter()
        .map(|(lo, hi, top)| bin_range(sample_rate, *lo, *hi, *top))
        .collect();
    let bins = ANALYSIS_FFT / 2 + 1;

    let mut sums = vec![BandSums::default(); bands.len()];
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(ANALYSIS_FFT);
    let mut scratch = vec![Complex64::new(0.0, 0.0); forward.get_scratch_len()];
    let mut spectrum_l = vec![Complex64::new(0.0, 0.0); forward.complex_len()];
    let mut spectrum_r = vec![Complex64::new(0.0, 0.0); forward.complex_len()];
    let win = hann_window(ANALYSIS_FFT);

    for index in 0..frames {
        let start = index * ANALYSIS_HOP;
        let mut block_l = vec![0.0f64; ANALYSIS_FFT];
        let mut block_r = vec![0.0f64; ANALYSIS_FFT];
        for (k, (slot_l, slot_r)) in block_l.iter_mut().zip(block_r.iter_mut()).enumerate() {
            *slot_l = f64::from(left.get(start + k).copied().unwrap_or(0.0)) * win[k];
            *slot_r = f64::from(right.get(start + k).copied().unwrap_or(0.0)) * win[k];
        }
        // Invariant: buffers follow the plan, so these cannot fail.
        forward
            .process_with_scratch(&mut block_l, &mut spectrum_l, &mut scratch)
            .expect("fft buffers are sized by the plan");
        forward
            .process_with_scratch(&mut block_r, &mut spectrum_r, &mut scratch)
            .expect("fft buffers are sized by the plan");
        for (band, (begin, end)) in ranges.iter().enumerate() {
            let mut cross = 0.0;
            let mut power_l = 0.0;
            let mut power_r = 0.0;
            for k in *begin..*end {
                let w = bin_weight(k, bins);
                let l = &spectrum_l[k];
                let r = &spectrum_r[k];
                cross += w * (l.re * r.re + l.im * r.im);
                power_l += w * (l.re * l.re + l.im * l.im);
                power_r += w * (r.re * r.re + r.im * r.im);
            }
            sums[band].cross += cross;
            sums[band].left += power_l;
            sums[band].right += power_r;
        }
    }

    Some(InterauralReport {
        b0_500: sums[0].correlation(),
        b500_2000: sums[1].correlation(),
        b2000_8000: sums[2].correlation(),
        b8000_nyquist: sums[3].correlation(),
        per_band: sums[4..].iter().map(BandSums::correlation).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    fn noise_channels(seed: u64, frames: usize, correlation_split_hz: Option<f64>) -> AudioBuffer {
        // `None`: fully correlated (L == R). `Some(split)`: identical below
        // `split` Hz, independent above (built by spectral masking).
        let mut left = Vec::with_capacity(frames);
        let mut right = Vec::with_capacity(frames);
        let mut rng = qinari_dsp::rng::Qrng::new(seed);
        for _ in 0..frames {
            left.push(rng.next_f32());
            right.push(rng.next_f32());
        }
        let channels = match correlation_split_hz {
            None => vec![left.clone(), left],
            Some(split) => {
                // Right = L below split, independent noise above. The mix is
                // built by spectral masking; c2r rescales by N, which the
                // correlation ratio cancels. The mask walks the
                // full-length spectrum, so the split bin must live on its
                // SR/frames grid — computing it on the 1024-point STFT
                // grid would place the 500 Hz split at 1.2 Hz.
                let bin_hz = f64::from(SR) / frames as f64;
                let split_bin = (split / bin_hz).ceil() as usize;
                let mut planner = RealFftPlanner::<f64>::new();
                let forward = planner.plan_fft_forward(frames);
                let mut scratch = vec![Complex64::new(0.0, 0.0); forward.get_scratch_len()];
                let mut out = vec![Complex64::new(0.0, 0.0); forward.complex_len()];
                let l_block: Vec<f64> = left.iter().map(|v| f64::from(*v)).collect();
                let r_block: Vec<f64> = right.iter().map(|v| f64::from(*v)).collect();
                let mut input = l_block;
                forward
                    .process_with_scratch(&mut input, &mut out, &mut scratch)
                    .expect("plan-sized buffers");
                let spec_l = out;
                let mut input = r_block;
                let mut out2 = vec![Complex64::new(0.0, 0.0); forward.complex_len()];
                forward
                    .process_with_scratch(&mut input, &mut out2, &mut scratch)
                    .expect("plan-sized buffers");
                let mut masked = spec_l.clone();
                for (k, bin) in masked.iter_mut().enumerate() {
                    if k < split_bin {
                        *bin = spec_l[k];
                    } else {
                        *bin = out2[k];
                    }
                }
                let mixed = crate::stft::inverse_real_fft(&masked, frames);
                let mixed: Vec<f32> = mixed.iter().map(|v| *v as f32).collect();
                vec![left, mixed]
            }
        };
        AudioBuffer::from_channels(channels, SR).expect("well formed")
    }

    #[test]
    fn identical_channels_correlate_at_one_everywhere() {
        let buffer = noise_channels(7, 44_100 * 5, None);
        let report = interaural_correlation(&buffer).expect("stereo");
        assert!(report.b0_500 > 0.999, "b0_500 {report:?}");
        assert!(report.b500_2000 > 0.999);
        assert!(report.b2000_8000 > 0.999);
        assert!(report.b8000_nyquist > 0.999);
        assert_eq!(report.per_band.len(), 24);
        assert!(
            report.per_band.iter().all(|v| *v > 0.999),
            "per_band {report:?}"
        );
    }

    #[test]
    fn hf_decorrelated_channels_split_at_500_hz() {
        let buffer = noise_channels(11, 44_100 * 10, Some(500.0));
        let report = interaural_correlation(&buffer).expect("stereo");
        assert!(report.b0_500 > 0.9, "below the split: {report:?}");
        assert!(report.b500_2000 < 0.2, "above the split: {report:?}");
        assert!(report.b2000_8000 < 0.2);
        assert!(report.b8000_nyquist < 0.2);
        // The §7 acceptance property, demonstrated.
        assert!(report.b500_2000 < 0.9 && report.b2000_8000 < 0.9);
    }

    #[test]
    fn per_band_log_bands_track_the_decorrelation_split() {
        // The same 500 Hz split, read through the 24 log bands: every
        // band entirely below the split stays correlated, every band
        // entirely above it decorrelates. The band straddling 500 Hz is
        // exempt — it mixes both regimes.
        let buffer = noise_channels(11, 44_100 * 10, Some(500.0));
        let report = interaural_correlation(&buffer).expect("stereo");
        assert_eq!(report.per_band.len(), 24);
        let edges = band24_edges(SR);
        for (i, pair) in edges.windows(2).enumerate() {
            let correlation = report.per_band[i];
            if pair[1] <= 500.0 {
                assert!(
                    correlation > 0.9,
                    "band {i} [{:.0}, {:.0}) Hz below the split: {correlation}",
                    pair[0],
                    pair[1]
                );
            } else if pair[0] >= 500.0 {
                assert!(
                    correlation < 0.2,
                    "band {i} [{:.0}, {:.0}) Hz above the split: {correlation}",
                    pair[0],
                    pair[1]
                );
            }
        }
    }

    #[test]
    fn mono_buffers_report_none_and_silent_stereo_reports_zero() {
        let mono = AudioBuffer::from_channels(vec![vec![0.1f32; 100]], SR).expect("well formed");
        assert!(interaural_correlation(&mono).is_none());
        let silent =
            AudioBuffer::from_channels(vec![vec![0.0f32; 44_100], vec![0.0f32; 44_100]], SR)
                .expect("well formed");
        let report = interaural_correlation(&silent).expect("stereo");
        assert_eq!(report.b0_500, 0.0);
        assert_eq!(report.b8000_nyquist, 0.0);
        assert_eq!(report.per_band.len(), 24);
        assert!(report.per_band.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn bin_ranges_cover_the_named_bands() {
        let (a, b) = bin_range(SR, 0.0, 500.0, false);
        assert_eq!(a, 0);
        assert_eq!(b, 12); // 500 / 43.066 = 11.61 -> ceil 12
        let (a, b) = bin_range(SR, 8000.0, 22_050.0, true);
        assert_eq!(a, 186); // 8000 / 43.066 = 185.8 -> ceil 186
        assert_eq!(b, 513); // through the Nyquist bin
        let (a, b) = bin_range(16_000, 2000.0, 8000.0, false);
        assert_eq!(a, 128);
        assert_eq!(b, 512);
        // A band narrower than one bin collapses to the single bin
        // holding its center (never an empty set).
        let (a, b) = bin_range(SR, 52.2, 67.7, false);
        assert_eq!(a, 1); // center ~60 Hz -> bin 1
        assert_eq!(b, 2);
        let (a, b) = bin_range(SR, 88.1, 114.6, false);
        assert_eq!(a, 2); // center ~101 Hz -> bin 2
        assert_eq!(b, 3);
    }
}
