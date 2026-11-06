//! Speech features: F0 track, F0 statistics, pause statistics.
//!
//! F0 track (autocorrelation, 40 ms frames, 10 ms hop, search 60–400 Hz):
//! each frame's linear autocorrelation is computed through a zero-padded
//! FFT (2x the frame, next power of two, no wrap-around). A frame is
//! voiced when its RMS exceeds 1e-5 and the maximum normalized
//! autocorrelation over the lag range reaches 0.5; among the local
//! maxima above 0.93x that maximum, the *smallest* lag is reported — the
//! standard guard against octave-halving. `f0 = sample_rate / lag`.
//! The voiced sequence is then median-filtered (window 5).
//!
//! Reported F0 statistics: mean in hertz, sigma and range in semitones
//! (`12*log2(f)`; sample standard deviation). All `None` when there are
//! too few voiced frames.
//!
//! Pause statistics (20 ms frames, 10 ms hop): a frame is silent when its
//! RMS is below 1e-4; a *pause* is a run of silent frames of at least
//! 150 ms with active audio on both sides (leading and trailing silence
//! is excluded). Reported: count, total seconds, rate per minute, mean
//! duration.

/// F0 search range in hertz.
const F0_MIN_HZ: f64 = 60.0;
const F0_MAX_HZ: f64 = 400.0;
/// Normalized-autocorrelation voicing threshold.
const VOICED_PEAK: f64 = 0.5;
/// Sub-maximum acceptance for the earliest strong peak.
const PEAK_SHARE: f64 = 0.93;
/// Voicing RMS floor.
const ENERGY_FLOOR: f32 = 1e-5;
/// Median-filter width (frames).
const MEDIAN_WIDTH: usize = 5;
/// Minimum pause length in seconds.
const PAUSE_MIN_S: f64 = 0.15;
/// Silence RMS threshold for pause frames.
const SILENCE_RMS: f32 = 1e-4;

/// The F0 track: per-analysis-frame time, voicing, and F0.
///
/// `f0_hz[i]` is meaningful only where `voiced[i]`; the median filter
/// runs over the voiced subsequence in time order.
#[derive(Debug, Clone, PartialEq)]
pub struct F0Track {
    /// Frame start times in seconds.
    pub frame_times_s: Vec<f64>,
    /// Voicing decision per frame.
    pub voiced: Vec<bool>,
    /// F0 in hertz per frame (valid where voiced).
    pub f0_hz: Vec<f64>,
}

/// F0 section of the speech report.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct F0Report {
    /// Number of voiced frames.
    pub voiced_frames: usize,
    /// Voiced frames over total frames.
    pub voiced_fraction: f64,
    /// Mean of the median-filtered F0 over voiced frames (`None` if none).
    pub f0_mean_hz: Option<f64>,
    /// Sample standard deviation of the F0 in semitones (`None` below 2).
    pub f0_sigma_st: Option<f64>,
    /// F0 range in semitones, `12*log2(max/min)` (`None` if none).
    pub f0_range_st: Option<f64>,
}

/// Pause section of the speech report.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PauseReport {
    /// Number of interior pauses of at least 150 ms.
    pub count: usize,
    /// Total pause seconds.
    pub total_s: f64,
    /// Pauses per minute over the buffer duration.
    pub rate_per_min: f64,
    /// Mean pause duration in seconds (`None` when no pauses).
    pub mean_duration_s: Option<f64>,
}

/// Speech section of the feature report.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SpeechReport {
    /// F0 statistics.
    pub f0: F0Report,
    /// Pause statistics.
    pub pauses: PauseReport,
}

/// Next power of two at least `n` (minimum 2).
fn next_pow2(n: usize) -> usize {
    let mut p = 2usize;
    while p < n {
        p *= 2;
    }
    p
}

/// Median of a non-empty slice.
fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        f64::midpoint(sorted[mid - 1], sorted[mid])
    }
}

