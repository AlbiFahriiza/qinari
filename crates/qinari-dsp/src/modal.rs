//! Modal synthesis: struck objects as exact banks of damped sinusoids.
//!
//! A struck rigid body rings as a bank of modes; each mode n answers with
//! `a_n(t) = A_n·e^(−t/τ_n)·cos(2π f_n t + φ_n)` where `τ_n = t60_n/ln(1000)`
//! (van den Doel & Pai; `docs/RESEARCH-AUDIO-REALISM.md` §2.3). Rendering
//! that formula directly makes each mode's T60 exact **by construction**:
//! after `t60_n` seconds the amplitude has fallen by exactly `e^(−t60/τ) =
//! 1/1000` = −60 dB. The 0.1.5 bank instead stacked a bandpass biquad
//! (which has its own Q-determined ring-down, T60 = ln(1000)·Q/(π·f0)
//! [Smith, *Introduction to Digital Filters*]) under a second external
//! exponential decay, so declared t60 values were fiction — metal declared
//! 0.75 s, rang 45 ms (plan 0.2.0 finding F1). That path is gone.
//!
//! # Excitation projection (the core of the design)
//!
//! How the object is struck decides how loud each mode rings. For a force
//! signal `F(u)` the modal response is the convolution with the mode's
//! impulse response
//!
//! ```text
//! q_n(t) = ∫₀ᵗ h_n(t−u)·F(u) du,   h_n(t) = e^(−t/τ_n)·cos(2π f_n t),
//! ```
//!
//! which expands to `q_n(t) = e^(−t/τ_n)·[I_c·cos(ω_n t) + I_s·sin(ω_n t)]`
//! with the excitation projected onto the mode's damped basis,
//! `I_c = ∫ e^(u/τ_n)cos(ω_n u)F(u)du` and `I_s = ∫ e^(u/τ_n)sin(ω_n u)F(u)du`
//! — initial amplitude `√(I_c²+I_s²)` and phase `atan2(I_s, I_c)` per mode,
//! with no free parameters or per-mode guesses. A soft hammer (slow force
//! pulse) projects little energy onto high modes; a metal-on-metal click
//! excites them all — the recipes only choose the force signal.
//!
//! Discretely the sampled kernel `h[m] = r^m·cos(mθ)` (`r = e^(−1/(τ·sr))`,
//! `θ = 2π f_n/sr`) obeys the exact second-order recursion
//! `h[m] = 2a·h[m−1] − r²·h[m−2]` (`a = r·cosθ`), implemented in direct
//! form as `y[k] = s·(x[k] − a·x[k−1]) + 2a·y[k−1] − r²·y[k−2]` with
//! `s = gain/sr`. The `1/sr` factor is the rectangle rule of the physical
//! integral (each sample is the force held over one sample period), and the
//! recursion state after the excitation window **is** the projection above:
//! free-running it afterwards emits `A·r^k·cos(kθ+φ)`. Cost per mode is two
//! multiply-adds per sample in the free decay (four while driven); 100+ mode
//! banks are cheap, and [`ModalBank::strike_at`] allocates only its output
//! buffer (offline contract, plan §10.B).
//!
//! # Normalization
//!
//! Only a **single global gain per material preset** touches the level.
//! Per-mode `gain` values are *relative modal gains* — part of the object's
//! identity — and pass through at their declared ratios (plan §6.2). The
//! 0.1.5 peak-normalization of each strike (recipes) is removed with it.
//!
//! # Preset values and their sources
//!
//! | material | base register (Hz) | mode layout (f) | per-mode t60 | relative gains | global gain |
//! |---|---|---|---|---|---|
//! | wood | 190–220 — Qinari | free-free bar, λ²/λ₁² table, Benson §3.9 | η = 0.030 — Qinari, **Not verified** | 1/n — Qinari, **Not verified** | calibrated, Qinari |
//! | metal | 800–900 — Qinari | free circular plate, Benson §3.11 (after Fletcher & Rossing §3.6) | η = 0.0020 — Qinari, **Not verified** | 1/n — Qinari, **Not verified** | calibrated, Qinari |
//! | ceramic | 1470–1620 — Qinari | free circular plate, Benson §3.11 | η = 0.0010 — **sourced**: Pauletto & Barahona 2019 (SMC), mug resonator Q 800–1200 (midpoint 1000) via t60 = ln(1000)·Q/(π·f) → η ≈ 1/Q ≈ 0.001 | 1/n — Qinari, **Not verified** | calibrated, Qinari |
//! | thud | 74–87 — Qinari | [1.0, 2.08] — Qinari, **Not verified** | η = 0.25 — Qinari, **Not verified** | 1/n — Qinari, **Not verified** | calibrated, Qinari |
//!
//! "Not verified" marks Qinari design values for which no primary source
//! could be opened this session; P1-2 replaces the whole table with
//! measured Object Acoustic Profiles (plan §6.2). Damping follows the
//! loss-factor law derived from the damped-oscillator ODE
//! `ẍ + 2πfηẋ + (2πf)²x = 0` (damping coefficient `2πfη`): envelope
//! `e^(−πfηt)`, so
//! `t60 = ln(1000)/(π·f·η) ≈ 2.198/(f·η)` — higher modes ring shorter.
//! The base registers are narrowed from the 0.1.5 draws (metal spanned
//! 720–1140 Hz, ceramic 1350–2050 Hz): with spans that wide, same-material
//! renders still reached over a third of the cross-material third-octave
//! distance (measured 13.2 dB seed-to-seed vs 35.1 dB material-to-material,
//! ratio 2.65 < 3), so the material a caller asks for would not reliably
//! dominate what they hear — the binding identity criterion of plan P0-1.
//! Per-hit jitter (frequency
//! ±f/300, gain ±g/5, t60 ±20% via the mug's Q range 800–1200) follows
//! Pauletto & Barahona's published mug randomization; the paper says the
//! level "differs slightly" per material but tabulates only the mug, so
//! applying it to all four is a Qinari choice.

