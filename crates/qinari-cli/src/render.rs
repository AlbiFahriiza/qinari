//! The shared finishing path every render flows through.
//!
//! [`finish_and_write`] takes a finished engine buffer and delivers it:
//! resample (only when rates differ) → mastering (or skip) → depth
//! finalize (TPDF dither) → encode to a `.qinari-tmp` sibling → atomic
//! rename. The returned [`RenderSummary`] feeds the log line and the
//! per-command output.

use crate::config::Config;
use crate::download::file_sha256;
use crate::xcode::QinariError;
use qinari_audio::{
    finalize_for_depth, master, resample, validate_output_path, write_mp3, write_wav, AudioFormat,
    BitDepth, Character, MasteringSettings, Mp3Bitrate,
};
use qinari_dsp::buffer::{linear_to_db, AudioBuffer};
use qinari_dsp::lufs::integrated_lufs;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

/// CLI flags overriding the `[output]` / `[mastering]` config sections.
///
/// `None` means "not given on the command line"; the config value (or its
/// default) applies instead.
#[derive(Debug, Clone, Default)]
pub struct OutputOverrides {
    /// Sample rate in hertz.
    pub sr: Option<u32>,
    /// WAV bit depth as written: `16`, `24`, or `32f`.
    pub depth: Option<String>,
    /// MP3 bitrate in kbps.
    pub bitrate: Option<u32>,
    /// Whether to run the mastering chain.
    pub master: Option<bool>,
    /// Integrated loudness target in LUFS.
    pub target_lufs: Option<f32>,
    /// Output ceiling in dBFS.
    pub ceiling_db: Option<f32>,
    /// Character chain name.
    pub character: Option<String>,
}

/// Fully resolved delivery settings for one render.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderOptions {
    /// Sample rate the file is written at.
    pub sr: u32,
    /// WAV sample format.
    pub depth: BitDepth,
    /// MP3 bitrate.
    pub mp3: Mp3Bitrate,
    /// Whether the mastering chain runs.
    pub master: bool,
    /// Mastering chain settings.
    pub settings: MasteringSettings,
}

/// Parses `16` / `24` / `32f` into a [`BitDepth`].
fn parse_depth(text: &str) -> Result<BitDepth, String> {
    match text.trim() {
        "16" => Ok(BitDepth::Bits16),
        "24" => Ok(BitDepth::Bits24),
        "32f" | "float" => Ok(BitDepth::Float32),
        other => Err(format!("unknown bit depth {other:?} (use 16, 24, or 32f)")),
    }
}

/// Parses `128` / `192` / `256` / `320` into an [`Mp3Bitrate`].
fn parse_bitrate(kbps: u32) -> Result<Mp3Bitrate, String> {
    match kbps {
        128 => Ok(Mp3Bitrate::Kbps128),
        192 => Ok(Mp3Bitrate::Kbps192),
        256 => Ok(Mp3Bitrate::Kbps256),
        320 => Ok(Mp3Bitrate::Kbps320),
        other => Err(format!(
            "unknown mp3 bitrate {other} (use 128, 192, 256, or 320)"
        )),
    }
}

/// Parses `transparent` / `warm` / `punchy` into a [`Character`].
fn parse_character(text: &str) -> Result<Character, String> {
    match text.trim() {
        "transparent" => Ok(Character::Transparent),
        "warm" => Ok(Character::Warm),
        "punchy" => Ok(Character::Punchy),
        other => Err(format!(
            "unknown character {other:?} (use transparent, warm, or punchy)"
        )),
    }
}

