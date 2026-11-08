//! Shared STFT/FFT plumbing for the feature extractors.
//!
//! Everything here is deterministic and allocation-light: windows and FFT
//! plans are created per call (planning is a few microseconds, far below
//! the analysis budget) so no state is shared between calls.

use realfft::num_complex::Complex64;
use realfft::RealFftPlanner;

/// Size of the main analysis STFT (Welch segments, band envelopes).
pub(crate) const ANALYSIS_FFT: usize = 1024;
/// Hop of the main analysis STFT (envelope time resolution).
pub(crate) const ANALYSIS_HOP: usize = 256;

/// Returns the periodic Hann window of `size` points.
///
/// Periodic form (`w[k] = 0.5 - 0.5*cos(2*pi*k/size)`), the convention of
/// `scipy.signal.get_window` and librosa; the DNSMOS mel pipeline uses the
/// same window.
pub(crate) fn hann_window(size: usize) -> Vec<f64> {
    (0..size)
        .map(|k| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * k as f64 / size as f64).cos())
        .collect()
}

/// One-sided power spectra `|X[k]|^2` of consecutive windowed frames.
///
/// Frames start every `hop` samples; a buffer shorter than `window` is
/// zero-padded to a single frame, and an empty buffer yields a single
/// all-zero frame. Each returned frame holds `window / 2 + 1` values.
pub(crate) fn power_frames(samples: &[f32], window: usize, hop: usize) -> Vec<Vec<f64>> {
    let frame_count = frame_count(samples.len(), window, hop);
    let mut frames = Vec::with_capacity(frame_count);
    for index in 0..frame_count {
        let start = index * hop;
        let mut input = vec![0.0f64; window];
        for (k, slot) in input.iter_mut().enumerate() {
            let sample = samples.get(start + k).copied().unwrap_or(0.0);
            *slot = f64::from(sample);
        }
        window_in_place(&mut input);
        frames.push(power_spectrum(&input));
    }
    frames
}

/// Number of frames of a windowed analysis of `len` samples.
pub(crate) fn frame_count(len: usize, window: usize, hop: usize) -> usize {
    if len >= window {
        1 + (len - window) / hop
    } else {
        1
    }
}

/// Applies the periodic Hann window of `input.len()` points in place.
pub(crate) fn window_in_place(input: &mut [f64]) {
    let size = input.len();
    for (k, slot) in input.iter_mut().enumerate() {
        let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * k as f64 / size as f64).cos();
        *slot *= w;
    }
}

/// One-sided power spectrum `|X[k]|^2` of one real block (no windowing).
///
/// Returns `block.len() / 2 + 1` values. The block is transformed as-is;
/// the caller owns all copies.
pub(crate) fn power_spectrum(block: &[f64]) -> Vec<f64> {
    real_fft(block)
        .iter()
        .map(|bin| bin.re * bin.re + bin.im * bin.im)
        .collect()
}

/// Forward real FFT of one block, returning the complex spectrum.
///
/// Returns `block.len() / 2 + 1` complex values.
pub(crate) fn real_fft(block: &[f64]) -> Vec<Complex64> {
    let window = block.len();
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(window);
    let mut scratch = vec![Complex64::new(0.0, 0.0); forward.get_scratch_len()];
    let mut spectrum = vec![Complex64::new(0.0, 0.0); forward.complex_len()];
    let mut input = block.to_vec();
    // Invariant: buffer lengths above follow the plan, so this cannot fail.
    forward
        .process_with_scratch(&mut input, &mut spectrum, &mut scratch)
        .expect("fft buffers are sized by the plan");
    spectrum
}

/// Inverse real FFT: spectrum in, real signal out.
///
/// `spectrum.len()` must be `signal_len / 2 + 1`; no scaling is applied
/// (this mirrors the forward transform, so a round trip is `N * x`).
pub(crate) fn inverse_real_fft(spectrum: &[Complex64], signal_len: usize) -> Vec<f64> {
    let mut planner = RealFftPlanner::<f64>::new();
    let inverse = planner.plan_fft_inverse(signal_len);
    let mut scratch = vec![Complex64::new(0.0, 0.0); inverse.get_scratch_len()];
    let mut time = vec![0.0f64; signal_len];
    let mut input = spectrum.to_vec();
    // Invariant: buffer lengths above follow the plan, so this cannot fail.
    inverse
        .process_with_scratch(&mut input, &mut time, &mut scratch)
        .expect("fft buffers are sized by the plan");
    time
}

/// Weight of one-sided bin `k` of a real spectrum with `bins` bins.
///
/// Interior bins appear twice in the two-sided spectrum, DC and Nyquist
/// once, so the weighted one-sided sum equals the two-sided sum.
pub(crate) fn bin_weight(k: usize, bins: usize) -> f64 {
    if k == 0 || k + 1 == bins {
        1.0
    } else {
        2.0
    }
}

