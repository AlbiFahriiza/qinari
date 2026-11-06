//! Distances between two buffers.
//!
//! - [`log_spectral_distance`]: root-mean-square of the dB difference of
//!   the two Welch power spectra, in dB.
//! - [`multi_resolution_stft`]: the Yamamoto et al. 2019
//!   (arXiv:1910.11480) multi-resolution STFT loss — per resolution
//!   (windows 512/1024/2048, hop window/4, periodic Hann) the sum of the
//!   spectral convergence
//!   `||A - B||_F^2 / (||A||_F^2 + ||B||_F^2)` and the mean absolute
//!   difference of natural-log magnitudes (clamped at 1e-8), summed over
//!   the three resolutions. Frames are uncentered (fully inside the
//!   signal).
//! - [`texture_distance`]: the multi-feature regression distance —
//!   `spectrum_db` (mean |dB difference| of the 24-band energy fractions,
//!   each floored at 1e-6 = -60 dB), `envelope_moments` and `modulation`
//!   (mean capped relative differences over the respective report
//!   statistics), and `events` (relative difference of rate, CV, and
//!   burstiness; a one-sided missing statistic counts as 1.0, both-sided
//!   missing as 0.0). The `total` is
//!   `spectrum_db + 10*(envelope + modulation + events)`: a 10% relative
//!   texture change is placed on the same footing as a 1 dB spectral
//!   change — the calibration the design's regression gates are authored
//!   against.

use crate::mono_mixdown;
use crate::stft::hann_window;
use crate::VerifyError;
use qinari_dsp::buffer::AudioBuffer;

/// Floor for Welch powers inside ratios.
const POWER_FLOOR: f64 = 1e-12;
/// Floor for band fractions inside dB ratios (-60 dB).
const FRACTION_FLOOR: f64 = 1e-6;
/// Log-magnitude clamp of the multi-resolution loss.
const MAG_CLAMP: f64 = 1e-8;

/// The per-resolution view of the multi-resolution STFT loss.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct ResolutionLoss {
    /// FFT window size in samples.
    pub window: usize,
    /// Hop in samples (window / 4).
    pub hop: usize,
    /// Spectral convergence term, `[0, 1)`.
    pub spectral_convergence: f64,
    /// Mean |log-magnitude difference| (natural log).
    pub log_magnitude: f64,
}

/// The multi-resolution STFT loss.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MultiResReport {
    /// One entry per resolution (512, 1024, 2048).
    pub resolutions: Vec<ResolutionLoss>,
    /// Sum of `spectral_convergence + log_magnitude` over resolutions.
    pub total: f64,
}

/// The texture-statistic distance.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TextureReport {
    /// Mean |dB difference| of the 24-band energy fractions.
    pub spectrum_db: f64,
    /// Mean capped relative difference of the envelope moments.
    pub envelope_moments: f64,
    /// Mean capped relative difference of the modulation statistics.
    pub modulation: f64,
    /// Relative difference of the event statistics.
    pub events: f64,
    /// `spectrum_db + 10*(envelope + modulation + events)`.
    pub total: f64,
}

/// Verifies both buffers share a sample rate.
fn check_rates(a: &AudioBuffer, b: &AudioBuffer) -> Result<(), VerifyError> {
    if a.sample_rate() != b.sample_rate() {
        return Err(VerifyError::MismatchedSampleRate {
            a: a.sample_rate(),
            b: b.sample_rate(),
        });
    }
    Ok(())
}

/// Log-spectral distance in dB between the two buffers' Welch spectra.
///
/// # Errors
///
/// Returns [`VerifyError::MismatchedSampleRate`] when the buffers differ
/// in sample rate.
pub fn log_spectral_distance(a: &AudioBuffer, b: &AudioBuffer) -> Result<f64, VerifyError> {
    check_rates(a, b)?;
    let mono_a = mono_mixdown(a);
    let mono_b = mono_mixdown(b);
    let pa = crate::spectrum::welch_power_spectrum(&mono_a);
    let pb = crate::spectrum::welch_power_spectrum(&mono_b);
    let mut acc = 0.0;
    let mut count = 0usize;
    for (x, y) in pa.iter().zip(pb.iter()) {
        let xa = x + POWER_FLOOR;
        let xb = y + POWER_FLOOR;
        let db = 10.0 * (xa / xb).log10();
        acc += db * db;
        count += 1;
    }
    Ok(if count == 0 {
        0.0
    } else {
        (acc / count as f64).sqrt()
    })
}

