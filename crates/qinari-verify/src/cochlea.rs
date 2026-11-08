//! Cochlear front-end of the McDermott–Simoncelli texture-statistic model.
//!
//! Everything here implements the frozen spec of
//! `docs/VERIFICATION-0.2.0.md` §A–§B (task P0-2); `[S1]` is the
//! McDermott–Simoncelli 2011 paper, *Neuron* 71(5):926–940, and
//! `[S1-impl]` is the authors' Sound Texture Synthesis Toolbox v1.7.
//!
//! Pipeline ([S1] p.936–937, Experimental Procedures "Auditory Model"):
//!
//! 1. **Filterbank** — 32 channels: 30 bandpass filters with half-cosine
//!    responses equally spaced on the Glasberg–Moore ERB scale, plus
//!    lowpass/highpass endpoint filters so the summed squared response is
//!    constant (perfect tiling). All filtering is zero-phase and circular
//!    (discrete-frequency domain).
//! 2. **Envelope** — magnitude of the analytic signal of each subband.
//! 3. **Compression** — envelope raised to the power 0.3 (basilar-membrane
//!    compression), at the native rate.
//! 4. **Downsampling** — compressed envelopes band-limited and resampled
//!    to 400 Hz (Nyquist 200 Hz).
//!
//! The analysis runs at the buffer's native sample rate: the 2011 pipeline
//! resampled to 20 kHz first, so at rates above 20 kHz everything above
//! 10 kHz lands in the highpass channel only (frozen spec §A, last row).
//! The buffer is measured as presented — no RMS normalization (§B: the
//! 0.01-RMS step of [S1-impl] `format_orig_sound.m` is a synthesis-stage
//! choice, not part of the measurement definition).

use crate::stft::{inverse_real_fft, real_fft};
use rustfft::num_complex::Complex64;
use rustfft::Fft;

/// Envelope rate in hertz after downsampling (Nyquist 200 Hz);
/// [S1] p.937, txt L1540–1542.
pub(crate) const ENV_RATE: f64 = 400.0;
/// Bandpass filters between the endpoint lowpass/highpass ("a bank of 30
/// such filters", [S1] p.936, txt L1507).
const BANDPASS_COUNT: usize = 30;
/// Total channel count indexed by the statistics: 30 bandpass filters
/// plus 1 lowpass and 1 highpass = 32 ([S1] p.936–937, txt L1524–1530;
/// every statistic formula indexes `k ∈ [1,32]`, txt
/// L1533/L1556/L1603/L1626/L1633).
pub(crate) const CHANNEL_COUNT: usize = BANDPASS_COUNT + 2;
/// First cochlear cutoff (Hz); [S1-impl] parameters `low_audio_f = 20`.
const LOW_AUDIO_HZ: f64 = 20.0;
/// Last cochlear cutoff (Hz); [S1-impl] `hi_audio_f = 10000` — the paper
/// quotes the resulting bandpass-center span "52–8844 Hz" (p.936, txt
/// L1507–1510).
const HI_AUDIO_HZ: f64 = 10_000.0;
/// Basilar-membrane compression exponent ([S1] p.937, txt L1539:
/// "Subband envelopes were raised to a power of 0.3").
const COMPRESS_EXPONENT: f64 = 0.3;
/// Anti-alias passband edge of the envelope downsampling (Hz). The paper
/// only says "following low-pass filtering" (txt L1541); the shape is a
/// frozen implementation choice (§B): gain 1 to 190 Hz, half-cosine
/// rolloff to zero at the 200 Hz envelope Nyquist.
const ENV_PASSBAND_HZ: f64 = 190.0;
/// Envelope Nyquist (half of [`ENV_RATE`]).
const ENV_NYQUIST_HZ: f64 = 200.0;
/// Measurement-window taper length at each signal end (s): "the weighting
/// window fell from one to zero (half cycle of a raised cosine) over the
/// 1 s intervals at the beginning and end of the signal" ([S1] p.937,
/// txt L1575–1577).
const WINDOW_TAPER_S: f64 = 1.0;

