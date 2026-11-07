//! Command handlers, their argument structs, and shared CLI parsing.
//!
//! Every subcommand of the `qinari` binary lives here as one function over
//! a [`RunContext`]; the argument structs are colocated with the handlers
//! that read them. The module also holds the small parsing helpers
//! (`--duration`, `--set`, seeds) and the error mappers that turn library
//! errors into prepared messages with the right exit code.

use crate::config::{Config, Paths};
use crate::download::{download, DownloadError, DownloadSpec};
use crate::render::{finish_and_write, OutputOverrides, RenderOptions, RenderSummary};
use crate::xcode::QinariError;
use clap::{Args, Parser};
use dialoguer::Confirm;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use qinari_audio::decode_audio;
use qinari_dsp::buffer::linear_to_db;
use qinari_dsp::lufs::integrated_lufs;
use qinari_dsp::rng::Seed;
use qinari_music::{render as render_track, MusicError, TrackRequest, STYLES};
use qinari_sfx::params::ParamSpec;
use qinari_sfx::{
    ambient_presets, recipes, render as render_sfx, render_ambient, AmbientError, RenderRequest,
    SfxError,
};
use qinari_tts::cloud::{CloudConfig, OpenAiCompatibleEngine};
use qinari_tts::engine::{ProsodyMode, StyleSpec, TtsEngine, DEFAULT_STEPS, MAX_STEPS, MIN_STEPS};
use qinari_tts::supertonic::SupertonicEngine;
use qinari_tts::voices::{
    default_voice, find_by_style_name, find_voice, voices, LicenseClass, VoiceEntry,
};
use qinari_tts::{Language, SynthRequest, TtsError};
use qinari_verify::dnsmos::{self, DnsmosScores, DNSMOS_ASSETS};
use qinari_verify::report::FeatureReport;
use qinari_verify::VerifyError;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use strsim::levenshtein;

/// Per-run state shared by every handler.
pub struct RunContext {
    /// Resolved filesystem layout.
    pub paths: Paths,
    /// Loaded configuration (defaults when no file exists).
    pub config: Config,
    /// `--quiet`: suppress informational output, never errors.
    pub quiet: bool,
}

/// Flags overriding the `[output]` and `[mastering]` config sections.
///
/// Shared by every command that writes audio.
#[derive(Debug, Clone, Default, Args)]
pub struct OverrideFlags {
    /// Sample rate in hertz (8000..=192000).
    #[arg(long)]
    pub sr: Option<u32>,
    /// WAV bit depth: 16, 24, or 32f.
    #[arg(long)]
    pub depth: Option<String>,
    /// MP3 bitrate in kbps: 128, 192, 256, or 320.
    #[arg(long)]
    pub bitrate: Option<u32>,
    /// Force the mastering chain on for this render.
    #[arg(long)]
    pub master: bool,
    /// Force the mastering chain off for this render.
    #[arg(long)]
    pub no_master: bool,
    /// Mastering loudness target in LUFS.
    #[arg(long)]
    pub target_lufs: Option<f32>,
    /// Output ceiling in dBFS.
    #[arg(long)]
    pub ceiling: Option<f32>,
    /// Mastering character: transparent, warm, or punchy.
    #[arg(long)]
    pub character: Option<String>,
}

/// `qinari setup`: install voices and prepare the workspace.
#[derive(Debug, Clone, Parser)]
#[command(allow_negative_numbers = true)]
pub struct SetupArgs {
    /// List the downloadable voices instead of setting up.
    #[arg(long)]
    pub list_voices: bool,
    /// Install only these voice ids (repeatable).
    #[arg(long = "voice")]
    pub voices: Vec<String>,
    /// Non-interactive: never prompt, assume the default answer.
    #[arg(long)]
    pub yes: bool,
    /// Accept the restricted (OpenRAIL-M) voice license without prompting.
    #[arg(long)]
    pub accept_license: bool,
}

/// `qinari sfx`: render one of the 29 procedural recipes.
#[derive(Debug, Clone, Parser)]
#[command(allow_negative_numbers = true)]
pub struct SfxArgs {
    /// Recipe id (see `qinari sfx --list`).
    pub recipe: Option<String>,
    /// List the recipes instead of rendering.
    #[arg(long)]
    pub list: bool,
    /// Output file (.wav or .mp3); default out.wav, out-NNN.ext with --batch.
    #[arg(long, short = 'o')]
    pub out: Option<PathBuf>,
    /// Duration: bare seconds, or with an ms/s/m/h unit.
    #[arg(long, short = 'd', default_value = "3")]
    pub duration: String,
    /// Seed text; the same seed renders identical audio. Empty = time-based.
    #[arg(long)]
    pub seed: Option<String>,
    /// Recipe parameter as NAME=VALUE (repeatable).
    #[arg(long = "set")]
    pub sets: Vec<String>,
    /// Render N variations as out-NNN.ext (seed plus run index).
    #[arg(long)]
    pub batch: Option<u32>,
    /// Channels: 1 (mono) or 2 (stereo).
    #[arg(long, default_value_t = 2)]
    pub channels: u16,
    /// Print machine-readable output (one JSON object per file).
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub overrides: OverrideFlags,
}

/// `qinari ambient`: render one of the five ambient beds.
#[derive(Debug, Clone, Parser)]
#[command(allow_negative_numbers = true)]
pub struct AmbientArgs {
    /// Preset id (see `qinari ambient --list`).
    pub preset: Option<String>,
    /// List the presets instead of rendering.
    #[arg(long)]
    pub list: bool,
    /// Output file (.wav or .mp3); default out.wav.
    #[arg(long, short = 'o')]
    pub out: Option<PathBuf>,
    /// Duration: bare seconds, or with an ms/s/m/h unit.
    #[arg(long, short = 'd', default_value = "60")]
    pub duration: String,
    /// Seed text; the same seed renders identical audio. Empty = time-based.
    #[arg(long)]
    pub seed: Option<String>,
    /// Channels: 1 (mono) or 2 (stereo).
    #[arg(long, default_value_t = 2)]
    pub channels: u16,
    /// Print machine-readable output.
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub overrides: OverrideFlags,
}

/// `qinari music`: render one of the five composed styles.
#[derive(Debug, Clone, Parser)]
#[command(allow_negative_numbers = true)]
pub struct MusicArgs {
    /// Style id (see `qinari music --list`).
    pub style: Option<String>,
    /// List the styles instead of rendering.
    #[arg(long)]
    pub list: bool,
    /// Output file (.wav or .mp3); default out.wav.
    #[arg(long, short = 'o')]
    pub out: Option<PathBuf>,
    /// Duration: bare seconds, or with an ms/s/m/h unit.
    #[arg(long, short = 'd', default_value = "30")]
    pub duration: String,
    /// Tempo in beats per minute; clamped into the style's range.
    #[arg(long)]
    pub bpm: Option<f32>,
    /// Seed text; the same seed renders identical audio. Empty = time-based.
    #[arg(long)]
    pub seed: Option<String>,
    /// Channels: 1 (mono) or 2 (stereo).
    #[arg(long, default_value_t = 2)]
    pub channels: u16,
    /// Print machine-readable output.
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub overrides: OverrideFlags,
}

/// `qinari tts`: speak text offline or through a cloud service.
#[derive(Debug, Clone, Parser)]
#[command(allow_negative_numbers = true)]
pub struct TtsArgs {
    /// Text to speak (omit to read from `--file`, default stdin).
    pub text: Option<String>,
    /// Read the text from this file; "-" means stdin.
    #[arg(long)]
    pub file: Option<String>,
    /// Language of the text: id or en.
    #[arg(long)]
    pub lang: String,
    /// Voice catalog id; default is the language's bundled voice.
    #[arg(long)]
    pub voice: Option<String>,
    /// Engine: auto, local, or cloud (default from config).
    #[arg(long)]
    pub engine: Option<String>,
    /// Speed factor 0.5..=2.0 (local voices clamp to 0.7..=2.0).
    #[arg(long, default_value_t = 1.0)]
    pub speed: f32,
    /// Denoising steps 1..=100; more steps trade speed for quality.
    #[arg(long, default_value_t = DEFAULT_STEPS)]
    pub steps: u8,
    /// Seed text; the same seed renders the same audio. Empty = time-based.
    #[arg(long)]
    pub seed: Option<String>,
    /// Style preset name (M1..M5, F1..F5) or full voice id.
    #[arg(long)]
    pub style: Option<String>,
    /// Custom style file (Supertonic voice-style JSON) instead of a preset.
    #[arg(long)]
    pub style_file: Option<PathBuf>,
    /// Prosody post-chain: natural (pauses + jitter) or flat (SDK behavior).
    #[arg(long, default_value = "natural")]
    pub prosody: String,
    /// Speak markup tags verbatim instead of parsing them.
    #[arg(long)]
    pub no_markup: bool,
    /// Output container: wav or mp3 (overrides the -o extension).
    #[arg(long)]
    pub format: Option<String>,
    /// Channels: 1 (mono) or 2 (stereo).
    #[arg(long, default_value_t = 1)]
    pub channels: u16,
    /// Output file (.wav or .mp3); default out.wav.
    #[arg(long, short = 'o')]
    pub out: Option<PathBuf>,
    /// Print machine-readable output.
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub overrides: OverrideFlags,
}