/// RMS of a frame.
fn frame_rms(samples: &[f32], start: usize, len: usize) -> f32 {
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for k in 0..len {
        if let Some(&v) = samples.get(start + k) {
            sum += f64::from(v) * f64::from(v);
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        (sum / count as f64).sqrt() as f32
    }
}

/// Computes the F0 track of a mono signal.
pub fn f0_track(samples: &[f32], sample_rate: u32) -> F0Track {
    let sr = f64::from(sample_rate);
    let frame_len = (0.04 * sr).round().max(4.0) as usize;
    let hop = (0.01 * sr).round().max(1.0) as usize;
    let lag_lo = (sr / F0_MAX_HZ).ceil().max(2.0) as usize;
    let lag_hi = (sr / F0_MIN_HZ).floor() as usize;
    let frames = crate::stft::frame_count(samples.len(), frame_len, hop);
    let fft_len = next_pow2(2 * frame_len);

    let mut times = Vec::with_capacity(frames);
    let mut voiced = Vec::with_capacity(frames);
    let mut f0s = Vec::with_capacity(frames);
    for index in 0..frames {
        let start = index * hop;
        times.push(start as f64 / sr);
        let rms = frame_rms(samples, start, frame_len);
        if rms <= ENERGY_FLOOR || lag_hi <= lag_lo + 1 {
            voiced.push(false);
            f0s.push(0.0);
            continue;
        }
        let mut block = vec![0.0f64; fft_len];
        for (k, slot) in block.iter_mut().enumerate() {
            *slot = f64::from(samples.get(start + k).copied().unwrap_or(0.0));
        }
        let spectrum = crate::stft::real_fft(&block);
        let powers: Vec<realfft::num_complex::Complex64> = spectrum
            .iter()
            .map(|bin| realfft::num_complex::Complex64::new(bin.re * bin.re + bin.im * bin.im, 0.0))
            .collect();
        let autocorr = crate::stft::inverse_real_fft(&powers, fft_len);
        let r0 = autocorr[0];
        if r0.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            voiced.push(false);
            f0s.push(0.0);
            continue;
        }
        let nr = |lag: usize| autocorr[lag] / r0;
        let mut peak = f64::MIN;
        for lag in lag_lo..=lag_hi {
            peak = peak.max(nr(lag));
        }
        if peak < VOICED_PEAK {
            voiced.push(false);
            f0s.push(0.0);
            continue;
        }
        let mut chosen: Option<usize> = None;
        for lag in lag_lo..=lag_hi {
            let value = nr(lag);
            if value < PEAK_SHARE * peak {
                continue;
            }
            let prev = if lag > lag_lo { nr(lag - 1) } else { f64::MIN };
            let next = if lag < lag_hi { nr(lag + 1) } else { f64::MIN };
            if value >= prev && value >= next {
                chosen = Some(lag);
                break;
            }
        }
        if let Some(lag) = chosen {
            voiced.push(true);
            f0s.push(sr / lag as f64);
        } else {
            // A peak above threshold exists but no interior maximum
            // materialized (pathological edge); treat as unvoiced.
            voiced.push(false);
            f0s.push(0.0);
        }
    }

    // Median filter over the voiced subsequence, in time order.
    let voiced_values: Vec<f64> = voiced
        .iter()
        .zip(f0s.iter())
        .filter_map(|(v, f)| v.then_some(*f))
        .collect();
    let mut filtered = vec![0.0f64; voiced_values.len()];
    let half = MEDIAN_WIDTH / 2;
    for (i, slot) in filtered.iter_mut().enumerate() {
        let lo = i.saturating_sub(half);
        let hi = (i + half + 1).min(voiced_values.len());
        *slot = median(&voiced_values[lo..hi]);
    }
    let mut f0_out = vec![0.0f64; f0s.len()];
    let mut next = 0usize;
    for (i, flag) in voiced.iter().enumerate() {
        if *flag {
            f0_out[i] = filtered[next];
            next += 1;
        }
    }

    F0Track {
        frame_times_s: times,
        voiced,
        f0_hz: f0_out,
    }
}

/// F0 statistics over a track's voiced frames.
fn f0_stats(track: &F0Track, total_frames: usize) -> F0Report {
    let values: Vec<f64> = track
        .voiced
        .iter()
        .zip(track.f0_hz.iter())
        .filter_map(|(v, f)| v.then_some(*f))
        .collect();
    let count = values.len();
    let fraction = if total_frames == 0 {
        0.0
    } else {
        count as f64 / total_frames as f64
    };
    let mean = if count == 0 {
        None
    } else {
        Some(values.iter().sum::<f64>() / count as f64)
    };
    let sigma = if count < 2 {
        None
    } else {
        let semitones: Vec<f64> = values.iter().map(|f| 12.0 * f.log2()).collect();
        let m = semitones.iter().sum::<f64>() / semitones.len() as f64;
        let var = semitones
            .iter()
            .map(|s| {
                let d = s - m;
                d * d
            })
            .sum::<f64>()
            / (semitones.len() as f64 - 1.0);
        Some(var.sqrt())
    };
    let range = if count == 0 {
        None
    } else {
        let min = values.iter().copied().fold(f64::MAX, f64::min);
        let max = values.iter().copied().fold(f64::MIN, f64::max);
        Some(12.0 * (max / min).log2())
    };
    F0Report {
        voiced_frames: count,
        voiced_fraction: fraction,
        f0_mean_hz: mean,
        f0_sigma_st: sigma,
        f0_range_st: range,
    }
}

