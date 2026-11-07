//! End-to-end tests for the `qinari` binary (`assert_cmd`).
//!
//! Every test runs in its own sandbox: an isolated `HOME` plus explicit
//! `XDG_*` overrides, so the resolved layout never leaks into or out of
//! the developer's real home, and a private working directory for the
//! files the commands write.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
use std::process::Output;

const ALL_RECIPES: [&str; 30] = [
    "rain",
    "wind",
    "thunder",
    "ocean",
    "fire",
    "stream",
    "explosion",
    "impact",
    "whoosh",
    "sword",
    "laser",
    "blaster",
    "teleport",
    "forcefield",
    "robot_talk",
    "ui_success",
    "ui_error",
    "ui_click",
    "sonar",
    "alarm",
    "engine",
    "glitch",
    "riser",
    "hit",
    "sub_drop",
    "heartbeat",
    "footsteps",
    "spell_cast",
    "crickets",
    "birdsong",
];

const ALL_PRESETS: [&str; 5] = ["storm", "campfire", "ocean", "spaceship", "meadow"];
const ALL_STYLES: [&str; 5] = ["lofi", "ambient", "synthwave", "cinematic", "chiptune"];

/// One test's isolated home and working directory.
struct Sandbox {
    home: PathBuf,
    work: PathBuf,
}

impl Sandbox {
    /// A fresh home and work directory under the system temp dir.
    fn new(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("qinari-cli-e2e-{tag}-{}", std::process::id()));
        let home = root.join("home");
        let work = root.join("work");
        std::fs::create_dir_all(&home).expect("create home");
        std::fs::create_dir_all(&work).expect("create work dir");
        Self { home, work }
    }

    /// A `qinari` invocation pinned to this sandbox.
    fn qinari(&self) -> Command {
        let mut command = Command::cargo_bin("qinari").expect("qinari binary is built");
        command
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env_remove("QINARI_TTS_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .env_remove("RUST_LOG")
            .env_remove("QINARI_SUPERTONIC_DIR")
            .current_dir(&self.work);
        command
    }

    /// Runs and returns the completed process (already asserted to pass).
    fn ok(&self, args: &[&str]) -> Output {
        self.qinari()
            .args(args)
            .assert()
            .success()
            .get_output()
            .clone()
    }

    /// Path inside this sandbox's working directory.
    fn work_path(&self, name: &str) -> PathBuf {
        self.work.join(name)
    }
}

/// Decodes the stdout text of the binary.
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The `integrated` LUFS value from an `analyze` run.
fn integrated_of(output: &Output) -> f64 {
    let text = stdout(output);
    let line = text
        .lines()
        .find(|line| line.starts_with("integrated "))
        .expect("an integrated line");
    let value = line
        .strip_prefix("integrated ")
        .expect("the integrated prefix")
        .trim_end_matches(" LUFS");
    value.parse().expect("a numeric LUFS value")
}

/// The duration in seconds from an `analyze` run.
fn duration_of(output: &Output) -> f64 {
    let text = stdout(output);
    let line = text
        .lines()
        .find(|line| line.starts_with("duration "))
        .expect("a duration line");
    let rest = line.strip_prefix("duration ").expect("the duration prefix");
    rest.split_once(" s @")
        .expect("seconds unit in the duration line")
        .0
        .parse()
        .expect("a numeric duration")
}

#[test]
fn version_and_help_work() {
    let sandbox = Sandbox::new("version");
    sandbox
        .qinari()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("qinari 0.1.5"));
    sandbox
        .qinari()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Render a procedural sound effect"))
        .stdout(predicate::str::contains("--config"));
}

#[test]
fn sfx_listing_shows_all_30_recipes_with_parameters() {
    let sandbox = Sandbox::new("sfx-list");
    let output = sandbox.ok(&["sfx", "--list"]);
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().collect();
    for recipe in ALL_RECIPES {
        assert!(lines.contains(&recipe), "missing {recipe}");
    }
    assert!(text.contains("--set intensity=<0..1>"));
    assert!(text.contains("Rain density"));
}

