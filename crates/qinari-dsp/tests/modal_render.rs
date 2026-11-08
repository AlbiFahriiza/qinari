//! Render-level verification of the exact modal bank (plan 0.2.0 P0-1):
//! measured per-mode T60 against the declared values, material-vs-seed
//! spectral separation, and per-configuration determinism.
//!
//! T60 is measured, not trusted: each mode is rendered through the same
//! strike path (same seed, same excitation, same gain and position
//! weight) as a single-mode bank, its envelope is tracked in dB by
//! complex heterodyne demodulation, and a linear regression over the fit
//! window (excitation end .. + `t60_mode`) yields the decay slope.
//! Per-mode isolation is deliberate: in the full bank the highest modes
//! sit closer together than a heterodyne window short enough to follow
//! their decay can separate, so a mixed-render measurement would read
//! inter-mode leakage, not the mode's own slope; the mixed render itself
//! is covered by the material-separation and determinism tests below.
//! The relative error `|T60_measured − t60_declared| / t60_declared`
//! must be ≤ 10% for ≥ 95% of a material's modes (plan §8); the
//! envelope fit error is a separate quantity, reported in dB — T60 is
//! seconds, fit error is dB, the units are never mixed.

use qinari_dsp::modal::{Material, ModalBank, Mode};
use qinari_dsp::rng::{Qrng, Seed};

const SR_44: f32 = 44_100.0;
const SR_48: f32 = 48_000.0;
/// Excitation length the recipes use (4 ms of white-noise force).
const EXCITATION_MILLIS: f32 = 4.0;
/// Envelope points more than this below the first fit point sit on the
/// harness noise floor (numerical tail of the recursion) and are dropped
/// from the regression; exponential decay makes any surviving span give
/// the true slope.
const NOISE_FLOOR_DB: f64 = -45.0;
/// Relative T60 tolerance from the plan (§8, corrected units).
const T60_TOLERANCE: f64 = 0.10;
/// Fraction of a material's modes that must pass the T60 criterion.
const PASS_FRACTION: f64 = 0.95;
/// Strikes per render in the material-vs-seed comparison.
const STRIKES_PER_RENDER: usize = 4;

/// Envelope in dB of the component at `hz`, one point per block of
/// samples fully inside `[from, to]`: complex demodulation at the mode
/// frequency brings the mode to DC and the boxcar rejects anything away
/// from `hz`. The block is an integer number of carrier cycles — as many
/// as fit eight times into the span, capped at eight — because the
/// demodulated boxcar of an exponentially decaying sinusoid scales every
/// block by the same constant (`Σ R^k = R^s·(1−R^W)/(1−R)` factorizes),
/// so the fitted slope carries no window-length bias however short the
/// blocks must be: a 90 Hz thud mode whose whole t60 spans only ~9
/// carrier cycles trades boxcar smoothness for regression points, while
/// high, long-ringing modes keep the smooth eight-cycle window.
fn heterodyne_envelope_db(
    samples: &[f32],
    hz: f32,
    sr: f32,
    from: usize,
    to: usize,
) -> Vec<(f64, f64)> {
    let cycle = (f64::from(sr) / f64::from(hz)).round().max(1.0);
    let span = to.saturating_sub(from) as f64;
    let cycles = (span / (8.0 * cycle)).floor().clamp(1.0, 8.0);
    let window = (cycles * cycle).round().max(4.0) as usize;
    let omega = 2.0 * std::f64::consts::PI * f64::from(hz) / f64::from(sr);
    let mut envelope = Vec::new();
    if window == 0 || window > samples.len() {
        return envelope;
    }
    // Blocks aligned to the render start; only blocks that lie entirely
    // inside [from, to] count — no block may straddle the excitation or
    // spill past the declared t60.
    let first = from.div_ceil(window);
    let last = (to / window).saturating_sub(1);
    for block in first..=last {
        let start = block * window;
        let mut re_sum = 0.0_f64;
        let mut im_sum = 0.0_f64;
        for (k, &sample) in samples[start..start + window].iter().enumerate() {
            let angle = omega * (start + k) as f64;
            re_sum += f64::from(sample) * angle.cos();
            im_sum -= f64::from(sample) * angle.sin();
        }
        let re = re_sum / window as f64;
        let im = im_sum / window as f64;
        let level = 20.0 * (re * re + im * im).sqrt().log10();
        let t = (start + window / 2) as f64 / f64::from(sr);
        envelope.push((t, level));
    }
    envelope
}

