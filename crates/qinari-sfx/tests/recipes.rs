//! Integration tests: every recipe renders non-silent, bounded, finite
//! and deterministic through the public API, and validation fails with
//! actionable messages.

use qinari_dsp::buffer::AudioBuffer;
use qinari_sfx::{
    ambient_presets, recipes, render, render_ambient, suggest_recipe, AmbientError, RenderRequest,
    SfxError,
};

const SR: u32 = 48_000;
const LOW_SR: u32 = 8_000;

/// The 29 catalog ids in canonical order; the count and spelling are part
/// of the public interface.
const EXPECTED_IDS: [&str; 29] = [
    "rain",
    "wind",
    "thunder",
    "ocean",
    "fire",
    "stream",
    "explosion",
    "impact",
    "whoosh",
    "sword",
    "laser",
    "blaster",
    "teleport",
    "forcefield",
    "robot_talk",
    "ui_success",
    "ui_error",
    "ui_click",
    "sonar",
    "alarm",
    "engine",
    "glitch",
    "riser",
    "hit",
    "sub_drop",
    "heartbeat",
    "footsteps",
    "spell_cast",
    "crickets",
];

fn request(recipe: &str, params: &[(&str, f64)]) -> RenderRequest {
    RenderRequest {
        recipe: recipe.to_string(),
        params: params
            .iter()
            .map(|(name, value)| ((*name).to_string(), *value))
            .collect(),
        duration_secs: 1.5,
        seed: 7,
        sr: SR,
        ch: 2,
    }
}

fn bits_equal(a: &AudioBuffer, b: &AudioBuffer) -> bool {
    if a.num_channels() != b.num_channels() || a.frames() != b.frames() {
        return false;
    }
    for channel in 0..a.num_channels() {
        for frame in 0..a.frames() {
            if a.get(channel, frame).to_bits() != b.get(channel, frame).to_bits() {
                return false;
            }
        }
    }
    true
}

#[test]
fn catalog_lists_exactly_the_29_planned_ids() {
    assert_eq!(recipes().len(), 29);
    let ids: Vec<&str> = recipes().iter().map(|recipe| recipe.id).collect();
    assert_eq!(ids, EXPECTED_IDS);
    let descriptions = recipes()
        .iter()
        .filter(|recipe| recipe.description.is_empty());
    assert_eq!(descriptions.count(), 0, "every recipe is described");
}

#[test]
fn every_recipe_stereo_48k_is_non_silent_bounded_finite() {
    for recipe in recipes() {
        let buffer = render(&request(recipe.id, &[])).unwrap();
        assert_eq!(buffer.num_channels(), 2, "{}", recipe.id);
        assert_eq!(buffer.frames(), 72_000, "{}", recipe.id);
        assert_eq!(buffer.sample_rate(), SR, "{}", recipe.id);
        assert!(!buffer.has_non_finite(), "{}", recipe.id);
        let peak = buffer.peak();
        assert!(peak > 0.02, "{} peak was {peak}", recipe.id);
        assert!(peak < 4.0, "{} peak was {peak}", recipe.id);
    }
}

#[test]
fn every_recipe_mono_2s_is_non_silent() {
    for recipe in recipes() {
        let mut req = request(recipe.id, &[]);
        req.duration_secs = 2.0;
        req.ch = 1;
        let buffer = render(&req).unwrap();
        assert_eq!(buffer.num_channels(), 1, "{}", recipe.id);
        assert_eq!(buffer.frames(), 96_000, "{}", recipe.id);
        assert!(!buffer.has_non_finite(), "{}", recipe.id);
        let peak = buffer.peak();
        assert!(peak > 0.02, "{} mono peak was {peak}", recipe.id);
        assert!(peak < 4.0, "{} mono peak was {peak}", recipe.id);
    }
}

#[test]
fn every_recipe_at_8k_stays_finite_bounded_and_audible() {
    // Nyquist-clamp regression: recipes designed above 4 kHz must not blow
    // up when the DSP clamps them below 2 kHz.
    for recipe in recipes() {
        let mut req = request(recipe.id, &[]);
        req.sr = LOW_SR;
        let buffer = render(&req).unwrap();
        assert_eq!(buffer.sample_rate(), LOW_SR, "{}", recipe.id);
        assert!(!buffer.has_non_finite(), "{}", recipe.id);
        let peak = buffer.peak();
        assert!(peak > 0.02, "{} 8k peak was {peak}", recipe.id);
        assert!(peak < 4.0, "{} 8k peak was {peak}", recipe.id);
    }
}

#[test]
fn every_recipe_is_deterministic_same_seed_same_bits() {
    for recipe in recipes() {
        let first = render(&request(recipe.id, &[])).unwrap();
        let second = render(&request(recipe.id, &[])).unwrap();
        assert!(
            bits_equal(&first, &second),
            "{} is not deterministic",
            recipe.id
        );
    }

    // A different seed must actually change the output.
    let mut req = request("rain", &[]);
    let seeded_a = render(&req).unwrap();
    req.seed = 8;
    let seeded_b = render(&req).unwrap();
    assert!(
        !bits_equal(&seeded_a, &seeded_b),
        "seed did not change rain"
    );
}

#[test]
fn parameters_change_the_render() {
    let calm = render(&request("rain", &[("intensity", 0.05), ("distance", 0.9)])).unwrap();
    let stormy = render(&request("rain", &[("intensity", 0.95), ("distance", 0.1)])).unwrap();
    // A close heavy shower is markedly louder than a distant drizzle.
    assert!(
        stormy.rms() > 1.5 * calm.rms(),
        "calm {} vs stormy {}",
        calm.rms(),
        stormy.rms()
    );

    let slow = render(&request("footsteps", &[("rate", 0.6)])).unwrap();
    let fast = render(&request("footsteps", &[("rate", 3.5)])).unwrap();
    assert!(fast.rms() > slow.rms(), "step rate did not raise energy");
}

