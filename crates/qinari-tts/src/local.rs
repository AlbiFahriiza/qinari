//! Offline VITS synthesis through ONNX Runtime, phonemized by the
//! pure-Rust eSpeak NG port.
//!
//! The engine loads a Piper-class voice (an `.onnx` model plus its
//! `.onnx.json` configuration sibling), phonemizes input text offline, maps
//! phonemes to symbol ids with the configuration's `phoneme_id_map`, and
//! runs the VITS graph: inputs `input`, `input_lengths`, `scales`
//! (`[noise_scale, length_scale / speed, noise_w]`), and — for
//! multi-speaker voices — `sid` (speaker 0). Both bundled voices output
//! 22 050 Hz mono audio.

use crate::engine::{retarget_channels, stats_for, SynthRequest, Synthesis, TtsEngine};
use crate::textnorm::{chunk_text, expand_numbers, Language};
use crate::{TtsError, VoiceEntry};
use espeak_ng::EspeakNg;
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;
use qinari_dsp::buffer::AudioBuffer;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

/// Characters of normalized text per synthesis chunk. Matches the cloud
/// engine's chunking so both engines bound memory the same way.
const CHUNK_CHARS: usize = 3000;

/// `ort` allows exactly one [`ort::environment::Environment`] per process
/// (creating a second one panics inside the runtime), so the environment is
/// created once and held for the process lifetime.
static ORT_ENV: LazyLock<Option<ort::environment::Environment>> =
    LazyLock::new(|| ort::init().build().ok());

/// Marker file prefix written by [`ensure_espeak_data`].
const MARKER_PREFIX: &str = ".qinari-espeak-";

/// The `audio` section of a Piper `.onnx.json` voice configuration.
#[derive(Debug, Deserialize)]
struct AudioSection {
    sample_rate: u32,
}

/// The `espeak` section of a Piper `.onnx.json` voice configuration.
#[derive(Debug, Deserialize)]
struct EspeakSection {
    voice: String,
}

/// The `inference` section of a Piper `.onnx.json` voice configuration.
#[derive(Debug, Deserialize)]
struct InferenceSection {
    noise_scale: f32,
    length_scale: f32,
    noise_w: f32,
}

/// A parsed Piper `.onnx.json` voice configuration.
#[derive(Debug, Deserialize)]
struct VoiceConfig {
    audio: AudioSection,
    espeak: EspeakSection,
    inference: InferenceSection,
    phoneme_id_map: HashMap<String, Vec<i64>>,
}

/// Writes the bundled eSpeak NG data for `langs` into `data_dir`.
///
/// The phoneme tables are shared by all languages; each language adds its
/// dictionary. A marker file `.qinari-espeak-<lang>` is written after each
/// successful install, so repeated calls (or concurrent setups) only write
/// what is missing.
///
/// # Errors
///
/// Returns [`TtsError::Io`] when the directory cannot be created, the data
/// cannot be written, or the marker cannot be recorded.
pub fn ensure_espeak_data(data_dir: &Path, langs: &[Language]) -> Result<(), TtsError> {
    std::fs::create_dir_all(data_dir)?;
    for lang in langs {
        let marker = data_dir.join(format!("{MARKER_PREFIX}{}", lang.as_code()));
        if marker.exists() {
            continue;
        }
        espeak_ng::install_bundled_language(data_dir, lang.as_code())?;
        std::fs::write(&marker, b"")?;
    }
    Ok(())
}

/// The offline VITS engine.
///
/// Load one with [`LocalVitsEngine::load`], then [`TtsEngine::synthesize`]
/// any number of requests for that voice. The engine is cheap to share
/// across threads: the ONNX session is guarded by a mutex and eSpeak NG
/// phonemization is stateless.
pub struct LocalVitsEngine {
    session: Mutex<Session>,
    espeak: EspeakNg,
    config: VoiceConfig,
    voice: VoiceEntry,
}