/// Least-squares slope of the envelope in dB, skipping points below
/// `floor_db`. Returns (`t60_seconds`, `fit_error_db`, `points_used`).
fn fit_t60(envelope: &[(f64, f64)], floor_db: f64) -> Option<(f64, f64, usize)> {
    let points: Vec<(f64, f64)> = envelope
        .iter()
        .copied()
        .filter(|&(_, db)| db > floor_db)
        .collect();
    if points.len() < 4 {
        return None;
    }
    let n = points.len() as f64;
    let sum_x: f64 = points.iter().map(|p| p.0).sum();
    let sum_y: f64 = points.iter().map(|p| p.1).sum();
    let sum_xx: f64 = points.iter().map(|p| p.0 * p.0).sum();
    let sum_prod: f64 = points.iter().map(|p| p.0 * p.1).sum();
    let slope = (n * sum_prod - sum_x * sum_y) / (n * sum_xx - sum_x * sum_x);
    // A non-decaying (or NaN) slope cannot yield a T60; NaN compares
    // unordered, so it lands here too.
    if slope.partial_cmp(&0.0) != Some(std::cmp::Ordering::Less) {
        return None;
    }
    let intercept = (sum_y - slope * sum_x) / n;
    let residual: f64 = (points
        .iter()
        .map(|&(t, db)| {
            let diff = db - (intercept + slope * t);
            diff * diff
        })
        .sum::<f64>()
        / n)
        .sqrt();
    Some((-60.0 / slope, residual, points.len()))
}

/// Measures one mode's rendered T60: the mode alone through the strike
/// path, envelope regression over its fit window, explicit noise floor.
fn measure_mode(mode: &Mode, excitation: &[f32], sr: f32) -> (f64, f64) {
    let bank = ModalBank::new(vec![*mode]).expect("preset modes are valid");
    let samples = bank
        .strike_at(sr, excitation, mode.t60 + 0.05, 0.5)
        .expect("valid arguments");
    // Fit window: the mode's free decay, from excitation end to one
    // declared t60 later (plan §8: the window is 0..t60_mode counted
    // from the start of the free decay).
    let from = excitation.len();
    let to = from + (f64::from(mode.t60) * f64::from(sr)) as usize;
    let envelope = heterodyne_envelope_db(&samples, mode.hz, sr, from, to.min(samples.len()));
    let floor = envelope
        .first()
        .map_or(f64::NEG_INFINITY, |&(_, db)| db + NOISE_FLOOR_DB);
    let (t60, fit_error_db, used) = fit_t60(&envelope, floor).unwrap_or_else(|| {
        panic!(
            "mode {} Hz: fewer than 4 envelope points above the noise floor",
            mode.hz as i32
        )
    });
    assert!(
        used >= 4,
        "mode {} Hz: only {used} points above the floor",
        mode.hz as i32
    );
    (t60, fit_error_db)
}

/// Runs the per-mode T60 harness for one material/seed and returns
/// (passed, measured, worst relative error, mean fit error dB). The rng
/// stream matches a recipe render exactly: bank draw first, then the
/// strike excitation.
fn measure_material(material: Material, seed: u64, sr: f32) -> (usize, usize, f64, f64) {
    let mut rng = Qrng::new(seed);
    let bank = ModalBank::material(material, &mut rng);
    let excitation: Vec<f32> = (0..(EXCITATION_MILLIS / 1000.0 * sr) as usize)
        .map(|_| rng.next_range(-1.0, 1.0))
        .collect();

    let mut passed = 0;
    let mut measured = 0;
    let mut worst = 0.0_f64;
    let mut fit_sum = 0.0_f64;
    for mode in bank.modes() {
        if mode.hz >= sr * 0.5 {
            continue; // not representable at this rate; excluded from the count
        }
        let (t60_measured, fit_error_db) = measure_mode(mode, &excitation, sr);
        let relative_error = (t60_measured - f64::from(mode.t60)).abs() / f64::from(mode.t60);
        println!(
            "{material:?} mode {:>6} Hz: t60 declared {:.4} s, measured {:.4} s (err {:+.1}%), fit error {:.2} dB",
            mode.hz as i32,
            mode.t60,
            t60_measured,
            100.0 * relative_error,
            fit_error_db
        );
        fit_sum += fit_error_db;
        worst = worst.max(relative_error);
        measured += 1;
        if relative_error <= T60_TOLERANCE {
            passed += 1;
        }
    }
    (passed, measured, worst, fit_sum / measured as f64)
}