/// `qinari master`: re-master an existing file.
#[derive(Debug, Clone, Parser)]
#[command(allow_negative_numbers = true)]
pub struct MasterArgs {
    /// Input file (.wav or .mp3).
    pub input: PathBuf,
    /// Output file (.wav or .mp3); default <input stem>-mastered.wav.
    #[arg(long, short = 'o')]
    pub out: Option<PathBuf>,
    /// Print machine-readable output.
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub overrides: OverrideFlags,
}

/// `qinari analyze`: measure a file.
#[derive(Debug, Clone, Parser)]
#[command(allow_negative_numbers = true)]
pub struct AnalyzeArgs {
    /// Input file (.wav or .mp3).
    pub file: PathBuf,
    /// Print the speech section: voicing, F0 statistics, pauses.
    #[arg(long)]
    pub speech: bool,
    /// Score the file with the vendored DNSMOS models (MOS estimates).
    #[arg(long)]
    pub mos: bool,
    /// Print machine-readable output (always includes the speech
    /// section; `dnsmos` appears with `--mos`).
    #[arg(long)]
    pub json: bool,
}

/// `qinari bench`: time reference renders.
#[derive(Debug, Clone, Parser)]
#[command(allow_negative_numbers = true)]
pub struct BenchArgs {
    /// Print machine-readable output.
    #[arg(long)]
    pub json: bool,
}

/// `qinari info`: show version, layout, config, and voices.
#[derive(Debug, Clone, Parser)]
#[command(allow_negative_numbers = true)]
pub struct InfoArgs {
    /// Print machine-readable output.
    #[arg(long)]
    pub json: bool,
}

/// `qinari completions`: print shell completion scripts.
#[derive(Debug, Clone, Parser)]
#[command(allow_negative_numbers = true)]
pub struct CompletionsArgs {
    /// Shell to generate completions for.
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

/// `qinari doctor`: scriptable health check.
#[derive(Debug, Clone, Parser)]
#[command(allow_negative_numbers = true)]
pub struct DoctorArgs {}

/// Parses a duration: bare seconds or `ms` / `s` / `m` / `h` units.
///
/// # Errors
///
/// Returns a usage error for anything that is not a positive, finite
/// number of seconds.
pub fn parse_duration(text: &str) -> Result<f64, QinariError> {
    let bad = |reason: &str| {
        QinariError::usage(format!(
            "could not parse duration {text:?} ({reason}); examples: 10, 10s, 250ms, 2m, 1h"
        ))
    };
    let trimmed = text.trim();
    let split = trimmed
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(trimmed.len());
    let (number_text, unit_text) = trimmed.split_at(split);
    let number: f64 = number_text
        .parse()
        .map_err(|_| bad("expected a number before the unit"))?;
    let seconds = match unit_text.to_ascii_lowercase().as_str() {
        "" | "s" => number,
        "ms" => number / 1000.0,
        "m" => number * 60.0,
        "h" => number * 3600.0,
        other => return Err(bad(&format!("unknown unit {other:?}"))),
    };
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(bad("expected a positive duration"));
    }
    Ok(seconds)
}

/// Parses `NAME=VALUE` pairs from repeated `--set` flags.
///
/// # Errors
///
/// Returns a usage error for a missing `=`, an empty name, or a
/// non-numeric value. Unknown *names* are rejected later by the recipe's
/// parameter table, with a "did you mean" hint.
pub fn parse_sets(items: &[String]) -> Result<Vec<(String, f64)>, QinariError> {
    items
        .iter()
        .map(|item| {
            let bad = |reason: &str| QinariError::usage(format!("--set {item:?}: {reason}"));
            let (name, value) = item
                .split_once('=')
                .ok_or_else(|| bad("expected NAME=VALUE"))?;
            if name.trim().is_empty() {
                return Err(bad("the parameter name is empty"));
            }
            let value: f64 = value
                .parse()
                .map_err(|_| bad("the value must be a number"))?;
            Ok((name.trim().to_string(), value))
        })
        .collect()
}

/// Resolves a `--seed` value into the u64 the engines take.
///
/// Empty or absent seeds fall back to the current time (each invocation
/// differs); numeric seeds pass through unchanged; anything else is hashed
/// with the stable text seed, so `--seed storm-night` is reproducible.
pub fn seed_from(seed: Option<&str>) -> u64 {
    let text = seed.map(str::trim).filter(|t| !t.is_empty());
    let Some(text) = text else {
        return SystemTime::now()
            .duration_since(UNIX_EPOCH)
            // A clock before 1970 cannot yield a useful nanosecond stamp;
            // fall back to a fixed, documented value.
            .map_or(1, |since| since.as_nanos() as u64);
    };
    if let Ok(raw) = text.parse::<u64>() {
        return raw;
    }
    Seed::from_text(text).0
}

/// Levenshtein-closest candidate within distance 3, first match wins ties.
pub fn suggest(typo: &str, candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .copied()
        .filter(|candidate| levenshtein(typo, candidate) <= 3)
        .min_by_key(|candidate| levenshtein(typo, candidate))
        .map(str::to_string)
}

/// The voice catalog ids, for suggestions.
fn voice_ids() -> Vec<&'static str> {
    voices().iter().map(|voice| voice.id).collect()
}

/// The music style ids, for suggestions.
fn style_ids() -> Vec<&'static str> {
    STYLES.iter().map(|style| style.id).collect()
}

/// Maps an sfx failure to a prepared message.
///
/// Unknown recipes and bad parameters are usage errors (the library's
/// message already carries the "did you mean" hint); a failed render is
/// a render error.
pub fn sfx_error(error: &SfxError) -> QinariError {
    match error {
        SfxError::UnknownRecipe { .. }
        | SfxError::BadParameter(_)
        | SfxError::BadDuration { .. }
        | SfxError::BadChannels { .. }
        | SfxError::BadSampleRate { .. } => QinariError::usage(error.to_string()),
        SfxError::Render(_) => QinariError::render(error.to_string()),
    }
}

/// Maps an ambient failure; sfx layer failures recurse through
/// [`sfx_error`].
pub fn ambient_error(error: &AmbientError) -> QinariError {
    match error {
        AmbientError::UnknownPreset { .. } | AmbientError::BadDuration { .. } => {
            QinariError::usage(error.to_string())
        }
        AmbientError::Sfx(inner) => sfx_error(inner),
        AmbientError::Render(_) => QinariError::render(error.to_string()),
    }
}

/// Maps a music failure; unknown styles gain a suggestion here because the
/// library error carries none.
pub fn music_error(error: &MusicError) -> QinariError {
    match error {
        MusicError::UnknownStyle { style } => {
            let hint = suggest(style, &style_ids()).map_or_else(
                || format!("unknown style {style:?}"),
                |close| format!("unknown style {style:?}; did you mean {close:?}?"),
            );
            QinariError::usage(hint)
        }
        MusicError::BadDuration { .. } | MusicError::BadChannels { .. } => {
            QinariError::usage(error.to_string())
        }
    }
}

/// Maps a TTS failure; every engine failure is a tts error.
pub fn tts_error(error: &TtsError) -> QinariError {
    QinariError::tts(error.to_string())
}

/// Maps a download failure.
pub fn download_error(error: &DownloadError) -> QinariError {
    QinariError::download(error.to_string())
}

/// Resolves `--master` / `--no-master` into one override.
fn master_flag(master: bool, no_master: bool) -> Result<Option<bool>, QinariError> {
    match (master, no_master) {
        (false, false) => Ok(None),
        (true, false) => Ok(Some(true)),
        (false, true) => Ok(Some(false)),
        (true, true) => Err(QinariError::usage(
            "--master and --no-master contradict each other",
        )),
    }
}

/// Converts the clap flag struct into the render module's override struct.
fn overrides_from(flags: &OverrideFlags) -> Result<OutputOverrides, QinariError> {
    Ok(OutputOverrides {
        sr: flags.sr,
        depth: flags.depth.clone(),
        bitrate: flags.bitrate,
        master: master_flag(flags.master, flags.no_master)?,
        target_lufs: flags.target_lufs,
        ceiling_db: flags.ceiling,
        character: flags.character.clone(),
    })
}

/// Reports one finished render: the machine-readable object goes to
/// stdout under `--json` (with the human summary on stderr, so stdout stays
/// parseable); without `--json` the human summary goes to stdout unless
/// `--quiet` is set. `engine`/`voice` name the TTS lane for `tts` renders.
///
/// # Errors
///
/// Returns an internal error when the JSON form cannot be serialized.
fn report_render(
    ctx: &RunContext,
    json: bool,
    summary: &RenderSummary,
    engine: Option<&str>,
    voice: Option<&str>,
) -> Result<(), QinariError> {
    if json {
        println!("{}", summary.to_json(engine, voice)?);
        if !ctx.quiet {
            eprintln!("{}", summary.describe());
        }
    } else if !ctx.quiet {
        println!("{}", summary.describe());
    }
    Ok(())
}