impl LocalVitsEngine {
    /// Loads a voice for synthesis.
    ///
    /// `model_path` points at the `.onnx` model file; its `.onnx.json`
    /// configuration sibling (same path with `.json` appended) must exist
    /// next to it. `espeak_dir` receives the bundled eSpeak NG data on
    /// first use (see [`ensure_espeak_data`]; the call is idempotent).
    ///
    /// # Errors
    ///
    /// Returns [`TtsError::VoiceConfig`] when the configuration is
    /// missing or malformed, [`TtsError::Espeak`] when phonemization data
    /// cannot be installed or loaded, [`TtsError::OnnxInit`] or
    /// [`TtsError::Onnx`] when ONNX Runtime cannot load the model, and
    /// [`TtsError::Io`] for filesystem failures.
    pub fn load(
        model_path: &Path,
        espeak_dir: &Path,
        voice: &VoiceEntry,
    ) -> Result<Self, TtsError> {
        let config_path = PathBuf::from(format!("{}.json", model_path.display()));
        let config_text =
            std::fs::read_to_string(&config_path).map_err(|error| TtsError::VoiceConfig {
                detail: format!("could not read {}: {error}", config_path.display()),
            })?;
        let config: VoiceConfig =
            serde_json::from_str(&config_text).map_err(|error| TtsError::VoiceConfig {
                detail: format!("{}: {error}", config_path.display()),
            })?;
        if config.audio.sample_rate == 0 {
            return Err(TtsError::VoiceConfig {
                detail: "sample_rate must be greater than zero".to_string(),
            });
        }
        for symbol in ["^", "$"] {
            if !config.phoneme_id_map.contains_key(symbol) {
                return Err(TtsError::VoiceConfig {
                    detail: format!("phoneme_id_map is missing the '{symbol}' symbol"),
                });
            }
        }

        let language =
            Language::from_code(voice.language).ok_or_else(|| TtsError::VoiceConfig {
                detail: format!("unsupported language '{}'", voice.language),
            })?;
        ensure_espeak_data(espeak_dir, &[language])?;
        let espeak = EspeakNg::with_data_dir(&config.espeak.voice, espeak_dir)?;

        let environment = ORT_ENV.as_ref().ok_or(TtsError::OnnxInit)?;
        let session = Session::builder(environment)?.commit_from_file(model_path)?;

        Ok(Self {
            session: Mutex::new(session),
            espeak,
            config,
            voice: voice.clone(),
        })
    }

    /// The voice this engine was loaded for.
    pub fn voice(&self) -> &VoiceEntry {
        &self.voice
    }

    /// Phonemizes `text` and maps it to model symbol ids.
    fn phonemes_to_ids(&self, text: &str) -> Result<Vec<i64>, TtsError> {
        phonemes_to_ids(&self.espeak, &self.config, text)
    }

    /// Runs the VITS graph for one id sequence and returns the waveform.
    fn run_model(&self, ids: &[i64], speed: f32) -> Result<Vec<f32>, TtsError> {
        // A poisoned lock means another thread panicked mid-run; the ONNX
        // session itself is still valid, so recover it.
        let mut session = self
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let count = ids.len();
        let input = Tensor::<i64>::from_array(([1usize, count], ids.to_vec()))?;
        let input_lengths = Tensor::<i64>::from_array(([1usize], vec![count as i64]))?;
        // Piper convention: higher `length_scale` stretches the audio, so
        // a faster `speed` divides it.
        let scales = Tensor::<f32>::from_array((
            [3usize],
            vec![
                self.config.inference.noise_scale,
                self.config.inference.length_scale / speed,
                self.config.inference.noise_w,
            ],
        ))?;

        let mut inputs: Vec<(&str, SessionInputValue)> = vec![
            ("input", input.into()),
            ("input_lengths", input_lengths.into()),
            ("scales", scales.into()),
        ];
        // Multi-speaker voices (e.g. libritts-high, 904 speakers) expose a
        // speaker id input; synthesize with the first speaker.
        if session.inputs().iter().any(|outlet| outlet.name() == "sid") {
            let sid = Tensor::<i64>::from_array(([1usize], vec![0_i64]))?;
            inputs.push(("sid", sid.into()));
        }

        let outputs = session.run(inputs)?;
        let wave: Vec<f32> = if let Some(value) = outputs.get("output") {
            value.try_extract_tensor::<f32>()?.1.to_vec()
        } else {
            outputs
                .into_iter()
                .next()
                .ok_or_else(|| TtsError::VoiceConfig {
                    detail: "the voice model produced no output".to_string(),
                })?
                .1
                .try_extract_tensor::<f32>()?
                .1
                .to_vec()
        };
        Ok(wave)
    }
}

impl TtsEngine for LocalVitsEngine {
    fn synthesize(&self, request: &SynthRequest) -> Result<Synthesis, TtsError> {
        request.validate()?;
        if request.voice != self.voice.id {
            return Err(TtsError::UnknownVoice {
                id: request.voice.clone(),
            });
        }
        if request.sr != self.config.audio.sample_rate {
            return Err(TtsError::UnsupportedSampleRate {
                requested: request.sr,
                supported: self.config.audio.sample_rate,
            });
        }

        let started = Instant::now();
        let normalized = expand_numbers(&request.text, request.language);
        let chunks = chunk_text(&normalized, CHUNK_CHARS);

        let mut samples: Vec<f32> = Vec::new();
        for chunk in &chunks {
            let ids = self.phonemes_to_ids(chunk)?;
            let wave = self.run_model(&ids, request.speed)?;
            samples.extend_from_slice(&wave);
        }

        let sample_rate = self.config.audio.sample_rate;
        let mono = AudioBuffer::from_channels(vec![samples], sample_rate).map_err(|_| {
            TtsError::VoiceConfig {
                detail: format!("invalid sample rate {sample_rate} in voice configuration"),
            }
        })?;
        let audio = retarget_channels(&mono, request.channels)?;
        Ok(Synthesis {
            stats: stats_for(started.elapsed(), &audio),
            audio,
        })
    }
}