#[test]
fn ambient_listing_shows_all_5_presets() {
    let sandbox = Sandbox::new("ambient-list");
    let output = sandbox.ok(&["ambient", "--list"]);
    let text = stdout(&output);
    for preset in ALL_PRESETS {
        assert!(text.contains(preset), "missing {preset}");
    }
}

#[test]
fn music_listing_shows_all_5_styles() {
    let sandbox = Sandbox::new("music-list");
    let output = sandbox.ok(&["music", "--list"]);
    let text = stdout(&output);
    for style in ALL_STYLES {
        assert!(text.contains(style), "missing {style}");
    }
    assert!(text.contains("bpm"));
}

#[test]
fn setup_voice_listing_marks_the_restricted_license() {
    let sandbox = Sandbox::new("voice-list");
    let output = sandbox.ok(&["setup", "--list-voices"]);
    let text = stdout(&output);
    assert!(text.contains("shared engine bundle"));
    assert!(text.contains("supertonic-M1"));
    assert!(text.contains("supertonic-F5"));
    assert!(text.contains("OpenRAIL-M"));
    for id in [
        "supertonic-M1",
        "supertonic-M5",
        "supertonic-F1",
        "supertonic-F5",
    ] {
        let line = text
            .lines()
            .find(|line| line.starts_with(id))
            .unwrap_or_else(|| panic!("the {id} line"));
        assert!(line.contains("[RESTRICTED]"), "{line}");
    }
    // The Piper voices are gone.
    assert!(!text.contains("libritts"));
    assert!(!text.contains("news_tts"));
}

#[test]
fn sfx_renders_decodable_non_silent_audio_at_the_requested_duration() {
    let sandbox = Sandbox::new("sfx-render");
    sandbox.ok(&[
        "sfx", "rain", "-o", "bed.wav", "-d", "2.5", "--seed", "7", "--quiet",
    ]);
    assert!(sandbox.work_path("bed.wav").is_file());
    let measured = sandbox.ok(&["analyze", "bed.wav"]);
    let duration = duration_of(&measured);
    assert!((duration - 2.5).abs() < 0.05, "duration {duration}");
    let loudness = integrated_of(&measured);
    // Mastering is on by default and targets -16 LUFS.
    assert!(
        (loudness + 16.0).abs() <= 1.0,
        "expected ~-16 LUFS, measured {loudness}"
    );
}

