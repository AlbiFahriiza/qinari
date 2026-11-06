//! Parity and smoke-test binary: renders one utterance through the
//! Supertonic engine and reports duration / sample rate / RMS / RTF.
//!
//! Used by `scripts/parity_supertonic.py` to compare the Rust port against
//! the reference SDK pipeline driven through Python's onnxruntime. The
//! model directory comes from `QINARI_SUPERTONIC_DIR`.
//!
//! ```text
//! QINARI_SUPERTONIC_DIR=/path/to/model cargo run -p qinari-tts \
//!     --release --example render -- --lang id --seed 1234 --out out.wav \
//!     --text "Halo, nama saya Albi."
//! ```

use std::path::PathBuf;
use std::time::Instant;

use qinari_tts::engine::{ProsodyMode, StyleSpec, DEFAULT_STEPS};
use qinari_tts::supertonic::SupertonicEngine;
use qinari_tts::textnorm::Language;
use qinari_tts::{find_voice, SynthRequest, TtsEngine};

/// Command-line surface of the example.
#[derive(Debug)]
struct Args {
    text: String,
    lang: Language,
    voice: String,
    steps: u8,
    seed: u64,
    speed: f32,
    out: Option<PathBuf>,
}

/// Parses `--flag value` pairs; the text arrives via `--text`.
fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        text: String::new(),
        lang: Language::En,
        voice: "supertonic-M1".to_string(),
        steps: DEFAULT_STEPS,
        seed: 0,
        speed: 1.0,
        out: None,
    };
    let mut raw = std::env::args().skip(1);
    while let Some(flag) = raw.next() {
        let mut value = || {
            raw.next()
                .ok_or_else(|| format!("missing value for {flag}"))
        };
        match flag.as_str() {
            "--text" => args.text = value()?,
            "--lang" => {
                let lang = value()?;
                args.lang = Language::from_code(&lang)
                    .ok_or_else(|| format!("unknown language {lang:?}"))?;
            }
            "--voice" => args.voice = value()?,
            "--steps" => args.steps = value()?.parse().map_err(|e| format!("{e}"))?,
            "--seed" => args.seed = value()?.parse().map_err(|e| format!("{e}"))?,
            "--speed" => args.speed = value()?.parse().map_err(|e| format!("{e}"))?,
            "--out" => args.out = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown flag {other:?}")),
        }
    }
    if args.text.trim().is_empty() {
        return Err("pass the text with --text".to_string());
    }
    Ok(args)
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("render example: {message}");
            std::process::exit(2);
        }
    };
    let model_dir = std::env::var("QINARI_SUPERTONIC_DIR").unwrap_or_else(|_| ".".to_string());
    let voice = find_voice(&args.voice).expect("voice in the catalog");
    let engine = SupertonicEngine::load(std::path::Path::new(&model_dir), voice)
        .expect("load the supertonic bundle");
    let request = SynthRequest {
        text: args.text,
        language: args.lang,
        speed: args.speed,
        voice: voice.id.to_string(),
        sr: 44_100,
        channels: 1,
        steps: args.steps,
        seed: Some(args.seed),
        // Parity compares the raw engine path: flat prosody, no markup,
        // the SDK's own fixed inter-chunk silence.
        prosody: ProsodyMode::Flat,
        style: StyleSpec::Default,
        markup: false,
    };
    let started = Instant::now();
    let synthesis = engine.synthesize(&request).expect("synthesis");
    let wall = started.elapsed();
    println!(
        "duration={:.4} sr={} rms={:.6} rtf={:.4} wall={:.3} samples={}",
        synthesis.audio.duration(),
        synthesis.audio.sample_rate(),
        synthesis.audio.rms(),
        wall.as_secs_f64() / synthesis.audio.duration(),
        wall.as_secs_f64(),
        synthesis.audio.frames()
    );
    if let Some(out) = args.out {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: synthesis.audio.sample_rate(),
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&out, spec).expect("create wav");
        for sample in synthesis.audio.channels()[0].as_slice() {
            writer.write_sample(*sample).expect("write sample");
        }
        writer.finalize().expect("finalize wav");
        eprintln!("wrote {}", out.display());
    }
}