/// Forces the `--format` container onto the output path.
///
/// `wav`/`mp3` rewrite the path's extension (a bare `out` becomes
/// `out.wav`); anything else is a usage error. `None` passes the path
/// through unchanged.
///
/// # Errors
///
/// Returns a usage error for a container other than `wav` or `mp3`.
fn apply_format(out: PathBuf, format: Option<&str>) -> Result<PathBuf, QinariError> {
    let Some(format) = format else {
        return Ok(out);
    };
    match format {
        "wav" | "mp3" => Ok(out.with_extension(format)),
        other => Err(QinariError::usage(format!(
            "--format must be wav or mp3, got {other:?}"
        ))),
    }
}

/// Parses `--prosody` into the library's mode.
///
/// # Errors
///
/// Returns a usage error for anything but `flat` or `natural`.
fn parse_prosody(text: &str) -> Result<ProsodyMode, QinariError> {
    match text {
        "natural" => Ok(ProsodyMode::Natural),
        "flat" => Ok(ProsodyMode::Flat),
        other => Err(QinariError::usage(format!(
            "--prosody must be flat or natural, got {other:?}"
        ))),
    }
}

/// Resolves `--style` / `--style-file` into the library's style spec.
///
/// # Errors
///
/// Returns a usage error when both flags are given or `--style-file` names
/// no existing file, and a tts error for an unknown style preset (with a
/// "did you mean" hint).
fn resolve_style(style: Option<&str>, style_file: Option<&Path>) -> Result<StyleSpec, QinariError> {
    match (style, style_file) {
        (Some(_), Some(_)) => Err(QinariError::usage(
            "--style and --style-file contradict each other",
        )),
        (Some(name), None) => {
            if find_by_style_name(name).is_none() {
                let hint = suggest(name, &voice_ids()).map_or_else(
                    || format!("unknown style {name:?}"),
                    |close| format!("unknown style {name:?}; did you mean {close:?}?"),
                );
                return Err(QinariError::tts(hint));
            }
            Ok(StyleSpec::Preset(name.to_string()))
        }
        (None, Some(path)) => {
            if !path.is_file() {
                return Err(QinariError::usage(format!(
                    "could not read the style file {}: no such file",
                    path.display()
                )));
            }
            Ok(StyleSpec::File(path.to_path_buf()))
        }
        (None, None) => Ok(StyleSpec::Default),
    }
}

/// Validates `--steps`.
fn validate_steps(steps: u8) -> Result<u8, QinariError> {
    if (MIN_STEPS..=MAX_STEPS).contains(&steps) {
        Ok(steps)
    } else {
        Err(QinariError::usage(format!(
            "--steps must be between {MIN_STEPS} and {MAX_STEPS}, got {steps}"
        )))
    }
}

/// Validates `--channels`.
fn validate_channels(raw: u16) -> Result<u16, QinariError> {
    match raw {
        1 | 2 => Ok(raw),
        other => Err(QinariError::usage(format!(
            "--channels must be 1 or 2, got {other}"
        ))),
    }
}

/// Output path for one render: plain default, or the numbered form when
/// `--batch` was given.
fn out_path(out: Option<&Path>, batch: Option<u32>, run: u32) -> PathBuf {
    let base = out.unwrap_or(Path::new("out.wav")).to_path_buf();
    if batch.is_none() {
        return base;
    }
    let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let ext = base.extension().and_then(|e| e.to_str()).unwrap_or("wav");
    PathBuf::from(format!("{stem}-{run:03}.{ext}"))
}

/// Whether a voice is installed: the shared engine bundle plus the
/// voice's style file are all present (digests are verified when the
/// engine loads).
fn voice_installed(paths: &Paths, voice: &VoiceEntry) -> bool {
    voice
        .bundle
        .files
        .iter()
        .all(|file| paths.supertonic_bundle_file(file.rel_path).is_file())
        && paths.supertonic_style(voice).is_file()
}

/// Reads TTS input text: a file path, or "-" for stdin.
///
/// # Errors
///
/// Returns a usage error when the file or stdin cannot be read.
pub fn read_tts_text(path: &str) -> Result<String, QinariError> {
    if path == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| {
                QinariError::usage(format!("could not read text from stdin: {error}"))
            })?;
        return Ok(text);
    }
    std::fs::read_to_string(path)
        .map_err(|error| QinariError::usage(format!("could not read text file {path:?}: {error}")))
}

/// Runs `qinari setup`.
///
/// # Errors
///
/// Setup errors for a missing curl or unwritable directories, download
/// errors for failed fetches, tts errors for failed validation synthesis.
pub fn setup(ctx: &mut RunContext, args: &SetupArgs) -> Result<(), QinariError> {
    if args.list_voices {
        list_voices();
        return Ok(());
    }
    if !ctx.quiet {
        println!("qinari {} — setup", env!("CARGO_PKG_VERSION"));
    }
    check_environment(&ctx.paths)?;
    ensure_config(ctx)?;
    let selected = selected_voices(&args.voices)?;
    // The overall bar spans every file the install still needs (voices
    // plus the DNSMOS pair); it stays absent under --quiet and when
    // everything is already in place.
    let progress = (!ctx.quiet)
        .then(|| planned_download_count(&ctx.paths, &selected))
        .filter(|&total| total > 0)
        .map(SetupProgress::new);
    for voice in selected {
        install_voice(ctx, voice, args, progress.as_ref())?;
    }
    install_dnsmos(ctx, progress.as_ref())?;
    if let Some(progress) = &progress {
        progress.finish();
    }
    if !ctx.quiet {
        println!("setup complete");
    }
    Ok(())
}

/// Setup's aggregate download progress: one multi-progress tree holding
/// the overall "file n/total" bar above the per-file byte bars, so the
/// 398 MB bundle shows how far along the whole install is.
struct SetupProgress {
    multi: MultiProgress,
    overall: ProgressBar,
}

impl SetupProgress {
    /// Creates the tree with an overall bar counting `total` file downloads.
    fn new(total: u64) -> Self {
        let multi = MultiProgress::new();
        let overall = ProgressBar::new(total);
        overall.set_style(
            ProgressStyle::with_template("{msg}: file {pos}/{len}")
                .unwrap_or_else(|_| ProgressStyle::default_bar()),
        );
        overall.set_message("installing voices".to_string());
        let overall = multi.add(overall);
        Self { multi, overall }
    }

    /// Marks one more fetched file.
    fn file_done(&self) {
        self.overall.inc(1);
    }

    /// Clears the bar once the install loop ends.
    fn finish(&self) {
        self.overall.finish_and_clear();
    }
}

/// How many files `setup` still needs to fetch: the shared bundle's
/// missing files (counted once — only the first selected voice downloads
/// them), each selected voice's missing style file, and the DNSMOS pair
/// when it is not installed yet.
fn planned_download_count(paths: &Paths, selected: &[&'static VoiceEntry]) -> u64 {
    let bundle_missing = selected.first().map_or(0, |voice| {
        voice
            .bundle
            .files
            .iter()
            .filter(|file| !paths.supertonic_bundle_file(file.rel_path).is_file())
            .count() as u64
    });
    let styles_missing = selected
        .iter()
        .filter(|voice| !paths.supertonic_style(voice).is_file())
        .count() as u64;
    let dnsmos_missing = DNSMOS_ASSETS
        .iter()
        .filter(|asset| {
            !paths
                .models_dir
                .join("dnsmos")
                .join(asset.rel_path)
                .is_file()
        })
        .count() as u64;
    bundle_missing + styles_missing + dnsmos_missing
}

/// Prints the voice catalog with sizes, licenses, and the restricted
/// marker, plus the shared engine bundle every voice needs.
fn list_voices() {
    let bundle = voices()
        .first()
        .map(|voice| voice.bundle)
        .expect("the catalog is never empty");
    println!(
        "shared engine bundle: {:.1} MiB (downloaded once, all voices)",
        bundle.size_bytes() as f64 / (1024.0 * 1024.0)
    );
    for voice in voices() {
        let marker = match voice.license_class {
            LicenseClass::NonCommercial => "  [NON-COMMERCIAL]",
            LicenseClass::Restricted => "  [RESTRICTED]",
            LicenseClass::Permissive => "",
        };
        println!(
            "{:<16} {:<10} {:<5} {:>6} Hz  {:>6}  {}{}",
            voice.id,
            voice.engine.as_str(),
            voice.language,
            voice.sample_rate,
            format!("{:.0} KiB", voice.style_size_bytes as f64 / 1024.0),
            voice.license,
            marker
        );
    }
}

/// Verifies curl works and the layout directories exist.
fn check_environment(paths: &Paths) -> Result<(), QinariError> {
    let status = Command::new("curl")
        .arg("--version")
        .output()
        .map_err(|error| QinariError::setup(format!("curl is required for downloads: {error}")))?;
    if !status.status.success() {
        return Err(QinariError::setup("curl is installed but not working"));
    }
    let dirs = [
        paths.config_file.parent().map(Path::to_path_buf),
        Some(paths.models_dir.clone()),
        Some(paths.supertonic_dir.clone()),
        Some(paths.state_dir.clone()),
    ];
    for dir in dirs.into_iter().flatten() {
        std::fs::create_dir_all(&dir).map_err(|error| {
            QinariError::setup(format!("could not create {}: {error}", dir.display()))
        })?;
    }
    Ok(())
}

/// Writes the default config when none exists yet.
fn ensure_config(ctx: &mut RunContext) -> Result<(), QinariError> {
    if ctx.paths.config_file.exists() {
        return Ok(());
    }
    ctx.config.save(&ctx.paths.config_file).map_err(|error| {
        QinariError::setup(format!(
            "could not write {}: {error}",
            ctx.paths.config_file.display()
        ))
    })?;
    if !ctx.quiet {
        println!(
            "wrote default config to {}",
            ctx.paths.config_file.display()
        );
    }
    Ok(())
}

/// Resolves `--voice` filters against the catalog.
fn selected_voices(filter: &[String]) -> Result<Vec<&'static VoiceEntry>, QinariError> {
    if filter.is_empty() {
        return Ok(voices().iter().collect());
    }
    filter
        .iter()
        .map(|id| {
            find_voice(id).ok_or_else(|| {
                let hint = suggest(id, &voice_ids()).map_or_else(
                    || format!("unknown voice {id:?}"),
                    |close| format!("unknown voice {id:?}; did you mean {close:?}?"),
                );
                QinariError::tts(hint)
            })
        })
        .collect()
}

