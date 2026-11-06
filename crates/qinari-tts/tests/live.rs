//! Live synthesis tests against the real Supertonic bundle.
//!
//! These are `#[ignore]`d because they need the model directory installed
//! (the layout `qinari setup` creates, or any populated bundle). Point
//! `QINARI_SUPERTONIC_DIR` at the directory containing `onnx/` and
//! `voice_styles/` and run:
//!
//! ```text
//! QINARI_SUPERTONIC_DIR=/path/to/supertonic \
//!     cargo test --release -p qinari-tts -- --ignored
//! ```
//!
//! Release mode matters: the RTF bounds below are measured against
//! optimized inference.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use qinari_tts::engine::{ProsodyMode, StyleSpec, DEFAULT_STEPS};
use qinari_tts::supertonic::SupertonicEngine;
use qinari_tts::textnorm::Language;
use qinari_tts::{find_voice, SynthRequest, TtsEngine};

/// Serializes the live tests: ONNX engines running in parallel on a small
/// CPU starve each other, which both inflates the measured RTF and (through
/// thread-count-dependent matmul reduction order) perturbs the graphs. One
/// synthesis at a time keeps the suite reproducible.
fn live_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The model directory from `QINARI_SUPERTONIC_DIR`.
fn model_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("QINARI_SUPERTONIC_DIR")
            .expect("QINARI_SUPERTONIC_DIR must point at the model dir (onnx/ + voice_styles/)"),
    )
}

/// Loading the ~400 MB bundle is slow; share one engine across the tests.
fn engine() -> &'static SupertonicEngine {
    static ENGINE: OnceLock<SupertonicEngine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let voice = find_voice("supertonic-M1").expect("catalog voice");
        SupertonicEngine::load(&model_dir(), voice).expect("load the supertonic bundle")
    })
}

fn test_request(text: &str, language: Language, seed: Option<u64>) -> SynthRequest {
    SynthRequest {
        text: text.to_string(),
        language,
        speed: 1.0,
        voice: "supertonic-M1".to_string(),
        sr: 44_100,
        channels: 1,
        steps: DEFAULT_STEPS,
        seed,
        prosody: ProsodyMode::Natural,
        style: StyleSpec::Default,
        markup: true,
    }
}

#[test]
#[ignore = "requires the supertonic bundle (set QINARI_SUPERTONIC_DIR)"]
fn renders_indonesian_faster_than_realtime() {
    let _guard = live_lock();
    let engine = engine();
    let request = test_request("Halo, nama saya Albi.", Language::Id, Some(1234));
    let started = Instant::now();
    let synthesis = engine.synthesize(&request).expect("synthesis");
    let wall = started.elapsed();

    assert_eq!(synthesis.audio.sample_rate(), 44_100);
    assert_eq!(synthesis.audio.num_channels(), 1);
    assert!(!synthesis.audio.has_non_finite(), "non-finite samples");
    assert!(
        synthesis.audio.peak() > 0.05,
        "audio is silent (peak {})",
        synthesis.audio.peak()
    );
    assert!(
        (1.0..=6.0).contains(&synthesis.audio.duration()),
        "suspicious duration: {:.2}s",
        synthesis.audio.duration()
    );
    assert!(
        synthesis.stats.rtf < 1.0,
        "RTF {} too slow (wall {wall:?}, audio {:.2}s)",
        synthesis.stats.rtf,
        synthesis.stats.audio_secs
    );
    println!(
        "id RTF {:.3} (wall {:.2?}s, audio {:.2}s)",
        synthesis.stats.rtf, synthesis.stats.elapsed, synthesis.stats.audio_secs
    );
}

#[test]
#[ignore = "requires the supertonic bundle (set QINARI_SUPERTONIC_DIR)"]
fn renders_english_faster_than_realtime() {
    let _guard = live_lock();
    let engine = engine();
    let request = test_request(
        "The quick brown fox jumps over the lazy dog.",
        Language::En,
        Some(1234),
    );
    let synthesis = engine.synthesize(&request).expect("synthesis");

    assert_eq!(synthesis.audio.sample_rate(), 44_100);
    assert_eq!(synthesis.audio.num_channels(), 1);
    assert!(!synthesis.audio.has_non_finite(), "non-finite samples");
    assert!(
        synthesis.audio.peak() > 0.05,
        "audio is silent (peak {})",
        synthesis.audio.peak()
    );
    assert!(
        (1.0..=8.0).contains(&synthesis.audio.duration()),
        "suspicious duration: {:.2}s",
        synthesis.audio.duration()
    );
    assert!(
        synthesis.stats.rtf < 1.0,
        "RTF {} too slow (audio {:.2}s)",
        synthesis.stats.rtf,
        synthesis.stats.audio_secs
    );
    println!(
        "en RTF {:.3} (wall {:.2?}s, audio {:.2}s)",
        synthesis.stats.rtf, synthesis.stats.elapsed, synthesis.stats.audio_secs
    );
}

