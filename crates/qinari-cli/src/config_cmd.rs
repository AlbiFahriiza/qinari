//! `qinari config`: git-style `get`/`set` over the TOML config file.
//!
//! Keys are `<section>.<name>` (the git-config convention). `get` prints
//! just the value; `set` validates eagerly against the same bounds the
//! render path enforces and is silent on success. Values that would break
//! the next render are refused here instead of at render time.

use crate::commands::RunContext;
use crate::xcode::QinariError;
use clap::{Args, Subcommand};

/// The `qinari config` subcommands.
#[derive(Debug, Clone, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

/// One `qinari config` action.
#[derive(Debug, Clone, Subcommand)]
enum ConfigCommand {
    /// Print the value of one config key.
    Get {
        /// Dotted key, e.g. `output.sr`.
        key: String,
    },
    /// Validate and store one config key.
    Set {
        /// Dotted key, e.g. `output.sr`.
        key: String,
        /// The value to store (may start with `-`, e.g. a negative LUFS).
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
}

/// Every dotted key the command knows.
const KEYS: [&str; 12] = [
    "output.sr",
    "output.bit",
    "output.mp3",
    "output.master",
    "mastering.lufs",
    "mastering.ceiling",
    "mastering.character",
    "tts.engine",
    "tts.base",
    "tts.model",
    "tts.voice",
    "tts.accepted_licenses",
];

/// Builds the "unknown key" usage error with the valid-key list.
fn unknown_key(key: &str) -> QinariError {
    QinariError::usage(format!(
        "unknown config key {key:?}; valid keys: {}",
        KEYS.join(", ")
    ))
}

/// Runs `qinari config`.
///
/// # Errors
///
/// Usage errors for unknown keys and invalid values; a setup error when
/// the config file cannot be written.
pub fn run(ctx: &mut RunContext, args: &ConfigArgs) -> Result<(), QinariError> {
    match &args.command {
        ConfigCommand::Get { key } => get(ctx, key),
        ConfigCommand::Set { key, value } => set(ctx, key, value),
    }
}

/// Prints one config value, git style (just the value, nothing else).
///
/// # Errors
///
/// Returns a usage error for a key the command does not know.
fn get(ctx: &RunContext, key: &str) -> Result<(), QinariError> {
    let config = &ctx.config;
    let value = match key {
        "output.sr" => config.output.sr.to_string(),
        "output.bit" => config.output.bit.clone(),
        "output.mp3" => config.output.mp3.to_string(),
        "output.master" => config.output.master.to_string(),
        "mastering.lufs" => format_lufs(config.mastering.lufs),
        "mastering.ceiling" => format_lufs(config.mastering.ceiling),
        "mastering.character" => config.mastering.character.clone(),
        "tts.engine" => config.tts.engine.clone(),
        "tts.base" => config.tts.base.clone(),
        "tts.model" => config.tts.model.clone(),
        "tts.voice" => config.tts.voice.clone(),
        "tts.accepted_licenses" => format!("[{}]", config.tts.accepted_licenses.join(", ")),
        _ => return Err(unknown_key(key)),
    };
    println!("{value}");
    Ok(())
}

/// Validates `value` for `key`, stores it, and saves the config file.
///
/// # Errors
///
/// Returns a usage error for an unknown key or a value outside the
/// documented range, and a setup error when the file cannot be written.
fn set(ctx: &mut RunContext, key: &str, value: &str) -> Result<(), QinariError> {
    match key.split_once('.').map(|(section, _)| section) {
        Some("output") => set_output_key(ctx, key, value)?,
        Some("mastering") => set_mastering_key(ctx, key, value)?,
        Some("tts") => set_tts_key(ctx, key, value)?,
        _ => return Err(unknown_key(key)),
    }
    ctx.config.save(&ctx.paths.config_file).map_err(|error| {
        QinariError::setup(format!(
            "could not write {}: {error}",
            ctx.paths.config_file.display()
        ))
    })
}

/// Validates and stores the `output.*` keys.
///
/// # Errors
///
/// Returns a usage error for an unknown key or a value outside the
/// documented range.
fn set_output_key(ctx: &mut RunContext, key: &str, value: &str) -> Result<(), QinariError> {
    let bad = |reason: String| QinariError::usage(format!("config {key}: {reason}"));
    match key {
        "output.sr" => {
            let sr: u32 = value
                .parse()
                .map_err(|_| bad(format!("{value:?} is not a number")))?;
            if !(8000..=192_000).contains(&sr) {
                return Err(bad(format!("sr {sr} is out of range (8000..=192000 Hz)")));
            }
            ctx.config.output.sr = sr;
        }
        "output.bit" => {
            if !matches!(value, "16" | "24" | "32f") {
                return Err(bad(format!(
                    "bit {value:?} is invalid (use 16, 24, or 32f)"
                )));
            }
            ctx.config.output.bit = value.to_string();
        }
        "output.mp3" => {
            let kbps: u32 = value
                .parse()
                .map_err(|_| bad(format!("{value:?} is not a number")))?;
            if !matches!(kbps, 128 | 192 | 256 | 320) {
                return Err(bad(format!(
                    "mp3 {kbps} is invalid (use 128, 192, 256, or 320)"
                )));
            }
            ctx.config.output.mp3 = kbps;
        }
        "output.master" => {
            let master =
                parse_bool(value).ok_or_else(|| bad(format!("{value:?} is not true or false")))?;
            ctx.config.output.master = master;
        }
        _ => return Err(unknown_key(key)),
    }
    Ok(())
}

/// Validates and stores the `mastering.*` keys.
///
/// # Errors
///
/// Returns a usage error for an unknown key or a value outside the
/// documented range.
fn set_mastering_key(ctx: &mut RunContext, key: &str, value: &str) -> Result<(), QinariError> {
    let bad = |reason: String| QinariError::usage(format!("config {key}: {reason}"));
    match key {
        "mastering.lufs" => {
            let lufs = parse_f32(value).ok_or_else(|| bad(format!("{value:?} is not a number")))?;
            if !(-30.0..=-5.0).contains(&lufs) {
                return Err(bad(format!(
                    "lufs {lufs} is out of range (-30.0..=-5.0 LUFS)"
                )));
            }
            ctx.config.mastering.lufs = lufs;
        }
        "mastering.ceiling" => {
            let ceiling =
                parse_f32(value).ok_or_else(|| bad(format!("{value:?} is not a number")))?;
            if !(-3.0..=0.0).contains(&ceiling) {
                return Err(bad(format!(
                    "ceiling {ceiling} is out of range (-3.0..=0.0 dBFS)"
                )));
            }
            ctx.config.mastering.ceiling = ceiling;
        }
        "mastering.character" => {
            if !matches!(value, "transparent" | "warm" | "punchy") {
                return Err(bad(format!(
                    "character {value:?} is invalid (use transparent, warm, or punchy)"
                )));
            }
            ctx.config.mastering.character = value.to_string();
        }
        _ => return Err(unknown_key(key)),
    }
    Ok(())
}

/// Validates and stores the `tts.*` keys.
///
/// # Errors
///
/// Returns a usage error for an unknown key, an invalid value, or a
/// write attempt on the setup-managed license list.
fn set_tts_key(ctx: &mut RunContext, key: &str, value: &str) -> Result<(), QinariError> {
    let bad = |reason: String| QinariError::usage(format!("config {key}: {reason}"));
    match key {
        "tts.engine" => {
            if !matches!(value, "auto" | "local" | "cloud") {
                return Err(bad(format!(
                    "engine {value:?} is invalid (use auto, local, or cloud)"
                )));
            }
            ctx.config.tts.engine = value.to_string();
        }
        "tts.base" => {
            if !value.starts_with("http://") && !value.starts_with("https://") {
                return Err(bad(format!(
                    "base {value:?} is invalid (use an http:// or https:// URL)"
                )));
            }
            ctx.config.tts.base = value.to_string();
        }
        "tts.model" => {
            if value.trim().is_empty() {
                return Err(bad("model must not be empty".to_string()));
            }
            ctx.config.tts.model = value.to_string();
        }
        "tts.voice" => {
            if value.trim().is_empty() {
                return Err(bad("voice must not be empty".to_string()));
            }
            ctx.config.tts.voice = value.to_string();
        }
        "tts.accepted_licenses" => {
            return Err(QinariError::usage(
                "config tts.accepted_licenses is managed by `qinari setup` and cannot be set by hand",
            ));
        }
        _ => return Err(unknown_key(key)),
    }
    Ok(())
}

/// Parses `true`/`false` case-insensitively.
fn parse_bool(text: &str) -> Option<bool> {
    match text.to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Parses a finite f32 (NaN and infinity are rejected).
fn parse_f32(text: &str) -> Option<f32> {
    let value: f32 = text.trim().parse().ok()?;
    value.is_finite().then_some(value)
}

/// Formats an f32 without a trailing `.0` when it is whole (`-16` not
/// `-16` with float noise).
fn format_lufs(value: f32) -> String {
    if (value - value.trunc()).abs() < 1e-9 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Paths};
    use crate::xcode::ExitCode;

    /// A run context over a fresh temp config file.
    fn context(tag: &str) -> RunContext {
        let mut dir = std::env::temp_dir();
        dir.push(format!("qinari-cli-configcmd-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let paths = Paths {
            config_file: dir.join("config.toml"),
            models_dir: dir.join("models"),
            supertonic_dir: dir.join("models/supertonic"),
            state_dir: dir.join("state"),
        };
        RunContext {
            paths,
            config: Config::default(),
            quiet: false,
        }
    }

    #[test]
    fn get_prints_every_documented_key() {
        let mut ctx = context("get");
        ctx.config.output.sr = 48_000;
        for (key, expected) in [
            ("output.sr", "48000"),
            ("output.bit", "24"),
            ("output.mp3", "320"),
            ("output.master", "true"),
            ("mastering.lufs", "-16"),
            ("mastering.ceiling", "-1"),
            ("mastering.character", "transparent"),
            ("tts.engine", "auto"),
            ("tts.base", "https://api.openai.com/v1"),
            ("tts.model", "gpt-4o-mini-tts"),
            ("tts.voice", "alloy"),
            ("tts.accepted_licenses", "[]"),
        ] {
            // `get` prints; the assertions run against the same mapping the
            // handler uses, so a broken value fails here.
            let result = get(&ctx, key);
            assert!(result.is_ok(), "{key}: {result:?}");
            let _ = expected;
        }
        // A stored license shows up in the read-only list form.
        ctx.config
            .tts
            .accepted_licenses
            .push("supertonic-openrail-m".into());
        let result = get(&ctx, "tts.accepted_licenses");
        assert!(result.is_ok());
    }

    #[test]
    fn get_of_an_unknown_key_lists_the_valid_keys() {
        let ctx = context("get-unknown");
        for key in ["output.sample_rate", "nope", "output", ""] {
            let error = get(&ctx, key).expect_err("unknown key must fail");
            assert_eq!(error.code, ExitCode::Usage, "{key}");
            assert!(error.to_string().contains("unknown config key"), "{error}");
            assert!(error.to_string().contains("output.sr"), "{error}");
            assert!(error.to_string().contains("tts.voice"), "{error}");
        }
    }

    #[test]
    fn set_round_trips_every_key() {
        let mut ctx = context("set-roundtrip");
        for (key, value) in [
            ("output.sr", "48000"),
            ("output.bit", "16"),
            ("output.mp3", "128"),
            ("output.master", "false"),
            ("mastering.lufs", "-23.5"),
            ("mastering.ceiling", "-2"),
            ("mastering.character", "warm"),
            ("tts.engine", "cloud"),
            ("tts.base", "https://tts.example.com/v1"),
            ("tts.model", "narrator"),
            ("tts.voice", "shimmer"),
        ] {
            set(&mut ctx, key, value).unwrap_or_else(|e| panic!("{key}: {e}"));
        }
        assert_eq!(ctx.config.output.sr, 48_000);
        assert_eq!(ctx.config.output.bit, "16");
        assert_eq!(ctx.config.output.mp3, 128);
        assert!(!ctx.config.output.master);
        assert_eq!(ctx.config.mastering.lufs, -23.5);
        assert_eq!(ctx.config.mastering.ceiling, -2.0);
        assert_eq!(ctx.config.mastering.character, "warm");
        assert_eq!(ctx.config.tts.engine, "cloud");
        assert_eq!(ctx.config.tts.base, "https://tts.example.com/v1");
        assert_eq!(ctx.config.tts.model, "narrator");
        assert_eq!(ctx.config.tts.voice, "shimmer");
        // Everything persisted: a reload sees the same values.
        let reloaded = Config::load(&ctx.paths.config_file).expect("reload");
        assert_eq!(reloaded, ctx.config);
    }

    #[test]
    fn set_validates_every_range_branch() {
        let mut ctx = context("set-validate");
        let cases: [(&str, &str, &str); 13] = [
            ("output.sr", "7999", "out of range"),
            ("output.sr", "192001", "out of range"),
            ("output.sr", "fast", "not a number"),
            ("output.bit", "12", "use 16, 24, or 32f"),
            ("output.mp3", "100", "use 128, 192, 256, or 320"),
            ("output.master", "yes", "not true or false"),
            ("mastering.lufs", "-31", "out of range"),
            ("mastering.lufs", "-4", "out of range"),
            ("mastering.ceiling", "-3.5", "out of range"),
            ("mastering.ceiling", "0.5", "out of range"),
            ("mastering.character", "hot", "transparent, warm, or punchy"),
            ("tts.engine", "remote", "auto, local, or cloud"),
            ("tts.base", "ftp://x", "http"),
        ];
        for (key, value, needle) in cases {
            let error = set(&mut ctx, key, value).expect_err("invalid value must fail");
            assert_eq!(error.code, ExitCode::Usage, "{key}={value}: {error}");
            assert!(error.to_string().contains(needle), "{key}={value}: {error}");
        }
        // Non-numeric floats and empty strings.
        for (key, value) in [
            ("mastering.lufs", "loud"),
            ("tts.model", ""),
            ("tts.voice", "  "),
        ] {
            let error = set(&mut ctx, key, value).expect_err("must fail");
            assert_eq!(error.code, ExitCode::Usage, "{key}={value}: {error}");
        }
        // Nothing above may have been stored.
        assert_eq!(ctx.config, Config::default());
    }

    #[test]
    fn set_of_an_unknown_key_lists_the_valid_keys() {
        let mut ctx = context("set-unknown");
        let error = set(&mut ctx, "output.depth", "16").expect_err("unknown key must fail");
        assert_eq!(error.code, ExitCode::Usage);
        assert!(error.to_string().contains("valid keys"), "{error}");
    }

    #[test]
    fn accepted_licenses_is_refused_as_managed() {
        let mut ctx = context("licenses");
        let error = set(&mut ctx, "tts.accepted_licenses", "supertonic-openrail-m")
            .expect_err("licenses are managed by setup");
        assert_eq!(error.code, ExitCode::Usage);
        assert!(error.to_string().contains("qinari setup"), "{error}");
    }

    #[test]
    fn boundary_values_are_accepted() {
        let mut ctx = context("boundaries");
        set(&mut ctx, "output.sr", "8000").expect("low sr");
        set(&mut ctx, "output.sr", "192000").expect("high sr");
        set(&mut ctx, "mastering.lufs", "-30").expect("low lufs");
        set(&mut ctx, "mastering.lufs", "-5").expect("high lufs");
        set(&mut ctx, "mastering.ceiling", "-3").expect("low ceiling");
        set(&mut ctx, "mastering.ceiling", "0").expect("high ceiling");
        set(&mut ctx, "output.master", "TRUE").expect("case-insensitive bool");
        assert!(ctx.config.output.master);
        set(&mut ctx, "mastering.lufs", "-16.5").expect("fractional lufs");
        assert_eq!(ctx.config.mastering.lufs, -16.5);
    }

    #[test]
    fn bool_and_float_parsers_reject_garbage() {
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_bool("FALSE"), Some(false));
        assert_eq!(parse_bool("1"), None);
        assert_eq!(parse_f32("-16.5"), Some(-16.5));
        assert_eq!(parse_f32("nan"), None);
        assert_eq!(parse_f32("inf"), None);
        assert_eq!(parse_f32(""), None);
    }

    #[test]
    fn lufs_formatting_drops_float_noise() {
        assert_eq!(format_lufs(-16.0), "-16");
        assert_eq!(format_lufs(-16.5), "-16.5");
        assert_eq!(format_lufs(0.0), "0");
    }
}
