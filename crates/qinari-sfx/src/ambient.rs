//! Ambient presets: layered recipe renders with slow gain motion.
//!
//! Each [`AmbientPreset`] layers 2-4 recipes. Bed layers (events per
//! minute = 0) render for the whole requested duration and ride a slow
//! gain swell; event layers fire short one-shot recipes at Cox-process
//! times instead (the rate rides the shared latent field), each instance
//! seeded independently, so a storm rolls thunder through a long render.
//! Like recipes, every random decision flows from the caller's seed
//! through text tags: the same request renders bit-identical audio.

use qinari_dsp::buffer::AudioBuffer;
use qinari_dsp::events::cox_events;
use qinari_dsp::latent::LatentField;
use qinari_dsp::rng::{Qrng, Seed};
use strsim::levenshtein;
use thiserror::Error;

use crate::params::ParamSet;
use crate::recipes::{find_recipe, seconds_to_frames, RenderContext};
use crate::SfxError;

/// Errors raised while rendering an ambient preset.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum AmbientError {
    /// No preset has that id; `suggestion` is the closest preset id.
    #[error("unknown ambient preset {unknown:?}; did you mean {suggestion:?}?")]
    UnknownPreset {
        /// The id the caller used.
        unknown: String,
        /// The closest declared preset id.
        suggestion: String,
    },
    /// Duration outside 1..=3600 seconds.
    #[error("ambient duration must be between 1 and 3600 seconds, got {seconds}")]
    BadDuration {
        /// The offending duration.
        seconds: f64,
    },
    /// A layer was rejected at the sfx level (bad channels, sample rate,
    /// or preset-authored parameters).
    #[error("sfx layer rejected the request: {0}")]
    Sfx(#[from] SfxError),
    /// A layer's recipe render failed.
    #[error("ambient render failed: {0}")]
    Render(String),
}

/// One layer of an [`AmbientPreset`].
#[derive(Debug)]
pub struct AmbientLayer {
    /// Recipe id rendered for this layer.
    pub recipe: &'static str,
    /// Fixed parameter pairs passed to the recipe.
    pub params: &'static [(&'static str, f64)],
    /// Base layer gain (before motion).
    pub gain: f32,
    /// Gain-motion timescale in hertz (the latent field's corner).
    pub motion_rate_hz: f32,
    /// How far the gain swings with the motion (0 = steady).
    pub motion_depth: f32,
    /// Expected one-shot events per minute; 0 for a bed layer.
    ///
    /// Event layers must use short one-shot recipes: a bed recipe would
    /// fill the whole remaining duration from each event.
    pub events_per_minute: f32,
}

/// One ambient preset.
#[derive(Debug)]
pub struct AmbientPreset {
    /// Preset id as the caller writes it.
    pub id: &'static str,
    /// One-line description shown by listings.
    pub description: &'static str,
    /// 2..4 layers.
    pub layers: &'static [AmbientLayer],
}