impl RenderOptions {
    /// Resolves flags over config into the settings one render uses.
    ///
    /// Values from the command line are usage errors when malformed;
    /// values that come from the config file are setup errors, so a
    /// hand-edited config names itself in the message.
    ///
    /// # Errors
    ///
    /// Returns a setup error when the sample rate is outside
    /// 8000..=192000 Hz or a config-file value is malformed, and a usage
    /// error when a command-line value is malformed.
    pub fn resolve(overrides: &OutputOverrides, config: &Config) -> Result<Self, QinariError> {
        let sr = overrides.sr.unwrap_or(config.output.sr);
        if !(8000..=192_000).contains(&sr) {
            return Err(QinariError::setup(format!(
                "sample rate {sr} is out of range (8000..=192000 Hz)"
            )));
        }
        let depth = match &overrides.depth {
            Some(text) => {
                parse_depth(text).map_err(|m| QinariError::usage(format!("--depth: {m}")))?
            }
            None => parse_depth(&config.output.bit)
                .map_err(|m| QinariError::setup(format!("config [output] bit: {m}")))?,
        };
        let mp3 = match overrides.bitrate {
            Some(kbps) => {
                parse_bitrate(kbps).map_err(|m| QinariError::usage(format!("--bitrate: {m}")))?
            }
            None => parse_bitrate(config.output.mp3)
                .map_err(|m| QinariError::setup(format!("config [output] mp3: {m}")))?,
        };
        let character = match &overrides.character {
            Some(text) => parse_character(text)
                .map_err(|m| QinariError::usage(format!("--character: {m}")))?,
            None => parse_character(&config.mastering.character)
                .map_err(|m| QinariError::setup(format!("config [mastering] character: {m}")))?,
        };
        let target_lufs = if let Some(value) = overrides.target_lufs {
            validate_target_lufs(value)
                .map_err(|m| QinariError::usage(format!("--target-lufs: {m}")))?;
            value
        } else {
            validate_target_lufs(config.mastering.lufs)
                .map_err(|m| QinariError::setup(format!("config [mastering] lufs: {m}")))?;
            config.mastering.lufs
        };
        let ceiling_db = if let Some(value) = overrides.ceiling_db {
            validate_ceiling(value).map_err(|m| QinariError::usage(format!("--ceiling: {m}")))?;
            value
        } else {
            validate_ceiling(config.mastering.ceiling)
                .map_err(|m| QinariError::setup(format!("config [mastering] ceiling: {m}")))?;
            config.mastering.ceiling
        };
        Ok(Self {
            sr,
            depth,
            mp3,
            master: overrides.master.unwrap_or(config.output.master),
            settings: MasteringSettings {
                target_lufs,
                ceiling_db,
                character,
            },
        })
    }
}

/// A loudness target outside this range cannot be delivered sensibly.
fn validate_target_lufs(value: f32) -> Result<(), String> {
    if !(-30.0..=-5.0).contains(&value) {
        return Err(format!("{value} is out of range (-30.0..=-5.0 LUFS)"));
    }
    Ok(())
}

/// A ceiling above 0 dBFS would ask for clipped output; far below -3
/// audibly pumps.
fn validate_ceiling(value: f32) -> Result<(), String> {
    if !(-3.0..=0.0).contains(&value) {
        return Err(format!("{value} is out of range (-3.0..=0.0 dBFS)"));
    }
    Ok(())
}

/// The CLI spelling of a WAV bit depth (`16`, `24`, `32f`).
fn depth_name(depth: BitDepth) -> &'static str {
    match depth {
        BitDepth::Bits16 => "16",
        BitDepth::Bits24 => "24",
        BitDepth::Float32 => "32f",
    }
}

/// The lowercase container name of an output format.
fn format_name(format: AudioFormat) -> &'static str {
    match format {
        AudioFormat::Wav => "wav",
        AudioFormat::Mp3 => "mp3",
    }
}