/// Magnitude frames of a buffer at one resolution (uncentered).
fn magnitude_frames(mono: &[f32], window: usize, hop: usize) -> Vec<Vec<f64>> {
    let count = crate::stft::frame_count(mono.len(), window, hop);
    let win = hann_window(window);
    let mut frames = Vec::with_capacity(count);
    for index in 0..count {
        let start = index * hop;
        let mut block = vec![0.0f64; window];
        for (k, slot) in block.iter_mut().enumerate() {
            *slot = f64::from(mono.get(start + k).copied().unwrap_or(0.0)) * win[k];
        }
        let spectrum = crate::stft::real_fft(&block);
        let mags: Vec<f64> = spectrum
            .iter()
            .map(|bin| (bin.re * bin.re + bin.im * bin.im).sqrt())
            .collect();
        frames.push(mags);
    }
    frames
}

/// One resolution's (sc, log-magnitude) pair.
fn resolution_loss(fa: &[Vec<f64>], fb: &[Vec<f64>], window: usize, hop: usize) -> ResolutionLoss {
    let mut diff2 = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    let mut log_l1 = 0.0;
    let mut elements = 0usize;
    for (frame_a, frame_b) in fa.iter().zip(fb.iter()) {
        for (a, b) in frame_a.iter().zip(frame_b.iter()) {
            let d = a - b;
            diff2 += d * d;
            norm_a += a * a;
            norm_b += b * b;
            log_l1 += ((a + MAG_CLAMP).ln() - (b + MAG_CLAMP).ln()).abs();
            elements += 1;
        }
    }
    let sc = diff2 / (norm_a + norm_b + f64::MIN_POSITIVE);
    let mag = if elements == 0 {
        0.0
    } else {
        log_l1 / elements as f64
    };
    ResolutionLoss {
        window,
        hop,
        spectral_convergence: sc,
        log_magnitude: mag,
    }
}

/// Multi-resolution STFT loss (Yamamoto-style; see module docs).
///
/// # Errors
///
/// Returns [`VerifyError::MismatchedSampleRate`] when the buffers differ
/// in sample rate.
pub fn multi_resolution_stft(
    a: &AudioBuffer,
    b: &AudioBuffer,
) -> Result<MultiResReport, VerifyError> {
    check_rates(a, b)?;
    let mono_a = mono_mixdown(a);
    let mono_b = mono_mixdown(b);
    let mut resolutions = Vec::with_capacity(3);
    for window in [512usize, 1024, 2048] {
        let hop = window / 4;
        let fa = magnitude_frames(&mono_a, window, hop);
        let fb = magnitude_frames(&mono_b, window, hop);
        resolutions.push(resolution_loss(&fa, &fb, window, hop));
    }
    let total: f64 = resolutions
        .iter()
        .map(|r| r.spectral_convergence + r.log_magnitude)
        .sum();
    Ok(MultiResReport { resolutions, total })
}

/// Capped relative difference `|a-b| / max(|a|, |b|, floor)`.
fn relative(a: f64, b: f64, floor: f64) -> f64 {
    let denom = a.abs().max(b.abs()).max(floor);
    ((a - b).abs() / denom).min(1.0)
}

/// Relative difference of `Option<f64>` statistics.
fn optional_relative(a: Option<f64>, b: Option<f64>, floor: f64) -> f64 {
    match (a, b) {
        (Some(x), Some(y)) => relative(x, y, floor),
        (None, None) => 0.0,
        (Some(_), None) | (None, Some(_)) => 1.0,
    }
}