use crate::rng::Qrng;
use thiserror::Error;

/// Errors from building or ringing a [`ModalBank`].
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum ModalError {
    /// A mode had a non-finite field, or non-positive `hz`/`t60`.
    #[error("mode {index}: hz {hz} and t60 {t60} must be finite and positive, gain {gain} must be finite")]
    InvalidMode {
        /// Index of the offending mode in the bank.
        index: usize,
        /// The offending center frequency in hertz.
        hz: f32,
        /// The offending decay time in seconds.
        t60: f32,
        /// The offending relative gain.
        gain: f32,
    },
    /// The sample rate was not finite or not positive.
    #[error("sample rate must be finite and greater than zero, got {rate}")]
    InvalidSampleRate {
        /// The offending sample rate.
        rate: f32,
    },
    /// The render duration was negative, not finite, or its frame count
    /// was unrepresentable on this platform.
    #[error("render duration must be finite, at least zero, and yield a representable frame count, got {until_s} seconds")]
    InvalidDuration {
        /// The offending duration in seconds.
        until_s: f32,
    },
    /// The strike position was outside `[0, 1]`.
    #[error("strike position must be within [0, 1], got {position}")]
    InvalidPosition {
        /// The offending position.
        position: f32,
    },
}

/// One resonance of a struck object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mode {
    /// Center frequency in hertz.
    pub hz: f32,
    /// Seconds the mode takes to decay 60 dB after the excitation — exact
    /// by construction in [`ModalBank::strike_at`].
    pub t60: f32,
    /// Relative modal gain. Ratios between modes are part of the object's
    /// identity and are never renormalized; only the bank's single global
    /// gain scales the whole bank (plan §6.2).
    pub gain: f32,
}

/// The struck-object material palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    /// Wooden bar: stiff, inharmonic overtones, short ring.
    Wood,
    /// Metal plate (gong family): dense inharmonic cluster, long ring.
    Metal,
    /// Ceramic plate: brittle, dense upper partials, medium ring.
    Ceramic,
    /// Dull thud: two low, heavily damped modes.
    Thud,
}

impl Material {
    /// Every material, in catalog order.
    pub const ALL: [Material; 4] = [
        Material::Wood,
        Material::Metal,
        Material::Ceramic,
        Material::Thud,
    ];

    /// The material's catalog name (`"wood"`, …).
    pub fn name(self) -> &'static str {
        match self {
            Material::Wood => "wood",
            Material::Metal => "metal",
            Material::Ceramic => "ceramic",
            Material::Thud => "thud",
        }
    }

    /// Resolves a material by catalog name (case-insensitive).
    pub fn from_name(name: &str) -> Option<Self> {
        Material::ALL
            .into_iter()
            .find(|material| material.name().eq_ignore_ascii_case(name))
    }
}