/// Installs one voice: license gate, bundle + style downloads, validation
/// synthesis. The shared engine bundle files are downloaded once —
/// `download` skips files that are already present and verified. The
/// overall bar ticks once per file that actually needed fetching.
fn install_voice(
    ctx: &mut RunContext,
    voice: &'static VoiceEntry,
    args: &SetupArgs,
    progress: Option<&SetupProgress>,
) -> Result<(), QinariError> {
    let accepted = ctx
        .config
        .tts
        .accepted_licenses
        .iter()
        .any(|id| id == voice.license_id);
    if voice.license_class != LicenseClass::Permissive && !accepted {
        if args.accept_license {
            record_acceptance(ctx, voice)?;
        } else if args.yes {
            skip_note(
                ctx,
                voice,
                "license not accepted (rerun with --accept-license)",
            );
            return Ok(());
        } else if !confirm_license(ctx, voice)? {
            skip_note(ctx, voice, "license declined");
            return Ok(());
        }
    }
    if !ctx.quiet {
        println!(
            "installing {} (style {:.0} KiB, shared bundle {:.1} MiB)",
            voice.id,
            voice.style_size_bytes as f64 / 1024.0,
            voice.bundle.size_bytes() as f64 / (1024.0 * 1024.0)
        );
    }
    let parent = progress.map(|p| &p.multi);
    for file in voice.bundle.files {
        let dest = ctx.paths.supertonic_bundle_file(file.rel_path);
        let needs_fetch = !dest.exists();
        download(
            &DownloadSpec {
                url: file.url.to_string(),
                dest,
                expected_size: Some(file.size_bytes),
                expected_sha256: Some(file.sha256.to_string()),
                label: format!("supertonic {}", file.rel_path),
            },
            parent,
        )
        .map_err(|error| download_error(&error))?;
        if needs_fetch {
            if let Some(progress) = progress {
                progress.file_done();
            }
        }
    }
    let style_dest = ctx.paths.supertonic_style(voice);
    let style_needs_fetch = !style_dest.exists();
    download(
        &DownloadSpec {
            url: voice.style_url.to_string(),
            dest: style_dest,
            expected_size: Some(voice.style_size_bytes),
            expected_sha256: Some(voice.style_sha256.to_string()),
            label: format!("{} style", voice.id),
        },
        parent,
    )
    .map_err(|error| download_error(&error))?;
    if style_needs_fetch {
        if let Some(progress) = progress {
            progress.file_done();
        }
    }
    validate_synthesis(ctx, voice)
}

/// Installs the two vendored DNSMOS models (CC BY 4.0, ~1.3 MiB) that
/// `analyze --mos` and the golden gates score with. The digests are
/// pinned in `qinari-verify`; files already in place are verified, not
/// re-downloaded.
///
/// # Errors
///
/// Download errors for failed fetches or digest mismatches.
fn install_dnsmos(ctx: &RunContext, progress: Option<&SetupProgress>) -> Result<(), QinariError> {
    let dir = ctx.paths.models_dir.join("dnsmos");
    let parent = progress.map(|p| &p.multi);
    for asset in &DNSMOS_ASSETS {
        let dest = dir.join(asset.rel_path);
        let needs_fetch = !dest.exists();
        download(
            &DownloadSpec {
                url: asset.url.to_string(),
                dest,
                expected_size: Some(asset.size_bytes),
                expected_sha256: Some(asset.sha256.to_string()),
                label: format!("dnsmos {}", asset.rel_path),
            },
            parent,
        )
        .map_err(|error| download_error(&error))?;
        if needs_fetch {
            if let Some(progress) = progress {
                progress.file_done();
            }
        }
    }
    if !ctx.quiet {
        println!("dnsmos models ready (`qinari analyze --mos`)");
    }
    Ok(())
}

/// Prints a skip note unless quiet.
fn skip_note(ctx: &RunContext, voice: &VoiceEntry, reason: &str) {
    if !ctx.quiet {
        println!("skipping {} — {reason}", voice.id);
    }
}

/// Persists a license acceptance into the config file. The record is the
/// license key (shared by every voice under the same license), so one
/// acceptance covers the whole Supertonic set.
fn record_acceptance(ctx: &mut RunContext, voice: &VoiceEntry) -> Result<(), QinariError> {
    if !ctx
        .config
        .tts
        .accepted_licenses
        .iter()
        .any(|id| id == voice.license_id)
    {
        ctx.config
            .tts
            .accepted_licenses
            .push(voice.license_id.to_string());
    }
    ctx.config.save(&ctx.paths.config_file).map_err(|error| {
        QinariError::setup(format!("could not record the license acceptance: {error}"))
    })
}

/// Asks interactively whether the voice's license is accepted.
fn confirm_license(ctx: &mut RunContext, voice: &VoiceEntry) -> Result<bool, QinariError> {
    let restrictions = match voice.license_class {
        LicenseClass::NonCommercial => "non-commercial use only".to_string(),
        LicenseClass::Restricted => "commercial use is allowed, but the license carries use \
             restrictions: the model must not be used to harm or deceive people, impersonate \
             anyone without consent, or run biometric identification"
            .to_string(),
        LicenseClass::Permissive => "no restrictions beyond attribution".to_string(),
    };
    let answer = Confirm::new()
        .with_prompt(format!(
            "{} is licensed under {} ({}; {}) — accept the license and install it?",
            voice.id, voice.license, voice.license_url, restrictions
        ))
        .default(false)
        .interact()
        .map_err(|error| QinariError::setup(format!("could not read the answer: {error}")))?;
    if answer {
        record_acceptance(ctx, voice)?;
    }
    Ok(answer)
}

/// Speaks one sentence through the freshly installed voice.
fn validate_synthesis(ctx: &RunContext, voice: &'static VoiceEntry) -> Result<(), QinariError> {
    // Every Supertonic voice is multilingual; validate in Indonesian, the
    // project's primary language.
    let sentence = "Qinari uji suara, satu dua tiga.";
    let engine = SupertonicEngine::load(&ctx.paths.supertonic_dir, voice)
        .map_err(|error| tts_error(&error))?;
    let request = SynthRequest {
        text: sentence.to_string(),
        language: Language::Id,
        speed: 1.0,
        voice: voice.id.to_string(),
        sr: voice.sample_rate,
        channels: 1,
        steps: DEFAULT_STEPS,
        seed: None,
        prosody: ProsodyMode::Natural,
        style: StyleSpec::Default,
        markup: true,
    };
    let synthesis = engine
        .synthesize(&request)
        .map_err(|error| tts_error(&error))?;
    if !ctx.quiet {
        println!(
            "{} speaks: {:.2} s of audio in {:.2} s (RTF {:.2})",
            voice.id,
            synthesis.stats.audio_secs,
            synthesis.stats.elapsed.as_secs_f64(),
            synthesis.stats.rtf
        );
    }
    Ok(())
}

/// Runs `qinari doctor` and returns whether the installation is healthy.
///
/// Health means: curl works, the shared Supertonic engine bundle is
/// installed, and at least one voice is installed. A missing config file
/// and a missing cloud key are informational, not unhealthy.
pub fn doctor(ctx: &RunContext) -> bool {
    let mut problems = 0;
    match Command::new("curl").arg("--version").output() {
        Ok(output) if output.status.success() => println!("ok: curl is available"),
        Ok(_) => {
            problems += 1;
            println!("problem: curl is installed but not working");
        }
        Err(_) => {
            problems += 1;
            println!("problem: curl is not installed (required for downloads)");
        }
    }
    if ctx.paths.config_file.exists() {
        match Config::load(&ctx.paths.config_file) {
            Ok(_) => println!("ok: config file is valid"),
            Err(error) => println!("warn: {error}"),
        }
    } else {
        println!("info: no config file (defaults in use); `qinari setup` writes one");
    }
    let sample_voice = voices()
        .first()
        .copied()
        .expect("the catalog is never empty");
    let bundle_installed = sample_voice
        .bundle
        .files
        .iter()
        .all(|file| ctx.paths.supertonic_bundle_file(file.rel_path).is_file());
    if bundle_installed {
        println!("ok: the supertonic engine bundle is installed");
    } else {
        problems += 1;
        println!("problem: the supertonic engine bundle is missing; run `qinari setup`");
    }
    let dnsmos_installed = DNSMOS_ASSETS.iter().all(|asset| {
        ctx.paths
            .models_dir
            .join("dnsmos")
            .join(asset.rel_path)
            .is_file()
    });
    if dnsmos_installed {
        println!("ok: the DNSMOS models are installed (analyze --mos)");
    } else {
        problems += 1;
        println!("problem: the DNSMOS models are missing; run `qinari setup`");
    }
    let installed: Vec<&VoiceEntry> = voices()
        .iter()
        .filter(|voice| ctx.paths.supertonic_style(voice).is_file())
        .collect();
    if installed.is_empty() {
        problems += 1;
        println!("problem: no voices installed; run `qinari setup`");
    } else {
        for voice in installed {
            println!("ok: voice {} is installed", voice.id);
        }
    }
    let key = std::env::var("QINARI_TTS_API_KEY")
        .ok()
        .or_else(|| std::env::var("OPENAI_API_KEY").ok());
    match key {
        Some(_) => println!("info: a cloud API key is set"),
        None => println!("info: no cloud API key (set QINARI_TTS_API_KEY to use cloud TTS)"),
    }
    println!("info: engine preference is {}", ctx.config.tts.engine);
    let healthy = problems == 0;
    println!("{}", if healthy { "healthy" } else { "problems found" });
    healthy
}