/// Texture-statistic distance (see module docs).
///
/// # Errors
///
/// Returns [`VerifyError::MismatchedSampleRate`] when the buffers differ
/// in sample rate.
pub fn texture_distance(a: &AudioBuffer, b: &AudioBuffer) -> Result<TextureReport, VerifyError> {
    check_rates(a, b)?;
    let report_a = crate::analyze_buffer(a);
    let report_b = crate::analyze_buffer(b);

    // Spectrum: mean |dB difference| of the 24-band fractions.
    let mut spectrum = 0.0;
    for (fa, fb) in report_a
        .spectrum
        .bands24_energy_fraction
        .iter()
        .zip(report_b.spectrum.bands24_energy_fraction.iter())
    {
        let db = 10.0 * ((fa.max(FRACTION_FLOOR)) / (fb.max(FRACTION_FLOOR))).log10();
        spectrum += db.abs();
    }
    let spectrum_db = spectrum / report_a.spectrum.bands24_energy_fraction.len().max(1) as f64;

    // Envelope moments.
    let mut envelope = 0.0;
    let mut envelope_n = 0usize;
    for (ea, eb) in report_a
        .envelopes
        .bands
        .iter()
        .zip(report_b.envelopes.bands.iter())
    {
        envelope += relative(ea.mean_db, eb.mean_db, 1e-9);
        envelope += relative(ea.variance, eb.variance, 1e-9);
        envelope += relative(ea.sparsity, eb.sparsity, 1e-9);
        envelope += relative(ea.kurtosis_excess, eb.kurtosis_excess, 1e-9);
        envelope_n += 4;
    }
    let envelope_moments = if envelope_n == 0 {
        0.0
    } else {
        envelope / envelope_n as f64
    };

    // Modulation.
    let mut modulation = 0.0;
    let mut modulation_n = 0usize;
    for (ma, mb) in report_a
        .modulation
        .per_band
        .iter()
        .zip(report_b.modulation.per_band.iter())
    {
        modulation += relative(ma.peak_mean_ratio, mb.peak_mean_ratio, 1e-9);
        modulation += relative(ma.peak_hz, mb.peak_hz, 0.01);
        modulation_n += 2;
    }
    let modulation = if modulation_n == 0 {
        0.0
    } else {
        modulation / modulation_n as f64
    };

    // Events.
    let events = {
        let mut e = 0.0;
        e += optional_relative(
            Some(report_a.events.rate_hz),
            Some(report_b.events.rate_hz),
            1e-9,
        );
        e += optional_relative(
            report_a.events.interval_cv,
            report_b.events.interval_cv,
            1e-9,
        );
        e += optional_relative(report_a.events.burstiness, report_b.events.burstiness, 1e-9);
        e / 3.0
    };

    let total = spectrum_db + 10.0 * (envelope_moments + modulation + events);
    Ok(TextureReport {
        spectrum_db,
        envelope_moments,
        modulation,
        events,
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    fn buffer(samples: Vec<f32>) -> AudioBuffer {
        AudioBuffer::from_channels(vec![samples], SR).expect("well formed")
    }

    fn sine(freq: f64, amp: f64, secs: f64) -> Vec<f32> {
        let n = (secs * f64::from(SR)).round() as usize;
        (0..n)
            .map(|i| {
                (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(SR)).sin()) as f32
            })
            .collect()
    }

    #[test]
    fn identical_buffers_have_zero_distances() {
        let a = buffer(sine(440.0, 0.4, 3.0));
        let b = buffer(sine(440.0, 0.4, 3.0));
        assert!(log_spectral_distance(&a, &b).expect("rates") < 1e-6);
        let multi = multi_resolution_stft(&a, &b).expect("rates");
        assert!(multi.total < 1e-6, "total {multi:?}");
        assert!(multi
            .resolutions
            .iter()
            .all(|r| r.spectral_convergence < 1e-8));
        let texture = texture_distance(&a, &b).expect("rates");
        assert!(texture.total < 1e-9, "texture {texture:?}");
    }

    #[test]
    fn a_20_db_level_shift_reads_as_20_db_lsd() {
        // Broadband noise at two levels: every frequency bin drops by
        // exactly 20 dB, so the RMS-over-bins LSD is exactly 20 (a pure
        // tone concentrates its energy in one bin and would read
        // 20/sqrt(bins)).
        let n = (3.0 * f64::from(SR)).round() as usize;
        let mut rng = qinari_dsp::rng::Qrng::new(1234);
        let noise: Vec<f32> = (0..n).map(|_| (rng.next_f32() * 2.0 - 1.0) * 0.4).collect();
        let quiet: Vec<f32> = noise.iter().map(|s| s * 0.1).collect();
        let a = buffer(noise);
        let b = buffer(quiet); // −20 dB
        let lsd = log_spectral_distance(&a, &b).expect("rates");
        eprintln!("LSD of a 20 dB level shift: {lsd}");
        assert!((lsd - 20.0).abs() < 1.5, "LSD {lsd} (expected ~20)");
        // The multi-resolution loss sees it in the log term: ln(10) = 2.303.
        let multi = multi_resolution_stft(&a, &b).expect("rates");
        for r in &multi.resolutions {
            assert!((r.log_magnitude - 10.0f64.ln()).abs() < 0.02, "mag {r:?}");
            // A −20 dB shift must dominate the spectral-convergence term
            // (its exact value depends on the normalization; it stays in
            // 0.7..1 for every resolution).
            assert!(r.spectral_convergence > 0.7 && r.spectral_convergence < 1.0);
        }
    }

    #[test]
    fn frequency_shift_moves_the_texture_spectrum_component() {
        let a = buffer(sine(300.0, 0.4, 3.0));
        let d = buffer(sine(3000.0, 0.4, 3.0));
        let texture = texture_distance(&a, &d).expect("rates");
        eprintln!("texture sine300 vs sine3000: {texture:?}");
        // The component is a mean over the 24 bands, so a single-band
        // relocation reads as ~2·(full-scale dB)/24 — small in absolute
        // terms but far above the identical-buffer zero.
        assert!(texture.spectrum_db > 5.0, "different bands: {texture:?}");
        assert!(texture.total > 10.0);
        let multi = multi_resolution_stft(&a, &d).expect("rates");
        assert!(multi.total > 1.0, "multi-res must see it: {multi:?}");
        let lsd = log_spectral_distance(&a, &d).expect("rates");
        // Two single-bin tones: both tone bins carry ~90 dB over the
        // floor, and the RMS over the analysis bins lands near 18 dB.
        assert!(lsd > 10.0, "LSD {lsd}");
    }

    #[test]
    fn mismatched_rates_are_rejected() {
        let a = buffer(sine(440.0, 0.4, 1.0));
        let b =
            AudioBuffer::from_channels(vec![sine(440.0, 0.4, 1.0)], 48_000).expect("well formed");
        for result in [
            log_spectral_distance(&a, &b).map(|_| ()),
            multi_resolution_stft(&a, &b).map(|_| ()),
            texture_distance(&a, &b).map(|_| ()),
        ] {
            assert!(
                matches!(result, Err(VerifyError::MismatchedSampleRate { .. })),
                "must reject rate mismatch"
            );
        }
    }

    #[test]
    fn amplitude_modulated_tone_moved_the_envelope_component() {
        // 300 Hz tone with a 1 Hz 40% AM vs a plain tone.
        let n = (3.0 * f64::from(SR)) as usize;
        let mut plain = vec![0.0f32; n];
        let mut am = vec![0.0f32; n];
        for i in 0..n {
            let t = i as f64 / f64::from(SR);
            let carrier = (2.0 * std::f64::consts::PI * 300.0 * t).sin();
            plain[i] = (0.4 * carrier) as f32;
            let gain = 0.7 + 0.3 * (2.0 * std::f64::consts::PI * t).sin();
            am[i] = (0.4 * gain * carrier) as f32;
        }
        let a = buffer(plain);
        let b = buffer(am);
        let texture = texture_distance(&a, &b).expect("rates");
        eprintln!("texture plain vs AM: {texture:?}");
        assert!(
            texture.envelope_moments > 0.02,
            "envelope must move: {texture:?}"
        );
        // The modulation component averages over all bands, so a
        // low-band-only AM reads small; it must merely be clearly above
        // the identical-buffer zero.
        assert!(
            texture.modulation > 0.005,
            "modulation must move: {texture:?}"
        );
    }
}
