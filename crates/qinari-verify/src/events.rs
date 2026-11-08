//! Event statistics: onsets, rate, inter-onset CV, burstiness.
//!
//! The onset detector works on the full-band dB envelope (1024-point
//! STFT, hop 256): the rectified frame-to-frame difference `d[t]` is
//! thresholded at `max(1.0 dB, q75 + 6*IQR(d))` — an adaptive level that
//! sits above the natural fluctuation of a noise bed (typically ~2.5 dB)
//! but far below a real attack. Candidates are local maxima of `d` above
//! the threshold; a 50 ms refractory period (keep-first) suppresses
//! double triggers.
//!
//! Reported statistics over the detected onset times:
//!
//! - `count` and `rate_hz` (`count / duration`);
//! - `interval_cv`: sample standard deviation of inter-onset intervals
//!   over their mean (`None` below two events);
//! - `burstiness`: `(sigma^2 - mean^2) / (sigma^2 + mean^2)` of the
//!   intervals (Goh & Barabasi 2008, Europhys. J. 81, 48002) — `-1` for
//!   a perfectly regular train, `0` for Poisson, positive for clustered
//!   arrivals. Unit-invariant, unlike the raw `(sigma^2 - mean)` form.

/// Minimum separation between onsets in seconds.
const REFRACTORY_S: f64 = 0.05;

/// Event section of the feature report.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct EventReport {
    /// Number of detected onsets.
    pub count: usize,
    /// Onsets per second over the covered duration.
    pub rate_hz: f64,
    /// Mean inter-onset interval in seconds (`None` below two events).
    pub mean_interval_s: Option<f64>,
    /// Sample CV of the inter-onset intervals (`None` below two events).
    pub interval_cv: Option<f64>,
    /// Burstiness of the inter-onset intervals (`None` below two events).
    pub burstiness: Option<f64>,
}

/// Linear-interpolated quantile of an ascending series.
fn quantile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let pos = p * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    let frac = pos - lo as f64;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

/// Detects onset times (seconds) in a dB envelope series.
///
/// `env_rate` is the envelope frame rate in hertz. Series with fewer
/// than three frames, non-positive rates, or no positive derivative
/// report no onsets.
pub fn detect_onsets(gain_db: &[f64], env_rate: f64) -> Vec<f64> {
    if gain_db.len() < 3 || env_rate <= 0.0 {
        return Vec::new();
    }
    // Rectified derivative d[i] = rise into envelope frame i+1.
    let d: Vec<f64> = (1..gain_db.len())
        .map(|t| (gain_db[t] - gain_db[t - 1]).max(0.0))
        .collect();
    if !d.iter().any(|v| v > &0.0) {
        return Vec::new();
    }
    // Adaptive threshold from the derivative distribution.
    let mut sorted = d.clone();
    sorted.sort_by(f64::total_cmp);
    let q25 = quantile(&sorted, 0.25);
    let q75 = quantile(&sorted, 0.75);
    let threshold = (q75 + 6.0 * (q75 - q25)).max(1.0);

    // Local maxima of d above the threshold; d index i maps to frame i+1.
    let mut candidates: Vec<usize> = Vec::new();
    for i in 0..d.len() {
        let here = d[i];
        if here <= threshold {
            continue;
        }
        let prev = if i > 0 { d[i - 1] } else { f64::MIN };
        let next = if i + 1 < d.len() { d[i + 1] } else { 0.0 };
        if here > prev && here >= next {
            candidates.push(i + 1);
        }
    }

    // Refractory: keep-first.
    let refractory = (REFRACTORY_S * env_rate).round().max(1.0) as usize;
    let mut onsets: Vec<usize> = Vec::new();
    for t in candidates {
        if onsets.last().is_some_and(|last| t - last < refractory) {
            continue;
        }
        onsets.push(t);
    }
    onsets.into_iter().map(|t| t as f64 / env_rate).collect()
}

