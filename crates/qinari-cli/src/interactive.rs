//! `qinari interactive`: a prompted render session.
//!
//! The loop picks a domain (sfx / music / tts), prompts for that domain's
//! parameters with the defaults shown, runs the very same command handler
//! the subcommands use, and returns to the picker. Leaving is always
//! clean: the `quit` item, Escape or `q` at a picker, or a closed prompt
//! channel (EOF, piped stdin) ends the session with exit 0. Ctrl-C at a
//! prompt restores the terminal and exits via SIGINT like any other
//! terminal program. A failed render prints its message and returns to
//! the picker — the session survives bad input.

use crate::commands::{self, OverrideFlags, RunContext};
use crate::xcode::QinariError;
use clap::Parser;
use dialoguer::{Input, Select};
use qinari_music::STYLES;
use qinari_sfx::recipes;
use qinari_tts::DEFAULT_STEPS;
use std::path::PathBuf;
use tracing::debug;

/// `qinari interactive`: prompted renders in a loop.
#[derive(Debug, Clone, Parser)]
#[command(allow_negative_numbers = true)]
pub struct InteractiveArgs {}

/// The prompts a session needs, as a trait so tests can script them.
///
/// `None` from any method means the user left the session (quit item,
/// Escape/q, EOF, or a prompt channel that cannot render).
pub(crate) trait Prompter {
    /// Picks one of `items` (returning its index) with `default`
    /// preselected.
    fn select(&mut self, prompt: &str, items: &[&str], default: usize) -> Option<usize>;
    /// Reads one line of free text, offering `default` on Enter.
    fn text(&mut self, prompt: &str, default: &str) -> Option<String>;
}

/// The terminal prompter: dialoguer's `Select` and `Input`.
struct DialoguerPrompter;

impl Prompter for DialoguerPrompter {
    fn select(&mut self, prompt: &str, items: &[&str], default: usize) -> Option<usize> {
        match Select::new()
            .with_prompt(prompt)
            .items(items)
            .default(default)
            .interact_opt()
        {
            Ok(choice) => choice,
            Err(error) => {
                // A non-terminal or closed stdin ends the session instead
                // of hanging it; the reason is log material, not terminal
                // material.
                debug!(%error, "prompt channel closed; leaving the session");
                None
            }
        }
    }

    fn text(&mut self, prompt: &str, default: &str) -> Option<String> {
        match Input::new()
            .with_prompt(prompt)
            .default(default.to_string())
            .interact_text()
        {
            Ok(text) => Some(text),
            Err(error) => {
                debug!(%error, "prompt channel closed; leaving the session");
                None
            }
        }
    }
}

/// Runs `qinari interactive`.
pub fn interactive(ctx: &mut RunContext, _args: &InteractiveArgs) {
    let mut prompts = DialoguerPrompter;
    run_session(ctx, &mut prompts);
}

/// The session loop: pick a domain, render, repeat until the user leaves.
pub fn run_session(ctx: &mut RunContext, prompts: &mut dyn Prompter) {
    const DOMAINS: [&str; 4] = ["sfx", "music", "tts", "quit"];
    if !ctx.quiet {
        println!(
            "qinari {} — interactive (choose quit, or press q/Esc to leave)",
            env!("CARGO_PKG_VERSION")
        );
    }
    loop {
        let Some(domain) = prompts.select("What should qinari render?", &DOMAINS, 0) else {
            return;
        };
        let result = match domain {
            0 => sfx_flow(ctx, prompts),
            1 => music_flow(ctx, prompts),
            2 => tts_flow(ctx, prompts),
            _ => return,
        };
        if let Err(failure) = result {
            eprintln!("{failure}");
        }
    }
}