/// Computes the pause statistics of a mono signal.
pub fn pause_stats(samples: &[f32], sample_rate: u32) -> PauseReport {
    let sr = f64::from(sample_rate);
    let frame_len = (0.02 * sr).round().max(2.0) as usize;
    let hop = (0.01 * sr).round().max(1.0) as usize;
    let frames = crate::stft::frame_count(samples.len(), frame_len, hop);
    let mut silent = vec![false; frames];
    for (index, slot) in silent.iter_mut().enumerate() {
        *slot = frame_rms(samples, index * hop, frame_len) < SILENCE_RMS;
    }

    // Interior runs: silent stretches with active audio on both sides.
    let mut pauses: Vec<usize> = Vec::new(); // run lengths in frames
    let mut run = 0usize;
    let mut seen_active_after = vec![false; frames + 1];
    for index in (0..frames).rev() {
        seen_active_after[index] = seen_active_after[index + 1] || !silent[index];
    }
    let mut seen_active_before = false;
    for index in 0..frames {
        if !silent[index] {
            seen_active_before = true;
            run = 0;
            continue;
        }
        run += 1;
        let run_ends = index + 1 == frames || !silent[index + 1];
        if run_ends {
            let interior =
                seen_active_before && seen_active_after.get(index + 1).copied().unwrap_or(false);
            if interior && run as f64 * hop as f64 / sr >= PAUSE_MIN_S {
                pauses.push(run);
            }
            run = 0;
        }
    }

    let duration = samples.len() as f64 / sr;
    let total: f64 = pauses.iter().map(|&r| r as f64 * hop as f64 / sr).sum();
    let rate = if duration > 0.0 {
        pauses.len() as f64 * 60.0 / duration
    } else {
        0.0
    };
    let mean = if pauses.is_empty() {
        None
    } else {
        Some(total / pauses.len() as f64)
    };
    PauseReport {
        count: pauses.len(),
        total_s: total,
        rate_per_min: rate,
        mean_duration_s: mean,
    }
}