/// Free-free Euler-Bernoulli bar eigenvalues `λ_n·L` for n = 1..=6, the
/// positive roots of `cosh λ·cos λ = 1` (Benson, *Music: A Mathematical
/// Offering*, §3.9, eq. 3.9.7; his 25-digit values correctly rounded to
/// the nearest f64). Mode frequencies are proportional to λ².
const FREE_FREE_BAR_LAMBDA: [f64; 6] = [
    4.730_040_744_862_704,
    7.853_204_624_095_838,
    10.995_607_838_001_671,
    14.137_165_491_257_464,
    17.278_759_657_399_48,
    20.420_352_245_626_06,
];

/// Relative frequencies of a free circular plate (gong family), sorted
/// ascending; the (k=0, n=2) mode is the fundamental. Benson §3.11, table
/// "Vibrational frequencies for a free circular plate", after Fletcher &
/// Rossing, *The Physics of Musical Instruments* §3.6.
const FREE_PLATE_RATIOS: [f64; 16] = [
    1.000, 1.73, 2.328, 3.91, 4.11, 6.30, 6.71, 7.34, 10.07, 11.40, 13.92, 15.97, 18.24, 21.19,
    27.18, 33.31,
];

/// Relative frequencies of the dull-thud object — Qinari design values
/// (Not verified; P1-2 replaces them with measured profiles).
const THUD_RATIOS: [f64; 2] = [1.0, 2.08];

/// Highest mode frequency a preset generates. Above this, game-impact
/// content is minimal and presets stay inside the 44.1 kHz Nyquist band.
/// Qinari design value.
const MODE_CEILING_HZ: f64 = 16_000.0;

/// ln(1000): the natural log of the 60 dB amplitude ratio.
const LN_1000: f64 = 6.907_755_278_982_137;

/// Per-material single global gains: the **only** normalization in the
/// bank (level calibration; relative modal gains pass through untouched).
/// Calibrated by measurement against a 4 ms white-noise excitation
/// (`docs/PLAN-0.2.0.md` P0-1); Qinari design values.
const WOOD_GAIN: f32 = 2_600.0;
const METAL_GAIN: f32 = 2_600.0;
const CERAMIC_GAIN: f32 = 2_600.0;
const THUD_GAIN: f32 = 4_000.0;

/// Per-material preset parameters. `eta` is the loss factor of the
/// `t60 = 2.198/(f·η)` law derived in the module docs; see the source
/// table there for what is sourced and what is a Qinari choice.
struct MaterialSpec {
    /// Lowest drawable fundamental frequency in hertz.
    base_lo: f32,
    /// Span of the fundamental draw in hertz.
    base_span: f32,
    /// Loss factor η of the material.
    eta: f32,
    /// The single global gain (level calibration only).
    global_gain: f32,
}

fn material_spec(material: Material) -> MaterialSpec {
    // Base registers are Qinari design values, narrowed from the 0.1.5
    // draws so material identity dominates seed variance (see the module
    // docs table for the measured numbers behind that choice).
    match material {
        Material::Wood => MaterialSpec {
            base_lo: 190.0,
            base_span: 30.0,
            eta: 0.030,
            global_gain: WOOD_GAIN,
        },
        Material::Metal => MaterialSpec {
            base_lo: 800.0,
            base_span: 100.0,
            eta: 0.0020,
            global_gain: METAL_GAIN,
        },
        Material::Ceramic => MaterialSpec {
            base_lo: 1_470.0,
            base_span: 150.0,
            eta: 0.0010,
            global_gain: CERAMIC_GAIN,
        },
        Material::Thud => MaterialSpec {
            base_lo: 74.0,
            base_span: 13.0,
            eta: 0.25,
            global_gain: THUD_GAIN,
        },
    }
}