/// Runs `qinari sfx`.
///
/// # Errors
///
/// Usage errors for bad flags and unknown recipes, render errors for
/// failed renders, encode errors for unsupported output paths.
pub fn sfx(ctx: &RunContext, args: &SfxArgs) -> Result<(), QinariError> {
    if args.list {
        list_recipes();
        return Ok(());
    }
    let Some(recipe) = args.recipe.as_deref() else {
        return Err(QinariError::usage(
            "qinari sfx needs a recipe id (or --list)",
        ));
    };
    let duration = parse_duration(&args.duration)?;
    let seed = seed_from(args.seed.as_deref());
    let sets = parse_sets(&args.sets)?;
    let channels = validate_channels(args.channels)?;
    let batch = args.batch.unwrap_or(1);
    if !(1..=100).contains(&batch) {
        return Err(QinariError::usage("--batch must be between 1 and 100"));
    }
    let options = RenderOptions::resolve(&overrides_from(&args.overrides)?, &ctx.config)?;
    for run in 0..batch {
        let request = RenderRequest {
            recipe: recipe.to_string(),
            params: sets.clone(),
            duration_secs: duration,
            seed: seed.wrapping_add(u64::from(run)),
            sr: options.sr,
            ch: usize::from(channels),
        };
        let started = Instant::now();
        let buffer = render_sfx(&request).map_err(|error| sfx_error(&error))?;
        let render_secs = started.elapsed().as_secs_f64();
        let out = out_path(args.out.as_deref(), args.batch, run);
        let summary = finish_and_write(&buffer, &options, &out, render_secs)?;
        report_render(ctx, args.json, &summary, None, None)?;
    }
    Ok(())
}

/// Prints the recipe catalog with parameters.
fn list_recipes() {
    for recipe in recipes() {
        println!("{}", recipe.id);
        println!("  {}", recipe.description);
        for spec in recipe.params {
            println!(
                "  --set {} | {} (default {})",
                param_signature(spec),
                spec.doc,
                spec.default
            );
        }
    }
}

/// `name=<min..max>` for one parameter.
fn param_signature(spec: &ParamSpec) -> String {
    format!(
        "{}=<{}..{}>",
        spec.name,
        fmt_num(spec.min),
        fmt_num(spec.max)
    )
}

/// Formats a parameter bound without trailing zeros.
fn fmt_num(value: f64) -> String {
    if (value - value.trunc()).abs() < 1e-9 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Runs `qinari ambient`.
///
/// # Errors
///
/// Usage errors for bad flags and unknown presets, render errors for
/// failed renders, encode errors for unsupported output paths.
pub fn ambient(ctx: &RunContext, args: &AmbientArgs) -> Result<(), QinariError> {
    if args.list {
        list_presets();
        return Ok(());
    }
    let Some(preset) = args.preset.as_deref() else {
        return Err(QinariError::usage(
            "qinari ambient needs a preset id (or --list)",
        ));
    };
    let duration = parse_duration(&args.duration)?;
    let seed = seed_from(args.seed.as_deref());
    let channels = validate_channels(args.channels)?;
    let options = RenderOptions::resolve(&overrides_from(&args.overrides)?, &ctx.config)?;
    let started = Instant::now();
    let buffer = render_ambient(preset, duration, seed, options.sr, usize::from(channels))
        .map_err(|error| ambient_error(&error))?;
    let render_secs = started.elapsed().as_secs_f64();
    let out = out_path(args.out.as_deref(), None, 0);
    let summary = finish_and_write(&buffer, &options, &out, render_secs)?;
    report_render(ctx, args.json, &summary, None, None)?;
    Ok(())
}

/// Prints the ambient preset catalog.
fn list_presets() {
    for preset in ambient_presets() {
        println!(
            "{} — {} ({} layers)",
            preset.id,
            preset.description,
            preset.layers.len()
        );
    }
}

/// Runs `qinari music`.
///
/// # Errors
///
/// Usage errors for bad flags and unknown styles, render errors for
/// failed renders, encode errors for unsupported output paths.
pub fn music(ctx: &RunContext, args: &MusicArgs) -> Result<(), QinariError> {
    if args.list {
        list_styles();
        return Ok(());
    }
    let Some(style) = args.style.as_deref() else {
        return Err(QinariError::usage(
            "qinari music needs a style id (or --list)",
        ));
    };
    let duration = parse_duration(&args.duration)?;
    let seed = seed_from(args.seed.as_deref());
    let channels = validate_channels(args.channels)?;
    let options = RenderOptions::resolve(&overrides_from(&args.overrides)?, &ctx.config)?;
    let request = TrackRequest {
        style: style.to_string(),
        duration_secs: duration as f32,
        bpm: args.bpm.unwrap_or(0.0),
        seed: Seed::from_u64(seed),
        sample_rate: options.sr,
        channels,
    };
    let started = Instant::now();
    let buffer = render_track(&request).map_err(|error| music_error(&error))?;
    let render_secs = started.elapsed().as_secs_f64();
    let out = out_path(args.out.as_deref(), None, 0);
    let summary = finish_and_write(&buffer, &options, &out, render_secs)?;
    report_render(ctx, args.json, &summary, None, None)?;
    Ok(())
}

/// Prints the style catalog with tempo ranges.
fn list_styles() {
    for style in STYLES {
        println!(
            "{} — {} ({}..{} bpm, typical {})",
            style.id, style.description, style.bpm_min, style.bpm_max, style.bpm_typical
        );
    }
}

/// Runs `qinari tts`.
///
/// `--lang` must be `id` or `en`. With `engine auto` (the default), a
/// voice whose files are installed is spoken locally; anything else goes
/// to the cloud service.
///
/// # Errors
///
/// Usage errors for bad flags, tts errors for engine failures, encode
/// errors for unsupported output paths.
pub fn tts(ctx: &RunContext, args: &TtsArgs) -> Result<(), QinariError> {
    let language = Language::from_code(&args.lang).ok_or_else(|| {
        QinariError::usage(format!("--lang must be id or en, got {:?}", args.lang))
    })?;
    let channels = validate_channels(args.channels)?;
    let steps = validate_steps(args.steps)?;
    let prosody = parse_prosody(&args.prosody)?;
    let style = resolve_style(args.style.as_deref(), args.style_file.as_deref())?;
    let seed = args.seed.as_deref().map(|text| seed_from(Some(text)));
    let preference = engine_preference(ctx, args.engine.as_deref())?;
    let voice = match args.voice.as_deref() {
        Some(id) => find_voice(id).ok_or_else(|| {
            let hint = suggest(id, &voice_ids()).map_or_else(
                || format!("unknown voice {id:?}"),
                |close| format!("unknown voice {id:?}; did you mean {close:?}?"),
            );
            QinariError::tts(hint)
        })?,
        None => default_voice(language.as_code()).ok_or_else(|| {
            QinariError::tts(format!("no bundled voice for {}", language.as_code()))
        })?,
    };
    let text = match args.file.as_deref() {
        // An explicit --file wins over the positional form; both accept
        // "-" for stdin.
        Some(path) => read_tts_text(path)?,
        None => match &args.text {
            Some(text) if text.as_str() != "-" => text.clone(),
            _ => read_tts_text("-")?,
        },
    };
    let options = RenderOptions::resolve(&overrides_from(&args.overrides)?, &ctx.config)?;
    let out = apply_format(
        out_path(args.out.as_deref(), None, 0),
        args.format.as_deref(),
    )?;
    let use_local = match preference.as_str() {
        "local" => true,
        "cloud" => false,
        _ => voice_installed(&ctx.paths, voice),
    };
    let started = Instant::now();
    let request = SynthRequest {
        text,
        language,
        speed: args.speed,
        voice: voice.id.to_string(),
        sr: options.sr,
        channels,
        steps,
        seed,
        prosody,
        style,
        markup: !args.no_markup,
    };
    let audio = if use_local {
        synthesize_locally(ctx, voice, &request)?
    } else {
        synthesize_in_cloud(ctx, &request)?
    };
    let render_secs = started.elapsed().as_secs_f64();
    let summary = finish_and_write(&audio, &options, &out, render_secs)?;
    let engine_id = if use_local {
        voice.engine.as_str()
    } else {
        "openai-compat"
    };
    let spoken_voice = if use_local {
        voice.id
    } else {
        // The cloud lane speaks with the configured service voice.
        ctx.config.tts.voice.as_str()
    };
    report_render(
        ctx,
        args.json,
        &summary,
        Some(engine_id),
        Some(spoken_voice),
    )
}

/// Resolves the engine preference: flag over config, both validated.
fn engine_preference(ctx: &RunContext, flag: Option<&str>) -> Result<String, QinariError> {
    let value = flag.unwrap_or(ctx.config.tts.engine.as_str()).to_string();
    match value.as_str() {
        "auto" | "local" | "cloud" => Ok(value),
        other => Err(if flag.is_some() {
            QinariError::usage(format!(
                "--engine must be auto, local, or cloud, got {other:?}"
            ))
        } else {
            QinariError::setup(format!(
                "config [tts] engine must be auto, local, or cloud, got {other:?}"
            ))
        }),
    }
}

/// Speaks through the installed local voice. The engine synthesizes at
/// its native 44 100 Hz and resamples to the target rate itself.
///
/// # Errors
///
/// Tts errors for a missing voice or engine failures.
fn synthesize_locally(
    ctx: &RunContext,
    voice: &'static VoiceEntry,
    request: &SynthRequest,
) -> Result<qinari_dsp::buffer::AudioBuffer, QinariError> {
    if !voice_installed(&ctx.paths, voice) {
        return Err(QinariError::tts(format!(
            "voice {} is not installed; run `qinari setup` first",
            voice.id
        )));
    }
    let (clamped, was_clamped) = SupertonicEngine::clamp_speed(request.speed);
    if was_clamped {
        eprintln!(
            "warning: --speed {} is outside the voice's 0.7..=2.0 range; using {clamped} instead",
            request.speed
        );
    }
    let engine = SupertonicEngine::load(&ctx.paths.supertonic_dir, voice)
        .map_err(|error| tts_error(&error))?;
    Ok(engine
        .synthesize(request)
        .map_err(|error| tts_error(&error))?
        .audio)
}

/// Speaks through the configured OpenAI-compatible service.
///
/// # Errors
///
/// Tts errors for a missing key, network failures, or unusable responses.
fn synthesize_in_cloud(
    ctx: &RunContext,
    request: &SynthRequest,
) -> Result<qinari_dsp::buffer::AudioBuffer, QinariError> {
    let config = CloudConfig {
        base_url: ctx.config.tts.base.clone(),
        model: ctx.config.tts.model.clone(),
        voice: ctx.config.tts.voice.clone(),
        api_key: None,
        timeout: Duration::from_secs(30),
        retries: 2,
    };
    let engine = OpenAiCompatibleEngine::new(config).map_err(|error| tts_error(&error))?;
    Ok(engine
        .synthesize(request)
        .map_err(|error| tts_error(&error))?
        .audio)
}

/// Runs `qinari master`: decode, master, write.
///
/// # Errors
///
/// Usage errors for undecodable input, setup errors for a bad config,
/// encode errors for unsupported output paths.
pub fn master_command(ctx: &RunContext, args: &MasterArgs) -> Result<(), QinariError> {
    if args.overrides.master || args.overrides.no_master {
        return Err(QinariError::usage(
            "--master/--no-master do not apply to qinari master (it always masters)",
        ));
    }
    let started = Instant::now();
    let buffer = decode_audio(&args.input).map_err(|error| {
        QinariError::usage(format!(
            "could not decode {}: {error}",
            args.input.display()
        ))
    })?;
    let render_secs = started.elapsed().as_secs_f64();
    let mut options = RenderOptions::resolve(&overrides_from(&args.overrides)?, &ctx.config)?;
    // master keeps the source rate and always runs the chain.
    options.sr = buffer.sample_rate();
    options.master = true;
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| mastered_default(&args.input));
    let summary = finish_and_write(&buffer, &options, &out, render_secs)?;
    report_render(ctx, args.json, &summary, None, None)
}