/// Phonemizes `text` with eSpeak NG and maps each symbol through the
/// configuration's `phoneme_id_map`, wrapping the sequence with the
/// beginning- and end-of-sentence symbols (`^` and `$`).
fn phonemes_to_ids(
    espeak: &EspeakNg,
    config: &VoiceConfig,
    text: &str,
) -> Result<Vec<i64>, TtsError> {
    let ipa = espeak.text_to_phonemes_phonemizer(text)?;
    Ok(map_symbols(config, &ipa))
}

/// Maps a phoneme string to model symbol ids. Symbols outside the map are
/// skipped, matching Piper.
fn map_symbols(config: &VoiceConfig, ipa: &str) -> Vec<i64> {
    let map = &config.phoneme_id_map;
    let mut ids = Vec::with_capacity(ipa.chars().count() + 2);
    ids.extend_from_slice(&map["^"]);
    for symbol in ipa.chars() {
        if let Some(symbol_ids) = map.get(&symbol.to_string()) {
            ids.extend_from_slice(symbol_ids);
        }
    }
    ids.extend_from_slice(&map["$"]);
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_json() -> String {
        r#"{
            "audio": {"sample_rate": 22050},
            "espeak": {"voice": "en-us"},
            "inference": {"noise_scale": 0.667, "length_scale": 1.0, "noise_w": 0.8},
            "phoneme_id_map": {"^": [1], "$": [2], "_": [0], " ": [3], "a": [5], "b": [6]}
        }"#
        .to_string()
    }

    /// Parses a voice configuration from raw JSON.
    fn parse_config(json: &str) -> Result<VoiceConfig, TtsError> {
        serde_json::from_str(json).map_err(|error| TtsError::VoiceConfig {
            detail: error.to_string(),
        })
    }

    #[test]
    fn config_parses_the_piper_schema() {
        let config = parse_config(&config_json()).expect("valid config");
        assert_eq!(config.audio.sample_rate, 22_050);
        assert_eq!(config.espeak.voice, "en-us");
        assert_eq!(config.inference.noise_scale, 0.667);
        assert_eq!(config.inference.length_scale, 1.0);
        assert_eq!(config.inference.noise_w, 0.8);
        assert_eq!(config.phoneme_id_map["a"], vec![5]);
    }

    #[test]
    fn config_rejects_missing_fields() {
        assert!(parse_config("{}").is_err());
        assert!(parse_config(r#"{"audio":{"sample_rate":22050}}"#).is_err());
        assert!(parse_config(
            r#"{"audio":{"sample_rate":22050},"espeak":{"voice":"en-us"},"inference":{"noise_scale":1.0,"length_scale":1.0,"noise_w":1.0}}"#
        )
        .is_err());
    }

    #[test]
    fn phoneme_mapping_wraps_with_bos_and_eos_and_skips_unknowns() {
        let config = parse_config(&config_json()).expect("valid config");
        assert_eq!(map_symbols(&config, "a b"), vec![1, 5, 3, 6, 2]);
        assert_eq!(map_symbols(&config, "ab\u{02c8}"), vec![1, 5, 6, 2]);
        assert_eq!(map_symbols(&config, ""), vec![1, 2]);
    }

    #[test]
    fn bundled_espeak_data_phonemizes_offline() {
        let dir = std::env::temp_dir().join("qinari-tts-phoneme-test-espeak");
        ensure_espeak_data(&dir, &[Language::En]).expect("espeak data");
        let espeak = EspeakNg::with_data_dir("en-us", &dir).expect("engine");
        let ipa = espeak
            .text_to_phonemes_phonemizer("Hello world.")
            .expect("phonemes");
        assert!(ipa.starts_with('h'), "unexpected phonemes: {ipa}");
        assert!(ipa.contains('w'), "unexpected phonemes: {ipa}");
    }

    #[test]
    fn ensure_espeak_data_is_idempotent_and_writes_markers() {
        let dir = std::env::temp_dir().join("qinari-tts-espeak-idempotent");
        ensure_espeak_data(&dir, &[Language::En, Language::Id]).expect("first install");
        assert!(dir.join(".qinari-espeak-en").exists());
        assert!(dir.join(".qinari-espeak-id").exists());
        // The engine can actually phonemize with the installed data.
        let espeak = EspeakNg::with_data_dir("id", &dir).expect("id engine");
        let ipa = espeak
            .text_to_phonemes_phonemizer("Selamat pagi.")
            .expect("id phonemes");
        assert!(!ipa.is_empty());
        ensure_espeak_data(&dir, &[Language::En, Language::Id]).expect("second install");
    }
}
