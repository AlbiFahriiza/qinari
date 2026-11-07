//! The `qinari` binary: procedural audio maker and AI TTS in one command.
//!
//! `main` stays thin: parse, resolve the layout (`--config` wins over the
//! default location *before* the config is loaded), start file logging,
//! dispatch to a handler in [`commands`], and turn every failure into a
//! prepared message plus the documented exit code. Full failure detail
//! goes to `~/.local/state/qinari/qinari.log`, never to the terminal.

mod commands;
mod config;
mod config_cmd;
mod download;
mod interactive;
mod render;
mod xcode;

use clap::{ArgAction, Parser, Subcommand};
use commands::RunContext;
use std::path::PathBuf;
use tracing::error;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use xcode::ExitCode;
use xcode::QinariError;

/// Command-line surface of the `qinari` binary.
// allow_negative_numbers must also sit on every subcommand struct (the
// value `-14` for `--target-lufs` would otherwise parse as a flag).
#[derive(Parser)]
#[command(name = "qinari", version, about, allow_negative_numbers = true)]
struct Cli {
    /// Use this config file instead of the default location.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Suppress informational output (errors still print).
    #[arg(long, short = 'q', global = true)]
    quiet: bool,
    /// Log more detail; repeat for debug level.
    #[arg(long, short = 'v', global = true, action = ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    command: Commands,
}

/// The subcommands, one struct variant per command.
#[derive(Subcommand)]
enum Commands {
    /// Install voices and prepare the workspace.
    Setup(commands::SetupArgs),
    /// Render a procedural sound effect.
    Sfx(commands::SfxArgs),
    /// Render a long ambient bed.
    Ambient(commands::AmbientArgs),
    /// Render a composed music track.
    Music(commands::MusicArgs),
    /// Speak text offline or through a cloud service.
    Tts(commands::TtsArgs),
    /// Re-master an existing audio file.
    Master(commands::MasterArgs),
    /// Measure loudness, peak, and duration of a file.
    Analyze(commands::AnalyzeArgs),
    /// Time the reference renders.
    Bench(commands::BenchArgs),
    /// Show version, layout, config, and voices.
    Info(commands::InfoArgs),
    /// Prompted renders in a loop.
    Interactive(interactive::InteractiveArgs),
    /// Read or write the configuration file.
    Config(config_cmd::ConfigArgs),
    /// Print shell completion scripts.
    Completions(commands::CompletionsArgs),
    /// Scriptable health check (exit 8 when problems are found).
    Doctor(commands::DoctorArgs),
}

/// Builds the command tree for completion generation.
pub fn build_command() -> clap::Command {
    <Cli as clap::CommandFactory>::command()
}

fn main() {
    install_broken_pipe_hook();
    let cli = Cli::parse();
    let mut paths = match config::Paths::resolve() {
        Ok(paths) => paths,
        Err(failure) => die(&failure),
    };
    // The --config override must land before Config::load reads the file.
    if let Some(path) = &cli.config {
        paths.config_file.clone_from(path);
    }
    init_logging(&paths, cli.verbose);
    let config = match config::Config::load(&paths.config_file) {
        Ok(config) => config,
        Err(failure) => die(&failure),
    };
    let mut ctx = RunContext {
        paths,
        config,
        quiet: cli.quiet,
    };
    let code = match run(cli.command, &mut ctx) {
        Ok(code) => code,
        Err(failure) => die(&failure),
    };
    std::process::exit(code.value());
}

/// Dispatches one parsed subcommand.
fn run(command: Commands, ctx: &mut RunContext) -> Result<ExitCode, QinariError> {
    match command {
        Commands::Setup(args) => commands::setup(ctx, &args)?,
        Commands::Sfx(args) => commands::sfx(ctx, &args)?,
        Commands::Ambient(args) => commands::ambient(ctx, &args)?,
        Commands::Music(args) => commands::music(ctx, &args)?,
        Commands::Tts(args) => commands::tts(ctx, &args)?,
        Commands::Master(args) => commands::master_command(ctx, &args)?,
        Commands::Analyze(args) => commands::analyze(ctx, &args)?,
        Commands::Bench(args) => commands::bench(ctx, &args)?,
        Commands::Info(args) => commands::info(ctx, &args)?,
        Commands::Interactive(args) => interactive::interactive(ctx, &args),
        Commands::Config(args) => config_cmd::run(ctx, &args)?,
        Commands::Completions(args) => commands::completions(&args),
        Commands::Doctor(_) => {
            if !commands::doctor(ctx) {
                return Ok(ExitCode::Unhealthy);
            }
        }
    }
    Ok(ExitCode::Ok)
}

/// Reports a fatal failure: log it, print the prepared message, exit.
fn die(failure: &QinariError) -> ! {
    error!(code = failure.code.value(), "{failure}");
    eprintln!("{failure}");
    std::process::exit(failure.code.value());
}

/// Appends to `<state>/qinari.log`, falling back to stderr.
///
/// The writer is a plain `Arc<File>`; a file-logging appender is not used
/// on purpose (it drags the `time` crate into the tree, and the advisory
/// against that path is why this exists).
fn init_logging(paths: &config::Paths, verbose: u8) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    match open_log_file(paths) {
        Ok(file) => {
            let writer = std::sync::Arc::new(file);
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().with_ansi(false).with_writer(writer))
                .init();
        }
        Err(failure) => {
            eprintln!(
                "warning: could not open {} ({failure}); logging to stderr",
                paths.log_file().display()
            );
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().with_ansi(false).with_writer(std::io::stderr))
                .init();
        }
    }
}

/// Opens the log file, creating the state directory first.
fn open_log_file(paths: &config::Paths) -> std::io::Result<std::fs::File> {
    std::fs::create_dir_all(&paths.state_dir)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_file())
}