/// Analytic signal `b + i·Hilbert(b)` of a real band given its one-sided
/// spectrum: the two-sided spectrum keeps DC and Nyquist, doubles the
/// interior positive bins, and zeroes the negative mirror; the rustfft
/// inverse is unnormalized, so the result is scaled by `1/frames`.
pub(crate) fn analytic_signal(one_sided: &[Complex64], frames: usize) -> Vec<Complex64> {
    let mut planner = rustfft::FftPlanner::new();
    let inverse = planner.plan_fft_inverse(frames);
    let mut buffer = vec![Complex64::new(0.0, 0.0); frames];
    for (j, slot) in buffer.iter_mut().enumerate() {
        if j <= frames / 2 {
            let unpaired = j == 0 || 2 * j == frames;
            *slot = one_sided[j] * if unpaired { 1.0 } else { 2.0 };
        }
    }
    inverse.process(&mut buffer);
    let scale = 1.0 / frames as f64;
    for bin in &mut buffer {
        *bin *= scale;
    }
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_hann_matches_reference_points() {
        let w = hann_window(8);
        assert!((w[0]).abs() < 1e-12);
        assert!((w[4] - 1.0).abs() < 1e-12, "w[4] = 0.5-0.5cos(pi) = 1");
        assert!((w[2] - 0.5).abs() < 1e-12, "w[2] = 0.5-0.5cos(pi/2) = 0.5");
        // Periodic: w[size/2] is one, symmetric around it.
        assert!((w[1] - w[7]).abs() < 1e-12);
    }

    #[test]
    fn power_spectrum_of_a_unit_sine_reads_quarter_amplitude() {
        // A sine of amplitude a on-bin: |X|^2 = (a*N/2)^2. Here a=1, N=8,
        // so the bin at 1 cycle/8 samples reads (8/2)^2 = 16.
        let n = 8usize;
        let block: Vec<f64> = (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * i as f64 / n as f64).sin())
            .collect();
        let powers = power_spectrum(&block);
        assert_eq!(powers.len(), 5);
        assert!((powers[1] - 16.0).abs() < 1e-9, "bin 1 {powers:?}");
        assert!(powers[0] < 1e-18 && powers[2] < 1e-18 && powers[4] < 1e-18);
    }

    #[test]
    fn frame_count_rules() {
        assert_eq!(frame_count(0, 1024, 256), 1);
        assert_eq!(frame_count(1023, 1024, 256), 1);
        assert_eq!(frame_count(1024, 1024, 256), 1);
        assert_eq!(frame_count(1025, 1024, 256), 1);
        assert_eq!(frame_count(1024 + 256, 1024, 256), 2);
        assert_eq!(frame_count(1024 + 511, 1024, 256), 2);
        assert_eq!(frame_count(1024 + 512, 1024, 256), 3);
    }

    #[test]
    fn power_frames_pads_short_buffers_and_matches_long_ones() {
        let samples: Vec<f32> = (0..2000)
            .map(|i| 0.4 * (2.0 * std::f32::consts::PI * 300.0 * i as f32 / 44_100.0).sin())
            .collect();
        let frames = power_frames(&samples, ANALYSIS_FFT, ANALYSIS_HOP);
        assert_eq!(frames.len(), 1 + (2000 - 1024) / 256);
        assert!(frames.iter().all(|f| f.len() == 513));
        assert!(frames.iter().all(|f| !f.iter().any(|p| p.is_nan())));

        let empty = power_frames(&[], 1024, 256);
        assert_eq!(empty.len(), 1);
        assert!(empty[0].iter().all(|p| p.abs() < f64::MIN_POSITIVE));
    }

    #[test]
    fn inverse_round_trip_rescales_by_n() {
        let n = 16usize;
        let block: Vec<f64> = (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * 3.0 * i as f64 / n as f64).sin() * 0.3)
            .collect();
        let spectrum = real_fft(&block);
        let time = inverse_real_fft(&spectrum, n);
        for (i, (&a, &b)) in block.iter().zip(time.iter()).enumerate() {
            // IFFT(FFT(x)) = N*x (realfft applies no 1/N).
            assert!(
                (b - a * n as f64).abs() < 1e-9,
                "sample {i}: {b} vs {}",
                a * n as f64
            );
        }
    }

    #[test]
    fn bin_weights_cover_dc_nyquist_and_interior() {
        assert_eq!(bin_weight(0, 513), 1.0);
        assert_eq!(bin_weight(512, 513), 1.0);
        assert_eq!(bin_weight(1, 513), 2.0);
        assert_eq!(bin_weight(511, 513), 2.0);
    }

    #[test]
    fn analytic_signal_of_an_on_bin_cosine_is_a_complex_exponential() {
        // cos(2*pi*k*t/N) has a one-bin spectrum; its analytic signal is
        // e^{i*2*pi*k*t/N} with unit magnitude everywhere.
        let n = 16usize;
        let k = 3usize;
        let block: Vec<f64> = (0..n)
            .map(|t| (2.0 * std::f64::consts::PI * k as f64 * t as f64 / n as f64).cos())
            .collect();
        let spectrum = real_fft(&block);
        let analytic = analytic_signal(&spectrum, n);
        for (t, a) in analytic.iter().enumerate() {
            let expected = 2.0 * std::f64::consts::PI * k as f64 * t as f64 / n as f64;
            assert!((a.re - expected.cos()).abs() < 1e-12, "re at {t}");
            assert!((a.im - expected.sin()).abs() < 1e-12, "im at {t}");
        }
        // Odd length: same property, no Nyquist bin exists.
        let n = 15usize;
        let k = 2usize;
        let block: Vec<f64> = (0..n)
            .map(|t| (2.0 * std::f64::consts::PI * k as f64 * t as f64 / n as f64).cos())
            .collect();
        let analytic = analytic_signal(&real_fft(&block), n);
        for (t, a) in analytic.iter().enumerate() {
            let expected = 2.0 * std::f64::consts::PI * k as f64 * t as f64 / n as f64;
            assert!((a.re - expected.cos()).abs() < 1e-12, "odd re at {t}");
            assert!((a.im - expected.sin()).abs() < 1e-12, "odd im at {t}");
        }
    }
}