/// The auditory front-end output consumed by the texture statistics.
pub(crate) struct Cochleagram {
    /// Compressed 400 Hz envelopes, channel-major: index 0 is the lowpass
    /// endpoint, 1..=30 the ERB bandpass channels, 31 the highpass.
    pub channels: Vec<Vec<f64>>,
    /// Measurement window at the envelope rate, normalized to sum 1
    /// ([S1] p.937, "Boundary Handling").
    pub window: Vec<f64>,
    /// Label frequency of each channel: cutoffs[1] (the innermost bandpass
    /// center) for the lowpass, the bandpass centers, cutoffs[30] for the
    /// highpass. Endpoints repeat their inner neighbor because they tile
    /// up to that frequency (§A).
    pub center_hz: Vec<f64>,
}

/// ERB number of a frequency (Glasberg & Moore 1990 as cited [S1] p.936
/// txt L1508; [S1-impl] `freq2erb.m`): `ERB(f) = 9.265·ln(1 + f/228.8455)`
/// with `228.8455 = 24.7·9.265`.
fn freq2erb(hz: f64) -> f64 {
    9.265 * (1.0 + hz / 228.8455).ln()
}

/// Inverse of [`freq2erb`]; [S1-impl] `erb2freq.m`.
fn erb2freq(erb: f64) -> f64 {
    228.8455 * ((erb / 9.265).exp() - 1.0)
}

/// The 32 cochlear cutoffs, equally spaced on the ERB scale from 20 Hz to
/// min(10000, Nyquist) — the only construction that reproduces every
/// Figure 2 axis label (§A). Bandpass k spans cutoffs k..k+2, so adjacent
/// filters overlap by 50% and their squared responses tile the spectrum.
/// Rates without cochlear resolution (Nyquist at/below the first cutoff)
/// yield an empty bank.
fn erb_cutoffs(sample_rate: u32) -> Vec<f64> {
    let nyquist = f64::from(sample_rate) / 2.0;
    let hi = HI_AUDIO_HZ.min(nyquist);
    if hi <= LOW_AUDIO_HZ {
        return Vec::new();
    }
    let low_erb = freq2erb(LOW_AUDIO_HZ);
    let step = (freq2erb(hi) - low_erb) / (BANDPASS_COUNT + 1) as f64;
    (0..=BANDPASS_COUNT + 1)
        .map(|i| low_erb + i as f64 * step)
        .collect()
}

/// Half-cosine response between ERB positions `e_lo` and `e_hi` ([S1]
/// p.936, txt L1505–1506: "Fourier amplitude shaped as the positive
/// portion of a cosine function"); zero outside the support.
fn cos_response(e_lo: f64, e_hi: f64, e: f64) -> f64 {
    if e > e_lo && e < e_hi {
        let mid = f64::midpoint(e_lo, e_hi);
        (std::f64::consts::PI * (e - mid) / (e_hi - e_lo)).cos()
    } else {
        0.0
    }
}

/// Response of cochlear `channel` at ERB position `e` (§A). Channels 0 and
/// 31 are the endpoint filters `sqrt(1 − H_neighbor²)` that complete the
/// perfect tiling ([S1] p.936–937, txt L1524–1530).
fn channel_response(erb_cutoffs: &[f64], channel: usize, e: f64) -> f64 {
    if channel == 0 {
        // Lowpass: flat 1 below the first cutoff, then the complement of
        // the first bandpass up to its center (cutoffs[1]).
        if e < erb_cutoffs[1] {
            let h = cos_response(erb_cutoffs[0], erb_cutoffs[2], e);
            (1.0 - h * h).sqrt()
        } else {
            0.0
        }
    } else if channel == CHANNEL_COUNT - 1 {
        // Highpass: complement of the last bandpass from its center
        // (cutoffs[30]) to Nyquist, flat 1 above the last cutoff.
        if e > erb_cutoffs[BANDPASS_COUNT] {
            let lo = erb_cutoffs[BANDPASS_COUNT - 1];
            let hi = erb_cutoffs[BANDPASS_COUNT + 1];
            let h = cos_response(lo, hi, e);
            (1.0 - h * h).sqrt()
        } else {
            0.0
        }
    } else {
        // Bandpass channel b spans cutoffs b..b+2, centered at cutoff b+1.
        let b = channel - 1;
        cos_response(erb_cutoffs[b], erb_cutoffs[b + 2], e)
    }
}

/// Anti-alias gain of the envelope downsampling at frequency `hz` (§B):
/// 1 to 190 Hz, half-cosine rolloff 190→200 Hz, 0 above.
fn anti_alias_gain(hz: f64) -> f64 {
    if hz <= ENV_PASSBAND_HZ {
        1.0
    } else if hz < ENV_NYQUIST_HZ {
        let edge = (hz - ENV_PASSBAND_HZ) / (ENV_NYQUIST_HZ - ENV_PASSBAND_HZ);
        (std::f64::consts::FRAC_PI_2 * edge).cos()
    } else {
        0.0
    }
}