/// What one finished render measured and wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderSummary {
    /// Path of the delivered file.
    pub out: PathBuf,
    /// Audio duration in seconds.
    pub duration_secs: f64,
    /// Sample rate of the file.
    pub sample_rate: u32,
    /// Channel count of the file.
    pub channels: usize,
    /// Container the file was written as.
    pub format: AudioFormat,
    /// WAV sample format the file carries (mp3 is lossy and has none).
    pub depth: BitDepth,
    /// Peak absolute sample value (linear).
    pub peak: f32,
    /// Integrated loudness before the mastering chain, when measurable.
    pub input_lufs: Option<f64>,
    /// Integrated loudness of the delivered file, when measurable.
    pub output_lufs: Option<f64>,
    /// Size of the file in bytes.
    pub size_bytes: u64,
    /// SHA-256 of the written file, lowercase hex.
    pub sha256: String,
    /// Wall-clock seconds the caller spent producing the audio (the
    /// render or synthesis call, before delivery).
    pub render_secs: f64,
    /// Whether the mastering chain ran.
    pub mastered: bool,
}

impl RenderSummary {
    /// One-line description for the terminal and the log.
    pub fn describe(&self) -> String {
        let lufs = |value: Option<f64>| {
            value.map_or("unmeasurable".to_string(), |v| format!("{v:.1} LUFS"))
        };
        format!(
            "{} — {:.1} s @ {} Hz, peak {:.1} dBFS, {} in / {} out, {}, {}",
            self.out.display(),
            self.duration_secs,
            self.sample_rate,
            linear_to_db(self.peak),
            lufs(self.input_lufs),
            lufs(self.output_lufs),
            human_bytes(self.size_bytes),
            if self.mastered {
                "mastered"
            } else {
                "unmastered"
            },
        )
    }

    /// Real-time factor of the render: `render_secs / duration_secs`.
    ///
    /// Values below 1.0 mean the audio was produced faster than it plays
    /// back. Unmeasurable (zero duration or zero clock) yields 0.0.
    pub fn rtf(&self) -> f64 {
        if self.duration_secs > 0.0 && self.render_secs > 0.0 {
            self.render_secs / self.duration_secs
        } else {
            0.0
        }
    }

    /// The machine-readable (`--json`) form of this render, one JSON
    /// object. `engine` and `voice` name the TTS lane that produced the
    /// audio (`None` for procedural renders).
    ///
    /// # Errors
    ///
    /// Returns an internal error when serialization fails (it cannot for
    /// this value shape).
    pub fn to_json(
        &self,
        engine: Option<&str>,
        voice: Option<&str>,
    ) -> Result<String, QinariError> {
        let report = serde_json::json!({
            "path": self.out,
            "format": format_name(self.format),
            "duration_s": self.duration_secs,
            "sample_rate": self.sample_rate,
            "channels": self.channels,
            "bits": if self.format == AudioFormat::Wav {
                serde_json::Value::from(depth_name(self.depth))
            } else {
                serde_json::Value::Null
            },
            "sha256": self.sha256,
            "size_bytes": self.size_bytes,
            "mastered": self.mastered,
            "engine": engine,
            "voice": voice,
            "rtf": self.rtf(),
        });
        serde_json::to_string(&report).map_err(|error| {
            QinariError::internal(format!("could not serialize the render report: {error}"))
        })
    }
}

/// `488 B`, `1.2 KiB`, `3.4 MiB`.
fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value >= KIB * KIB {
        format!("{:.1} MiB", value / (KIB * KIB))
    } else if value >= KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// Delivers `buffer` as a finished file at `out`.