static PRESETS: [AmbientPreset; 5] = [
    AmbientPreset {
        id: "storm",
        description: "Heavy rain and gusting wind with thunder rolling in.",
        layers: &[
            AmbientLayer {
                recipe: "rain",
                params: &[("intensity", 0.9), ("distance", 0.55)],
                gain: 0.85,
                motion_rate_hz: 0.05,
                motion_depth: 0.3,
                events_per_minute: 0.0,
            },
            AmbientLayer {
                recipe: "wind",
                params: &[("intensity", 0.75)],
                gain: 0.5,
                motion_rate_hz: 0.03,
                motion_depth: 0.4,
                events_per_minute: 0.0,
            },
            AmbientLayer {
                recipe: "thunder",
                params: &[("distance", 0.55)],
                gain: 0.8,
                motion_rate_hz: 0.04,
                motion_depth: 0.25,
                events_per_minute: 1.6,
            },
            AmbientLayer {
                recipe: "thunder",
                params: &[("distance", 0.95)],
                gain: 0.55,
                motion_rate_hz: 0.03,
                motion_depth: 0.25,
                events_per_minute: 4.0,
            },
        ],
    },
    AmbientPreset {
        id: "campfire",
        description: "A crackling fire with crickets and a light night breeze.",
        layers: &[
            AmbientLayer {
                recipe: "fire",
                params: &[("intensity", 0.7)],
                gain: 0.9,
                motion_rate_hz: 0.02,
                motion_depth: 0.15,
                events_per_minute: 0.0,
            },
            AmbientLayer {
                recipe: "crickets",
                params: &[("intensity", 0.5)],
                gain: 0.45,
                motion_rate_hz: 0.04,
                motion_depth: 0.35,
                events_per_minute: 0.0,
            },
            AmbientLayer {
                recipe: "wind",
                params: &[("intensity", 0.25)],
                gain: 0.22,
                motion_rate_hz: 0.05,
                motion_depth: 0.45,
                events_per_minute: 0.0,
            },
        ],
    },
    AmbientPreset {
        id: "ocean",
        description: "Wide surf with a distant wave set and a breeze.",
        layers: &[
            AmbientLayer {
                recipe: "ocean",
                params: &[("intensity", 0.75)],
                gain: 0.95,
                motion_rate_hz: 0.03,
                motion_depth: 0.25,
                events_per_minute: 0.0,
            },
            AmbientLayer {
                recipe: "ocean",
                params: &[("intensity", 0.4)],
                gain: 0.5,
                motion_rate_hz: 0.045,
                motion_depth: 0.3,
                events_per_minute: 0.0,
            },
            AmbientLayer {
                recipe: "wind",
                params: &[("intensity", 0.45)],
                gain: 0.4,
                motion_rate_hz: 0.05,
                motion_depth: 0.4,
                events_per_minute: 0.0,
            },
        ],
    },
    AmbientPreset {
        id: "spaceship",
        description: "An engine hum with a shield field, occasional warps and console confirms.",
        layers: &[
            AmbientLayer {
                recipe: "engine",
                params: &[("rpm", 1900.0), ("load", 0.55)],
                gain: 0.6,
                motion_rate_hz: 0.08,
                motion_depth: 0.2,
                events_per_minute: 0.0,
            },
            AmbientLayer {
                recipe: "forcefield",
                params: &[("intensity", 0.4)],
                gain: 0.38,
                motion_rate_hz: 0.06,
                motion_depth: 0.3,
                events_per_minute: 0.0,
            },
            AmbientLayer {
                recipe: "teleport",
                params: &[],
                gain: 0.5,
                motion_rate_hz: 0.04,
                motion_depth: 0.2,
                events_per_minute: 0.8,
            },
            AmbientLayer {
                recipe: "ui_success",
                params: &[],
                gain: 0.42,
                motion_rate_hz: 0.03,
                motion_depth: 0.25,
                events_per_minute: 1.1,
            },
        ],
    },
    AmbientPreset {
        id: "meadow",
        description: "Crickets, a light breeze and a stream nearby.",
        layers: &[
            AmbientLayer {
                recipe: "crickets",
                params: &[("intensity", 0.8)],
                gain: 0.8,
                motion_rate_hz: 0.03,
                motion_depth: 0.3,
                events_per_minute: 0.0,
            },
            AmbientLayer {
                recipe: "wind",
                params: &[("intensity", 0.28)],
                gain: 0.3,
                motion_rate_hz: 0.05,
                motion_depth: 0.35,
                events_per_minute: 0.0,
            },
            AmbientLayer {
                recipe: "stream",
                params: &[("intensity", 0.45)],
                gain: 0.42,
                motion_rate_hz: 0.04,
                motion_depth: 0.28,
                events_per_minute: 0.0,
            },
        ],
    },
];

/// The full preset catalog, in canonical order.
pub fn ambient_presets() -> &'static [AmbientPreset] {
    &PRESETS
}

/// Returns the preset with that exact id.
pub fn find_preset(id: &str) -> Option<&'static AmbientPreset> {
    ambient_presets().iter().find(|preset| preset.id == id)
}

/// Renders `preset` for `secs` seconds at `sr` with `ch` channels.
///
/// Validation order: preset id, duration (1..=3600 s inclusive), sample
/// rate, channels (1 or 2). Bed layers render once for the whole duration
/// and ride the layer's slow gain motion; event layers fire the recipe as
/// one-shots at Cox-process times (rate driven by the shared field) inside
/// a six-second window. The result is exactly `ch` channels and
/// `secs`-worth of frames.
///
/// # Errors
///
/// [`AmbientError::UnknownPreset`] for an unknown id,
/// [`AmbientError::BadDuration`] outside 1..=3600 s, and
/// [`AmbientError::Sfx`] for bad channels or sample rate (duration is
/// checked before those, so a bad duration always wins over them).
pub fn render_ambient(
    preset: &str,
    secs: f64,
    seed: u64,
    sr: u32,
    ch: usize,
) -> Result<AudioBuffer, AmbientError> {
    let found = find_preset(preset).ok_or_else(|| AmbientError::UnknownPreset {
        unknown: preset.to_string(),
        suggestion: closest_preset(preset),
    })?;
    if !(1.0..=3600.0).contains(&secs) {
        return Err(AmbientError::BadDuration { seconds: secs });
    }
    if sr == 0 {
        return Err(AmbientError::Sfx(SfxError::BadSampleRate { rate: 0 }));
    }
    if ch != 1 && ch != 2 {
        return Err(AmbientError::Sfx(SfxError::BadChannels { channels: ch }));
    }
    let frames = seconds_to_frames(secs, sr);
    let mut master = AudioBuffer::new(ch, frames, sr);
    // One shared latent field per preset: every layer's swell and every
    // event stream rides it, so layers co-vary and events cluster with
    // the swells. The periodic sinusoid motion this replaces put an
    // exact spectral line under every ambient render.
    let corner = found
        .layers
        .iter()
        .map(|layer| layer.motion_rate_hz)
        .sum::<f32>()
        / found.layers.len() as f32;
    let mut field_rng = Qrng::from_seed(Seed::from_text(&format!("{}:field:{seed}", found.id)));
    let field = LatentField::tilted(
        &mut field_rng,
        master.sample_rate() as f32,
        master.frames(),
        corner.max(0.01),
        0.5,
        0.0,
        1.0,
    );
    for (index, layer) in found.layers.iter().enumerate() {
        render_layer(&mut master, found, index, layer, &field, seed)?;
    }
    Ok(master)
}