/// Default output for master: `<input stem>-mastered.wav`.
fn mastered_default(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("mastered");
    PathBuf::from(format!("{stem}-mastered.wav"))
}

/// Runs `qinari analyze`: print loudness, peak, rms, duration, and —
/// on request — the speech section (`--speech`) and DNSMOS scores
/// (`--mos`, models from `<models>/dnsmos`, vendored by `setup`).
///
/// # Errors
///
/// Usage errors for a missing or undecodable input file; a setup error
/// when `--mos` is asked for but the models are not installed; render
/// errors when scoring itself fails.
pub fn analyze(ctx: &RunContext, args: &AnalyzeArgs) -> Result<(), QinariError> {
    let buffer = decode_audio(&args.file).map_err(|error| {
        QinariError::usage(format!("could not decode {}: {error}", args.file.display()))
    })?;
    let lufs = integrated_lufs(&buffer);
    // The speech section rides along in the full report; the JSON form
    // always carries it, the human form only with --speech.
    let report = if args.speech || args.json {
        Some(qinari_verify::analyze_buffer(&buffer))
    } else {
        None
    };
    let scores = if args.mos {
        Some(dnsmos::dnsmos(&ctx.paths.models_dir.join("dnsmos"), &buffer).map_err(verify_error)?)
    } else {
        None
    };
    if args.json {
        print_analyze_json(&args.file, &buffer, lufs, report.as_ref(), scores.as_ref());
        return Ok(());
    }
    println!("{}", args.file.display());
    println!(
        "duration {:.2} s @ {} Hz, {} ch",
        buffer.duration(),
        buffer.sample_rate(),
        buffer.num_channels()
    );
    println!(
        "integrated {}",
        lufs.map_or_else(
            || "too short or quiet to measure".to_string(),
            |value| format!("{value:.1} LUFS")
        )
    );
    println!("peak {:.1} dBFS", linear_to_db(buffer.peak()));
    println!("rms {:.1} dBFS", linear_to_db(buffer.rms()));
    if let Some(report) = &report {
        print_speech_human(&report.speech);
    }
    if let Some(scores) = &scores {
        print_dnsmos_human(scores);
    }
    Ok(())
}

/// Maps measurement failures onto the CLI's exit codes: missing DNSMOS
/// models are a setup problem (the fix is `qinari setup`), unreadable
/// model files likewise, everything else is a render-time failure.
fn verify_error(error: VerifyError) -> QinariError {
    match error {
        VerifyError::ModelMissing { .. } => QinariError::setup(format!(
            "the DNSMOS models are not installed; run `qinari setup` first ({error})"
        )),
        VerifyError::Io(_) => {
            QinariError::setup(format!("could not read the DNSMOS models: {error}"))
        }
        other => QinariError::render(format!("measurement failed: {other}")),
    }
}

/// Prints the speech section in the human form: one line of voicing,
/// F0, and pause statistics.
fn print_speech_human(speech: &qinari_verify::speech::SpeechReport) {
    let f0 = speech.f0.f0_mean_hz.map_or_else(
        || "no F0".to_string(),
        |hz| {
            format!(
                "f0 {hz:.0} Hz (sigma {} st, range {} st)",
                speech
                    .f0
                    .f0_sigma_st
                    .map_or("—".to_string(), |v| format!("{v:.1}")),
                speech
                    .f0
                    .f0_range_st
                    .map_or("—".to_string(), |v| format!("{v:.1}"))
            )
        },
    );
    println!(
        "speech   voiced {:.0}% | {f0} | {} pauses ({:.2} s total, {:.1}/min)",
        speech.f0.voiced_fraction * 100.0,
        speech.pauses.count,
        speech.pauses.total_s,
        speech.pauses.rate_per_min
    );
}

/// Prints the DNSMOS scores in the human form.
fn print_dnsmos_human(scores: &DnsmosScores) {
    println!(
        "dnsmos   sig {:.2} bak {:.2} ovrl {:.2} (p808 {:.2})",
        scores.sig, scores.bak, scores.ovrl, scores.p808
    );
}