/// One raised-cosine taper value: 0 at sample 0, 1 from sample `taper` on
/// (half cycle of a raised cosine, [S1] p.937 txt L1575–1577).
fn taper_rise(i: usize, taper: usize) -> f64 {
    if taper == 0 {
        return 1.0;
    }
    let x = i.min(taper) as f64 / taper as f64;
    0.5 - 0.5 * (std::f64::consts::PI * x).cos()
}

/// The measurement window at the envelope rate, normalized to sum 1:
/// rising 1 s taper, flat middle, falling 1 s taper ([S1] p.937, txt
/// L1572–1587). Signals shorter than 2 s get tapers of half their length
/// (short-signal rule, §B); if even that degenerates (≤2 frames) the
/// window falls back to uniform so the weights stay usable.
fn measurement_window(frames: usize) -> Vec<f64> {
    if frames == 0 {
        return Vec::new();
    }
    let full_taper = (WINDOW_TAPER_S * ENV_RATE) as usize;
    let taper = full_taper.min(frames / 2);
    let mut window: Vec<f64> = (0..frames)
        .map(|i| taper_rise(i, taper) * taper_rise(frames - 1 - i, taper))
        .collect();
    let sum: f64 = window.iter().sum();
    if sum <= 0.0 {
        let uniform = 1.0 / frames as f64;
        window.fill(uniform);
    } else {
        for w in &mut window {
            *w /= sum;
        }
    }
    window
}

/// Compressed envelope of one subband, given its one-sided spectrum. The
/// analytic signal has the two-sided spectrum `2·S` on the interior
/// positive bins (DC and Nyquist stay unpaired), so one unnormalized
/// complex inverse FFT scaled by `1/n` yields `b + i·Hilbert(b)`, whose
/// magnitude — raised to the compression exponent — is the output
/// ([S1] p.937, txt L1536–1540).
fn compressed_envelope(
    subband: &[Complex64],
    inverse: &dyn Fft<f64>,
    scratch: &mut [Complex64],
) -> Vec<f64> {
    let n = scratch.len();
    for (j, slot) in scratch.iter_mut().enumerate() {
        *slot = if j <= n / 2 {
            let unpaired = j == 0 || 2 * j == n;
            subband[j] * if unpaired { 1.0 } else { 2.0 }
        } else {
            Complex64::new(0.0, 0.0)
        };
    }
    inverse.process(scratch);
    let scale = 1.0 / n as f64;
    scratch
        .iter()
        .map(|a| ((a.re * a.re + a.im * a.im).sqrt() * scale).powf(COMPRESS_EXPONENT))
        .collect()
}

/// Band-limited resampling of one compressed envelope from the native
/// rate to 400 Hz (§B): the native-rate spectrum is copied onto the
/// envelope-rate grid under the anti-alias gain. Bin `j` keeps its exact
/// source frequency `j·sr/n`; when `400·n/sr` is not an integer the
/// destination grid rescales time by the sub-sample rounding, which every
/// correlation statistic is invariant to. Resampling ringing can push
/// samples below zero; they are clipped ([S1-impl]
/// `measure_texture_stats.m`).
fn downsample_to_env_rate(envelope: &[f64], sample_rate: u32) -> Vec<f64> {
    let n = envelope.len();
    let frames = ((n as f64) * ENV_RATE / f64::from(sample_rate))
        .round()
        .max(1.0) as usize;
    let spectrum = real_fft(envelope);
    let dst_bins = frames / 2 + 1;
    let scale = frames as f64 / n as f64;
    let bin_hz = f64::from(sample_rate) / n as f64;
    let mut masked: Vec<Complex64> = vec![Complex64::new(0.0, 0.0); dst_bins];
    for (j, slot) in masked.iter_mut().enumerate() {
        if j >= spectrum.len() {
            break;
        }
        let gain = anti_alias_gain(j as f64 * bin_hz) * scale;
        *slot = spectrum[j] * gain;
    }
    let mut out = inverse_real_fft(&masked, frames);
    let norm = 1.0 / frames as f64;
    for sample in &mut out {
        *sample *= norm;
        if *sample < 0.0 {
            *sample = 0.0;
        }
    }
    out
}