/// One-shot events render into a window of this length; every one-shot
/// recipe completes well inside it.
const EVENT_WINDOW_SECS: f32 = 6.0;

/// Renders one layer into `master`.
fn render_layer(
    master: &mut AudioBuffer,
    preset: &'static AmbientPreset,
    index: usize,
    layer: &AmbientLayer,
    field: &LatentField,
    seed: u64,
) -> Result<(), AmbientError> {
    let recipe = find_recipe(layer.recipe).ok_or_else(|| {
        AmbientError::Render(format!(
            "preset {:?} references unknown recipe {:?}",
            preset.id, layer.recipe
        ))
    })?;
    let values: Vec<(String, f64)> = layer
        .params
        .iter()
        .map(|(name, value)| ((*name).to_string(), *value))
        .collect();
    let params = ParamSet::new(recipe.params, recipe.id, &values).map_err(SfxError::from)?;
    let sr = master.sample_rate();

    if layer.events_per_minute > 0.0 {
        let mut stream = Qrng::from_seed(Seed::from_text(&layer_seed_text(
            preset.id, index, "events", seed,
        )));
        // The event stream is a Cox process on the shared field: hits
        // cluster in the swells instead of ticking metronome-Poisson.
        let events = cox_events(
            &mut stream,
            master.frames(),
            sr,
            layer.events_per_minute / 60.0,
            |frame| 0.5 + field.value_at(frame),
        );
        let window = (EVENT_WINDOW_SECS * sr as f32) as usize;
        for (hit, &at) in events.iter().enumerate() {
            let frames = (master.frames() - at).min(window);
            if frames == 0 {
                continue;
            }
            let context = RenderContext {
                params: params.clone(),
                seed: Seed::from_text(&layer_seed_text(
                    preset.id,
                    index,
                    &format!("hit{hit}"),
                    seed,
                ))
                .0,
                sr,
                channels: master.num_channels(),
                frames,
            };
            let rendered = (recipe.render)(&context).map_err(AmbientError::Render)?;
            let swell = field.value_at(at);
            let gain = layer.gain * (1.0 - layer.motion_depth + layer.motion_depth * swell);
            mix_at(master, &rendered, at, gain);
        }
    } else {
        let context = RenderContext {
            params,
            seed: Seed::from_text(&layer_seed_text(preset.id, index, "bed", seed)).0,
            sr,
            channels: master.num_channels(),
            frames: master.frames(),
        };
        let rendered = (recipe.render)(&context).map_err(AmbientError::Render)?;
        apply_motion(master, &rendered, layer, field);
    }
    Ok(())
}

/// Mixes a bed layer into `master` with its per-frame motion gain.
fn apply_motion(
    master: &mut AudioBuffer,
    layer_buffer: &AudioBuffer,
    layer: &AmbientLayer,
    field: &LatentField,
) {
    let frames = master.frames().min(layer_buffer.frames());
    for frame in 0..frames {
        let swell = field.value_at(frame);
        let gain = layer.gain * (1.0 - layer.motion_depth + layer.motion_depth * swell);
        for channel in 0..master.num_channels() {
            let value = layer_buffer.get(channel, frame) * gain;
            master.set(channel, frame, master.get(channel, frame) + value);
        }
    }
}

/// Mixes `layer_buffer` into `master` at frame `at` with a fixed gain.
fn mix_at(master: &mut AudioBuffer, layer_buffer: &AudioBuffer, at: usize, gain: f32) {
    for frame in 0..layer_buffer.frames() {
        for channel in 0..master.num_channels() {
            let value = layer_buffer.get(channel, frame) * gain;
            master.set(channel, at + frame, master.get(channel, at + frame) + value);
        }
    }
}