/// Prints the `analyze --json` object: one line, stdout only.
fn print_analyze_json(
    file: &Path,
    buffer: &qinari_dsp::buffer::AudioBuffer,
    lufs: Option<f64>,
    report: Option<&FeatureReport>,
    scores: Option<&DnsmosScores>,
) {
    let speech = report.map(|report| {
        let speech = &report.speech;
        serde_json::json!({
            "voiced_fraction": speech.f0.voiced_fraction,
            "f0_mean_hz": speech.f0.f0_mean_hz,
            "f0_sigma_st": speech.f0.f0_sigma_st,
            "f0_range_st": speech.f0.f0_range_st,
            "pause_count": speech.pauses.count,
            "pause_total_s": speech.pauses.total_s,
            "pause_rate_per_min": speech.pauses.rate_per_min,
        })
    });
    // The realism section rides the same full report: the periodic-gain
    // detector, interaural correlation bands, and event statistics the
    // benchmark tables and golden gates read.
    let realism = report.map(|report| {
        // Line share: the fraction of the 0–20 Hz gain-modulation
        // spectrum's normalized energy sitting in its peak bin. A pure
        // sinusoid AM reads ~1.0 (a spectral line); a latent-driven bed
        // spreads the same energy across bins and reads far lower.
        let spectrum = &report.modulation.gain_spectrum_0_20hz;
        let sum: f64 = spectrum.iter().sum();
        let line_share = if sum > 0.0 { 1.0 / sum } else { 0.0 };
        serde_json::json!({
            "gain_peak_mean_ratio": report.modulation.gain_peak_mean_ratio,
            "gain_peak_hz": report.modulation.gain_peak_hz,
            "gain_line_share": line_share,
            "interaural": report.interaural.as_ref().map(|interaural| serde_json::json!({
                "b0_500": interaural.b0_500,
                "b500_2000": interaural.b500_2000,
                "b2000_8000": interaural.b2000_8000,
                "b8000_nyquist": interaural.b8000_nyquist,
            })),
            "event_count": report.events.count,
            "event_rate_hz": report.events.rate_hz,
            "event_interval_cv": report.events.interval_cv,
        })
    });
    let mut json = serde_json::json!({
        "file": file.display().to_string(),
        "duration_s": buffer.duration(),
        "sample_rate": buffer.sample_rate(),
        "channels": buffer.num_channels(),
        "lufs": lufs,
        "peak_dbfs": linear_to_db(buffer.peak()),
        "rms_dbfs": linear_to_db(buffer.rms()),
        "speech": speech,
        "realism": realism,
    });
    if let Some(scores) = scores {
        json["dnsmos"] = serde_json::json!({
            "sig": f64::from(scores.sig),
            "bak": f64::from(scores.bak),
            "ovrl": f64::from(scores.ovrl),
            "p808": f64::from(scores.p808),
        });
    }
    println!("{json}");
}

/// Runs `qinari bench`: times the reference renders and reports speedups.
///
/// # Errors
///
/// Propagates render errors from either reference render.
pub fn bench(ctx: &RunContext, args: &BenchArgs) -> Result<(), QinariError> {
    const SR: u32 = 44_100;
    let sfx_started = Instant::now();
    let sfx_buffer = render_sfx(&RenderRequest {
        recipe: "rain".to_string(),
        params: Vec::new(),
        duration_secs: 10.0,
        seed: 1,
        sr: SR,
        ch: 2,
    })
    .map_err(|error| sfx_error(&error))?;
    let sfx_secs = sfx_started.elapsed().as_secs_f64();

    let music_started = Instant::now();
    let music_buffer = render_track(&TrackRequest {
        style: "lofi".to_string(),
        duration_secs: 10.0,
        bpm: 0.0,
        seed: Seed::from_u64(1),
        sample_rate: SR,
        channels: 2,
    })
    .map_err(|error| music_error(&error))?;
    let music_secs = music_started.elapsed().as_secs_f64();

    let sfx = benchmark_entry("rain", &sfx_buffer, sfx_secs);
    let music = benchmark_entry("lofi", &music_buffer, music_secs);
    if args.json {
        let report = serde_json::json!({
            "sfx": {
                "recipe": sfx.name, "audio_secs": sfx.audio_secs,
                "render_secs": sfx.render_secs, "speedup": sfx.speedup,
            },
            "music": {
                "style": music.name, "audio_secs": music.audio_secs,
                "render_secs": music.render_secs, "speedup": music.speedup,
            },
            "sample_rate": SR,
        });
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|error| {
                QinariError::internal(format!("could not serialize the bench report: {error}"))
            })?
        );
    } else if !ctx.quiet {
        println!(
            "sfx {} 10 s: {:.2} s ({:.1}x realtime)",
            sfx.name, sfx.render_secs, sfx.speedup
        );
        println!(
            "music {} 10 s: {:.2} s ({:.1}x realtime)",
            music.name, music.render_secs, music.speedup
        );
    }
    Ok(())
}

/// One timed reference render.
struct BenchEntry {
    name: &'static str,
    audio_secs: f64,
    render_secs: f64,
    speedup: f64,
}

/// Collects the numbers for one reference render.
fn benchmark_entry(
    name: &'static str,
    buffer: &qinari_dsp::buffer::AudioBuffer,
    render_secs: f64,
) -> BenchEntry {
    let audio_secs = buffer.duration();
    BenchEntry {
        name,
        audio_secs,
        render_secs,
        speedup: if render_secs > 0.0 {
            audio_secs / render_secs
        } else {
            0.0
        },
    }
}

/// Runs `qinari info`.
///
/// # Errors
///
/// Internal errors only, when serialization fails.
pub fn info(ctx: &RunContext, args: &InfoArgs) -> Result<(), QinariError> {
    let installed: Vec<(&'static VoiceEntry, bool)> = voices()
        .iter()
        .map(|voice| (voice, voice_installed(&ctx.paths, voice)))
        .collect();
    if args.json {
        let config = serde_json::to_value(&ctx.config).map_err(|error| {
            QinariError::internal(format!("could not serialize the config: {error}"))
        })?;
        let report = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "paths": {
                "config": ctx.paths.config_file,
                "models": ctx.paths.models_dir,
                "supertonic": ctx.paths.supertonic_dir,
                "state": ctx.paths.state_dir,
                "log": ctx.paths.log_file(),
            },
            "config": config,
            "engine": ctx.config.tts.engine,
            "voices": installed.iter().map(|(voice, is_installed)| {
                serde_json::json!({"id": voice.id, "installed": is_installed})
            }).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|error| {
                QinariError::internal(format!("could not serialize the info report: {error}"))
            })?
        );
        return Ok(());
    }
    println!("qinari {}", env!("CARGO_PKG_VERSION"));
    println!("config file: {}", ctx.paths.config_file.display());
    println!("models dir: {}", ctx.paths.models_dir.display());
    println!("supertonic dir: {}", ctx.paths.supertonic_dir.display());
    println!("state dir: {}", ctx.paths.state_dir.display());
    println!("engine preference: {}", ctx.config.tts.engine);
    println!(
        "output: {} Hz, {}-bit wav, {} kbps mp3, mastering {}",
        ctx.config.output.sr,
        ctx.config.output.bit,
        ctx.config.output.mp3,
        if ctx.config.output.master {
            "on"
        } else {
            "off"
        }
    );
    for (voice, is_installed) in installed {
        println!(
            "voice {} ({}) — {}",
            voice.id,
            voice.language,
            if is_installed {
                "installed"
            } else {
                "not installed"
            }
        );
    }
    Ok(())
}