/// One prompted sfx render.
///
/// # Errors
///
/// Propagates the sfx handler's errors (bad values, render or encode
/// failures) so the session can print them and return to the picker.
fn sfx_flow(ctx: &mut RunContext, prompts: &mut dyn Prompter) -> Result<(), QinariError> {
    let recipe_ids: Vec<&str> = recipes().iter().map(|recipe| recipe.id).collect();
    let Some(recipe) = prompts.select("Recipe", &recipe_ids, 0) else {
        return Ok(());
    };
    let Some(duration) = prompts.text("Duration (bare seconds or ms/s/m/h)", "3") else {
        return Ok(());
    };
    let Some(seed) = prompts.text("Seed (empty = time-based)", "") else {
        return Ok(());
    };
    let Some(out) = prompts.text("Output file", "out.wav") else {
        return Ok(());
    };
    let args = commands::SfxArgs {
        recipe: Some(recipe_ids[recipe].to_string()),
        list: false,
        out: Some(PathBuf::from(out)),
        duration,
        seed: non_empty(&seed),
        sets: Vec::new(),
        batch: None,
        channels: 2,
        json: false,
        overrides: OverrideFlags::default(),
    };
    commands::sfx(ctx, &args)
}

/// One prompted music render.
///
/// # Errors
///
/// Propagates the music handler's errors (bad values, render or encode
/// failures) so the session can print them and return to the picker.
fn music_flow(ctx: &mut RunContext, prompts: &mut dyn Prompter) -> Result<(), QinariError> {
    let style_ids: Vec<&str> = STYLES.iter().map(|style| style.id).collect();
    let Some(style) = prompts.select("Style", &style_ids, 0) else {
        return Ok(());
    };
    let Some(duration) = prompts.text("Duration (bare seconds or ms/s/m/h)", "30") else {
        return Ok(());
    };
    let Some(bpm) = prompts.text("Tempo in bpm (empty = the style's typical)", "") else {
        return Ok(());
    };
    let Some(seed) = prompts.text("Seed (empty = time-based)", "") else {
        return Ok(());
    };
    let Some(out) = prompts.text("Output file", "out.wav") else {
        return Ok(());
    };
    let bpm = match non_empty(&bpm) {
        Some(text) => Some(
            text.parse::<f32>()
                .map_err(|_| QinariError::usage(format!("tempo {text:?} is not a number")))?,
        ),
        None => None,
    };
    let args = commands::MusicArgs {
        style: Some(style_ids[style].to_string()),
        list: false,
        out: Some(PathBuf::from(out)),
        duration,
        bpm,
        seed: non_empty(&seed),
        channels: 2,
        json: false,
        overrides: OverrideFlags::default(),
    };
    commands::music(ctx, &args)
}

/// One prompted tts render.
///
/// # Errors
///
/// Propagates the tts handler's errors (bad values, missing voice or key,
/// engine or encode failures) so the session can print them and return to
/// the picker.
fn tts_flow(ctx: &mut RunContext, prompts: &mut dyn Prompter) -> Result<(), QinariError> {
    let langs = ["id", "en"];
    let Some(text) = prompts.text("Text to speak", "Qinari uji suara.") else {
        return Ok(());
    };
    let Some(lang) = prompts.select("Language", &langs, 0) else {
        return Ok(());
    };
    let Some(voice) = prompts.text("Voice id (empty = the language default)", "") else {
        return Ok(());
    };
    let Some(speed) = prompts.text("Speed (0.5..=2.0)", "1.0") else {
        return Ok(());
    };
    let Some(out) = prompts.text("Output file", "out.wav") else {
        return Ok(());
    };
    let speed: f32 = speed
        .trim()
        .parse()
        .map_err(|_| QinariError::usage(format!("speed {speed:?} is not a number")))?;
    let args = commands::TtsArgs {
        text: Some(text),
        file: None,
        lang: langs[lang].to_string(),
        voice: non_empty(&voice),
        engine: None,
        speed,
        steps: DEFAULT_STEPS,
        seed: None,
        style: None,
        style_file: None,
        prosody: "natural".to_string(),
        no_markup: false,
        format: None,
        channels: 1,
        out: Some(PathBuf::from(out)),
        json: false,
        overrides: OverrideFlags::default(),
    };
    commands::tts(ctx, &args)
}