/// Builds the seed text for one layer (or one event of it).
fn layer_seed_text(preset: &str, index: usize, tag: &str, seed: u64) -> String {
    format!("qinari-ambient:{preset}:{index}:{tag}:{seed}")
}

/// The Levenshtein-closest preset id to `typo` (ties in catalog order).
fn closest_preset(typo: &str) -> String {
    ambient_presets()
        .iter()
        .min_by_key(|preset| levenshtein(typo, preset.id))
        .map(|preset| preset.id)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_tables_reference_valid_recipes_and_params() {
        for preset in ambient_presets() {
            assert!((2..=4).contains(&preset.layers.len()), "{}", preset.id);
            for layer in preset.layers {
                let recipe = find_recipe(layer.recipe)
                    .unwrap_or_else(|| panic!("{} references unknown recipe", preset.id));
                let values: Vec<(String, f64)> = layer
                    .params
                    .iter()
                    .map(|(name, value)| ((*name).to_string(), *value))
                    .collect();
                ParamSet::new(recipe.params, recipe.id, &values)
                    .unwrap_or_else(|error| panic!("{} layer params rejected: {error}", preset.id));
                assert!((0.0..=1.0).contains(&layer.motion_depth), "{}", preset.id);
                assert!(layer.motion_rate_hz >= 0.0, "{}", preset.id);
                assert!(layer.events_per_minute >= 0.0, "{}", preset.id);
                assert!((0.0..=3.0).contains(&layer.gain), "{}", preset.id);
            }
        }
    }

    #[test]
    fn find_preset_resolves_ids_and_closest_gives_storm() {
        assert_eq!(find_preset("storm").map(|preset| preset.id), Some("storm"));
        assert!(find_preset("stormm").is_none());
        assert_eq!(closest_preset("stormm"), "storm");
        assert_eq!(closest_preset("campfir"), "campfire");
        assert_eq!(ambient_presets().len(), 5);
    }

    #[test]
    fn motion_gain_swings_between_steady_and_full_depth() {
        // A shared field for the test: corner 1 Hz over 1 s so the swell
        // genuinely traverses its range inside the window.
        let mut rng = Qrng::from_seed(Seed::from_text("ambient-motion-test"));
        let field = LatentField::tilted(&mut rng, 48_000.0, 48_000, 1.0, 0.5, 0.0, 1.0);

        // Depth 0: the gain is the base gain everywhere.
        let layer = AmbientLayer {
            recipe: "rain",
            params: &[],
            gain: 0.5,
            motion_rate_hz: 0.1,
            motion_depth: 0.0,
            events_per_minute: 0.0,
        };
        let mut master = AudioBuffer::new(2, 4800, 48_000);
        let flat =
            AudioBuffer::from_channels(vec![vec![1.0; 4800], vec![1.0; 4800]], 48_000).unwrap();
        apply_motion(&mut master, &flat, &layer, &field);
        for frame in 0..4800 {
            assert!((master.get(0, frame) - 0.5).abs() < 1e-6);
            assert!((master.get(1, frame) - 0.5).abs() < 1e-6);
        }

        // Depth 1: the gain follows the field, swinging across its
        // range over the second — the swell moves and stays around the
        // base gain on average.
        let mut master = AudioBuffer::new(2, 48_000, 48_000);
        let flat =
            AudioBuffer::from_channels(vec![vec![1.0; 48_000], vec![1.0; 48_000]], 48_000).unwrap();
        let deep = AmbientLayer {
            recipe: "rain",
            params: &[],
            gain: 0.5,
            motion_rate_hz: 1.0,
            motion_depth: 1.0,
            events_per_minute: 0.0,
        };
        apply_motion(&mut master, &flat, &deep, &field);
        let channel = &master.channels()[0];
        let peak = channel.iter().fold(0.0_f32, |top, &s| top.max(s));
        let min = channel.iter().fold(f32::INFINITY, |low, &s| low.min(s));
        assert!(peak > 0.3, "the swell must reach high ({peak})");
        assert!(min < 0.1, "the swell must reach low ({min})");
        let mean = channel.iter().sum::<f32>() / channel.len() as f32;
        assert!((0.1..=0.6).contains(&mean), "mean gain {mean}");
    }

    #[test]
    fn mix_at_offsets_and_truncates() {
        let mut master = AudioBuffer::new(1, 10, 48_000);
        let layer_buffer = AudioBuffer::from_channels(vec![vec![0.5; 8]], 48_000).unwrap();
        mix_at(&mut master, &layer_buffer, 4, 2.0);
        assert!((master.get(0, 4) - 1.0).abs() < 1e-6, "offset start");
        assert!((master.get(0, 9) - 1.0).abs() < 1e-6, "last in-range frame");
        assert_eq!(master.get(0, 3), 0.0, "before the offset");
    }
}
