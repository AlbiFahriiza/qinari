//! Modal synthesis: struck objects as sums of decaying resonances.
//!
//! A struck rigid body rings as a bank of modes — each a resonance with
//! its own frequency, damping, and decay time (van den Doel & Pai;
//! `docs/RESEARCH-AUDIO-REALISM.md` §2.3). [`ModalBank`] implements each
//! mode as one existing [`Biquad`] bandpass resonator fed by a short
//! excitation, shaped by an explicit per-mode exponential decay whose
//! `t60` is the time the mode takes to fall 60 dB. Material presets
//! (wood, metal, ceramic, thud) cover the struck-object palette the SFX
//! recipes migrate onto; the per-mode jitter keeps two strikes of the
//! same material from ringing identically.

use crate::filter::{Biquad, BiquadKind};
use crate::rng::Qrng;

/// One resonance of a struck object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mode {
    /// Center frequency in hertz.
    pub hz: f32,
    /// Resonance quality factor (bandwidth of the transient response).
    pub q: f32,
    /// Seconds the mode takes to decay 60 dB after the excitation.
    pub t60: f32,
}

/// The struck-object material palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    /// Wooden bar: stiff, mildly inharmonic overtones, short ring.
    Wood,
    /// Metal bar/bell: strongly inharmonic partials, long bright ring.
    Metal,
    /// Ceramic: brittle, dense upper partials, medium ring.
    Ceramic,
    /// Dull thud: one or two low, heavily damped modes.
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

/// Partial-ratio tables per material. Wooden and ceramic bars are close
/// to harmonic with mild stiffness inharmonicity; metal carries the
/// classic bell-family ratios; a thud is a single dominant flex.
const WOOD_RATIOS: [f32; 4] = [1.0, 2.32, 4.25, 6.63];
const METAL_RATIOS: [f32; 5] = [1.0, 2.76, 5.40, 8.93, 13.34];
const CERAMIC_RATIOS: [f32; 4] = [1.0, 2.05, 3.15, 4.50];
const THUD_RATIOS: [f32; 2] = [1.0, 2.08];

/// A bank of resonances excited as one struck object.
#[derive(Debug, Clone, PartialEq)]
pub struct ModalBank {
    modes: Vec<Mode>,
}

impl ModalBank {
    /// A bank over exactly these modes.
    pub fn new(modes: Vec<Mode>) -> Self {
        Self { modes }
    }

    /// A material preset: the ratio table over a drawn base frequency,
    /// with per-mode frequency (±4%) and decay (±15%) jitter so repeated
    /// strikes of one material vary like real objects.
    pub fn material(material: Material, rng: &mut Qrng) -> Self {
        let (base, quality, decay): (f32, f32, f32) = match material {
            Material::Wood => (185.0 + rng.next_f32() * 60.0, 13.0, 0.15),
            Material::Metal => (720.0 + rng.next_f32() * 420.0, 32.0, 0.75),
            Material::Ceramic => (1_350.0 + rng.next_f32() * 700.0, 19.0, 0.20),
            Material::Thud => (68.0 + rng.next_f32() * 26.0, 4.0, 0.09),
        };
        let (ratios, decay_span) = match material {
            Material::Wood => (&WOOD_RATIOS[..], 0.4),
            Material::Metal => (&METAL_RATIOS[..], 0.55),
            Material::Ceramic => (&CERAMIC_RATIOS[..], 0.45),
            Material::Thud => (&THUD_RATIOS[..], 0.3),
        };
        let modes = ratios
            .iter()
            .enumerate()
            .map(|(index, &ratio)| {
                let hz = base * ratio * (1.0 + (rng.next_f32() - 0.5) * 0.08);
                // Higher partials ring shorter and broader.
                let thin = index as f32 / ratios.len() as f32;
                Mode {
                    hz,
                    q: (quality * (1.0 - 0.45 * thin)).max(1.5),
                    t60: decay * (1.0 - decay_span * thin) * (1.0 + (rng.next_f32() - 0.5) * 0.3),
                }
            })
            .collect();
        Self { modes }
    }

    /// The bank's modes, strongest (lowest) first.
    pub fn modes(&self) -> &[Mode] {
        &self.modes
    }

