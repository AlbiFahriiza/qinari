//! Pure-computation DSP primitives for the Qinari audio engines.
//!
//! Everything in this crate is I/O-free and depends only on `thiserror`:
//! [`buffer`] provides the planar [`buffer::AudioBuffer`] all engines render
//! into, [`rng`] the deterministic [`rng::Qrng`] every random decision flows
//! through (so renders are reproducible from a single seed), [`analysis`]
//! tone-selective measurement, [`filter`] RBJ biquad / TPT SVF / one-pole /
//! DC-blocker designs, and [`osc`] band-limited oscillators with FM and pitch
//! helpers.
//!
//! On top of those sit [`noise`] generators and shaped noise sources,
//! [`crate::env`] ADSR and T60-calibrated percussive envelopes, [`lfo`]
//! low-frequency oscillators, [`pan`] equal-power panning, [`fx`] delay /
//! reverb / dynamics / waveshaping effects, [`karplus`] plucked-string
//! synthesis, [`lufs`] ITU-R BS.1770-4 loudness measurement, and [`dither`]
//! requantizer dither with noise shaping.
//!
//! Realism-oriented layers: [`latent`] provides the one shared slow
//! stochastic process a whole render breathes on, [`events`] the renewal
//! and Cox event processes beyond memoryless Poisson, [`spatial`] bed
//! decorrelation / distance / early-reflection helpers, and [`modal`]
//! exact damped-sinusoid banks for struck objects.

pub mod analysis;
pub mod buffer;
pub mod dither;
pub mod env;
pub mod events;
pub mod filter;
pub mod fx;
pub mod karplus;
pub mod latent;
pub mod lfo;
pub mod lufs;
pub mod modal;
pub mod noise;
pub mod osc;
pub mod pan;
pub mod rng;
pub mod spatial;