#[test]
#[ignore = "requires the supertonic bundle (set QINARI_SUPERTONIC_DIR)"]
fn same_seed_renders_byte_identical_audio() {
    let _guard = live_lock();
    let engine = engine();
    let request = test_request(
        "Pada zaman dahulu, hiduplah seorang anak yang sangat rajin.",
        Language::Id,
        Some(987_654_321),
    );
    let first = engine.synthesize(&request).expect("first render");
    let second = engine.synthesize(&request).expect("second render");
    assert_eq!(
        first.audio.channels()[0],
        second.audio.channels()[0],
        "the same seed must render byte-equal audio"
    );
    // A different seed must differ (the initial latent is the only
    // stochastic input; ORT on this box is deterministic).
    let other = engine
        .synthesize(&test_request("Halo!", Language::Id, Some(42)))
        .expect("other render");
    assert_ne!(first.audio.channels()[0], other.audio.channels()[0]);
}

#[test]
#[ignore = "requires the supertonic bundle (set QINARI_SUPERTONIC_DIR)"]
fn markup_tags_and_exact_pauses_reach_the_audio() {
    let _guard = live_lock();
    let engine = engine();
    // Same fragmentation + same seed on both renders: the fragments and
    // their per-chunk RNG draws are identical, so the duration difference
    // isolates exactly the inserted pause (comparing against the
    // unfragmented sentence instead would couple in the model's own
    // fragment-reading prosody, which no trim can separate).
    let long_pause = engine
        .synthesize(&test_request(
            "Pada zaman dahulu [pause:400] hiduplah seorang anak.",
            Language::Id,
            Some(77),
        ))
        .expect("400 ms pause");
    let short_pause = engine
        .synthesize(&test_request(
            "Pada zaman dahulu [pause:1] hiduplah seorang anak.",
            Language::Id,
            Some(77),
        ))
        .expect("1 ms pause");
    let delta = long_pause.audio.duration() - short_pause.audio.duration();
    assert!(
        (delta - 0.399).abs() < 0.005,
        "pause 400 vs 1 ms should differ by 0.399 s, differed {delta:.4}"
    );

    // The explicit pause still beats no pause at all on the same text.
    let plain = engine
        .synthesize(&test_request(
            "Pada zaman dahulu hiduplah seorang anak.",
            Language::Id,
            Some(77),
        ))
        .expect("plain");
    assert!(
        long_pause.audio.duration() > plain.audio.duration(),
        "a 400 ms pause must lengthen the render ({:.2} vs {:.2})",
        long_pause.audio.duration(),
        plain.audio.duration()
    );

    // Unknown tags are hard errors naming the tag.
    let error = engine
        .synthesize(&test_request("boo [cough] hoo", Language::Id, Some(1)))
        .expect_err("unknown tag");
    assert!(error.to_string().contains("[cough]"), "{error}");

    // --no-markup keeps the tag text verbatim and still renders.
    let verbatim = SynthRequest {
        text: "Take [laugh] literally.".to_string(),
        markup: false,
        ..test_request("Take [laugh] literally.", Language::En, Some(5))
    };
    let synthesis = engine.synthesize(&verbatim).expect("no-markup render");
    assert!(synthesis.audio.duration() > 0.5);
}

#[test]
#[ignore = "requires the supertonic bundle (set QINARI_SUPERTONIC_DIR)"]
fn faster_speed_shortens_audio() {
    let _guard = live_lock();
    let engine = engine();
    let text = "This sentence is spoken twice, once slowly and once quickly.";
    let normal = engine
        .synthesize(&test_request(text, Language::En, Some(3)))
        .expect("normal speed");
    let faster = engine
        .synthesize(&test_request(text, Language::En, Some(3)))
        .expect("same seed");
    // Same seed, same speed: identical (sanity for the comparison below).
    assert_eq!(normal.audio.channels()[0], faster.audio.channels()[0]);
    let quick = engine
        .synthesize(&SynthRequest {
            speed: 1.5,
            ..test_request(text, Language::En, Some(3))
        })
        .expect("faster speed");
    assert!(
        quick.audio.duration() < normal.audio.duration() * 0.85,
        "speed 1.5 ({:.2}s) should be clearly shorter than 1.0 ({:.2}s)",
        quick.audio.duration(),
        normal.audio.duration()
    );
}

#[test]
#[ignore = "requires the supertonic bundle (set QINARI_SUPERTONIC_DIR)"]
fn non_native_sample_rates_are_resampled() {
    let _guard = live_lock();
    let engine = engine();
    let request = SynthRequest {
        sr: 48_000,
        ..test_request("Halo, dunia.", Language::Id, Some(11))
    };
    let synthesis = engine.synthesize(&request).expect("resampled synthesis");
    assert_eq!(synthesis.audio.sample_rate(), 48_000);
    let expected = 48_000.0 / 44_100.0;
    assert!(
        (synthesis.audio.frames() as f64 / 44_100.0 / expected) > 0.5,
        "resampled audio suspiciously short"
    );
    assert!(synthesis.audio.peak() > 0.01, "resampled audio is silent");
}

#[test]
#[ignore = "requires the supertonic bundle (set QINARI_SUPERTONIC_DIR)"]
fn speed_out_of_engine_range_is_clamped() {
    let _guard = live_lock();
    let engine = engine();
    let request = SynthRequest {
        speed: 0.5, // accepted by the request contract, clamped by the engine
        ..test_request("Clamp me please.", Language::En, Some(21))
    };
    let synthesis = engine.synthesize(&request).expect("clamped synthesis");
    assert!(synthesis.audio.duration() > 0.3);
    let (value, clamped) = SupertonicEngine::clamp_speed(0.5);
    assert_eq!((value, clamped), (0.7, true));
}