#[test]
fn unknown_recipe_error_contains_suggestion() {
    let error = render(&request("rian", &[])).unwrap_err();
    assert!(matches!(error, SfxError::UnknownRecipe { .. }));
    let message = error.to_string();
    assert!(message.contains("did you mean"), "{message}");
    assert!(message.contains("rain"), "{message}");

    assert_eq!(suggest_recipe("rian", 3).first(), Some(&"rain"));
}

#[test]
fn param_validation_suggests_and_shows_ranges() {
    // Near-miss name.
    let error = render(&request("rain", &[("intensit", 0.5)])).unwrap_err();
    assert!(matches!(error, SfxError::BadParameter(_)));
    let message = error.to_string();
    assert!(message.contains("did you mean"), "{message}");
    assert!(message.contains("intensity"), "{message}");

    // Out of range, float and int tables.
    let error = render(&request("rain", &[("intensity", 5.0)])).unwrap_err();
    assert!(error.to_string().contains("out of range (0..1)"), "{error}");
    let error = render(&request("engine", &[("rpm", 9999.0)])).unwrap_err();
    assert!(
        error.to_string().contains("out of range (600..6000)"),
        "{error}"
    );
    let error = render(&request("alarm", &[("speed", 0.1)])).unwrap_err();
    assert!(
        error.to_string().contains("out of range (0.2..3)"),
        "{error}"
    );

    // A parameter on a zero-param recipe explains itself.
    let error = render(&request("ui_click", &[("intensity", 0.5)])).unwrap_err();
    assert!(error.to_string().contains("takes no parameters"), "{error}");
}

#[test]
fn duration_and_channel_ranges_are_enforced() {
    for (duration, note) in [(0.04, "below"), (600.01, "above"), (f64::NAN, "nan")] {
        let mut req = request("rain", &[]);
        req.duration_secs = duration;
        let error = render(&req).unwrap_err();
        assert!(matches!(error, SfxError::BadDuration { .. }), "{note}");
        assert!(
            error.to_string().contains("0.05 and 600"),
            "{note}: {error}"
        );
    }
    for bad in [0, 3, 8] {
        let mut req = request("rain", &[]);
        req.ch = bad;
        let error = render(&req).unwrap_err();
        assert!(matches!(error, SfxError::BadChannels { .. }), "{bad}");
    }
    let mut req = request("rain", &[]);
    req.sr = 0;
    assert!(matches!(
        render(&req).unwrap_err(),
        SfxError::BadSampleRate { .. }
    ));
}

#[test]
fn ambient_presets_render_5s_non_silent() {
    for preset in ambient_presets() {
        let buffer = render_ambient(preset.id, 5.0, 21, SR, 2).unwrap();
        assert_eq!(buffer.num_channels(), 2, "{}", preset.id);
        assert_eq!(buffer.frames(), 240_000, "{}", preset.id);
        assert!(!buffer.has_non_finite(), "{}", preset.id);
        let peak = buffer.peak();
        assert!(peak > 0.02, "{} peak was {peak}", preset.id);
        assert!(peak < 4.0, "{} peak was {peak}", preset.id);
    }

    // Mono works too.
    let mono = render_ambient("storm", 5.0, 21, SR, 1).unwrap();
    assert_eq!(mono.num_channels(), 1);
    assert!(mono.peak() > 0.02, "mono storm peak {}", mono.peak());
}

#[test]
fn ambient_long_render_is_deterministic_and_safe_at_8k() {
    // Long enough to fire several one-shot thunder events; 8 k exercises
    // the Nyquist clamps through the ambient path as well.
    let first = render_ambient("storm", 120.0, 5, LOW_SR, 1).unwrap();
    let second = render_ambient("storm", 120.0, 5, LOW_SR, 1).unwrap();
    assert!(
        bits_equal(&first, &second),
        "long ambient render is not deterministic"
    );
    assert!(!first.has_non_finite());
    let peak = first.peak();
    assert!(peak > 0.02, "peak {peak}");
    assert!(peak < 4.0, "peak {peak}");
}

#[test]
fn ambient_unknown_preset_and_duration_errors() {
    let error = render_ambient("stormm", 10.0, 1, SR, 2).unwrap_err();
    assert!(matches!(error, AmbientError::UnknownPreset { .. }));
    let message = error.to_string();
    assert!(message.contains("did you mean"), "{message}");
    assert!(message.contains("storm"), "{message}");

    for seconds in [0.5, 3601.0, f64::NAN] {
        let error = render_ambient("campfire", seconds, 1, SR, 2).unwrap_err();
        assert!(
            matches!(error, AmbientError::BadDuration { .. }),
            "{seconds}"
        );
        assert!(
            error.to_string().contains("1 and 3600"),
            "{seconds}: {error}"
        );
    }
    for bad in [0, 5] {
        let error = render_ambient("campfire", 5.0, 1, SR, bad).unwrap_err();
        assert!(matches!(error, AmbientError::Sfx(_)), "{bad}");
    }
    assert!(matches!(
        render_ambient("campfire", 5.0, 1, 0, 2).unwrap_err(),
        AmbientError::Sfx(_)
    ));

    // The lower duration boundary is inclusive.
    let boundary = render_ambient("meadow", 1.0, 3, SR, 2).unwrap();
    assert_eq!(boundary.frames(), 48_000);
    assert!(boundary.peak() > 0.02, "boundary render was silent");
}
