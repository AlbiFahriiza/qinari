//! Filesystem layout and the user configuration file.
//!
//! [`Paths`] resolves the XDG layout (`~/.config/qinari/config.toml`,
//! `~/.local/share/qinari/models`, `~/.local/state/qinari`) and the
//! per-voice file locations inside it. [`Config`] is the TOML document
//! those paths point at; every field has a default, so a missing or
//! partial file degrades to the documented defaults instead of failing.

use crate::xcode::QinariError;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::debug;

/// Resolved filesystem layout for one machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Configuration file: `~/.config/qinari/config.toml`.
    pub config_file: PathBuf,
    /// Downloaded voice models: `~/.local/share/qinari/models`.
    pub models_dir: PathBuf,
    /// eSpeak NG data: `<models>/espeak-data`.
    pub espeak_dir: PathBuf,
    /// State (log) directory: `~/.local/state/qinari`.
    pub state_dir: PathBuf,
}

impl Paths {
    /// Resolves the layout from the platform's project directories.
    ///
    /// # Errors
    ///
    /// Returns a setup error when the platform provides no home directory
    /// to hang the layout off.
    pub fn resolve() -> Result<Self, QinariError> {
        let dirs = ProjectDirs::from("", "", "qinari").ok_or_else(|| {
            QinariError::setup("could not determine the home directory for the qinari layout")
        })?;
        // macOS and Windows have no state convention; keep logs inside the
        // data tree there instead of failing.
        let state_dir = dirs
            .state_dir()
            .map_or_else(|| dirs.data_dir().join("state"), Path::to_path_buf);
        Ok(Self {
            config_file: dirs.config_dir().join("config.toml"),
            models_dir: dirs.data_dir().join("models"),
            espeak_dir: dirs.data_dir().join("models").join("espeak-data"),
            state_dir,
        })
    }

    /// The `.onnx` model file for the voice with this catalog id.
    pub fn voice_model(&self, id: &str) -> PathBuf {
        self.models_dir.join(id).join("voice.onnx")
    }

    /// The `.onnx.json` configuration sibling for the voice with this id.
    pub fn voice_config(&self, id: &str) -> PathBuf {
        self.models_dir.join(id).join("voice.onnx.json")
    }

    /// The log file: `<state>/qinari.log`.
    pub fn log_file(&self) -> PathBuf {
        self.state_dir.join("qinari.log")
    }
}

/// Output defaults (`[output]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    /// Target sample rate in hertz (8000..=192000).
    pub sr: u32,
    /// WAV bit depth: `16`, `24`, or `32f`.
    pub bit: String,
    /// MP3 bitrate in kbps: 128, 192, 256, or 320.
    pub mp3: u32,
    /// Run the mastering chain on every render.
    pub master: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            sr: 44_100,
            bit: "24".to_string(),
            mp3: 320,
            master: true,
        }
    }
}

/// Mastering defaults (`[mastering]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MasteringConfig {
    /// Integrated loudness target in LUFS.
    pub lufs: f32,
    /// Output ceiling in dBFS.
    pub ceiling: f32,
    /// Character chain: `transparent`, `warm`, or `punchy`.
    pub character: String,
}

impl Default for MasteringConfig {
    fn default() -> Self {
        Self {
            lufs: -16.0,
            ceiling: -1.0,
            character: "transparent".to_string(),
        }
    }
}

/// TTS defaults (`[tts]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    /// Engine preference: `auto`, `local`, or `cloud`.
    pub engine: String,
    /// Base URL of the OpenAI-compatible speech service.
    pub base: String,
    /// Model name the service expects.
    pub model: String,
    /// Voice name the service expects.
    pub voice: String,
    /// Catalog ids whose licenses the user already accepted.
    pub accepted_licenses: Vec<String>,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            engine: "auto".to_string(),
            base: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o-mini-tts".to_string(),
            voice: "alloy".to_string(),
            accepted_licenses: Vec::new(),
        }
    }
}

/// The user configuration document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Schema version of the file (currently 1).
    pub version: u32,
    /// Output defaults.
    pub output: OutputConfig,
    /// Mastering defaults.
    pub mastering: MasteringConfig,
    /// TTS defaults.
    pub tts: TtsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            output: OutputConfig::default(),
            mastering: MasteringConfig::default(),
            tts: TtsConfig::default(),
        }
    }
}