/// Inter-onset statistics: (mean, cv, burstiness), each `None` when the
/// interval count cannot support it.
fn interval_stats(onsets: &[f64]) -> (Option<f64>, Option<f64>, Option<f64>) {
    let intervals: Vec<f64> = onsets.windows(2).map(|w| w[1] - w[0]).collect();
    if intervals.is_empty() {
        return (None, None, None);
    }
    let n = intervals.len() as f64;
    let mean = intervals.iter().sum::<f64>() / n;
    if intervals.len() < 2 {
        return (Some(mean), None, None);
    }
    let var = intervals
        .iter()
        .map(|i| {
            let dv = i - mean;
            dv * dv
        })
        .sum::<f64>()
        / (n - 1.0);
    let sd = var.sqrt();
    let cv = sd / mean;
    let burst = (var - mean * mean) / (var + mean * mean);
    (Some(mean), Some(cv), Some(burst))
}

/// Builds the event report from the gain envelope.
pub(crate) fn analyze(gain_db: &[f64], env_rate: f64) -> EventReport {
    let onsets = detect_onsets(gain_db, env_rate);
    let duration = gain_db.len() as f64 / env_rate;
    let rate = if duration > 0.0 {
        onsets.len() as f64 / duration
    } else {
        0.0
    };
    let (mean, cv, burst) = interval_stats(&onsets);
    EventReport {
        count: onsets.len(),
        rate_hz: rate,
        mean_interval_s: mean,
        interval_cv: cv,
        burstiness: burst,
    }
}