/// Frequency ratios over the fundamental for `material`'s mode family, up
/// to `ceiling_hz` for the drawn `base`. Wood uses the free-free bar law
/// (exact λ table for n ≤ 6, then Benson's asymptotic `λ_n ≈ (n+½)π`,
/// accurate to better than 1e-9 relative for n ≥ 7 — the correction terms
/// of his eq. 3.9.9 are below 2e−10 there); metal and ceramic use the free
/// circular plate table; thud uses its two hand values.
fn layout_ratios(material: Material, base: f32, ceiling_hz: f64) -> Vec<f64> {
    match material {
        Material::Wood => {
            let mut ratios = Vec::new();
            let mut n = 1;
            loop {
                let lambda = if n <= FREE_FREE_BAR_LAMBDA.len() {
                    FREE_FREE_BAR_LAMBDA[n - 1]
                } else {
                    (n as f64 + 0.5) * std::f64::consts::PI
                };
                let ratio = (lambda / FREE_FREE_BAR_LAMBDA[0]).powi(2);
                if f64::from(base) * ratio >= ceiling_hz {
                    break;
                }
                ratios.push(ratio);
                n += 1;
            }
            ratios
        }
        Material::Metal | Material::Ceramic => FREE_PLATE_RATIOS
            .iter()
            .copied()
            .take_while(|&ratio| f64::from(base) * ratio < ceiling_hz)
            .collect(),
        Material::Thud => THUD_RATIOS.to_vec(),
    }
}

/// A bank of resonances excited as one struck object.
#[derive(Debug, Clone, PartialEq)]
pub struct ModalBank {
    modes: Vec<Mode>,
    /// The one global gain applied to the whole bank; 1.0 for user-built
    /// banks, a calibrated constant for material presets.
    global_gain: f32,
}

impl ModalBank {
    /// A bank over exactly these modes.
    ///
    /// # Errors
    ///
    /// Returns [`ModalError::InvalidMode`] if any mode has non-finite
    /// fields, or non-positive `hz`/`t60`.
    pub fn new(modes: Vec<Mode>) -> Result<Self, ModalError> {
        for (index, mode) in modes.iter().enumerate() {
            let valid = mode.hz.is_finite()
                && mode.t60.is_finite()
                && mode.gain.is_finite()
                && mode.hz > 0.0
                && mode.t60 > 0.0;
            if !valid {
                return Err(ModalError::InvalidMode {
                    index,
                    hz: mode.hz,
                    t60: mode.t60,
                    gain: mode.gain,
                });
            }
        }
        Ok(Self {
            modes,
            global_gain: 1.0,
        })
    }

    /// A material preset: the material's cited mode family over a drawn
    /// base frequency, with per-mode jitter so repeated strikes of one
    /// material vary like real objects — all three magnitudes are the
    /// mug's published per-hit randomization [S11] (frequency ±f/300,
    /// gain ±g/5, resonator Q drawn from 800–1200, i.e. t60 ±20%),
    /// applied to every material as a Qinari choice (the paper tabulates
    /// only the mug).
    pub fn material(material: Material, rng: &mut Qrng) -> Self {
        let spec = material_spec(material);
        let base = spec.base_lo + rng.next_f32() * spec.base_span;
        let ratios = layout_ratios(material, base, MODE_CEILING_HZ);
        let modes = ratios
            .iter()
            .enumerate()
            .map(|(index, &ratio)| {
                // Jitter magnitudes from the mug's randomization [S11]:
                // ±f/300 ≈ ±0.33% frequency, ±g/5 = ±20% gain, and its
                // Q range 800–1200 is ±20% around the midpoint → t60 ±20%.
                let hz = (f64::from(base) * ratio) as f32 * (1.0 + (rng.next_f32() - 0.5) / 150.0);
                let t60 = 2.198 / (spec.eta * hz) * (1.0 + (rng.next_f32() - 0.5) * 0.40);
                let gain = (1.0 / (index as f32 + 1.0)) * (1.0 + (rng.next_f32() - 0.5) * 0.40);
                Mode { hz, t60, gain }
            })
            .collect();
        Self {
            modes,
            global_gain: spec.global_gain,
        }
    }

    /// The bank's modes, strongest (lowest) first.
    pub fn modes(&self) -> &[Mode] {
        &self.modes
    }

    /// Rings the whole bank from the object's centre — the 0.1.5 entry
    /// point kept working for existing callers. Equivalent to
    /// [`ModalBank::strike_at`] with position 0.5; returns an empty buffer
    /// instead of an error when arguments are invalid, so new code should
    /// prefer `strike_at`, which reports errors.
    pub fn strike(&self, sr: f32, excitation: &[f32], until_s: f32) -> Vec<f32> {
        self.strike_at(sr, excitation, until_s, 0.5)
            .unwrap_or_default()
    }