impl Config {
    /// Loads the configuration from `path`.
    ///
    /// A missing file yields the defaults; a file that cannot be read or
    /// parsed is a hard setup error that names the offending path.
    ///
    /// # Errors
    ///
    /// Returns a setup error when `path` exists but is unreadable or is
    /// not valid TOML for the documented shape.
    pub fn load(path: &Path) -> Result<Self, QinariError> {
        if !path.exists() {
            debug!(file = %path.display(), "no config file; using defaults");
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path).map_err(|error| {
            QinariError::setup(format!("could not read {}: {error}", path.display()))
        })?;
        toml::from_str(&text).map_err(|error| {
            QinariError::setup(format!("could not parse {}: {error}", path.display()))
        })
    }

    /// Writes the configuration to `path`, creating parent directories.
    ///
    /// # Errors
    ///
    /// Returns the filesystem error when a directory or the file itself
    /// cannot be created or written.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        fs::write(path, text)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("qinari-cli-config-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn defaults_are_the_documented_house_values() {
        let config = Config::default();
        assert_eq!(config.version, 1);
        assert_eq!(config.output.sr, 44_100);
        assert_eq!(config.output.bit, "24");
        assert_eq!(config.output.mp3, 320);
        assert!(config.output.master);
        assert_eq!(config.mastering.lufs, -16.0);
        assert_eq!(config.mastering.ceiling, -1.0);
        assert_eq!(config.mastering.character, "transparent");
        assert_eq!(config.tts.engine, "auto");
        assert_eq!(config.tts.base, "https://api.openai.com/v1");
        assert_eq!(config.tts.model, "gpt-4o-mini-tts");
        assert_eq!(config.tts.voice, "alloy");
        assert_eq!(config.tts.accepted_licenses.len(), 0);
    }

    #[test]
    fn missing_file_yields_defaults_without_error() {
        let dir = temp_dir("missing");
        let path = dir.join("does-not-exist.toml");
        let config = Config::load(&path).expect("missing file loads defaults");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn partial_file_fills_from_defaults() {
        let dir = temp_dir("partial");
        let path = dir.join("config.toml");
        fs::write(&path, "[mastering]\nlufs = -14.0\n").expect("write config");
        let config = Config::load(&path).expect("partial file loads");
        assert_eq!(config.mastering.lufs, -14.0);
        assert_eq!(config.output, OutputConfig::default());
        assert_eq!(config.tts, TtsConfig::default());
    }

    #[test]
    fn corrupt_file_is_a_hard_error_naming_the_path() {
        let dir = temp_dir("corrupt");
        let path = dir.join("config.toml");
        fs::write(&path, "not [valid toml").expect("write config");
        let error = Config::load(&path).expect_err("corrupt file must fail");
        assert_eq!(error.code, crate::xcode::ExitCode::Setup);
        let message = error.to_string();
        assert!(message.contains(&path.display().to_string()), "{message}");
        assert!(message.contains("could not parse"), "{message}");
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("nested").join("config.toml");
        let mut config = Config::default();
        config.output.sr = 48_000;
        config.output.master = false;
        config
            .tts
            .accepted_licenses
            .push("id_ID-news_tts-medium".to_string());
        config.save(&path).expect("save creates parents");
        let loaded = Config::load(&path).expect("round trip loads");
        assert_eq!(loaded, config);
    }

    #[test]
    fn voice_files_live_under_their_catalog_id() {
        let paths = Paths {
            config_file: PathBuf::from("/tmp/qinari-config/config.toml"),
            models_dir: PathBuf::from("/tmp/qinari-models"),
            espeak_dir: PathBuf::from("/tmp/qinari-models/espeak-data"),
            state_dir: PathBuf::from("/tmp/qinari-state"),
        };
        assert_eq!(
            paths.voice_model("en_US-libritts-high"),
            PathBuf::from("/tmp/qinari-models/en_US-libritts-high/voice.onnx")
        );
        assert_eq!(
            paths.voice_config("en_US-libritts-high"),
            PathBuf::from("/tmp/qinari-models/en_US-libritts-high/voice.onnx.json")
        );
        assert_eq!(
            paths.log_file(),
            PathBuf::from("/tmp/qinari-state/qinari.log")
        );
    }
}
