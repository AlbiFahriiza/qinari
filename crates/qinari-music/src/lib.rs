//! Procedural music composition for Qinari.
//!
//! [`render`] turns a [`TrackRequest`] into a complete arrangement in one
//! of five styles. The composer lays the track out as intro / verse /
//! bridge / outro sections, picks a chord progression from a per-style
//! pool, generates a melodic motif and develops it across the sections
//! (octave transposition and doubling in later verses, contour mirroring
//! in the bridge, thinned phrasing in the outro), and arranges
//! style-specific drums, bass lines and pad beds around it.
//!
//! Every decision flows from [`TrackRequest::seed`], so the same request
//! always renders sample-identical audio.
//!
//! Length: a request between 5 and 600 seconds renders at least the
//! requested duration; the final section may run past it so the musical
//! form completes.
//!
//! Tempo: the requested `bpm` is clamped into the style's supported range
//! ([`Style::bpm_min`] ..= [`Style::bpm_max`]); zero, negative or
//! non-finite values fall back to [`Style::bpm_typical`].
//!
//! # Examples
//!
//! ```
//! use qinari_dsp::rng::Seed;
//! use qinari_music::{render, TrackRequest};
//!
//! let request = TrackRequest {
//!     style: "lofi".to_string(),
//!     duration_secs: 12.0,
//!     bpm: 84.0,
//!     seed: Seed::from_text("evening demo"),
//!     sample_rate: 44_100,
//!     channels: 2,
//! };
//! let track = render(&request).expect("valid request");
//! assert!(track.frames() >= 12 * 44_100);
//! ```

mod compose;
#[cfg(test)]
mod tests;

use qinari_dsp::buffer::AudioBuffer;
use qinari_dsp::rng::Seed;
use thiserror::Error;

/// One composer style: identifier, character, and supported tempo range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// Lowercase identifier to put in [`TrackRequest::style`].
    pub id: &'static str,
    /// One-line description of the style's character.
    pub description: &'static str,
    /// Slowest tempo the style accepts, in beats per minute.
    pub bpm_min: f32,
    /// Typical tempo, used when a request carries no usable one.
    pub bpm_typical: f32,
    /// Fastest tempo the style accepts, in beats per minute.
    pub bpm_max: f32,
}

/// The composer's style catalog, in listing order.
pub const STYLES: &[Style] = &[
    Style {
        id: "lofi",
        description: "warm jazzy keys over a swung, vinyl-worn groove",
        bpm_min: 72.0,
        bpm_typical: 82.0,
        bpm_max: 90.0,
    },
    Style {
        id: "ambient",
        description: "slowly evolving pads with no drums",
        bpm_min: 58.0,
        bpm_typical: 64.0,
        bpm_max: 72.0,
    },
    Style {
        id: "synthwave",
        description: "driving minor-key synth pop",
        bpm_min: 100.0,
        bpm_typical: 110.0,
        bpm_max: 118.0,
    },
    Style {
        id: "cinematic",
        description: "tense sub bass, toms and a sustained lead",
        bpm_min: 76.0,
        bpm_typical: 88.0,
        bpm_max: 96.0,
    },
    Style {
        id: "chiptune",
        description: "fast square leads and arpeggios",
        bpm_min: 128.0,
        bpm_typical: 140.0,
        bpm_max: 150.0,
    },
];

/// Finds a style by its exact [`Style::id`].
pub fn find_style(id: &str) -> Option<&'static Style> {
    STYLES.iter().find(|style| style.id == id)
}

/// A request for one procedurally composed track.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRequest {
    /// Style identifier; must match a [`Style::id`] exactly.
    pub style: String,
    /// Target length in seconds. Valid range: 5.0 ..= 600.0; the render
    /// is at least this long.
    pub duration_secs: f32,
    /// Desired tempo in beats per minute, clamped into the style's
    /// supported range. Zero, negative or non-finite values fall back to
    /// the style's typical tempo.
    pub bpm: f32,
    /// Seed driving every random decision in the composition.
    pub seed: Seed,
    /// Sample rate in hertz; must be greater than zero.
    pub sample_rate: u32,
    /// Output channel count: 1 (mono) or 2 (stereo).
    pub channels: u16,
}

/// Errors returned by [`render`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum MusicError {
    /// The requested style matches no catalog entry.
    #[error("unknown style \"{style}\"")]
    UnknownStyle {
        /// The offending style identifier.
        style: String,
    },
    /// `duration_secs` was outside 5.0 ..= 600.0.
    #[error("duration must be between 5 and 600 seconds, got {requested}")]
    BadDuration {
        /// The offending duration in seconds.
        requested: f32,
    },
    /// `channels` was neither 1 nor 2.
    #[error("channels must be 1 or 2, got {channels}")]
    BadChannels {
        /// The offending channel count.
        channels: u16,
    },
}

/// Resolves the tempo actually used for a request: clamped into the
/// style's range, or the typical tempo when the request carries no usable
/// value.
fn effective_bpm(requested: f32, style: &Style) -> f32 {
    if requested.is_finite() && requested > 0.0 {
        requested.clamp(style.bpm_min, style.bpm_max)
    } else {
        style.bpm_typical
    }
}

/// Composes and renders one track.
///
/// The output covers at least `duration_secs`; the final section may run
/// past it so the form completes. The peak of the returned buffer never
/// exceeds 1.0.
///
/// # Errors
///
/// Returns [`MusicError::UnknownStyle`] when `style` matches no catalog
/// entry, [`MusicError::BadDuration`] when `duration_secs` is outside
/// 5.0 ..= 600.0, and [`MusicError::BadChannels`] when `channels` is not
/// 1 or 2.
///
/// # Panics
///
/// Panics when `sample_rate` is zero: there is no valid render at that
/// rate.
pub fn render(request: &TrackRequest) -> Result<AudioBuffer, MusicError> {
    let Some(style) = find_style(&request.style) else {
        return Err(MusicError::UnknownStyle {
            style: request.style.clone(),
        });
    };
    if !(5.0..=600.0).contains(&request.duration_secs) {
        return Err(MusicError::BadDuration {
            requested: request.duration_secs,
        });
    }
    if !matches!(request.channels, 1 | 2) {
        return Err(MusicError::BadChannels {
            channels: request.channels,
        });
    }
    assert!(
        request.sample_rate > 0,
        "sample_rate must be greater than zero"
    );
    let bpm = effective_bpm(request.bpm, style);
    Ok(compose::compose(request, style, bpm))
}
