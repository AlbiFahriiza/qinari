//! Qinari sfx crate: procedural sound effects and ambient beds.
//!
//! [`recipes`] holds the 29 deterministic sound-effect recipes (rain,
//! thunder, laser, ui clicks, ...), [`ambient`] layers them into five
//! long beds with slow gain motion, and [`params`] validates the
//! `NAME=VALUE` pairs each recipe accepts. The entry points are
//! [`render`] for one effect and [`ambient::render_ambient`] for a bed.
//!
//! Every random decision in a render flows from the request's seed
//! through `qinari_dsp::rng::Qrng` text tags, so the same request produces
//! bit-identical audio on every machine.

pub mod ambient;
pub mod params;
pub mod recipes;

use qinari_dsp::buffer::AudioBuffer;
use strsim::levenshtein;
use thiserror::Error;

pub use ambient::{
    ambient_presets, find_preset, render_ambient, AmbientError, AmbientLayer, AmbientPreset,
};
pub use params::{ParamError, ParamKind, ParamSet, ParamSpec};
pub use recipes::{
    find_recipe, poisson_events, recipes, seconds_to_frames, suggest_recipe, Bus, Recipe,
    RenderContext,
};

/// A request for one sound-effect render.
#[derive(Debug, Clone)]
pub struct RenderRequest {
    /// Recipe id (see [`recipes`]).
    pub recipe: String,
    /// `NAME=VALUE` pairs validated against the recipe's parameters.
    pub params: Vec<(String, f64)>,
    /// Render length in seconds (0.05..=600).
    pub duration_secs: f64,
    /// Reproducibility seed.
    pub seed: u64,
    /// Sample rate in hertz.
    pub sr: u32,
    /// Channel count, 1 or 2.
    pub ch: usize,
}

/// Errors raised while rendering a sound effect.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum SfxError {
    /// No recipe has that id; `suggestion` is the closest catalog id.
    #[error("unknown recipe {unknown:?}; did you mean {suggestion:?}?")]
    UnknownRecipe {
        /// The id the caller used.
        unknown: String,
        /// The closest declared recipe id.
        suggestion: String,
    },
    /// Duration outside 0.05..=600 seconds.
    #[error("duration must be between 0.05 and 600 seconds, got {seconds}")]
    BadDuration {
        /// The offending duration.
        seconds: f64,
    },
    /// Channel count is not 1 or 2.
    #[error("channels must be 1 or 2, got {channels}")]
    BadChannels {
        /// The offending channel count.
        channels: usize,
    },
    /// Sample rate is zero.
    #[error("sample rate must be greater than zero, got {rate}")]
    BadSampleRate {
        /// The offending sample rate.
        rate: u32,
    },
    /// A parameter failed validation.
    #[error(transparent)]
    BadParameter(#[from] params::ParamError),
    /// The recipe render itself failed.
    #[error("render failed: {0}")]
    Render(String),
}

/// The Levenshtein-closest recipe id to `typo` (ties in catalog order).
fn closest_recipe(typo: &str) -> String {
    recipes()
        .iter()
        .min_by_key(|recipe| levenshtein(typo, recipe.id))
        .map(|recipe| recipe.id)
        .unwrap_or_default()
        .to_string()
}

/// Renders one sound effect.
///
/// Validation order: recipe id, sample rate, duration, channels, then
/// parameters. The returned buffer has `ch` channels and
/// `duration_secs`-worth of frames at `sr`, and is checked for finiteness
/// before it is returned.
///
/// # Errors
///
/// One [`SfxError`] variant per validation failure; see the enum.
pub fn render(request: &RenderRequest) -> Result<AudioBuffer, SfxError> {
    let recipe = find_recipe(&request.recipe).ok_or_else(|| SfxError::UnknownRecipe {
        unknown: request.recipe.clone(),
        suggestion: closest_recipe(&request.recipe),
    })?;
    if request.sr == 0 {
        return Err(SfxError::BadSampleRate { rate: 0 });
    }
    if !(0.05..=600.0).contains(&request.duration_secs) {
        return Err(SfxError::BadDuration {
            seconds: request.duration_secs,
        });
    }
    if request.ch != 1 && request.ch != 2 {
        return Err(SfxError::BadChannels {
            channels: request.ch,
        });
    }
    let params = ParamSet::new(recipe.params, recipe.id, &request.params)?;
    let context = RenderContext {
        params,
        seed: request.seed,
        sr: request.sr,
        channels: request.ch,
        frames: seconds_to_frames(request.duration_secs, request.sr),
    };
    let buffer = (recipe.render)(&context).map_err(SfxError::Render)?;
    if buffer.has_non_finite() {
        return Err(SfxError::Render(format!(
            "recipe {:?} produced non-finite samples",
            recipe.id
        )));
    }
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(recipe: &str, duration: f64, sr: u32, ch: usize) -> RenderRequest {
        RenderRequest {
            recipe: recipe.to_string(),
            params: Vec::new(),
            duration_secs: duration,
            seed: 1,
            sr,
            ch,
        }
    }

    #[test]
    fn closest_recipe_resolves_typos() {
        assert_eq!(closest_recipe("rian"), "rain");
        assert_eq!(closest_recipe("laserr"), "laser");
        assert_eq!(closest_recipe("hartbeat"), "heartbeat");
    }

    #[test]
    fn unknown_recipe_outranks_other_validation() {
        // Documented order: the recipe id is checked first.
        let error = render(&request("zzz", 9_999.0, 0, 7)).unwrap_err();
        assert!(matches!(error, SfxError::UnknownRecipe { .. }));
        let message = error.to_string();
        assert!(message.contains("did you mean"), "{message}");
    }

    #[test]
    fn validation_errors_carry_their_ranges() {
        let too_short = render(&request("rain", 0.04, 48_000, 2)).unwrap_err();
        assert!(matches!(too_short, SfxError::BadDuration { .. }));
        assert!(too_short.to_string().contains("0.05 and 600"));

        let too_long = render(&request("rain", 600.01, 48_000, 2)).unwrap_err();
        assert!(matches!(too_long, SfxError::BadDuration { .. }));

        let nan = render(&request("rain", f64::NAN, 48_000, 2)).unwrap_err();
        assert!(matches!(nan, SfxError::BadDuration { .. }));

        for bad in [0, 3, 9] {
            let error = render(&request("rain", 1.5, 48_000, bad)).unwrap_err();
            assert!(matches!(error, SfxError::BadChannels { .. }), "{bad}");
            assert!(error.to_string().contains("1 or 2"));
        }

        let error = render(&request("rain", 1.5, 0, 2)).unwrap_err();
        assert!(matches!(error, SfxError::BadSampleRate { .. }));
        assert!(error.to_string().contains("greater than zero"));
    }

    #[test]
    fn minimum_duration_renders_usable_audio() {
        let buffer = render(&request("ui_click", 0.05, 48_000, 2)).unwrap();
        assert_eq!(buffer.frames(), 2400);
        assert_eq!(buffer.num_channels(), 2);
        assert!(!buffer.has_non_finite());
        assert!(buffer.peak() > 0.02, "peak {}", buffer.peak());
    }
}