/// Exits 141 (SIGPIPE convention) when a print hits a closed pipe.
///
/// No unsafe code is involved: `println!` panics with a message naming the
/// broken pipe, and this hook turns exactly that panic into a silent exit.
/// Every other panic keeps the default reporter.
fn install_broken_pipe_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let broken_pipe = payload
            .downcast_ref::<String>()
            .is_some_and(|message| is_broken_pipe_message(message))
            || payload
                .downcast_ref::<&str>()
                .is_some_and(|message| is_broken_pipe_message(message));
        if broken_pipe {
            std::process::exit(141);
        }
        default_hook(info);
    }));
}

/// Whether a panic payload message describes a broken pipe.
fn is_broken_pipe_message(message: &str) -> bool {
    message.to_ascii_lowercase().contains("broken pipe")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_parses_a_full_render_invocation() {
        let cli = Cli::try_parse_from([
            "qinari",
            "--quiet",
            "sfx",
            "rain",
            "-o",
            "out.wav",
            "-d",
            "250ms",
            "--seed",
            "storm",
            "--set",
            "intensity=0.9",
            "--sr",
            "48000",
            "--target-lufs",
            "-14",
            "--ceiling",
            "-0.5",
            "--character",
            "warm",
            "--channels",
            "1",
        ])
        .expect("valid invocation");
        assert!(cli.quiet);
        assert_eq!(cli.verbose, 0);
        let Commands::Sfx(args) = cli.command else {
            panic!("expected the sfx subcommand");
        };
        assert_eq!(args.recipe.as_deref(), Some("rain"));
        assert_eq!(args.out, Some(PathBuf::from("out.wav")));
        assert_eq!(args.duration, "250ms");
        assert_eq!(args.seed.as_deref(), Some("storm"));
        assert_eq!(args.sets, vec!["intensity=0.9".to_string()]);
        assert_eq!(args.overrides.sr, Some(48_000));
        assert_eq!(args.overrides.target_lufs, Some(-14.0));
        assert_eq!(args.overrides.ceiling, Some(-0.5));
        assert_eq!(args.overrides.character.as_deref(), Some("warm"));
        assert_eq!(args.channels, 1);
    }

    #[test]
    fn negative_numbers_parse_after_the_subcommand() {
        // This is the regression the per-subcommand allow_negative_numbers
        // setting exists for: --target-lufs -14 after the subcommand.
        let cli = Cli::try_parse_from([
            "qinari",
            "sfx",
            "ui_click",
            "--target-lufs",
            "-14",
            "-o",
            "out.wav",
        ])
        .expect("negative values must parse");
        let Commands::Sfx(args) = cli.command else {
            panic!("expected the sfx subcommand");
        };
        assert_eq!(args.overrides.target_lufs, Some(-14.0));
    }

    #[test]
    fn global_flags_work_after_the_subcommand() {
        let cli = Cli::try_parse_from(["qinari", "info", "--json", "--config", "/tmp/c.toml"])
            .expect("globals after the subcommand");
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/c.toml")));
        assert!(matches!(cli.command, Commands::Info(_)));
    }

    #[test]
    fn completions_cover_every_shell() {
        for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
            let cli = Cli::try_parse_from(["qinari", "completions", shell]).expect("shell parses");
            let Commands::Completions(args) = cli.command else {
                panic!("expected completions");
            };
            assert_eq!(args.shell.to_string(), shell);
        }
        assert!(Cli::try_parse_from(["qinari", "completions", "tcsh"]).is_err());
    }

    #[test]
    fn broken_pipe_messages_are_detected() {
        assert!(is_broken_pipe_message(
            "failed printing to stdout: Broken pipe (os error 32)"
        ));
        assert!(!is_broken_pipe_message("index out of bounds"));
    }

    #[test]
    fn the_command_tree_builds() {
        let mut command = Cli::command();
        let help = command.render_help().to_string();
        for name in [
            "setup",
            "sfx",
            "ambient",
            "music",
            "tts",
            "master",
            "analyze",
            "bench",
            "info",
            "interactive",
            "config",
            "completions",
            "doctor",
        ] {
            assert!(help.contains(name), "missing subcommand {name}");
        }
    }

    #[test]
    fn tts_parses_the_full_tuning_surface() {
        let cli = Cli::try_parse_from([
            "qinari",
            "tts",
            "--lang",
            "id",
            "--voice",
            "supertonic-M2",
            "--engine",
            "local",
            "--speed",
            "1.2",
            "--steps",
            "16",
            "--seed",
            "storm",
            "--style",
            "F1",
            "--prosody",
            "flat",
            "--no-markup",
            "--format",
            "mp3",
            "--channels",
            "2",
            "--json",
            "-o",
            "take.wav",
            "Halo!",
        ])
        .expect("valid invocation");
        let Commands::Tts(args) = cli.command else {
            panic!("expected the tts subcommand");
        };
        assert_eq!(args.lang, "id");
        assert_eq!(args.voice.as_deref(), Some("supertonic-M2"));
        assert_eq!(args.engine.as_deref(), Some("local"));
        assert_eq!(args.speed, 1.2);
        assert_eq!(args.steps, 16);
        assert_eq!(args.seed.as_deref(), Some("storm"));
        assert_eq!(args.style.as_deref(), Some("F1"));
        assert_eq!(args.prosody, "flat");
        assert!(args.no_markup);
        assert_eq!(args.format.as_deref(), Some("mp3"));
        assert_eq!(args.channels, 2);
        assert!(args.json);
        assert_eq!(args.text.as_deref(), Some("Halo!"));
    }
}
