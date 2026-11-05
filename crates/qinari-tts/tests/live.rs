//! Live synthesis tests against the real downloaded voices.
//!
//! These are `#[ignore]`d because they need the voice models installed
//! under `~/.local/share/qinari/models/` (the layout `qinari setup`
//! creates: `models/<voice-id>/voice.onnx` plus its `.onnx.json`
//! sibling). Run them with:
//!
//! ```text
//! cargo test --release -p qinari-tts -- --ignored
//! ```
//!
//! Release mode matters: the RTF bounds below are measured against
//! optimized inference.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

use qinari_tts::local::{ensure_espeak_data, LocalVitsEngine};
use qinari_tts::textnorm::Language;
use qinari_tts::{find_voice, SynthRequest, TtsEngine, TtsError};

/// Root of the CLI's data layout.
fn data_root() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local/share/qinari")
}

fn voice_dir(id: &str) -> PathBuf {
    data_root().join("models").join(id)
}

fn model_path(id: &str) -> PathBuf {
    voice_dir(id).join("voice.onnx")
}

fn espeak_dir() -> PathBuf {
    data_root().join("models").join("espeak-data")
}

/// Loading a 63-137 MB model is slow; share it across the tests.
fn en_engine() -> &'static LocalVitsEngine {
    static ENGINE: OnceLock<LocalVitsEngine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let voice = find_voice("en_US-libritts-high").expect("catalog voice");
        LocalVitsEngine::load(&model_path(voice.id), &espeak_dir(), voice).expect("load en voice")
    })
}

fn id_engine() -> &'static LocalVitsEngine {
    static ENGINE: OnceLock<LocalVitsEngine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let voice = find_voice("id_ID-news_tts-medium").expect("catalog voice");
        LocalVitsEngine::load(&model_path(voice.id), &espeak_dir(), voice).expect("load id voice")
    })
}

fn request(text: &str, language: Language, voice: &str, speed: f32) -> SynthRequest {
    SynthRequest {
        text: text.to_string(),
        language,
        speed,
        voice: voice.to_string(),
        sr: 22_050,
        channels: 1,
    }
}

#[test]
#[ignore = "needs the en_US-libritts-high voice installed"]
fn en_voice_synthesizes_faster_than_realtime() {
    ensure_espeak_data(&espeak_dir(), &[Language::En, Language::Id]).expect("espeak data");
    let engine = en_engine();
    let request = request(
        "The quick brown fox jumps over the lazy dog, twice.",
        Language::En,
        "en_US-libritts-high",
        1.0,
    );
    let started = Instant::now();
    let synthesis = engine.synthesize(&request).expect("synthesis");
    let wall = started.elapsed();

    assert_eq!(synthesis.audio.sample_rate(), 22_050);
    assert_eq!(synthesis.audio.num_channels(), 1);
    assert!(!synthesis.audio.has_non_finite(), "non-finite samples");
    assert!(
        synthesis.audio.peak() > 0.05,
        "audio is silent (peak {})",
        synthesis.audio.peak()
    );
    assert!(
        synthesis.audio.duration() > 1.0,
        "suspiciously short: {}s",
        synthesis.audio.duration()
    );
    assert!(
        synthesis.stats.rtf < 1.5,
        "RTF {} too slow (wall {wall:?}, audio {:.2}s)",
        synthesis.stats.rtf,
        synthesis.stats.audio_secs
    );
    println!(
        "en RTF {:.3} (wall {:.2?}s, audio {:.2}s)",
        synthesis.stats.rtf, synthesis.stats.elapsed, synthesis.stats.audio_secs
    );
}

#[test]
#[ignore = "needs the id_ID-news_tts-medium voice installed"]
fn id_voice_synthesizes_faster_than_realtime() {
    ensure_espeak_data(&espeak_dir(), &[Language::En, Language::Id]).expect("espeak data");
    let engine = id_engine();
    let request = request(
        "Selamat pagi, semoga hari Anda menyenangkan.",
        Language::Id,
        "id_ID-news_tts-medium",
        1.0,
    );
    let synthesis = engine.synthesize(&request).expect("synthesis");

    assert_eq!(synthesis.audio.sample_rate(), 22_050);
    assert_eq!(synthesis.audio.num_channels(), 1);
    assert!(!synthesis.audio.has_non_finite(), "non-finite samples");
    assert!(
        synthesis.audio.peak() > 0.05,
        "audio is silent (peak {})",
        synthesis.audio.peak()
    );
    assert!(
        synthesis.stats.rtf < 0.5,
        "RTF {} too slow (audio {:.2}s)",
        synthesis.stats.rtf,
        synthesis.stats.audio_secs
    );
    println!(
        "id RTF {:.3} (wall {:.2?}s, audio {:.2}s)",
        synthesis.stats.rtf, synthesis.stats.elapsed, synthesis.stats.audio_secs
    );
}

#[test]
#[ignore = "needs the en_US-libritts-high voice installed"]
fn faster_speed_shortens_audio() {
    ensure_espeak_data(&espeak_dir(), &[Language::En, Language::Id]).expect("espeak data");
    let engine = en_engine();
    let text = "This sentence is spoken twice, once slowly and once quickly.";
    let normal = engine
        .synthesize(&request(text, Language::En, "en_US-libritts-high", 1.0))
        .expect("normal speed");
    let faster = engine
        .synthesize(&request(text, Language::En, "en_US-libritts-high", 1.5))
        .expect("faster speed");
    assert!(
        faster.audio.duration() < normal.audio.duration() * 0.9,
        "speed 1.5 ({:.2}s) should be clearly shorter than 1.0 ({:.2}s)",
        faster.audio.duration(),
        normal.audio.duration()
    );
    println!(
        "speed 1.0: {:.2}s, speed 1.5: {:.2}s",
        normal.audio.duration(),
        faster.audio.duration()
    );
}

#[test]
#[ignore = "needs the en_US-libritts-high voice installed"]
fn local_engine_rejects_mismatched_voice_and_rate() {
    ensure_espeak_data(&espeak_dir(), &[Language::En, Language::Id]).expect("espeak data");
    let engine = en_engine();
    let wrong_voice = request("Hello.", Language::En, "id_ID-news_tts-medium", 1.0);
    assert!(matches!(
        engine.synthesize(&wrong_voice),
        Err(TtsError::UnknownVoice { .. })
    ));
    let wrong_rate = SynthRequest {
        sr: 44_100,
        ..request("Hello.", Language::En, "en_US-libritts-high", 1.0)
    };
    assert!(matches!(
        engine.synthesize(&wrong_rate),
        Err(TtsError::UnsupportedSampleRate { .. })
    ));
}
