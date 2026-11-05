//! The shared finishing path every render flows through.
//!
//! [`finish_and_write`] takes a finished engine buffer and delivers it:
//! resample (only when rates differ) → mastering (or skip) → depth
//! finalize (TPDF dither) → encode to a `.qinari-tmp` sibling → atomic
//! rename. The returned [`RenderSummary`] feeds the log line and the
//! per-command output.

use crate::config::Config;
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
        Ok(Self {
            sr,
            depth,
            mp3,
            master: overrides.master.unwrap_or(config.output.master),
            settings: MasteringSettings {
                target_lufs: overrides.target_lufs.unwrap_or(config.mastering.lufs),
                ceiling_db: overrides.ceiling_db.unwrap_or(config.mastering.ceiling),
                character,
            },
        })
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
    /// Peak absolute sample value (linear).
    pub peak: f32,
    /// Integrated loudness before the mastering chain, when measurable.
    pub input_lufs: Option<f64>,
    /// Integrated loudness of the delivered file, when measurable.
    pub output_lufs: Option<f64>,
    /// Size of the file in bytes.
    pub size_bytes: u64,
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
/// and atomically renamed onto `out`.
///
/// # Errors
///
/// Returns an encode error for an unsupported output path or a failing
/// encoder, and a render error when resampling fails.
pub fn finish_and_write(
    buffer: &AudioBuffer,
    options: &RenderOptions,
    out: &Path,
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

    let summary = RenderSummary {
        out: out.to_path_buf(),
        duration_secs: finalized.duration(),
        sample_rate: finalized.sample_rate(),
        peak,
        input_lufs,
        output_lufs,
        size_bytes,
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
        let error =
            finish_and_write(&tone(44_100), &options, &out).expect_err("flac must be rejected");
        assert_eq!(error.code, crate::xcode::ExitCode::Encode);
        assert!(error.to_string().contains(".wav and .mp3 only"), "{error}");
        assert!(!out.exists());
    }

    #[test]
    fn finish_and_write_resamples_and_hits_the_loudness_target() {
        let options = RenderOptions::resolve(&OutputOverrides::default(), &Config::default())
            .expect("default options");
        let out = temp_path("resampled.wav");
        let summary = finish_and_write(&tone(48_000), &options, &out).expect("render succeeds");
        assert_eq!(summary.sample_rate, 44_100);
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
        let summary = finish_and_write(&tone(44_100), &options, &out).expect("render succeeds");
        assert!(!summary.mastered);
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
            peak: 0.98,
            input_lufs: Some(-23.4),
            output_lufs: Some(-16.1),
            size_bytes: 1_500_000,
            mastered: true,
        };
        let text = summary.describe();
        assert!(text.contains("qinari.wav"), "{text}");
        assert!(text.contains("44100 Hz"), "{text}");
        assert!(text.contains("-16.1 LUFS"), "{text}");
        assert!(text.contains("mastered"), "{text}");
        assert!(text.contains("MiB"), "{text}");
    }

    #[test]
    fn human_bytes_picks_readble_units() {
        assert_eq!(human_bytes(488), "488 B");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(3_145_728), "3.0 MiB");
    }
}