/// Label frequencies of the 32 channels from ERB cutoffs (see
/// [`Cochleagram::center_hz`]); an empty bank labels everything 0.
fn channel_labels(erb_cutoffs: &[f64]) -> Vec<f64> {
    if erb_cutoffs.is_empty() {
        return vec![0.0; CHANNEL_COUNT];
    }
    let hz: Vec<f64> = erb_cutoffs.iter().map(|&e| erb2freq(e)).collect();
    let mut labels = vec![0.0; CHANNEL_COUNT];
    labels[0] = hz[1];
    labels[CHANNEL_COUNT - 1] = hz[BANDPASS_COUNT];
    labels[1..=BANDPASS_COUNT].copy_from_slice(&hz[1..=BANDPASS_COUNT]);
    labels
}

/// Runs the cochlear front-end on a mono signal.
///
/// Degenerate inputs (empty or single-sample buffers, rates without any
/// cochlear resolution) produce an all-degenerate gram — empty envelopes
/// and a zero-length window, from which every texture statistic reads 0 —
/// following the crate's total-function convention for measurements.
pub(crate) fn analyze(mono: &[f32], sample_rate: u32) -> Cochleagram {
    let cutoffs = erb_cutoffs(sample_rate);
    let labels = channel_labels(&cutoffs);
    if mono.len() < 2 || cutoffs.is_empty() {
        return Cochleagram {
            channels: vec![Vec::new(); CHANNEL_COUNT],
            window: Vec::new(),
            center_hz: labels,
        };
    }

    let signal: Vec<f64> = mono.iter().map(|&s| f64::from(s)).collect();
    let spectrum = real_fft(&signal);
    let bin_hz = f64::from(sample_rate) / signal.len() as f64;
    let bin_erbs: Vec<f64> = (0..spectrum.len())
        .map(|j| freq2erb(j as f64 * bin_hz))
        .collect();
    let mut planner = rustfft::FftPlanner::new();
    let inverse = planner.plan_fft_inverse(signal.len());
    let mut subband = vec![Complex64::new(0.0, 0.0); spectrum.len()];
    let mut scratch = vec![Complex64::new(0.0, 0.0); signal.len()];
    let mut channels = Vec::with_capacity(CHANNEL_COUNT);
    for channel in 0..CHANNEL_COUNT {
        for (j, slot) in subband.iter_mut().enumerate() {
            *slot = spectrum[j] * channel_response(&cutoffs, channel, bin_erbs[j]);
        }
        let envelope = compressed_envelope(&subband, inverse.as_ref(), &mut scratch);
        channels.push(downsample_to_env_rate(&envelope, sample_rate));
    }
    let frames = channels.first().map_or(0, Vec::len);
    Cochleagram {
        channels,
        window: measurement_window(frames),
        center_hz: labels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    #[test]
    fn cutoffs_and_centers_match_the_frozen_spec() {
        let cutoffs = erb_cutoffs(SR);
        assert_eq!(cutoffs.len(), CHANNEL_COUNT);
        let hz: Vec<f64> = cutoffs.iter().map(|&e| erb2freq(e)).collect();
        // 20 Hz and 10000 Hz endpoints, ERB-evenly spaced (§A).
        assert!((hz[0] - 20.0).abs() < 1e-9);
        assert!((hz[31] - 10_000.0).abs() < 1e-6);
        let steps: Vec<f64> = cutoffs.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(steps.iter().all(|s| (s - steps[0]).abs() < 1e-12));
        // Bandpass centers reproduce the Figure 2 axis labels (§A).
        let labels = channel_labels(&cutoffs);
        for (k, expected) in [
            (0, 51.7),
            (9, 596.3),
            (18, 2198.3),
            (19, 2507.4),
            (29, 8844.5),
        ] {
            assert!(
                (labels[k + 1] - expected).abs() < 0.05,
                "bandpass center {}: {} vs {expected}",
                k + 1,
                labels[k + 1]
            );
        }
        // Endpoint labels repeat the innermost bandpass centers.
        assert!((labels[0] - labels[1]).abs() < 1e-12);
        assert!((labels[31] - labels[30]).abs() < 1e-12);
    }

    #[test]
    fn filterbank_tiles_the_spectrum_perfectly() {
        // Summed squared response = 1 across the spectrum (§A; [S1]
        // p.936–937 txt L1524–1530). Sampled densely on the ERB scale.
        let cutoffs = erb_cutoffs(SR);
        let samples = 4_000;
        for s in 0..=samples {
            let e = freq2erb(1.0)
                + (freq2erb(22_050.0) - freq2erb(1.0)) * f64::from(s) / f64::from(samples);
            let mut energy = 0.0;
            for channel in 0..CHANNEL_COUNT {
                let h = channel_response(&cutoffs, channel, e);
                energy += h * h;
            }
            assert!(
                (energy - 1.0).abs() < 1e-9,
                "tiling broken at {e} ERB: {energy}"
            );
        }
    }

    #[test]
    fn bandpass_response_is_half_cosine_with_erb_3db_bandwidth() {
        let cutoffs = erb_cutoffs(SR);
        // Filter 10 (channel 10, bandpass b=9): support cutoffs[9]..cutoffs[11],
        // center cutoffs[10], ERB half-support = one cutoff spacing.
        let (lo, center, hi) = (cutoffs[9], cutoffs[10], cutoffs[11]);
        assert!((channel_response(&cutoffs, 10, center) - 1.0).abs() < 1e-12);
        assert!(channel_response(&cutoffs, 10, lo - 1e-6).abs() < 1e-9);
        assert!(channel_response(&cutoffs, 10, hi + 1e-6).abs() < 1e-9);
        // 3 dB (half power) one half-spacing from the center (§A).
        let e_3db = center - 0.5 * (hi - lo) / 2.0;
        let h = channel_response(&cutoffs, 10, e_3db);
        assert!((h * h - 0.5).abs() < 1e-9, "3 dB point: {}", h * h);
        // The 3 dB bandwidth is one center spacing, ≈1.11 ERB — the paper
        // describes the outcome as "≈5% larger than ERBs" loosely (§A).
        let spacing = cutoffs[10] - cutoffs[9];
        assert!((spacing - 1.1106).abs() < 1e-3, "spacing {spacing}");
    }

    #[test]
    fn low_and_high_pass_are_flat_at_the_extremes() {
        let cutoffs = erb_cutoffs(SR);
        // Below 20 Hz only the lowpass responds, with gain 1.
        assert!((channel_response(&cutoffs, 0, freq2erb(1.0)) - 1.0).abs() < 1e-12);
        for channel in 1..CHANNEL_COUNT {
            assert!(channel_response(&cutoffs, channel, freq2erb(1.0)) < 1e-12);
        }
        // Above 10 kHz only the highpass responds, with gain 1.
        assert!(
            (channel_response(&cutoffs, 31, freq2erb(15_000.0)) - 1.0).abs() < 1e-12,
            "highpass flat"
        );
        for channel in 0..CHANNEL_COUNT - 1 {
            assert!(channel_response(&cutoffs, channel, freq2erb(15_000.0)) < 1e-12);
        }
    }

    #[test]
    fn anti_alias_gain_shape() {
        assert!((anti_alias_gain(0.0) - 1.0).abs() < 1e-12);
        assert!((anti_alias_gain(190.0) - 1.0).abs() < 1e-12);
        assert!((anti_alias_gain(195.0) - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
        assert!(anti_alias_gain(200.0).abs() < 1e-12);
        assert!(anti_alias_gain(250.0).abs() < 1e-12);
    }

    #[test]
    fn window_shape_and_short_signal_rule() {
        // 7 s at 400 Hz: 1 s raised-cosine tapers, flat middle; the
        // paper's constraint Σw = 1 (txt L1592) normalizes the whole
        // window, so shape assertions compare flat-region samples to
        // each other, not to 1.
        let w = measurement_window(2_800);
        assert_eq!(w.len(), 2_800);
        assert!(w[0] < 1e-9);
        // The rise reaches 1.0 exactly at sample 400 and the fall starts
        // only at 2399, so the whole 400..=2399 range is one flat value.
        assert!((w[400] - w[1_400]).abs() < 1e-12);
        assert!((w[2_399] - w[1_400]).abs() < 1e-12);
        assert!(w[1_400] > 0.0);
        assert!(w[2_799] < 1e-9);
        assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        // Short signal: tapers of half the length (§B short-signal rule).
        let short = measurement_window(500);
        assert!((short[250] - short.iter().copied().fold(0.0, f64::max)).abs() < 1e-12);
        assert!(short[0] < 1e-9 && short[499] < 1e-9);
        // Degenerate tiny signals stay normalized and usable.
        for frames in [1, 2, 3] {
            let w = measurement_window(frames);
            assert_eq!(w.len(), frames);
            assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-9);
            assert!(w.iter().all(|v| v.is_finite() && *v >= 0.0));
        }
    }

    /// AM tone at `carrier` with modulation `depth·sin(2π·f_m·t)`,
    /// pre-compensated for the 0.3 compression so the measured
    /// compressed envelope is exactly `1 + depth·sin` ([S1] p.937, txt
    /// L1536–1540).
    fn am_tone(carrier: f64, f_m: f64, depth: f64, seconds: f64) -> Vec<f32> {
        let n = (seconds * f64::from(SR)).round() as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(SR);
                let u = depth * (2.0 * std::f64::consts::PI * f_m * t).sin();
                let m = (1.0 + u).powf(1.0 / COMPRESS_EXPONENT);
                (m * (2.0 * std::f64::consts::PI * carrier * t).cos()) as f32
            })
            .collect()
    }

    #[test]
    fn envelope_extraction_recovers_a_known_modulation() {
        // AM tone at the center of bandpass channel 11 (labels[11] ≈ 71 Hz
        // region): the compressed envelope must come back as the
        // pre-compensated modulation 1 + 0.2·sin to high accuracy.
        let labels = channel_labels(&erb_cutoffs(SR));
        let carrier = labels[11];
        let mono = am_tone(carrier, 8.0, 0.2, 4.0);
        let gram = analyze(&mono, SR);
        assert_eq!(gram.channels.len(), CHANNEL_COUNT);
        let frames = gram.window.len();
        assert_eq!(frames, 1_600, "4 s at 400 Hz");
        let envelope = &gram.channels[11];
        // Compare against the analytic envelope away from the tapers.
        let mut max_err = 0.0_f64;
        for (s, &v) in envelope.iter().enumerate().skip(600).take(400) {
            let t = s as f64 / ENV_RATE;
            let expected = 1.0 + 0.2 * (2.0 * std::f64::consts::PI * 8.0 * t).sin();
            max_err = max_err.max((v - expected).abs());
        }
        assert!(max_err < 5e-3, "envelope recovery error {max_err}");
        // A neighboring channel (offset 3, structurally disjoint
        // support) must hear far less than the tone's own channel. It
        // cannot hear exact silence: the carrier is not an exact bin of
        // the native grid (channel centers are irrational in Hz), its
        // finite-window spectrum leaks, and the 0.3 compression expands
        // sub-ε leakage by ε^(−0.7) — so the honest bound is two orders
        // of relative margin, not zero.
        let tone = gram.channels[11].iter().map(|v| v * v).sum::<f64>() / frames as f64;
        let leakage = gram.channels[14].iter().map(|v| v * v).sum::<f64>() / frames as f64;
        assert!(
            leakage < 0.01 * tone,
            "offset-3 leakage {leakage} vs tone {tone}"
        );
    }

    #[test]
    fn degenerate_inputs_produce_degenerate_grams() {
        for mono in [Vec::new(), vec![0.5f32]] {
            let gram = analyze(&mono, SR);
            assert_eq!(gram.channels.len(), CHANNEL_COUNT);
            assert!(gram.channels.iter().all(Vec::is_empty));
            assert_eq!(gram.window.len(), 0);
            assert_eq!(gram.center_hz.len(), CHANNEL_COUNT);
        }
        // Silence is not degenerate: constant zero envelopes.
        let silence = vec![0.0f32; SR as usize];
        let gram = analyze(&silence, SR);
        assert_eq!(gram.window.len(), 400);
        assert!(gram
            .channels
            .iter()
            .all(|c| c.len() == 400 && c.iter().all(|v| v.abs() < 1e-12)));
    }

    #[test]
    fn odd_length_signals_analyze_without_panicking() {
        // 1.5 s minus one sample: odd native and envelope lengths exercise
        // the odd-size FFT paths of realfft/rustfft.
        let n = (1.5 * f64::from(SR)).round() as usize - 1;
        let mono: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * 440.0 * i as f64 / f64::from(SR)).sin() as f32)
            .collect();
        let gram = analyze(&mono, SR);
        assert_eq!(gram.channels.len(), CHANNEL_COUNT);
        assert!(gram.channels.iter().all(|c| c.len() == gram.window.len()));
        assert!(gram
            .channels
            .iter()
            .all(|c| c.iter().all(|v| v.is_finite())));
    }
}