    /// Rings the whole bank with `excitation` (typically a few
    /// milliseconds of force signal) struck at `position` (0–1 along the
    /// object), rendering `until_s` seconds (at least the excitation
    /// length). The excitation is projected onto every mode's damped
    /// basis — see the module docs — so the strike's force shape decides
    /// the inter-mode energy distribution. Modes at or above the Nyquist
    /// frequency cannot be represented at `sr` and are skipped (the same
    /// bank rings them at a higher rate); relative modal gains and the
    /// single global gain are the only level controls.
    ///
    /// # Errors
    ///
    /// Returns [`ModalError::InvalidSampleRate`], [`ModalError::InvalidDuration`]
    /// or [`ModalError::InvalidPosition`] for invalid arguments.
    pub fn strike_at(
        &self,
        sr: f32,
        excitation: &[f32],
        until_s: f32,
        position: f32,
    ) -> Result<Vec<f32>, ModalError> {
        validate_strike_args(sr, until_s, position)?;
        let frames = ((until_s * sr).ceil() as usize).max(excitation.len());
        let mut out = vec![0.0_f32; frames];
        for (index, mode) in self.modes.iter().enumerate() {
            if mode.hz >= sr * 0.5 {
                continue;
            }
            let weight = position_weight(index + 1, position);
            render_mode_into(&mut out, mode, mode.gain * weight, sr, excitation);
        }
        // The single global gain — the only level control, applied once
        // over the finished bank (1.0 for user-built banks changes
        // nothing, bit-exactly).
        for sample in &mut out {
            *sample *= self.global_gain;
        }
        Ok(out)
    }
}

/// Rejects the render arguments the bank cannot honour (plan §10.B:
/// physical parameters are refused with a defined error, never a panic).
fn validate_strike_args(sr: f32, until_s: f32, position: f32) -> Result<(), ModalError> {
    if !(sr.is_finite() && sr > 0.0) {
        return Err(ModalError::InvalidSampleRate { rate: sr });
    }
    if !(until_s.is_finite() && until_s >= 0.0) {
        return Err(ModalError::InvalidDuration { until_s });
    }
    // A frame count past the largest Vec<f32> capacity would abort on
    // allocation instead of failing cleanly — plan §10.B requires a
    // defined error, never a panic, on user input.
    let frames = f64::from(until_s) * f64::from(sr);
    let capacity_limit = (isize::MAX as f64) / std::mem::size_of::<f32>() as f64;
    if frames > capacity_limit {
        return Err(ModalError::InvalidDuration { until_s });
    }
    if !(position.is_finite() && (0.0..=1.0).contains(&position)) {
        return Err(ModalError::InvalidPosition { position });
    }
    Ok(())
}

/// Strike-position weight of mode `n` (1-based): the mode shape evaluated
/// at the strike point. Qinari modeling choice: the quarter-wave sine
/// shapes `φ_n(p) = sin((n − ½)·π·p)` — exact eigenfunctions of a string
/// fixed at one end and free at the other, applied uniformly to every
/// material as a cheap, node-carrying model until measured position
/// weights arrive (plan P1-2); they are *not* the true bar/plate
/// eigenfunctions. Striking a node (e.g. p = 2/3 for mode 2) silences
/// that mode; the clamped end p = 0 absorbs everything.
fn position_weight(n: usize, position: f32) -> f32 {
    ((n as f32 - 0.5) * std::f32::consts::PI * position).sin()
}