/// Empty prompt answers mean "not given", so the command default applies.
fn non_empty(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Paths};
    use std::collections::VecDeque;

    /// A context over fresh temp directories (no models installed), quiet
    /// so the renders do not print during the test run.
    fn test_context(tag: &str) -> RunContext {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "qinari-cli-interactive-{tag}-{}",
            std::process::id()
        ));
        for sub in ["", "models", "models/supertonic", "state"] {
            std::fs::create_dir_all(dir.join(sub)).expect("temp dir");
        }
        RunContext {
            paths: Paths {
                config_file: dir.join("config.toml"),
                models_dir: dir.join("models"),
                supertonic_dir: dir.join("models/supertonic"),
                state_dir: dir.join("state"),
            },
            config: Config::default(),
            quiet: true,
        }
    }

    /// A prompter that replays scripted answers; an exhausted queue acts
    /// like EOF (the session leaves).
    #[derive(Default)]
    struct ScriptedPrompter {
        selects: VecDeque<usize>,
        texts: VecDeque<String>,
    }

    impl Prompter for ScriptedPrompter {
        fn select(&mut self, _prompt: &str, _items: &[&str], _default: usize) -> Option<usize> {
            self.selects.pop_front()
        }

        fn text(&mut self, _prompt: &str, _default: &str) -> Option<String> {
            self.texts.pop_front()
        }
    }

    #[test]
    fn a_scripted_sfx_session_renders_a_file() {
        let mut ctx = test_context("sfx");
        let out = ctx.paths.state_dir.join("session.wav");
        let mut prompts = ScriptedPrompter {
            selects: VecDeque::from([0, 0]),
            texts: VecDeque::from([
                "1".to_string(),
                "session-seed".to_string(),
                out.display().to_string(),
            ]),
        };
        run_session(&mut ctx, &mut prompts);
        assert!(out.is_file(), "the sfx render must land");
    }

    #[test]
    fn a_scripted_music_session_renders_a_file() {
        let mut ctx = test_context("music");
        let out = ctx.paths.state_dir.join("session.mp3");
        let mut prompts = ScriptedPrompter {
            selects: VecDeque::from([1, 0]),
            texts: VecDeque::from([
                "5".to_string(),
                String::new(),
                String::new(),
                out.display().to_string(),
            ]),
        };
        run_session(&mut ctx, &mut prompts);
        assert!(out.is_file(), "the music render must land");
    }

    #[test]
    fn a_failed_render_returns_to_the_picker() {
        // No voice is installed and no cloud key is set, so the tts render
        // fails; the session must print the failure and keep running (the
        // next picker answer is the exhausted queue = leave).
        let mut ctx = test_context("tts-fail");
        let out = ctx.paths.state_dir.join("never.wav");
        let mut prompts = ScriptedPrompter {
            selects: VecDeque::from([2, 0]),
            texts: VecDeque::from([
                "halo".to_string(),
                String::new(),
                "1.0".to_string(),
                out.display().to_string(),
            ]),
        };
        run_session(&mut ctx, &mut prompts);
        assert!(!out.exists(), "nothing may be written by a failed render");
    }

    #[test]
    fn an_exhausted_prompt_channel_leaves_immediately() {
        let mut ctx = test_context("eof");
        let mut prompts = ScriptedPrompter::default();
        run_session(&mut ctx, &mut prompts);
    }

    #[test]
    fn the_quit_item_leaves_the_session() {
        let mut ctx = test_context("quit");
        let mut prompts = ScriptedPrompter {
            selects: VecDeque::from([3]),
            texts: VecDeque::new(),
        };
        run_session(&mut ctx, &mut prompts);
    }

    #[test]
    fn empty_answers_become_none_and_are_trimmed() {
        assert_eq!(non_empty(""), None);
        assert_eq!(non_empty("   "), None);
        assert_eq!(non_empty(" storm "), Some("storm".to_string()));
    }
}