#[test]
fn unknown_recipe_gets_a_did_you_mean_hint() {
    let sandbox = Sandbox::new("unknown-recipe");
    sandbox
        .qinari()
        .args(["sfx", "rian", "-o", "x.wav"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("did you mean"));
}

#[test]
fn unknown_parameter_gets_a_did_you_mean_hint() {
    let sandbox = Sandbox::new("unknown-param");
    sandbox
        .qinari()
        .args(["sfx", "rain", "--set", "intens=0.9", "-o", "x.wav"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("did you mean"));
}

#[test]
fn flac_is_rejected_as_an_encode_error() {
    let sandbox = Sandbox::new("flac");
    sandbox
        .qinari()
        .args(["sfx", "rain", "-o", "song.flac"])
        .assert()
        .failure()
        .code(6)
        .stderr(predicate::str::contains(".wav and .mp3 only"));
    assert!(!sandbox.work_path("song.flac").exists());
}

#[test]
fn the_same_seed_renders_byte_identical_files() {
    let sandbox = Sandbox::new("determinism");
    sandbox.ok(&[
        "sfx", "thunder", "-d", "2", "--seed", "9", "-o", "a.wav", "--quiet",
    ]);
    sandbox.ok(&[
        "sfx", "thunder", "-d", "2", "--seed", "9", "-o", "b.wav", "--quiet",
    ]);
    let first = std::fs::read(sandbox.work_path("a.wav")).expect("first render");
    let second = std::fs::read(sandbox.work_path("b.wav")).expect("second render");
    assert_eq!(first, second, "same seed must render identical bytes");
}

#[test]
fn batch_renders_numbered_variations_from_the_seed() {
    let sandbox = Sandbox::new("batch");
    sandbox.ok(&[
        "sfx", "laser", "-d", "1", "--seed", "5", "--batch", "3", "-o", "shot.wav", "--quiet",
    ]);
    for name in ["shot-000.wav", "shot-001.wav", "shot-002.wav"] {
        assert!(sandbox.work_path(name).is_file(), "missing {name}");
    }
    let first = std::fs::read(sandbox.work_path("shot-000.wav")).expect("first variation");
    let second = std::fs::read(sandbox.work_path("shot-001.wav")).expect("second variation");
    assert_ne!(first, second, "the run offset must vary the seed");
}

#[test]
fn mp3_renders_round_trip_through_the_decoder() {
    let sandbox = Sandbox::new("mp3");
    sandbox.ok(&[
        "sfx", "thunder", "-d", "2", "--seed", "3", "-o", "m.mp3", "--quiet",
    ]);
    assert!(sandbox.work_path("m.mp3").is_file());
    let measured = sandbox.ok(&["analyze", "m.mp3"]);
    let duration = duration_of(&measured);
    assert!((duration - 2.0).abs() < 0.2, "duration {duration}");
    // A decodeable, non-silent file reports a loudness value at all.
    let _ = integrated_of(&measured);
}

#[test]
fn master_and_analyze_hit_the_default_target() {
    let sandbox = Sandbox::new("master");
    sandbox.ok(&[
        "sfx",
        "ocean",
        "-d",
        "3",
        "--seed",
        "2",
        "-o",
        "src.wav",
        "--no-master",
        "--quiet",
    ]);
    sandbox.ok(&["master", "src.wav", "-o", "done.wav", "--quiet"]);
    let measured = sandbox.ok(&["analyze", "done.wav"]);
    let loudness = integrated_of(&measured);
    assert!(
        (loudness + 16.0).abs() <= 1.0,
        "master should reach ~-16 LUFS, measured {loudness}"
    );
    assert!(!sandbox.work_path("done.wav.qinari-tmp").exists());
}

#[test]
fn master_rejects_the_master_flags() {
    let sandbox = Sandbox::new("master-flags");
    sandbox
        .qinari()
        .args(["master", "in.wav", "--no-master"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("always masters"));
}

#[test]
fn tts_unknown_voice_is_a_tts_error() {
    let sandbox = Sandbox::new("tts-voice");
    sandbox
        .qinari()
        .args(["tts", "--lang", "en", "--voice", "librits-x", "hi"])
        .assert()
        .failure()
        .code(7)
        .stderr(predicate::str::contains("unknown voice"));
}

#[test]
fn tts_auto_without_an_installed_voice_needs_a_cloud_key() {
    let sandbox = Sandbox::new("tts-auto");
    sandbox
        .qinari()
        .args(["tts", "--lang", "en", "hello there"])
        .assert()
        .failure()
        .code(7)
        .stderr(predicate::str::contains("no API key"));
}

#[test]
fn tts_local_without_an_installed_voice_points_at_setup() {
    let sandbox = Sandbox::new("tts-local");
    sandbox
        .qinari()
        .args(["tts", "--lang", "id", "--engine", "local", "halo"])
        .assert()
        .failure()
        .code(7)
        .stderr(predicate::str::contains("qinari setup"));
}

#[test]
fn bad_durations_are_usage_errors() {
    let sandbox = Sandbox::new("bad-duration");
    sandbox
        .qinari()
        .args(["sfx", "rain", "-d", "700"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("0.05 and 600"));
    sandbox
        .qinari()
        .args(["ambient", "storm", "-d", "0.5"])
        .assert()
        .failure()
        .code(2);
    sandbox
        .qinari()
        .args(["music", "lofi", "-d", "3"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("5 and 600"));
    sandbox
        .qinari()
        .args(["sfx", "rain", "-d", "2parsec"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("could not parse duration"));
}

#[test]
fn bad_lang_is_a_usage_error() {
    let sandbox = Sandbox::new("bad-lang");
    sandbox
        .qinari()
        .args(["tts", "--lang", "fr", "bonjour"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("--lang must be id or en"));
}

#[test]
fn doctor_reports_unhealthy_on_an_empty_home() {
    let sandbox = Sandbox::new("doctor-empty");
    sandbox
        .qinari()
        .arg("doctor")
        .assert()
        .code(8)
        .stdout(predicate::str::contains("problems found"));
}

/// Installs the doctor stubs: config file, the engine bundle files, one
/// voice style, and the DNSMOS model pair.
fn install_doctor_stubs(sandbox: &Sandbox) {
    let config = sandbox.home.join(".config/qinari");
    std::fs::create_dir_all(&config).expect("config dir");
    std::fs::write(config.join("config.toml"), "").expect("config file");
    let model = sandbox.home.join(".local/share/qinari/models/supertonic");
    let onnx = model.join("onnx");
    std::fs::create_dir_all(&onnx).expect("onnx dir");
    for name in [
        "duration_predictor.onnx",
        "text_encoder.onnx",
        "vector_estimator.onnx",
        "vocoder.onnx",
        "tts.json",
        "unicode_indexer.json",
    ] {
        std::fs::write(onnx.join(name), b"stub").expect("bundle stub");
    }
    let styles = model.join("voice_styles");
    std::fs::create_dir_all(&styles).expect("styles dir");
    std::fs::write(styles.join("M1.json"), b"{}").expect("style stub");
    let dnsmos = sandbox.home.join(".local/share/qinari/models/dnsmos");
    std::fs::create_dir_all(&dnsmos).expect("dnsmos dir");
    for name in ["sig_bak_ovr.onnx", "model_v8.onnx"] {
        std::fs::write(dnsmos.join(name), b"stub").expect("dnsmos stub");
    }
}

#[test]
fn doctor_is_healthy_with_all_stubs_in_place() {
    let sandbox = Sandbox::new("doctor-stubs");
    install_doctor_stubs(&sandbox);
    sandbox
        .qinari()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("healthy"))
        .stdout(predicate::str::contains("voice supertonic-M1 is installed"));
}

#[test]
fn info_json_reports_the_whole_state() {
    let sandbox = Sandbox::new("info-json");
    let output = sandbox.ok(&["info", "--json"]);
    let text = stdout(&output);
    assert!(text.contains("\"version\":\"0.1.5\""), "{text}");
    assert!(text.contains("\"paths\""));
    assert!(text.contains("\"engine\":\"auto\""));
    assert!(text.contains("\"supertonic-M1\""));
    assert!(text.contains("\"installed\":false"));
    // The JSON is valid: the document ends in a closing brace.
    assert!(text.trim_end().ends_with('}'), "{text}");
}

#[test]
fn the_config_flag_is_wired_into_the_layout() {
    let sandbox = Sandbox::new("config-flag");
    let custom = sandbox.work_path("custom.toml");
    std::fs::write(&custom, "[output]\nsr = 48000\n").expect("write config");
    let output = sandbox.ok(&["--config", "custom.toml", "info"]);
    let text = stdout(&output);
    assert!(text.contains("config file: custom.toml"), "{text}");
    // And the file is actually loaded, not just named.
    sandbox
        .qinari()
        .args([
            "--config",
            "custom.toml",
            "sfx",
            "ui_click",
            "-d",
            "0.5",
            "-o",
            "c.wav",
            "--quiet",
        ])
        .assert()
        .success();
    let measured = sandbox.ok(&["analyze", "c.wav"]);
    assert!(
        stdout(&measured).contains("48000 Hz"),
        "sr from the custom config"
    );
}

#[test]
fn negative_numbers_parse_after_the_subcommand() {
    // The per-subcommand allow_negative_numbers setting exists for this:
    // --target-lufs -14 must be a value, not a flag. A dense bed (rain)
    // verifies the value also reaches the mastering chain.
    let sandbox = Sandbox::new("negative");
    sandbox
        .qinari()
        .args([
            "sfx",
            "rain",
            "-d",
            "2",
            "-o",
            "n.wav",
            "--target-lufs",
            "-14",
            "--quiet",
        ])
        .assert()
        .success();
    let measured = sandbox.ok(&["analyze", "n.wav"]);
    let loudness = integrated_of(&measured);
    assert!(
        (loudness + 14.0).abs() <= 1.0,
        "target was -14, measured {loudness}"
    );
}

#[test]
fn completions_are_generated_for_every_shell() {
    let sandbox = Sandbox::new("completions");
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        sandbox
            .qinari()
            .args(["completions", shell])
            .assert()
            .success()
            .stdout(predicate::str::contains("qinari"));
    }
}

#[test]
fn analyze_names_a_missing_file_as_a_usage_error() {
    let sandbox = Sandbox::new("analyze-missing");
    sandbox
        .qinari()
        .args(["analyze", "no-such-file.wav"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("could not decode"));
}

#[test]
fn analyze_speech_prints_the_speech_section() {
    let sandbox = Sandbox::new("analyze-speech");
    sandbox.ok(&["sfx", "rain", "-d", "1", "--seed", "s", "-o", "r.wav"]);
    let measured = sandbox.ok(&["analyze", "r.wav", "--speech"]);
    let text = stdout(&measured);
    let line = text
        .lines()
        .find(|line| line.starts_with("speech   "))
        .expect("a speech line");
    assert!(line.contains("voiced"), "{line}");
    assert!(line.contains("pauses"), "{line}");
    // Without the flag the section stays away.
    let plain = sandbox.ok(&["analyze", "r.wav"]);
    assert!(!stdout(&plain).contains("speech   "));
}

#[test]
fn analyze_mos_without_models_points_at_setup() {
    let sandbox = Sandbox::new("analyze-mos-missing");
    sandbox.ok(&["sfx", "rain", "-d", "1", "--seed", "s", "-o", "r.wav"]);
    sandbox
        .qinari()
        .args(["analyze", "r.wav", "--mos"])
        .assert()
        .failure()
        .code(3)
        .stderr(predicate::str::contains("qinari setup"));
}

#[test]
fn analyze_json_carries_the_speech_section() {
    let sandbox = Sandbox::new("analyze-json");
    sandbox.ok(&["sfx", "rain", "-d", "1", "--seed", "s", "-o", "r.wav"]);
    let measured = sandbox.ok(&["analyze", "r.wav", "--json"]);
    let value: serde_json::Value = serde_json::from_str(&stdout(&measured)).expect("valid json");
    assert!(
        value["duration_s"].as_f64().expect("duration") > 0.9,
        "{value}"
    );
    assert!(value["speech"]["pause_count"].is_u64(), "{value}");
    assert!(
        value["lufs"].is_number() || value["lufs"].is_null(),
        "{value}"
    );
    assert!(value.get("dnsmos").is_none(), "no dnsmos without --mos");
}

#[test]
fn corrupt_config_is_a_hard_setup_error_naming_the_file() {
    let sandbox = Sandbox::new("corrupt-config");
    let config = sandbox.home.join(".config/qinari");
    std::fs::create_dir_all(&config).expect("config dir");
    std::fs::write(config.join("config.toml"), "not [toml at all").expect("write config");
    sandbox
        .qinari()
        .args(["sfx", "rain", "-d", "1", "-o", "x.wav"])
        .assert()
        .failure()
        .code(3)
        .stderr(predicate::str::contains("could not parse"))
        .stderr(predicate::str::contains("config.toml"));
}

#[test]
fn quiet_suppresses_informational_output() {
    let sandbox = Sandbox::new("quiet");
    let output = sandbox.ok(&["sfx", "ui_click", "-d", "0.5", "-o", "q.wav", "--quiet"]);
    assert!(
        stdout(&output).trim().is_empty(),
        "quiet must not print the summary: {:?}",
        stdout(&output)
    );
}

#[test]
fn ambient_and_music_render_through_the_shared_pipeline() {
    let sandbox = Sandbox::new("engines");
    sandbox.ok(&[
        "ambient", "campfire", "-d", "5", "--seed", "4", "-o", "fire.wav", "--quiet",
    ]);
    let fire = sandbox.ok(&["analyze", "fire.wav"]);
    assert!(
        (duration_of(&fire) - 5.0).abs() < 0.1,
        "duration {}",
        duration_of(&fire)
    );
    let loudness = integrated_of(&fire);
    assert!((loudness + 16.0).abs() <= 1.0, "target -16, got {loudness}");

    sandbox.ok(&[
        "music", "chiptune", "-d", "8", "--seed", "4", "-o", "chip.wav", "--quiet",
    ]);
    let chip = sandbox.ok(&["analyze", "chip.wav"]);
    assert!(duration_of(&chip) >= 8.0, "music runs at least the request");
    let _ = integrated_of(&chip);
}

#[test]
fn unknown_style_and_preset_get_hints() {
    let sandbox = Sandbox::new("unknown-style");
    sandbox
        .qinari()
        .args(["music", "lofy", "-d", "10"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("did you mean"));
    sandbox
        .qinari()
        .args(["ambient", "strom", "-d", "10"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("did you mean"));
}

#[test]
fn render_json_prints_one_object_per_file_to_stdout() {
    let sandbox = Sandbox::new("render-json");
    let output = sandbox.ok(&[
        "sfx",
        "ui_click",
        "-d",
        "1",
        "--seed",
        "3",
        "-o",
        "click.wav",
        "--json",
    ]);
    let text = stdout(&output);
    let lines: Vec<&str> = text.trim().lines().collect();
    assert_eq!(lines.len(), 1, "one JSON object, nothing else: {text}");
    let report: serde_json::Value = serde_json::from_str(lines[0]).expect("valid json");
    assert_eq!(report["path"], "click.wav");
    assert_eq!(report["format"], "wav");
    assert_eq!(report["sample_rate"], 44_100);
    assert_eq!(report["channels"], 2);
    assert_eq!(report["bits"], "24");
    assert_eq!(report["mastered"], true);
    assert_eq!(report["engine"], serde_json::Value::Null);
    assert_eq!(report["voice"], serde_json::Value::Null);
    assert_eq!(report["sha256"].as_str().map(str::len), Some(64));
    assert!(report["duration_s"].as_f64().is_some_and(|d| d > 0.9));
    assert!(report["size_bytes"].as_u64().is_some_and(|s| s > 0));
    assert!(report["rtf"].as_f64().is_some_and(|r| r >= 0.0));
    assert!(sandbox.work_path("click.wav").is_file());
    // The human summary moved to stderr so stdout stays parseable.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("click.wav"), "{stderr}");
}

#[test]
fn render_json_reports_null_bits_for_mp3() {
    let sandbox = Sandbox::new("render-json-mp3");
    let output = sandbox.ok(&[
        "sfx",
        "ui_click",
        "-d",
        "1",
        "--seed",
        "3",
        "-o",
        "click.mp3",
        "--json",
        "--quiet",
    ]);
    let report: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("valid json");
    assert_eq!(report["format"], "mp3");
    assert_eq!(report["bits"], serde_json::Value::Null);
    // --json still prints under --quiet; the human stderr line does not.
    assert!(
        String::from_utf8_lossy(&output.stderr).trim().is_empty(),
        "quiet + json must stay clean on stderr"
    );
}

#[test]
fn render_json_prints_one_line_per_batch_file() {
    let sandbox = Sandbox::new("render-json-batch");
    let output = sandbox.ok(&[
        "sfx", "laser", "-d", "0.5", "--seed", "4", "--batch", "2", "--json",
    ]);
    let text = stdout(&output);
    let lines: Vec<&str> = text.trim().lines().collect();
    assert_eq!(lines.len(), 2, "one JSON object per rendered file");
    let first: serde_json::Value = serde_json::from_str(lines[0]).expect("first object");
    let second: serde_json::Value = serde_json::from_str(lines[1]).expect("second object");
    assert_eq!(first["path"], "out-000.wav");
    assert_eq!(second["path"], "out-001.wav");
}

#[test]
fn music_json_reports_the_music_lane() {
    let sandbox = Sandbox::new("music-json");
    let output = sandbox.ok(&["music", "lofi", "-d", "5", "--seed", "2", "--json"]);
    let report: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("valid json");
    assert_eq!(report["format"], "wav");
    assert_eq!(report["engine"], serde_json::Value::Null);
    assert_eq!(report["voice"], serde_json::Value::Null);
    assert!(report["duration_s"].as_f64().is_some_and(|d| d >= 5.0));
}

#[test]
fn tts_new_flags_are_validated_with_usage_errors() {
    let sandbox = Sandbox::new("tts-flags");
    sandbox
        .qinari()
        .args(["tts", "--lang", "en", "--prosody", "bouncy", "hi"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "--prosody must be flat or natural",
        ));
    sandbox
        .qinari()
        .args(["tts", "--lang", "en", "--steps", "0", "hi"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "--steps must be between 1 and 100",
        ));
    sandbox
        .qinari()
        .args(["tts", "--lang", "en", "--steps", "101", "hi"])
        .assert()
        .failure()
        .code(2);
    sandbox
        .qinari()
        .args(["tts", "--lang", "en", "--format", "flac", "hi"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("--format must be wav or mp3"));
    sandbox
        .qinari()
        .args([
            "tts",
            "--lang",
            "en",
            "--style",
            "F1",
            "--style-file",
            "s.json",
            "hi",
        ])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("contradict each other"));
    // An unknown style preset is a tts error with the hint discipline.
    sandbox
        .qinari()
        .args(["tts", "--lang", "en", "--style", "M9", "hi"])
        .assert()
        .failure()
        .code(7)
        .stderr(predicate::str::contains("unknown style"));
    // A missing style file is a usage error naming the path.
    sandbox
        .qinari()
        .args(["tts", "--lang", "en", "--style-file", "no-such.json", "hi"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("no such file"));
}

#[test]
fn tts_missing_text_file_is_a_usage_error() {
    let sandbox = Sandbox::new("tts-missing-file");
    sandbox
        .qinari()
        .args(["tts", "--lang", "id", "--file", "no-such.txt", "halo"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("could not read text file"));
}

#[test]
fn master_missing_input_is_a_usage_error() {
    let sandbox = Sandbox::new("master-missing");
    sandbox
        .qinari()
        .args(["master", "no-such-input.wav"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("could not decode"));
}

#[test]
fn config_get_and_set_round_trip() {
    let sandbox = Sandbox::new("config-roundtrip");
    let output = sandbox.ok(&["config", "get", "output.sr"]);
    assert_eq!(stdout(&output).trim(), "44100");
    // set is silent on success (git style) and persists.
    let output = sandbox.ok(&["config", "set", "output.sr", "48000"]);
    assert!(
        stdout(&output).trim().is_empty(),
        "config set prints nothing on success"
    );
    let output = sandbox.ok(&["config", "get", "output.sr"]);
    assert_eq!(stdout(&output).trim(), "48000");
    // The stored value actually drives renders.
    sandbox.ok(&["sfx", "ui_click", "-d", "0.5", "-o", "cfg.wav", "--quiet"]);
    let measured = sandbox.ok(&["analyze", "cfg.wav"]);
    assert!(stdout(&measured).contains("48000 Hz"));
}

#[test]
fn config_get_and_set_work_through_the_config_flag() {
    let sandbox = Sandbox::new("config-flag-file");
    let custom = sandbox.work_path("custom.toml");
    sandbox.ok(&[
        "--config",
        "custom.toml",
        "config",
        "set",
        "output.mp3",
        "128",
    ]);
    // The file was created and holds the value.
    assert!(custom.is_file());
    let output = sandbox.ok(&["--config", "custom.toml", "config", "get", "output.mp3"]);
    assert_eq!(stdout(&output).trim(), "128");
    // The sandbox default config was not touched.
    let output = sandbox.ok(&["config", "get", "output.mp3"]);
    assert_eq!(stdout(&output).trim(), "320");
}

#[test]
fn config_unknown_key_is_a_usage_error_listing_keys() {
    let sandbox = Sandbox::new("config-unknown");
    sandbox
        .qinari()
        .args(["config", "get", "output.sample_rate"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("unknown config key"))
        .stderr(predicate::str::contains("output.sr"))
        .stderr(predicate::str::contains("tts.voice"));
    sandbox
        .qinari()
        .args(["config", "set", "nope", "1"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("valid keys"));
}

#[test]
fn config_invalid_values_are_usage_errors() {
    let sandbox = Sandbox::new("config-invalid");
    for (key, value) in [
        ("output.sr", "44100.5"),
        ("output.sr", "192001"),
        ("output.bit", "12"),
        ("output.mp3", "100"),
        ("output.master", "yes"),
        ("mastering.lufs", "-31"),
        ("mastering.ceiling", "-5"),
        ("mastering.character", "hot"),
        ("tts.engine", "remote"),
        ("tts.base", "ftp://x"),
        ("tts.model", ""),
    ] {
        sandbox
            .qinari()
            .args(["config", "set", key, value])
            .assert()
            .failure()
            .code(2)
            .stderr(predicate::str::contains("config"))
            .stderr(predicate::str::contains(key));
    }
    // The license list is managed by setup, not by hand.
    sandbox
        .qinari()
        .args([
            "config",
            "set",
            "tts.accepted_licenses",
            "supertonic-openrail-m",
        ])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("qinari setup"));
    // A missing value argument is a clap usage error.
    sandbox
        .qinari()
        .args(["config", "set", "output.sr"])
        .assert()
        .failure()
        .code(2);
    // Nothing above was stored: the defaults still read back.
    let output = sandbox.ok(&["config", "get", "output.sr"]);
    assert_eq!(stdout(&output).trim(), "44100");
}

#[test]
fn interactive_help_works() {
    let sandbox = Sandbox::new("interactive-help");
    sandbox
        .qinari()
        .args(["interactive", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Prompted renders in a loop"));
}

#[test]
fn interactive_leaves_cleanly_off_a_terminal() {
    // Under piped stdin/stderr the prompts cannot render, which dialoguer
    // reports as an error; the session treats that as a clean leave.
    let sandbox = Sandbox::new("interactive-piped");
    let mut command = sandbox.qinari();
    command
        .arg("interactive")
        .write_stdin("")
        .assert()
        .success()
        .code(0);
}

#[test]
#[ignore = "golden gate: needs the installed Supertonic voice and DNSMOS models"]
fn golden_tts_gates_hold() {
    // The golden paragraph and seed of the 0.1.5 gate tier
    // (DESIGN-TTS-EXPRESSIVE.md: DNSMOS OVRL >= 3.35, F0 sigma >= 3.5 st,
    // >= 2 pauses, duration floor +/- 5%). Every threshold below was
    // measured on this exact render: 17.43 s, F0 sigma 4.1 st, 2 pauses,
    // DNSMOS OVRL 3.48. Byte determinism is same-machine (the qinari-tts
    // live suite pins it); the duration tolerance is the cross-machine
    // belt-and-braces.
    let paragraph = "Selamat datang di Qinari, mesin suara prosedural. \
[pause:400] Hari ini kita uji ekspresi dan prosodi natural: jeda, napas, \
dan intonasi yang hidup. [pause:600] Angka juga diuji: tiga puluh empat, \
dan tujuh ratus ribu rupiah. Sampai jumpa di sesi berikutnya.";
    let out = std::env::temp_dir().join(format!("qinari-golden-{}.wav", std::process::id()));
    let render = Command::cargo_bin("qinari")
        .expect("qinari binary is built")
        .env_remove("QINARI_TTS_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .args([
            "tts",
            "--lang",
            "id",
            "--seed",
            "golden-0.1.5",
            "-o",
            out.to_str().unwrap(),
            "--json",
            paragraph,
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let report: serde_json::Value = serde_json::from_str(&stdout(&render)).expect("tts --json");
    let duration = report["duration_s"].as_f64().expect("duration_s");
    assert!(
        (duration - 17.43).abs() / 17.43 <= 0.05,
        "golden duration {duration} s drifted past ±5% of 17.43 s"
    );
    let measured = Command::cargo_bin("qinari")
        .expect("qinari binary is built")
        .args([
            "analyze",
            out.to_str().unwrap(),
            "--mos",
            "--speech",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let analysis: serde_json::Value =
        serde_json::from_str(&stdout(&measured)).expect("analyze --json");
    let ovrl = analysis["dnsmos"]["ovrl"].as_f64().expect("ovrl");
    assert!(ovrl >= 3.35, "DNSMOS OVRL {ovrl} is below the 3.35 gate");
    let sigma = analysis["speech"]["f0_sigma_st"]
        .as_f64()
        .expect("f0_sigma_st");
    assert!(sigma >= 3.5, "F0 sigma {sigma} st is below the 3.5 st gate");
    let pauses = analysis["speech"]["pause_count"]
        .as_u64()
        .expect("pause_count");
    assert!(pauses >= 2, "{pauses} pauses is below the 2-pause gate");
    let _ = std::fs::remove_file(&out);
}