/// Builds the speech report (F0 + pauses) for a mono signal.
pub(crate) fn analyze(mono: &[f32], sample_rate: u32) -> SpeechReport {
    let track = f0_track(mono, sample_rate);
    let f0 = f0_stats(&track, track.voiced.len());
    let pauses = pause_stats(mono, sample_rate);
    SpeechReport { f0, pauses }
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
    fn helpers() {
        assert_eq!(next_pow2(1), 2);
        assert_eq!(next_pow2(2), 2);
        assert_eq!(next_pow2(3), 4);
        assert_eq!(next_pow2(16), 16);
        assert_eq!(next_pow2(17), 32);
        assert!((median(&[3.0, 1.0, 2.0]) - 2.0).abs() < 1e-12);
        assert!((median(&[4.0, 1.0, 3.0, 2.0]) - 2.5).abs() < 1e-12);
    }

    #[test]
    fn f0_of_a_150_hz_sine_reads_150_with_flat_variation() {
        let samples = sine(150.0, 0.4, 2.0);
        let track = f0_track(&samples, SR);
        let report = f0_stats(&track, track.voiced.len());
        eprintln!(
            "f0 150 Hz: voiced {}/{}, mean {:?}",
            report.voiced_frames,
            track.voiced.len(),
            report.f0_mean_hz
        );
        assert!(report.voiced_fraction > 0.95, "fraction {report:?}");
        let mean = report.f0_mean_hz.expect("voiced frames exist");
        assert!((mean - 150.0).abs() < 1.0, "mean {mean}");
        assert!(report.f0_sigma_st.expect("present") < 0.05);
        assert!(report.f0_range_st.expect("present") < 0.2);
    }

    #[test]
    fn f0_of_75_and_220_hz_sines_avoids_octave_errors() {
        let samples = sine(75.0, 0.4, 2.0);
        let report = analyze(&samples, SR).f0;
        let mean = report.f0_mean_hz.expect("voiced");
        assert!((mean - 75.0).abs() < 1.5, "75 Hz read {mean}");
        let samples = sine(220.0, 0.4, 2.0);
        let report = analyze(&samples, SR).f0;
        let mean = report.f0_mean_hz.expect("voiced");
        assert!((mean - 220.0).abs() < 2.0, "220 Hz read {mean}");
    }

    #[test]
    fn white_noise_is_unvoiced() {
        let mut rng = qinari_dsp::rng::Qrng::new(99);
        let noise: Vec<f32> = (0..44_100 * 4)
            .map(|_| rng.next_f32() * 2.0 - 1.0)
            .collect();
        let report = analyze(&noise, SR).f0;
        eprintln!(
            "noise f0: voiced {} of {}",
            report.voiced_frames,
            4 * 44_100 / 441
        );
        assert_eq!(report.voiced_frames, 0, "no voiced frames in white noise");
        assert_eq!(report.f0_mean_hz, None);
        assert_eq!(report.f0_sigma_st, None);
        assert_eq!(report.f0_range_st, None);
    }

    #[test]
    fn silence_and_empty_input_are_unvoiced_without_panicking() {
        let report = analyze(&[], SR);
        assert_eq!(report.f0.voiced_frames, 0);
        assert_eq!(report.f0.f0_mean_hz, None);
        assert_eq!(report.pauses.count, 0);
        let report = analyze(&vec![0.0f32; 44_100], SR);
        assert_eq!(report.f0.voiced_frames, 0);
        assert_eq!(report.pauses.count, 0);
    }

    #[test]
    fn vibrato_sine_shows_f0_sigma_in_semitones() {
        // 185 -> 220 Hz glissando over 2 s: 12*log2(220/185) = 3.0 st.
        let n = 44_100 * 2;
        let mut samples = vec![0.0f32; n];
        let mut phase = 0.0f64;
        for (i, slot) in samples.iter_mut().enumerate() {
            let t = i as f64 / 44_100.0;
            let f = 185.0 + (220.0 - 185.0) * t / 2.0;
            phase += 2.0 * std::f64::consts::PI * f / 44_100.0;
            *slot = (0.4 * phase.sin()) as f32;
        }
        let report = analyze(&samples, SR).f0;
        let range = report.f0_range_st.expect("voiced");
        assert!((range - 3.0).abs() < 0.6, "range {range} st (expect ~3)");
        assert!(report.f0_sigma_st.expect("present") > 0.8);
    }

    #[test]
    fn known_pause_pattern_is_counted_exactly() {
        // 0.4 s tone, 0.3 s silence x3, then 0.4 s tone, 0.12 s silence,
        // 0.4 s tone, 0.12 s silence, 0.4 s tone: 3 pauses >= 150 ms.
        let mut samples = Vec::new();
        let push_tone = |samples: &mut Vec<f32>| {
            let n = (0.4 * 44_100.0) as usize;
            for i in 0..n {
                samples.push(
                    (0.3 * (2.0 * std::f64::consts::PI * 300.0 * i as f64 / 44_100.0).sin()) as f32,
                );
            }
        };
        let push_silence = |samples: &mut Vec<f32>, secs: f64| {
            for _ in 0..(secs * 44_100.0) as usize {
                samples.push(0.0);
            }
        };
        push_tone(&mut samples);
        for _ in 0..3 {
            push_silence(&mut samples, 0.3);
            push_tone(&mut samples);
        }
        push_silence(&mut samples, 0.12);
        push_tone(&mut samples);
        push_silence(&mut samples, 0.12);
        push_tone(&mut samples);
        let report = pause_stats(&samples, SR);
        eprintln!(
            "pauses: count {}, total {}, mean {:?}",
            report.count, report.total_s, report.mean_duration_s
        );
        assert_eq!(report.count, 3, "only the >=150 ms interior gaps");
        // Three 0.3 s gaps measure as ~0.29 s each at the frame resolution.
        assert!(
            report.total_s > 0.85 && report.total_s < 0.92,
            "total {report:?}"
        );
        let expected_rate = 3.0 * 60.0 / (samples.len() as f64 / 44_100.0);
        assert!((report.rate_per_min - expected_rate).abs() < 0.5);
        assert!(report.mean_duration_s.expect("present") > 0.24);
    }

    #[test]
    fn all_silent_and_all_active_buffers_have_no_pauses() {
        let report = pause_stats(&vec![0.0f32; 44_100 * 2], SR);
        assert_eq!(report.count, 0);
        assert_eq!(report.total_s, 0.0);
        let tone = sine(300.0, 0.3, 2.0);
        let report = pause_stats(&tone, SR);
        assert_eq!(report.count, 0);
    }
}