#[test]
fn measured_t60_matches_declaration_within_ten_percent() {
    for material in Material::ALL {
        for seed in [11_u64, 29] {
            let (passed, measured, worst, mean_fit) = measure_material(material, seed, SR_44);
            println!(
                "{material:?} seed {seed}: {passed}/{measured} modes within 10% (worst {:+.1}%), mean envelope fit error {:.2} dB",
                100.0 * worst, mean_fit
            );
            assert!(
                measured > 0,
                "{material:?}: harness measured no modes at all"
            );
            assert!(
                passed as f64 >= PASS_FRACTION * measured as f64,
                "{material:?} seed {seed}: only {passed}/{measured} modes match their declared t60 within 10%"
            );
            // The envelope fit error is reported in dB, separately from
            // the seconds-based T60 error (plan §8 unit correction).
            assert!(
                mean_fit < 1.0,
                "{material:?} seed {seed}: envelope fit error {mean_fit} dB is too rough for the regression to be trusted"
            );
        }
    }
}

/// In-place radix-2 FFT (test-local; no new dependencies).
fn fft_fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    assert!(n.is_power_of_two());
    let mut j = 0;
    for i in 0..n {
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
        let mut m = n >> 1;
        while m >= 1 && j & m != 0 {
            j ^= m;
            m >>= 1;
        }
        j |= m;
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * std::f64::consts::PI / len as f64;
        let (wr, wi) = (angle.cos(), angle.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let even = (re[i + k], im[i + k]);
                let odd = (
                    re[i + k + len / 2] * cr - im[i + k + len / 2] * ci,
                    re[i + k + len / 2] * ci + im[i + k + len / 2] * cr,
                );
                re[i + k] = even.0 + odd.0;
                im[i + k] = even.1 + odd.1;
                re[i + k + len / 2] = even.0 - odd.0;
                im[i + k + len / 2] = even.1 - odd.1;
                let next_cr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = next_cr;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// Spectral power per one-third-octave band from 40 Hz to 20 kHz.
/// Band aggregation (not per-bin comparison) is what makes the distance
/// measure material identity: per-bin log distances are dominated by
/// narrow spectral peaks moving with the seeded per-strike jitter, while
/// a material's register, mode-family spacing and decay live in the
/// band envelope.
fn band_powers(samples: &[f32], sr: f32) -> Vec<f64> {
    let n = samples.len().next_power_of_two();
    let mut re = vec![0.0_f64; n];
    let mut im = vec![0.0_f64; n];
    for (i, &sample) in samples.iter().enumerate() {
        let hann = (std::f64::consts::PI * i as f64 / samples.len() as f64).sin();
        re[i] = f64::from(sample) * hann * hann;
    }
    fft_fft(&mut re, &mut im);
    let bin_hz = f64::from(sr) / n as f64;
    let mut edges = Vec::new();
    let mut edge = 40.0_f64;
    while edge < 20_000.0 {
        edges.push(edge);
        edge *= 2.0_f64.powf(1.0 / 3.0);
    }
    edges.push(20_000.0);
    (0..edges.len() - 1)
        .map(|band| {
            let lo = (edges[band] / bin_hz).ceil() as usize;
            let hi = ((edges[band + 1] / bin_hz).floor() as usize).clamp(lo + 1, n / 2);
            (lo..hi).map(|k| re[k] * re[k] + im[k] * im[k]).sum()
        })
        .collect()
}

/// Band powers as dB levels; the epsilon floor keeps empty bands finite.
fn powers_to_db(powers: &[f64]) -> Vec<f64> {
    powers.iter().map(|p| 10.0 * (p + 1e-15).log10()).collect()
}

/// Mean absolute log-energy distance between two band-level spectra,
/// in dB per band.
fn band_distance(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f64>() / a.len() as f64
}

/// One render for the identity comparison: the object a seed draws,
/// struck `STRIKES_PER_RENDER` times with excitations drawn from the
/// same stream (the recipe rng order: bank first, then strikes), band
/// powers averaged over the strikes in the power domain. Averaging a
/// series is what a listener of a soundscape integrates, and it is the
/// paper's protocol (one audio file "with a series of hits" per object);
/// the per-strike projection randomness — the same per-hit variation
/// Pauletto & Barahona model — averages out, leaving object identity.
fn series_band_levels(material: Material, seed: u64, sr: f32) -> Vec<f64> {
    let mut rng = Qrng::new(seed);
    let bank = ModalBank::material(material, &mut rng);
    let mut accumulated: Vec<f64> = Vec::new();
    for _ in 0..STRIKES_PER_RENDER {
        let excitation: Vec<f32> = (0..(EXCITATION_MILLIS / 1000.0 * sr) as usize)
            .map(|_| rng.next_range(-1.0, 1.0))
            .collect();
        let samples = bank
            .strike_at(sr, &excitation, 1.6, 0.5)
            .expect("preset banks and test arguments are valid");
        let peak = samples.iter().fold(0.0_f32, |top, s| top.max(s.abs()));
        assert!(
            peak > 1e-3,
            "{material:?} seed {seed} strike was near-silent"
        );
        let powers = band_powers(&samples, sr);
        if accumulated.is_empty() {
            accumulated = powers;
        } else {
            for (slot, power) in accumulated.iter_mut().zip(powers) {
                *slot += power;
            }
        }
    }
    powers_to_db(
        &accumulated
            .iter()
            .map(|power| power / STRIKES_PER_RENDER as f64)
            .collect::<Vec<_>>(),
    )
}

#[test]
fn materials_separate_far_more_than_seeds_do() {
    // Plan P0-1: 4 materials × 6 seeds; the mean third-octave spectral
    // distance between materials must exceed 3× the distance between
    // seeds of one material. On the 0.1.5 bank the modal layer died
    // within 50 ms and the materials were near-identical (finding F6).
    const SEEDS: [u64; 6] = [1, 2, 3, 4, 5, 6];
    let mut spectra: Vec<Vec<f64>> = Vec::new();
    for material in Material::ALL {
        for &seed in &SEEDS {
            spectra.push(series_band_levels(material, seed, SR_44));
        }
    }
    let at =
        |material: usize, seed: usize| -> &Vec<f64> { &spectra[material * SEEDS.len() + seed] };

    let mut material_sum = 0.0_f64;
    let mut material_count = 0_usize;
    for left in 0..Material::ALL.len() {
        for right in left + 1..Material::ALL.len() {
            for seed in 0..SEEDS.len() {
                material_sum += band_distance(at(left, seed), at(right, seed));
                material_count += 1;
            }
        }
    }
    let mut seed_sum = 0.0_f64;
    let mut seed_count = 0_usize;
    for material in 0..Material::ALL.len() {
        for left in 0..SEEDS.len() {
            for right in left + 1..SEEDS.len() {
                seed_sum += band_distance(at(material, left), at(material, right));
                seed_count += 1;
            }
        }
    }
    let material_distance = material_sum / material_count as f64;
    let seed_distance = seed_sum / seed_count as f64;
    println!("material distance {material_distance:.2} dB vs seed distance {seed_distance:.2} dB");
    assert!(
        material_distance > 3.0 * seed_distance,
        "materials must dominate seeds: {material_distance:.2} dB vs {seed_distance:.2} dB"
    );
}

/// A full recipe-style render: the bank and its single strike excitation
/// both drawn from the one seed stream.
fn render_strike(material: Material, seed: u64, sr: f32, until_s: f32, position: f32) -> Vec<f32> {
    let mut rng = Qrng::new(seed);
    let bank = ModalBank::material(material, &mut rng);
    let excitation: Vec<f32> = (0..(EXCITATION_MILLIS / 1000.0 * sr) as usize)
        .map(|_| rng.next_range(-1.0, 1.0))
        .collect();
    bank.strike_at(sr, &excitation, until_s, position)
        .expect("preset banks and test arguments are valid")
}

#[test]
fn same_seed_renders_bit_identically_per_configuration() {
    // Per-configuration determinism: identical bits at 44.1 kHz and,
    // separately, at 48 kHz. Cross-rate equality is not claimed.
    for material in Material::ALL {
        for &(sr, label) in &[(SR_44, "44.1 kHz"), (SR_48, "48 kHz")] {
            let first = render_strike(material, 42, sr, 0.6, 0.5);
            let second = render_strike(material, 42, sr, 0.6, 0.5);
            assert_eq!(first.len(), second.len());
            for (a, b) in first.iter().zip(&second) {
                assert_eq!(
                    a.to_bits(),
                    b.to_bits(),
                    "{material:?} at {label} is not bit-identical"
                );
            }
        }
    }
}

#[test]
fn preset_banks_are_seeded_reproducibly() {
    let seed = Seed::from_text("modal-render");
    let mut a = Qrng::from_seed(seed);
    let mut b = Qrng::from_seed(seed);
    let first = ModalBank::material(Material::Metal, &mut a);
    let replay = ModalBank::material(Material::Metal, &mut b);
    assert_eq!(first.modes(), replay.modes());
}