/// Median-across-channels event report: the aggregate of the per-channel
/// event sections (the stereo-aware contract of `crate::analyze_buffer`).
///
/// Optional interval statistics (mean interval, CV, burstiness) quote
/// only when every channel quotes them — one channel below its
/// two-event floor withholds the aggregate too. `count` medians round
/// to nearest (ties away from zero), which is exact for one channel and
/// for two equal channels.
pub(crate) fn aggregate(reports: &[EventReport]) -> EventReport {
    let mut scratch = Vec::with_capacity(reports.len());
    EventReport {
        count: crate::median_stat(&mut scratch, reports, |r| r.count as f64).round() as usize,
        rate_hz: crate::median_stat(&mut scratch, reports, |r| r.rate_hz),
        mean_interval_s: crate::median_option(&mut scratch, reports, |r| r.mean_interval_s),
        interval_cv: crate::median_option(&mut scratch, reports, |r| r.interval_cv),
        burstiness: crate::median_option(&mut scratch, reports, |r| r.burstiness),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 200.0; // 5 ms frames

    fn staircase(len: usize, jump_frames: &[usize], depth_db: f64) -> Vec<f64> {
        let mut series = vec![-30.0f64; len];
        let mut level = -30.0;
        let mut next = 0;
        for (t, slot) in series.iter_mut().enumerate() {
            if next < jump_frames.len() && t == jump_frames[next] {
                level += depth_db;
                next += 1;
            }
            *slot = level;
        }
        series
    }

    #[test]
    fn quantile_interpolates_positions() {
        let data = vec![1.0, 2.0, 3.0, 4.0];
        assert!((quantile(&data, 0.0) - 1.0).abs() < 1e-12);
        assert!((quantile(&data, 0.5) - 2.5).abs() < 1e-12);
        assert!((quantile(&data, 1.0) - 4.0).abs() < 1e-12);
        assert_eq!(quantile(&[], 0.5), 0.0);
    }

    #[test]
    fn regular_train_reports_the_designed_rate_cv_and_burstiness() {
        // Jumps every 0.5 s (100 frames) for 20 s: 40 onsets at 2 Hz.
        // The first jump lands at frame 99 so all 40 fit inside the
        // 4000-frame series (an earlier draft started at 200 and lost the
        // last two jumps past the end).
        let jumps: Vec<usize> = (0..40).map(|i| 99 + i * 100).collect();
        let series = staircase(4000, &jumps, 40.0);
        let onsets = detect_onsets(&series, RATE);
        assert_eq!(onsets.len(), 40, "one onset per jump");
        let report = analyze(&series, RATE);
        assert!(
            (report.rate_hz - 2.0).abs() < 0.06,
            "rate {}",
            report.rate_hz
        );
        let (mean, cv, burst) = interval_stats(&onsets);
        assert!((mean.expect("present") - 0.5).abs() < 1e-9);
        assert!(cv.expect("present").abs() < 1e-12, "regular: cv 0");
        assert!((burst.expect("present") + 1.0).abs() < 1e-12, "regular: -1");
    }

    #[test]
    fn clustered_train_reports_cv_above_one_and_positive_burstiness() {
        // 13 clusters, 2 s apart; three jumps per cluster at 0, 0.05, 0.1 s
        // within the cluster. Interval multiset: 26 x 0.05 s (two per
        // cluster) and 12 x 1.9 s (one per cluster pair) = 38 intervals.
        let mut jumps: Vec<usize> = Vec::new();
        for cluster in 0..13 {
            let s = 200 + cluster * 400;
            jumps.push(s);
            jumps.push(s + 10);
            jumps.push(s + 20);
        }
        // 5400 frames so the last cluster (starting at 5000) fits: the
        // earlier 4000-frame draft silently dropped its three jumps.
        let series = staircase(5400, &jumps, 40.0);
        let onsets = detect_onsets(&series, RATE);
        assert_eq!(onsets.len(), 39, "three per cluster x 13");
        let (mean, cv, burst) = interval_stats(&onsets);
        // Ground truth from the designed multiset, sample statistics.
        let multiset = [vec![0.05_f64; 26], vec![1.9_f64; 12]].concat();
        let n = multiset.len() as f64;
        let t_mean = multiset.iter().sum::<f64>() / n;
        let t_var = multiset
            .iter()
            .map(|i| {
                let dv = i - t_mean;
                dv * dv
            })
            .sum::<f64>()
            / (n - 1.0);
        let t_cv = t_var.sqrt() / t_mean;
        let t_burst = (t_var - t_mean * t_mean) / (t_var + t_mean * t_mean);
        eprintln!(
            "clustered: mean {mean:?}, cv {cv:?} (expect {t_cv:.4}), burst {burst:?} (expect {t_burst:.4})"
        );
        assert!((mean.expect("present") - t_mean).abs() < 0.02);
        assert!(
            (cv.expect("present") - t_cv).abs() < 0.03,
            "cv {cv:?} vs {t_cv}"
        );
        assert!(
            (burst.expect("present") - t_burst).abs() < 0.05,
            "burst {burst:?} vs {t_burst}"
        );
        assert!(cv.expect("present") > 1.0, "clustered: cv > 1");
        assert!(burst.expect("present") > 0.0, "clustered: burst > 0");
    }

    #[test]
    fn flat_and_quiet_series_report_no_events() {
        assert_eq!(detect_onsets(&[], RATE), [] as [f64; 0]);
        assert_eq!(detect_onsets(&[-10.0; 2], RATE), [] as [f64; 0]);
        let flat = vec![-12.0f64; 500];
        let report = analyze(&flat, RATE);
        assert_eq!(report.count, 0);
        assert_eq!(report.rate_hz, 0.0);
        assert_eq!(report.mean_interval_s, None);
        assert_eq!(report.interval_cv, None);
        assert_eq!(report.burstiness, None);
        // Slow ramp (0.05 dB/frame) never crosses the threshold.
        let ramp: Vec<f64> = (0..1000).map(|t| -30.0 + 0.05 * f64::from(t)).collect();
        assert_eq!(detect_onsets(&ramp, RATE), [] as [f64; 0]);
        // Tiny wiggle: below the 1 dB floor.
        let wiggle: Vec<f64> = (0..1000)
            .map(|t| -20.0 + 0.2 * (f64::from(t) * 0.9).sin())
            .collect();
        assert_eq!(detect_onsets(&wiggle, RATE), [] as [f64; 0]);
    }

    #[test]
    fn refractory_suppresses_double_triggers() {
        // Two jumps 0.02 s apart (4 frames < 50 ms): only the first counts.
        let series = staircase(600, &[100, 104], 40.0);
        let onsets = detect_onsets(&series, RATE);
        assert_eq!(onsets.len(), 1, "refractory 50 ms at 200 fps = 10 frames");
        // Jumps 0.1 s apart both count.
        let series = staircase(600, &[100, 120], 40.0);
        let onsets = detect_onsets(&series, RATE);
        assert_eq!(onsets.len(), 2);
    }

    #[test]
    fn single_event_has_no_intervals() {
        let series = staircase(600, &[100], 40.0);
        let report = analyze(&series, RATE);
        assert_eq!(report.count, 1);
        assert_eq!(report.mean_interval_s, None);
        assert_eq!(report.interval_cv, None);
        assert_eq!(report.burstiness, None);
    }
}