    /// Rings the whole bank with `excitation` (typically a few
    /// milliseconds of shaped noise), rendering `until_s` seconds of
    /// strike output: every mode's bandpass sees the excitation, each
    /// decays over its own `t60`, and the sum is the struck object.
    ///
    /// Modes are normalized by Q so a resonant metal ring does not
    /// overpower a damped thud at equal excitation energy.
    pub fn strike(&self, sr: f32, excitation: &[f32], until_s: f32) -> Vec<f32> {
        let frames = ((until_s * sr).ceil() as usize).max(excitation.len());
        let mut out = vec![0.0_f32; frames];
        for mode in &self.modes {
            let mut resonator = Biquad::design(BiquadKind::Bandpass, sr, mode.hz, mode.q, 0.0);
            // −60 dB after exactly t60 seconds.
            let decay_per_sample = (-6.907_755 / (mode.t60.max(1e-4) * sr)).exp();
            let mut decay = 1.0_f32;
            // Q normalization keeps the summed bank level comparable
            // across materials.
            let weight = 1.0 / mode.q.sqrt();
            for (i, slot) in out.iter_mut().enumerate() {
                let x = excitation.get(i).copied().unwrap_or(0.0);
                *slot += resonator.process(x) * decay * weight;
                decay *= decay_per_sample;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Seed;

    fn rng() -> Qrng {
        Qrng::from_seed(Seed::from_text("modal-test"))
    }

    #[test]
    fn material_presets_ring_in_their_registers() {
        let wood = ModalBank::material(Material::Wood, &mut rng());
        assert_eq!(wood.modes().len(), 4);
        assert!(
            wood.modes()[0].hz > 120.0 && wood.modes()[0].hz < 300.0,
            "wood base {}",
            wood.modes()[0].hz
        );

        let metal = ModalBank::material(Material::Metal, &mut rng());
        assert_eq!(metal.modes().len(), 5);
        assert!(
            metal.modes()[0].t60 > 0.4,
            "metal rings long, t60 {}",
            metal.modes()[0].t60
        );
        // Bell-family inharmonicity: the second partial is well past 2×
        // the fundamental (a harmonic bar would sit at exactly 2).
        let second = metal.modes()[1].hz / metal.modes()[0].hz;
        assert!((2.4..2.9).contains(&second), "metal ratio {second}");

        let ceramic = ModalBank::material(Material::Ceramic, &mut rng());
        assert_eq!(ceramic.modes().len(), 4);
        assert!(ceramic.modes()[0].hz > 1_100.0, "ceramic is high");

        let thud = ModalBank::material(Material::Thud, &mut rng());
        assert_eq!(thud.modes().len(), 2);
        assert!(thud.modes()[0].hz < 130.0, "thud is low");
        assert!(thud.modes()[0].t60 < 0.16, "thud dies fast");
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
    fn strike_decays_sixty_db_within_t60() {
        let bank = ModalBank::new(vec![Mode {
            hz: 800.0,
            q: 20.0,
            t60: 0.25,
        }]);
        let sr = 48_000.0;
        let excitation: Vec<f32> = (0..64).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
        let strike = bank.strike(sr, &excitation, 0.5);
        assert_eq!(strike.len(), (0.5 * sr) as usize);
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
        let early = rms(0.0, 0.02);
        let late = rms(0.28, 0.34);
        // t60 = 0.25 s means the window starting just past it sits near
        // or below −60 dB of the early energy.
        assert!(
            late < early * 0.02,
            "early {early} vs late {late}: the mode must decay within its t60"
        );
    }

    #[test]
    fn repeated_material_draws_differ_but_repeat_deterministically() {
        let mut a = rng();
        let mut b = rng();
        let first = ModalBank::material(Material::Wood, &mut a);
        let replay = ModalBank::material(Material::Wood, &mut b);
        let second = ModalBank::material(Material::Wood, &mut a);
        assert_eq!(first, replay, "same seed = same bank");
        assert_ne!(first, second, "jitter decorrelates repeated draws");
    }

    #[test]
    fn strike_output_scales_and_stays_finite_for_every_material() {
        for material in Material::ALL {
            let bank = ModalBank::material(material, &mut rng());
            let excitation: Vec<f32> = (0..96)
                .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
                .collect();
            let strike = bank.strike(44_100.0, &excitation, 0.3);
            assert!(strike.iter().all(|s| s.is_finite()), "{material:?}");
            assert!(
                strike.iter().any(|s| s.abs() > 1e-4),
                "{material:?} must ring at all"
            );
        }
    }
}