/// Adds one mode's exact response to `out`: the convolution of the
/// excitation with the sampled damped-cosine kernel `h[m] = r^m·cos(mθ)`,
/// via the kernel's own second-order recursion (module docs). While the
/// excitation lasts the recursion is driven; afterwards it free-runs with
/// two multiply-adds per sample and a decay of exactly `r` per sample.
fn render_mode_into(out: &mut [f32], mode: &Mode, gain: f32, sr: f32, excitation: &[f32]) {
    let tau = f64::from(mode.t60) / LN_1000;
    let r = (-1.0 / (tau * f64::from(sr))).exp();
    let theta = 2.0 * std::f64::consts::PI * f64::from(mode.hz) / f64::from(sr);
    let a = (r * theta.cos()) as f32;
    let r2 = (r * r) as f32;
    let scale = gain / sr;
    let (mut y1, mut y2, mut x1) = (0.0_f32, 0.0_f32, 0.0_f32);
    let driven = excitation.len().min(out.len());
    for (k, slot) in out[..driven].iter_mut().enumerate() {
        let x = excitation[k];
        let y = scale * (x - a * x1) + 2.0 * a * y1 - r2 * y2;
        *slot += y;
        x1 = x;
        y2 = y1;
        y1 = y;
    }
    for slot in &mut out[driven..] {
        let y = 2.0 * a * y1 - r2 * y2;
        *slot += y;
        y2 = y1;
        y1 = y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::goertzel_magnitude;
    use crate::rng::Seed;

    /// Renders with a white-noise excitation of `millis` milliseconds.
    fn noise_excitation(rng: &mut Qrng, sr: f32, millis: f32) -> Vec<f32> {
        (0..(millis / 1000.0 * sr) as usize)
            .map(|_| rng.next_range(-1.0, 1.0))
            .collect()
    }

    #[test]
    fn materials_round_trip_through_their_names() {
        for material in Material::ALL {
            assert_eq!(Material::from_name(material.name()), Some(material));
            assert_eq!(
                Material::from_name(&material.name().to_uppercase()),
                Some(material)
            );
        }
        assert_eq!(Material::from_name("jelly"), None);
    }

    #[test]
    fn presets_follow_their_cited_mode_families() {
        // Wood: free-free bar — second partial near the bar ratio 2.7565
        // (Benson §3.9), a dozen-plus modes under the 16 kHz ceiling.
        let mut rng = Qrng::from_seed(Seed::from_text("preset-wood"));
        let wood = ModalBank::material(Material::Wood, &mut rng);
        assert!(wood.modes().len() >= 10, "wood had {}", wood.modes().len());
        let second = wood.modes()[1].hz / wood.modes()[0].hz;
        assert!((2.6..2.9).contains(&second), "wood ratio {second}");

        // Metal: free circular plate — dense cluster starting at 1.73
        // (Benson §3.11), lower register than ceramic.
        let mut rng = Qrng::from_seed(Seed::from_text("preset-metal"));
        let metal = ModalBank::material(Material::Metal, &mut rng);
        assert!(
            metal.modes().len() >= 8,
            "metal had {}",
            metal.modes().len()
        );
        let second = metal.modes()[1].hz / metal.modes()[0].hz;
        assert!((1.6..1.9).contains(&second), "metal ratio {second}");
        assert!(metal.modes()[0].hz > 795.0 && metal.modes()[0].hz < 905.0);

        // Ceramic: the same plate family, but higher and less damped than
        // metal — the loss-factor law gives t60·f = 2.198/η ≈ 2198 Hz·s
        // for ceramic (η 0.0010) vs ≈ 1099 for metal (η 0.0020).
        let mut rng = Qrng::from_seed(Seed::from_text("preset-ceramic"));
        let ceramic = ModalBank::material(Material::Ceramic, &mut rng);
        assert!(ceramic.modes().len() >= 6);
        assert!(ceramic.modes()[0].hz > 1_465.0 && ceramic.modes()[0].hz < 1_625.0);
        let ceramic_law = ceramic.modes()[0].t60 * ceramic.modes()[0].hz;
        let metal_law = metal.modes()[0].t60 * metal.modes()[0].hz;
        assert!(
            ceramic_law > 1.5 * metal_law,
            "ceramic must ring longer than metal at equal frequency: {ceramic_law} vs {metal_law} Hz·s"
        );

        // Thud: two low, heavily damped modes.
        let mut rng = Qrng::from_seed(Seed::from_text("preset-thud"));
        let thud = ModalBank::material(Material::Thud, &mut rng);
        assert_eq!(thud.modes().len(), 2);
        assert!(thud.modes()[0].hz < 130.0, "thud is low");
        assert!(thud.modes()[0].t60 < 0.2, "thud dies fast");

        for bank in [&wood, &metal, &ceramic, &thud] {
            assert!(bank
                .modes()
                .iter()
                .all(|mode| mode.hz > 0.0 && mode.hz < MODE_CEILING_HZ as f32 && mode.t60 > 0.0));
        }
    }

    #[test]
    fn construction_and_render_validate_their_arguments() {
        let good = Mode {
            hz: 440.0,
            t60: 0.5,
            gain: 1.0,
        };
        assert!(ModalBank::new(vec![good]).is_ok());
        for bad in [
            Mode {
                hz: 0.0,
                t60: 0.5,
                gain: 1.0,
            },
            Mode {
                hz: 440.0,
                t60: 0.0,
                gain: 1.0,
            },
            Mode {
                hz: f32::NAN,
                t60: 0.5,
                gain: 1.0,
            },
            Mode {
                hz: 440.0,
                t60: 0.5,
                gain: f32::INFINITY,
            },
        ] {
            assert!(
                matches!(
                    ModalBank::new(vec![bad]),
                    Err(ModalError::InvalidMode { .. })
                ),
                "mode {bad:?} must be rejected"
            );
        }

        let bank = ModalBank::new(vec![good]).unwrap();
        let excitation = [0.5_f32; 8];
        assert!(bank.strike_at(0.0, &excitation, 0.1, 0.5).is_err());
        assert!(bank.strike_at(f32::NAN, &excitation, 0.1, 0.5).is_err());
        assert!(bank.strike_at(48_000.0, &excitation, -0.1, 0.5).is_err());
        // Finite but unrepresentable: 1e30 s × 48 kHz overflows any Vec.
        assert!(bank.strike_at(48_000.0, &excitation, 1e30, 0.5).is_err());
        assert!(bank.strike_at(48_000.0, &excitation, 0.1, -0.01).is_err());
        assert!(bank.strike_at(48_000.0, &excitation, 0.1, 1.01).is_err());
        assert!(bank
            .strike_at(48_000.0, &excitation, 0.1, f32::NAN)
            .is_err());
        assert!(bank.strike_at(48_000.0, &excitation, 0.1, 0.5).is_ok());
        // The legacy entry point keeps its signature and centre position.
        assert_eq!(bank.strike(48_000.0, &excitation, 0.1).len(), 4_800);
    }

    #[test]
    fn a_single_mode_decays_exactly_sixty_db_over_its_t60() {
        // The sharp replacement of the 0.1.5 test, whose −34 dB bound let
        // decays up to 250× too fast pass (plan F2): the envelope must sit
        // within 10% of exactly −60 dB after the declared t60.
        let bank = ModalBank::new(vec![Mode {
            hz: 800.0,
            t60: 0.25,
            gain: 1.0,
        }])
        .unwrap();
        let sr = 48_000.0_f32;
        let strike = bank.strike_at(sr, &[1.0], 0.3, 1.0).unwrap();
        assert!(strike.iter().all(|s| s.is_finite()));

        let rms = |from_s: f64, to_s: f64| {
            let a = (from_s * f64::from(sr)) as usize;
            let b = (to_s * f64::from(sr)) as usize;
            (strike[a..b]
                .iter()
                .map(|s| f64::from(*s) * f64::from(*s))
                .sum::<f64>()
                / (b - a) as f64)
                .sqrt()
        };
        let early = rms(0.002, 0.012);
        let late = rms(0.252, 0.262);
        let ratio = late / early;
        assert!(
            (0.000_9..0.001_1).contains(&ratio),
            "decay over one t60 was {ratio} (expected 0.001 = −60 dB); early {early} late {late}"
        );
    }

    #[test]
    fn strike_position_weights_modes_by_their_mode_shapes() {
        // Mode 2's quarter-wave shape has a node at p = 2/3: striking
        // there must silence mode 2 while mode 1 keeps ringing.
        let bank = ModalBank::new(vec![
            Mode {
                hz: 440.0,
                t60: 0.3,
                gain: 1.0,
            },
            Mode {
                hz: 1_100.0,
                t60: 0.3,
                gain: 1.0,
            },
        ])
        .unwrap();
        let sr = 48_000.0_f32;
        let mut rng = Qrng::from_seed(Seed::from_text("position"));
        let excitation = noise_excitation(&mut rng, sr, 4.0);
        let at_node = bank.strike_at(sr, &excitation, 0.2, 2.0 / 3.0).unwrap();
        let at_end = bank.strike_at(sr, &excitation, 0.2, 1.0).unwrap();

        let node_mode2 = goertzel_magnitude(&at_node, 1_100.0, sr);
        let end_mode2 = goertzel_magnitude(&at_end, 1_100.0, sr);
        assert!(
            node_mode2 < 0.05 * end_mode2,
            "mode 2 at its node rang at {node_mode2} vs {end_mode2}"
        );
        let node_mode1 = goertzel_magnitude(&at_node, 440.0, sr);
        assert!(
            node_mode1 > 0.3 * goertzel_magnitude(&at_end, 440.0, sr),
            "mode 1 must keep ringing away from its own node"
        );
    }

    #[test]
    fn excitation_hardness_shapes_inter_mode_energy() {
        // The projection, not a parameter, decides the balance: a soft
        // one-arch force pulse leaves the high mode far weaker relative to
        // the low mode than a hard single-sample click.
        let bank = ModalBank::new(vec![
            Mode {
                hz: 200.0,
                t60: 0.4,
                gain: 1.0,
            },
            Mode {
                hz: 4_000.0,
                t60: 0.4,
                gain: 1.0,
            },
        ])
        .unwrap();
        let sr = 48_000.0_f32;
        let click = vec![1.0_f32];
        // One smooth 10 ms arch at 100 Hz: force rises and falls before
        // the high mode can be driven.
        let soft: Vec<f32> = (0..(0.01 * sr) as usize)
            .map(|k| {
                let phase = k as f32 / (0.01 * sr);
                (std::f32::consts::PI * phase).sin()
            })
            .collect();

        let balance = |excitation: &[f32]| {
            let strike = bank.strike_at(sr, excitation, 0.3, 1.0).unwrap();
            goertzel_magnitude(&strike, 4_000.0, sr) / goertzel_magnitude(&strike, 200.0, sr)
        };
        let hard_balance = balance(&click);
        let soft_balance = balance(&soft);
        assert!(
            hard_balance > 3.0 * soft_balance,
            "hard strike balance {hard_balance} must dominate soft {soft_balance}"
        );
    }

    #[test]
    fn a_hundred_mode_bank_rings_finite_and_audible() {
        let modes: Vec<Mode> = (1..=100)
            .map(|n| Mode {
                hz: 80.0 * n as f32,
                t60: 0.6 / (n as f32).sqrt(),
                gain: 1.0 / n as f32,
            })
            .collect();
        let bank = ModalBank::new(modes).unwrap();
        let sr = 44_100.0_f32;
        let mut rng = Qrng::from_seed(Seed::from_text("hundred"));
        let excitation = noise_excitation(&mut rng, sr, 4.0);
        let strike = bank.strike_at(sr, &excitation, 1.0, 0.5).unwrap();
        assert_eq!(strike.len(), 44_100);
        assert!(strike.iter().all(|s| s.is_finite()));
        let peak = strike.iter().fold(0.0_f32, |top, s| top.max(s.abs()));
        assert!(peak > 1e-4, "100-mode bank was silent, peak {peak}");
    }

    #[test]
    fn repeated_material_draws_differ_but_repeat_deterministically() {
        let mut a = Qrng::from_seed(Seed::from_text("modal-test"));
        let mut b = Qrng::from_seed(Seed::from_text("modal-test"));
        let first = ModalBank::material(Material::Wood, &mut a);
        let replay = ModalBank::material(Material::Wood, &mut b);
        let second = ModalBank::material(Material::Wood, &mut a);
        assert_eq!(first, replay, "same seed = same bank");
        assert_ne!(first, second, "jitter decorrelates repeated draws");
    }

    #[test]
    fn strikes_stay_finite_and_audible_for_every_material() {
        for material in Material::ALL {
            let mut rng = Qrng::from_seed(Seed::from_text("materials"));
            let bank = ModalBank::material(material, &mut rng);
            let excitation = noise_excitation(&mut rng, 44_100.0, 4.0);
            let strike = bank
                .strike_at(44_100.0, &excitation, 0.3, 0.5)
                .unwrap_or_default();
            assert!(strike.iter().all(|s| s.is_finite()), "{material:?}");
            let peak = strike.iter().fold(0.0_f32, |top, s| top.max(s.abs()));
            assert!(peak > 0.05, "{material:?} must ring audibly, peak {peak}");
            assert!(peak < 4.0, "{material:?} peak {peak} out of layer range");
        }
    }
}