///
/// The output path must end in `.wav` or `.mp3`. The buffer is resampled
/// to `options.sr` when the rates differ, mastered (or measured as-is),
/// dithered for the target depth, encoded into a `.qinari-tmp` sibling,
/// and atomically renamed onto `out`. `render_secs` is the wall-clock time
/// the caller spent producing `buffer`; it is reported verbatim and as the
/// real-time factor in [`RenderSummary::to_json`].
///
/// # Errors
///
/// Returns an encode error for an unsupported output path or a failing
/// encoder, a render error when resampling fails, and an internal error
/// when the written file cannot be re-read for its digest.
pub fn finish_and_write(
    buffer: &AudioBuffer,
    options: &RenderOptions,
    out: &Path,
    render_secs: f64,
) -> Result<RenderSummary, QinariError> {
    let format = validate_output_path(out)?;
    let resampled = resample(buffer, options.sr).map_err(|error| {
        QinariError::render(format!("resampling to {} Hz failed: {error}", options.sr))
    })?;

    let (delivered, input_lufs, output_lufs, peak) = if options.master {
        let (mastered, report) = master(&resampled, &options.settings);
        (
            mastered,
            report.input_lufs,
            report.output_lufs,
            report.output_peak,
        )
    } else {
        let unmeasured = integrated_lufs(&resampled);
        let peak = resampled.peak();
        (resampled, unmeasured, unmeasured, peak)
    };

    let finalized = finalize_for_depth(&delivered, options.depth);
    let tmp = out.with_extension("qinari-tmp");
    match format {
        AudioFormat::Wav => write_wav(&tmp, &finalized, options.depth),
        AudioFormat::Mp3 => write_mp3(&tmp, &finalized, options.mp3),
    }
    .map_err(|error| QinariError::encode(format!("could not encode {}: {error}", tmp.display())))?;
    fs::rename(&tmp, out).map_err(|error| {
        QinariError::encode(format!(
            "could not move {} into place: {error}",
            tmp.display()
        ))
    })?;
    let size_bytes = fs::metadata(out)
        .map_err(|error| QinariError::encode(format!("could not stat {}: {error}", out.display())))?
        .len();
    let sha256 = file_sha256(out).map_err(|error| {
        QinariError::internal(format!("could not hash {}: {error}", out.display()))
    })?;

    let summary = RenderSummary {
        out: out.to_path_buf(),
        duration_secs: finalized.duration(),
        sample_rate: finalized.sample_rate(),
        channels: finalized.num_channels(),
        format,
        depth: options.depth,
        peak,
        input_lufs,
        output_lufs,
        size_bytes,
        sha256,
        render_secs,
        mastered: options.master,
    };
    info!(summary = summary.describe(), "render complete");
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qinari_dsp::buffer::AudioBuffer;

    /// A 1-second sine pair at 48 kHz, loud enough to measure.
    fn tone(sr: u32) -> AudioBuffer {
        let frames = sr as usize;
        let left: Vec<f32> = (0..frames)
            .map(|n| 0.5 * (2.0 * std::f32::consts::PI * 997.0 * n as f32 / sr as f32).sin())
            .collect();
        AudioBuffer::from_channels(vec![left.clone(), left], sr).expect("valid buffer")
    }

    fn temp_path(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("qinari-cli-render-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join(name)
    }

    #[test]
    fn resolve_validates_the_sample_rate_as_a_setup_error() {
        let config = Config::default();
        for bad_sr in [7_999u32, 192_001, 0] {
            let overrides = OutputOverrides {
                sr: Some(bad_sr),
                ..OutputOverrides::default()
            };
            let error = RenderOptions::resolve(&overrides, &config)
                .expect_err("out-of-range rate must fail");
            assert_eq!(error.code, crate::xcode::ExitCode::Setup);
            assert!(error.to_string().contains("8000..=192000"), "{error}");
        }
    }

    #[test]
    fn resolve_maps_flag_and_config_mistakes_differently() {
        let mut config = Config::default();
        let bad_flag = OutputOverrides {
            depth: Some("12".to_string()),
            ..OutputOverrides::default()
        };
        let error = RenderOptions::resolve(&bad_flag, &config).expect_err("bad --depth");
        assert_eq!(error.code, crate::xcode::ExitCode::Usage);
        assert!(error.to_string().contains("--depth"), "{error}");

        config.output.bit = "12".to_string();
        let error = RenderOptions::resolve(&OutputOverrides::default(), &config)
            .expect_err("bad config depth");
        assert_eq!(error.code, crate::xcode::ExitCode::Setup);
        assert!(error.to_string().contains("config [output] bit"), "{error}");
    }

    #[test]
    fn resolve_accepts_every_documented_value() {
        let config = Config::default();
        let overrides = OutputOverrides {
            sr: Some(48_000),
            depth: Some("32f".to_string()),
            bitrate: Some(128),
            master: Some(false),
            target_lufs: Some(-14.0),
            ceiling_db: Some(-0.5),
            character: Some("punchy".to_string()),
        };
        let options = RenderOptions::resolve(&overrides, &config).expect("valid overrides");
        assert_eq!(options.sr, 48_000);
        assert_eq!(options.depth, BitDepth::Float32);
        assert_eq!(options.mp3, Mp3Bitrate::Kbps128);
        assert!(!options.master);
        assert_eq!(options.settings.target_lufs, -14.0);
        assert_eq!(options.settings.ceiling_db, -0.5);
        assert_eq!(options.settings.character, Character::Punchy);

        let defaults =
            RenderOptions::resolve(&OutputOverrides::default(), &config).expect("defaults resolve");
        assert_eq!(defaults.sr, 44_100);
        assert_eq!(defaults.depth, BitDepth::Bits24);
        assert_eq!(defaults.mp3, Mp3Bitrate::Kbps320);
        assert!(defaults.master);
        assert_eq!(defaults.settings.character, Character::Transparent);
    }

    #[test]
    fn finish_and_write_rejects_flac_paths_as_encode_errors() {
        let options = RenderOptions::resolve(&OutputOverrides::default(), &Config::default())
            .expect("default options");
        let out = temp_path("out.flac");
        let error = finish_and_write(&tone(44_100), &options, &out, 0.1)
            .expect_err("flac must be rejected");
        assert_eq!(error.code, crate::xcode::ExitCode::Encode);
        assert!(error.to_string().contains(".wav and .mp3 only"), "{error}");
        assert!(!out.exists());
    }

    #[test]
    fn finish_and_write_resamples_and_hits_the_loudness_target() {
        let options = RenderOptions::resolve(&OutputOverrides::default(), &Config::default())
            .expect("default options");
        let out = temp_path("resampled.wav");
        let summary =
            finish_and_write(&tone(48_000), &options, &out, 0.25).expect("render succeeds");
        assert_eq!(summary.sample_rate, 44_100);
        assert_eq!(summary.channels, 2);
        assert_eq!(summary.format, AudioFormat::Wav);
        assert_eq!(summary.depth, BitDepth::Bits24);
        assert_eq!(summary.sha256.len(), 64);
        assert!(summary
            .sha256
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(
            summary.sha256,
            file_sha256(&out).expect("digest of the file")
        );
        assert!(
            (summary.duration_secs - 1.0).abs() < 0.05,
            "{}",
            summary.duration_secs
        );
        let measured = summary.output_lufs.expect("a 1 s tone is measurable");
        let target = f64::from(options.settings.target_lufs);
        assert!(
            (measured - target).abs() <= 1.5,
            "target {target}, measured {measured:.2}"
        );
        assert!(summary.mastered);
        assert!(out.exists());
        assert!(
            !out.with_extension("qinari-tmp").exists(),
            "no tmp left behind"
        );
    }

    #[test]
    fn finish_and_write_can_skip_mastering() {
        let overrides = OutputOverrides {
            master: Some(false),
            ..OutputOverrides::default()
        };
        let options = RenderOptions::resolve(&overrides, &Config::default()).expect("options");
        let out = temp_path("unmastered.mp3");
        let summary =
            finish_and_write(&tone(44_100), &options, &out, 0.1).expect("render succeeds");
        assert!(!summary.mastered);
        assert_eq!(summary.format, AudioFormat::Mp3);
        // An unmastered 48 kHz-tone bed still reports its measured state.
        assert_eq!(summary.input_lufs, summary.output_lufs);
        assert!(summary.size_bytes > 0);
    }

    #[test]
    fn summary_describe_mentions_every_component() {
        let summary = RenderSummary {
            out: PathBuf::from("/tmp/qinari.wav"),
            duration_secs: 10.0,
            sample_rate: 44_100,
            channels: 2,
            format: AudioFormat::Wav,
            depth: BitDepth::Bits24,
            peak: 0.98,
            input_lufs: Some(-23.4),
            output_lufs: Some(-16.1),
            size_bytes: 1_500_000,
            sha256: "a".repeat(64),
            render_secs: 2.5,
            mastered: true,
        };
        let text = summary.describe();
        assert!(text.contains("qinari.wav"), "{text}");
        assert!(text.contains("44100 Hz"), "{text}");
        assert!(text.contains("-16.1 LUFS"), "{text}");
        assert!(text.contains("mastered"), "{text}");
        assert!(text.contains("MiB"), "{text}");
        // The real-time factor: 2.5 s of work for 10 s of audio = 0.25.
        assert!((summary.rtf() - 0.25).abs() < 1e-9);
    }

    #[test]
    fn summary_json_carries_the_documented_fields() {
        let wav = RenderSummary {
            out: PathBuf::from("out.wav"),
            duration_secs: 4.0,
            sample_rate: 44_100,
            channels: 1,
            format: AudioFormat::Wav,
            depth: BitDepth::Bits16,
            peak: 0.9,
            input_lufs: Some(-20.0),
            output_lufs: Some(-16.0),
            size_bytes: 352_800,
            sha256: "ab".repeat(32),
            render_secs: 1.0,
            mastered: true,
        };
        let text = wav
            .to_json(Some("supertonic"), Some("supertonic-M1"))
            .expect("json");
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["path"], "out.wav");
        assert_eq!(value["format"], "wav");
        assert_eq!(value["duration_s"], 4.0);
        assert_eq!(value["sample_rate"], 44_100);
        assert_eq!(value["channels"], 1);
        assert_eq!(value["bits"], "16");
        assert_eq!(value["sha256"], "ab".repeat(32));
        assert_eq!(value["size_bytes"], 352_800);
        assert_eq!(value["mastered"], true);
        assert_eq!(value["engine"], "supertonic");
        assert_eq!(value["voice"], "supertonic-M1");
        assert!((value["rtf"].as_f64().expect("number") - 0.25).abs() < 1e-9);

        let mp3 = RenderSummary {
            out: PathBuf::from("out.mp3"),
            format: AudioFormat::Mp3,
            duration_secs: 2.0,
            ..wav
        };
        let text = mp3.to_json(None, None).expect("json");
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["bits"], serde_json::Value::Null);
        assert_eq!(value["engine"], serde_json::Value::Null);
        assert_eq!(value["voice"], serde_json::Value::Null);
        assert_eq!(value["format"], "mp3");
    }

    #[test]
    fn rtf_is_zero_when_not_measurable() {
        let summary = RenderSummary {
            duration_secs: 0.0,
            render_secs: 1.0,
            ..summary_fixture()
        };
        assert_eq!(summary.rtf(), 0.0);
        let summary = RenderSummary {
            render_secs: 0.0,
            ..summary_fixture()
        };
        assert_eq!(summary.rtf(), 0.0);
    }

    /// A structurally valid summary for field-override tests.
    fn summary_fixture() -> RenderSummary {
        RenderSummary {
            out: PathBuf::from("out.wav"),
            duration_secs: 1.0,
            sample_rate: 44_100,
            channels: 1,
            format: AudioFormat::Wav,
            depth: BitDepth::Bits24,
            peak: 0.5,
            input_lufs: None,
            output_lufs: None,
            size_bytes: 100,
            sha256: "0".repeat(64),
            render_secs: 0.5,
            mastered: false,
        }
    }

    #[test]
    fn human_bytes_picks_readble_units() {
        assert_eq!(human_bytes(488), "488 B");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(3_145_728), "3.0 MiB");
    }
}