/// Runs `qinari completions`.
pub fn completions(args: &CompletionsArgs) {
    let mut command = crate::build_command();
    clap_complete::generate(args.shell, &mut command, "qinari", &mut std::io::stdout());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xcode::ExitCode;

    #[test]
    fn parse_duration_accepts_bare_seconds_and_units() {
        assert_eq!(parse_duration("10").unwrap(), 10.0);
        assert_eq!(parse_duration("10s").unwrap(), 10.0);
        assert_eq!(parse_duration("1.5s").unwrap(), 1.5);
        assert_eq!(parse_duration("250ms").unwrap(), 0.25);
        assert_eq!(parse_duration("2m").unwrap(), 120.0);
        assert_eq!(parse_duration("1h").unwrap(), 3600.0);
        assert_eq!(parse_duration("2H").unwrap(), 7200.0);
    }

    #[test]
    fn parse_duration_rejects_garbage() {
        for bad in ["abc", "", "-3", "10parsec", "1.2.3", "0", "-1s", "ms"] {
            let error = parse_duration(bad).expect_err("must be rejected");
            assert_eq!(error.code, ExitCode::Usage, "{bad}");
        }
    }

    #[test]
    fn parse_sets_splits_and_validates() {
        let sets = parse_sets(&["intensity=0.8".to_string(), "distance=-0.5".to_string()])
            .expect("valid sets");
        assert_eq!(
            sets,
            vec![
                ("intensity".to_string(), 0.8),
                ("distance".to_string(), -0.5)
            ]
        );
        let missing_eq = parse_sets(&["intensity".to_string()]).unwrap_err();
        assert!(missing_eq.to_string().contains("NAME=VALUE"));
        let bad_number = parse_sets(&["intensity=loud".to_string()]).unwrap_err();
        assert!(bad_number.to_string().contains("must be a number"));
        let empty_name = parse_sets(&["=1".to_string()]).unwrap_err();
        assert!(empty_name.to_string().contains("name is empty"));
    }

    #[test]
    fn seed_from_passes_numbers_and_hashes_text() {
        assert_eq!(seed_from(Some("42")), 42);
        assert_eq!(seed_from(Some("0")), 0);
        let text_seed = seed_from(Some("storm-night"));
        assert_eq!(text_seed, seed_from(Some("storm-night")));
        assert_ne!(text_seed, Seed::from_text("other").0);
        assert!(seed_from(None) > 0, "time-based seeds are positive");
        assert!(seed_from(Some("")) > 0, "empty seeds fall back to the time");
        assert!(seed_from(Some("   ")) > 0);
    }

    #[test]
    fn suggest_stays_within_distance_three() {
        let recipe_ids: Vec<&'static str> = recipes().iter().map(|recipe| recipe.id).collect();
        assert_eq!(suggest("rian", &recipe_ids).as_deref(), Some("rain"));
        assert_eq!(suggest("laserr", &recipe_ids).as_deref(), Some("laser"));
        assert_eq!(suggest("zzzzzzzz", &recipe_ids), None);
        let styles = style_ids();
        assert_eq!(suggest("lofy", &styles).as_deref(), Some("lofi"));
        assert_eq!(suggest("synthwav", &styles).as_deref(), Some("synthwave"));
        assert_eq!(suggest("zzzzzzzz", &styles), None);
    }

    #[test]
    fn error_mappers_pick_the_right_codes() {
        let unknown_recipe = sfx_error(&SfxError::UnknownRecipe {
            unknown: "rian".to_string(),
            suggestion: "rain".to_string(),
        });
        assert_eq!(unknown_recipe.code, ExitCode::Usage);
        assert!(
            unknown_recipe.to_string().contains("did you mean"),
            "{unknown_recipe}"
        );

        let bad_param = sfx_error(&SfxError::BadParameter(qinari_sfx::ParamError::Unknown {
            recipe: "rain".to_string(),
            name: "intens".to_string(),
            suggestion: "; did you mean \"intensity\"?".to_string(),
        }));
        assert_eq!(bad_param.code, ExitCode::Usage);
        assert!(
            bad_param.to_string().contains("did you mean"),
            "{bad_param}"
        );

        let render_failure = sfx_error(&SfxError::Render("exploded".to_string()));
        assert_eq!(render_failure.code, ExitCode::Render);

        let unknown_style = music_error(&MusicError::UnknownStyle {
            style: "lofy".to_string(),
        });
        assert_eq!(unknown_style.code, ExitCode::Usage);
        assert!(
            unknown_style.to_string().contains("did you mean"),
            "{unknown_style}"
        );

        let unknown_preset = ambient_error(&AmbientError::UnknownPreset {
            unknown: "strom".to_string(),
            suggestion: "storm".to_string(),
        });
        assert_eq!(unknown_preset.code, ExitCode::Usage);
        assert!(
            unknown_preset.to_string().contains("did you mean"),
            "{unknown_preset}"
        );
    }

    #[test]
    fn master_flag_combinations() {
        assert_eq!(master_flag(false, false).unwrap(), None);
        assert_eq!(master_flag(true, false).unwrap(), Some(true));
        assert_eq!(master_flag(false, true).unwrap(), Some(false));
        assert!(master_flag(true, true).is_err());
    }

    #[test]
    fn out_path_defaults_and_batch_numbering() {
        assert_eq!(out_path(None, None, 0), PathBuf::from("out.wav"));
        assert_eq!(
            out_path(Some(Path::new("take.mp3")), None, 0),
            PathBuf::from("take.mp3")
        );
        assert_eq!(out_path(None, Some(3), 1), PathBuf::from("out-001.wav"));
        assert_eq!(
            out_path(Some(Path::new("take.wav")), Some(3), 2),
            PathBuf::from("take-002.wav")
        );
    }

    #[test]
    fn channels_validation() {
        assert_eq!(validate_channels(1).unwrap(), 1);
        assert_eq!(validate_channels(2).unwrap(), 2);
        for bad in [0, 3, 9] {
            let error = validate_channels(bad).unwrap_err();
            assert_eq!(error.code, ExitCode::Usage);
            assert!(error.to_string().contains("1 or 2"));
        }
    }

    #[test]
    fn mastered_default_derives_from_the_input_stem() {
        assert_eq!(
            mastered_default(Path::new("/tmp/song.wav")),
            PathBuf::from("song-mastered.wav")
        );
        assert_eq!(
            mastered_default(Path::new("mp3")),
            PathBuf::from("mp3-mastered.wav")
        );
    }

    #[test]
    fn apply_format_rewrites_the_extension() {
        assert_eq!(
            apply_format(PathBuf::from("out.wav"), None).unwrap(),
            PathBuf::from("out.wav")
        );
        assert_eq!(
            apply_format(PathBuf::from("out.wav"), Some("mp3")).unwrap(),
            PathBuf::from("out.mp3")
        );
        assert_eq!(
            apply_format(PathBuf::from("take"), Some("wav")).unwrap(),
            PathBuf::from("take.wav")
        );
        assert_eq!(
            apply_format(PathBuf::from("song.flac"), Some("mp3")).unwrap(),
            PathBuf::from("song.mp3")
        );
        for bad in ["flac", "ogg", "WAV", ""] {
            let error = apply_format(PathBuf::from("out.wav"), Some(bad)).unwrap_err();
            assert_eq!(error.code, ExitCode::Usage, "{bad}");
            assert!(error.to_string().contains("wav or mp3"), "{error}");
        }
    }

    #[test]
    fn prosody_parsing_covers_both_modes() {
        assert_eq!(parse_prosody("natural").unwrap(), ProsodyMode::Natural);
        assert_eq!(parse_prosody("flat").unwrap(), ProsodyMode::Flat);
        for bad in ["bouncy", "", "NATURAL"] {
            let error = parse_prosody(bad).unwrap_err();
            assert_eq!(error.code, ExitCode::Usage, "{bad}");
            assert!(error.to_string().contains("flat or natural"), "{error}");
        }
    }

    #[test]
    fn style_resolution_covers_every_branch() {
        assert_eq!(
            resolve_style(None, None).unwrap(),
            StyleSpec::Default,
            "no flags: the voice's own style"
        );
        assert_eq!(
            resolve_style(Some("F1"), None).unwrap(),
            StyleSpec::Preset("F1".to_string())
        );
        assert_eq!(
            resolve_style(Some("supertonic-M2"), None).unwrap(),
            StyleSpec::Preset("supertonic-M2".to_string())
        );
        // Unknown preset: a tts error with a suggestion.
        let error = resolve_style(Some("M9"), None).unwrap_err();
        assert_eq!(error.code, ExitCode::Tts);
        assert!(
            error.to_string().contains("unknown style \"M9\""),
            "{error}"
        );
        // Contradicting flags are a usage error.
        let error = resolve_style(Some("F1"), Some(Path::new("s.json"))).unwrap_err();
        assert_eq!(error.code, ExitCode::Usage);
        assert!(error.to_string().contains("contradict"), "{error}");
        // A missing style file is a usage error naming the path.
        let error = resolve_style(None, Some(Path::new("/no/such/style.json"))).unwrap_err();
        assert_eq!(error.code, ExitCode::Usage);
        assert!(error.to_string().contains("style file"), "{error}");
        assert!(error.to_string().contains("no such file"), "{error}");
        // An existing file passes through as the file spec.
        let dir = std::env::temp_dir().join(format!("qinari-cli-style-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("my-style.json");
        std::fs::write(&file, "{}").expect("style file");
        assert_eq!(
            resolve_style(None, Some(&file)).unwrap(),
            StyleSpec::File(file.clone())
        );
    }

    #[test]
    fn steps_validation_matches_the_library_bounds() {
        assert_eq!(validate_steps(1).unwrap(), 1);
        assert_eq!(validate_steps(DEFAULT_STEPS).unwrap(), 8);
        assert_eq!(validate_steps(100).unwrap(), 100);
        for bad in [0_u8, 101, 255] {
            let error = validate_steps(bad).unwrap_err();
            assert_eq!(error.code, ExitCode::Usage, "{bad}");
            assert!(error.to_string().contains("1 and 100"), "{error}");
        }
    }

    #[test]
    fn planned_download_count_counts_missing_files_once() {
        let root = std::env::temp_dir().join(format!("qinari-cli-planned-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths {
            config_file: root.join("config.toml"),
            models_dir: root.join("models"),
            supertonic_dir: root.join("models/supertonic"),
            state_dir: root.join("state"),
        };
        let catalog = voices();
        let two: Vec<&'static VoiceEntry> = catalog.iter().take(2).collect();
        // Nothing installed: the shared bundle's files, two styles, and
        // the DNSMOS pair.
        assert_eq!(planned_download_count(&paths, &two), 6 + 2 + 2);
        assert_eq!(planned_download_count(&paths, &[]), 2);
        // The bundle present, one style present: only one style to fetch.
        for file in two[0].bundle.files {
            let path = paths.supertonic_bundle_file(file.rel_path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(&path, b"stub").expect("bundle stub");
        }
        std::fs::create_dir_all(paths.supertonic_style(two[0]).parent().expect("parent"))
            .expect("dir");
        std::fs::write(paths.supertonic_style(two[0]), b"stub").expect("style stub");
        assert_eq!(planned_download_count(&paths, &two), 1 + 2);
        // Everything present: nothing planned.
        std::fs::write(paths.supertonic_style(two[1]), b"stub").expect("style stub");
        assert_eq!(planned_download_count(&paths, &two), 2);
        // With the DNSMOS pair in place, a fully installed setup plans
        // zero files.
        let dnsmos = paths.models_dir.join("dnsmos");
        std::fs::create_dir_all(&dnsmos).expect("dir");
        for asset in &DNSMOS_ASSETS {
            std::fs::write(dnsmos.join(asset.rel_path), b"stub").expect("dnsmos stub");
        }
        assert_eq!(planned_download_count(&paths, &two), 0);
        assert_eq!(planned_download_count(&paths, &[]), 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
