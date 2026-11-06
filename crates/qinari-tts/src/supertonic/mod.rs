//! The bundled local engine: Supertonic 3 driven through four ONNX
//! Runtime sessions, ported from the archived reference SDK
//! (`supertonic-py` `core.py`/`pipeline.py`) and the official Rust example.
//!
//! Pipeline per chunk (batch size 1, matching the SDK's own chunking):
//!
//! 1. [`text`] preprocesses and tokenizes the chunk (the SDK's ten text
//!    passes plus the `<lang>…</lang>` wrap, then the
//!    `unicode_indexer.json` codepoint→unit lookup).
//! 2. `duration_predictor.onnx` (`text_ids`, `style_dp`, `text_mask`)
//!    predicts the chunk's length in seconds; the requested speed divides
//!    it.
//! 3. `text_encoder.onnx` (`text_ids`, `style_ttl`, `text_mask`) embeds
//!    the chunk.
//! 4. The initial latent is drawn from a seeded [`Qrng`] (the SDK uses an
//!    unseeded `np.random.randn`; seeding it is what makes Qinari renders
//!    reproducible), shaped `(1, latent_dim × compress, ceil(wav_len /
//!    chunk))` and masked to the predicted length.
//! 5. `vector_estimator.onnx` denoises the latent `steps` times
//!    (flow-matching Euler loop; `current_step`/`total_step` are `f32[1]`).
//! 6. `vocoder.onnx` decodes the latent to 44 100 Hz mono audio, truncated
//!    to the predicted length.
//!
//! Chunks are capped at 300 characters (sentence boundaries first, via
//! [`crate::textnorm::chunk_text`]) and joined with the prosody post-chain
//! ([`crate::prosody`]); `[pause:<ms>]` markup inserts exact silence.
//! Requests naming another sample rate are resampled through
//! `qinari-audio`'s rubato path. Speed is clamped into the model's
//! `0.7..=2.0` (see [`SupertonicEngine::clamp_speed`]; the CLI's wider
//! `0.5..=2.0` range is accepted and clamped with a warning there).

mod nfkd;
mod nfkd_data;
mod style;
mod text;

pub use style::Style;
pub use text::{preprocess, Tokenizer};

use crate::engine::{
    retarget_channels, stats_for, ProsodyMode, SynthRequest, Synthesis, TtsEngine,
};
use crate::markup;
use crate::prosody;
use crate::textnorm::{chunk_text, expand_numbers};
use crate::voices::VoiceEntry;
use crate::TtsError;
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;
use qinari_audio::resample;
use qinari_dsp::buffer::AudioBuffer;
use qinari_dsp::lufs::integrated_lufs;
use qinari_dsp::rng::Qrng;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Characters of normalized text per synthesis chunk: the reference SDK's
/// own cap (`DEFAULT_MAX_CHUNK_LENGTH`).
pub const CHUNK_CHARS: usize = 300;

/// Lowest speed the model itself accepts (the SDK's `MIN_SPEED`).
pub const MIN_ENGINE_SPEED: f32 = 0.7;

/// Highest speed the model itself accepts (the SDK's `MAX_SPEED`).
pub const MAX_ENGINE_SPEED: f32 = 2.0;

/// Default inter-chunk pause in seconds when the prosody post-chain has no
/// stronger opinion (the SDK's `DEFAULT_SILENCE_DURATION`).
pub const DEFAULT_PAUSE_SECS: f64 = 0.3;

/// `ort` allows exactly one [`ort::environment::Environment`] per process
/// (creating a second one panics inside the runtime), so the environment is
/// created once and held for the process lifetime.
static ORT_ENV: LazyLock<Option<ort::environment::Environment>> =
    LazyLock::new(|| ort::init().build().ok());

/// The `ae` section of `onnx/tts.json`.
#[derive(Debug, Deserialize)]
struct AeSection {
    sample_rate: u32,
    base_chunk_size: usize,
}

/// The `ttl` section of `onnx/tts.json`.
#[derive(Debug, Deserialize)]
struct TtlSection {
    chunk_compress_factor: usize,
    latent_dim: usize,
}

/// The parts of `onnx/tts.json` the engine needs.
#[derive(Debug, Deserialize)]
struct ConfigFile {
    ae: AeSection,
    ttl: TtlSection,
}

/// Model geometry resolved from `tts.json`.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    sample_rate: u32,
    base_chunk_size: usize,
    chunk_compress_factor: usize,
    latent_dim: usize,
}

impl Geometry {
    /// Samples of waveform one latent frame covers.
    fn latent_frame_samples(&self) -> usize {
        self.base_chunk_size * self.chunk_compress_factor
    }

    /// Channels of the (flattened) latent: `latent_dim × compress`.
    fn latent_channels(&self) -> usize {
        self.latent_dim * self.chunk_compress_factor
    }
}

/// The four ONNX sessions, guarded by one mutex: a synthesis run touches
/// all of them in order.
struct Sessions {
    duration_predictor: Session,
    text_encoder: Session,
    vector_estimator: Session,
    vocoder: Session,
}

/// The bundled Supertonic engine.
///
/// Build one with [`SupertonicEngine::load`], then [`TtsEngine::synthesize`]
/// any number of requests. The engine is cheap to share across threads:
/// the sessions sit behind a mutex, everything else is immutable.
pub struct SupertonicEngine {
    sessions: Mutex<Sessions>,
    geometry: Geometry,
    tokenizer: Tokenizer,
    model_dir: PathBuf,
    voice: &'static VoiceEntry,
    style: Style,
}

impl SupertonicEngine {
    /// Loads the engine for `voice` from `model_dir` (the directory holding
    /// `onnx/` and `voice_styles/`, as created by `qinari setup`).
    ///
    /// Every bundle file and the voice's style file is verified against the
    /// catalog pins before a session is created, so the engine only ever
    /// runs the pinned bundle.
    ///
    /// # Errors
    ///
    /// Returns [`TtsError::ModelDir`] when files are missing, unreadable,
    /// or fail their pinned digest; [`TtsError::Style`] when the style file
    /// is malformed; [`TtsError::OnnxInit`]/[`TtsError::Onnx`] when ONNX
    /// Runtime cannot load the graphs.
    pub fn load(model_dir: &Path, voice: &'static VoiceEntry) -> Result<Self, TtsError> {
        verify_bundle(model_dir, voice)?;
        let config_path = model_dir.join("onnx/tts.json");
        let config_text =
            std::fs::read_to_string(&config_path).map_err(|error| TtsError::ModelDir {
                detail: format!("could not read {}: {error}", config_path.display()),
            })?;
        let config: ConfigFile =
            serde_json::from_str(&config_text).map_err(|error| TtsError::ModelDir {
                detail: format!(
                    "{} is not a valid model configuration: {error}",
                    config_path.display()
                ),
            })?;
        if config.ae.sample_rate == 0
            || config.ae.base_chunk_size == 0
            || config.ttl.chunk_compress_factor == 0
            || config.ttl.latent_dim == 0
        {
            return Err(TtsError::ModelDir {
                detail: format!("{} has non-positive geometry values", config_path.display()),
            });
        }
        let geometry = Geometry {
            sample_rate: config.ae.sample_rate,
            base_chunk_size: config.ae.base_chunk_size,
            chunk_compress_factor: config.ttl.chunk_compress_factor,
            latent_dim: config.ttl.latent_dim,
        };

        let tokenizer = Tokenizer::load(&model_dir.join("onnx/unicode_indexer.json"))?;
        let style = Style::load(&style_path(model_dir, voice))?;

        let environment = ORT_ENV.as_ref().ok_or(TtsError::OnnxInit)?;
        let commit = |file: &str| -> Result<Session, TtsError> {
            Session::builder(environment)?
                .commit_from_file(model_dir.join(file))
                .map_err(TtsError::Onnx)
        };
        let sessions = Sessions {
            duration_predictor: commit("onnx/duration_predictor.onnx")?,
            text_encoder: commit("onnx/text_encoder.onnx")?,
            vector_estimator: commit("onnx/vector_estimator.onnx")?,
            vocoder: commit("onnx/vocoder.onnx")?,
        };

        Ok(Self {
            sessions: Mutex::new(sessions),
            geometry,
            tokenizer,
            model_dir: model_dir.to_path_buf(),
            voice,
            style,
        })
    }

    /// The voice this engine was loaded for.
    pub fn voice(&self) -> &'static VoiceEntry {
        self.voice
    }

    /// The engine's native sample rate in hertz.
    pub fn sample_rate(&self) -> u32 {
        self.geometry.sample_rate
    }

    /// Clamps a requested speed into the model's `0.7..=2.0`, reporting
    /// whether clamping happened (the caller owns the user-facing warning).
    #[must_use]
    pub fn clamp_speed(speed: f32) -> (f32, bool) {
        if speed < MIN_ENGINE_SPEED {
            (MIN_ENGINE_SPEED, true)
        } else if speed > MAX_ENGINE_SPEED {
            (MAX_ENGINE_SPEED, true)
        } else {
            (speed, false)
        }
    }

    /// Resolves the request's [`crate::engine::StyleSpec`] to a loaded
    /// style: the engine's own voice style, another installed preset
    /// (verified against its catalog pin), or a custom style file.
    fn resolve_style(&self, spec: &crate::engine::StyleSpec) -> Result<Style, TtsError> {
        match spec {
            crate::engine::StyleSpec::Default => Ok(self.style.clone()),
            crate::engine::StyleSpec::Preset(name) => {
                let voice =
                    crate::voices::find_by_style_name(name).ok_or_else(|| TtsError::Style {
                        detail: format!("unknown style preset {name:?}"),
                    })?;
                let path = style_path(&self.model_dir, voice);
                if !path.exists() {
                    return Err(TtsError::Style {
                        detail: format!(
                            "style preset {} is not installed; run `qinari setup --voice {}`",
                            voice.style_name, voice.id
                        ),
                    });
                }
                verify_style_file(&path, voice)?;
                Style::load(&path)
            }
            crate::engine::StyleSpec::File(path) => Style::load(path),
        }
    }

    /// Synthesizes one chunk (batch size 1) through the four graphs and
    /// returns the truncated waveform.
    #[allow(clippy::too_many_arguments)]
    fn synthesize_chunk(
        &self,
        sessions: &mut Sessions,
        style: &Style,
        chunk: &str,
        lang: &str,
        steps: u8,
        speed: f32,
        rng: &mut Qrng,
    ) -> Result<Vec<f32>, TtsError> {
        let geometry = self.geometry;
        let ids = self.tokenizer.encode(chunk, lang)?;
        let count = ids.len();

        let text_ids = Tensor::<i64>::from_array(([1_usize, count], ids))?;
        let text_mask = Tensor::<f32>::from_array(([1_usize, 1, count], vec![1.0_f32; count]))?;
        let ttl_dims: [usize; 3] = style.ttl_dims().try_into().map_err(|_| TtsError::Style {
            detail: "style_ttl is not 3-dimensional".to_string(),
        })?;
        let dp_dims: [usize; 3] = style.dp_dims().try_into().map_err(|_| TtsError::Style {
            detail: "style_dp is not 3-dimensional".to_string(),
        })?;
        let style_ttl = Tensor::<f32>::from_array((ttl_dims, style.ttl_data().to_vec()))?;
        let style_dp = Tensor::<f32>::from_array((dp_dims, style.dp_data().to_vec()))?;

        // 1. Duration, conditioned on the speed request.
        let inputs: Vec<(&str, SessionInputValue)> = vec![
            ("text_ids", (&text_ids).into()),
            ("style_dp", (&style_dp).into()),
            ("text_mask", (&text_mask).into()),
        ];
        let outputs = sessions.duration_predictor.run(inputs)?;
        let duration = extract_scalar(&outputs, "duration")? / speed;

        // 2. Text embedding (kept flat; the shape comes back from the
        //    graph when it is re-wrapped each denoise step).
        let inputs: Vec<(&str, SessionInputValue)> = vec![
            ("text_ids", (&text_ids).into()),
            ("style_ttl", (&style_ttl).into()),
            ("text_mask", (&text_mask).into()),
        ];
        let outputs = sessions.text_encoder.run(inputs)?;
        let text_emb = extract_tensor(&outputs, "text_emb")?;

        // 3. Seeded initial latent, masked to the predicted length.
        let frame = geometry.latent_frame_samples();
        let sr = geometry.sample_rate as f32;
        let wav_len = (duration * sr) as usize;
        // The SDK's float ceil: ((max + frame - 1) / frame).floor().
        let latent_len = (((duration * sr) + (frame - 1) as f32) / frame as f32)
            .floor()
            .max(1.0) as usize;
        let latent_mask_len = wav_len.div_ceil(frame).clamp(1, latent_len);
        let channels = geometry.latent_channels();
        // Box-Muller pairs fill the flat (channel-major, `d * L + t`)
        // latent; the values are iid, so only the determinism of the draw
        // order matters.
        let wanted = channels * latent_len;
        let mut noisy = Vec::with_capacity(wanted);
        let mut spare: Option<f32> = None;
        while noisy.len() < wanted {
            let next = if let Some(b) = spare.take() {
                b
            } else {
                let (a, b) = next_normal(rng);
                spare = Some(b);
                a
            };
            noisy.push(next);
        }
        // The SDK multiplies by the (B, 1, L) mask: every channel is zero
        // past the masked length.
        for d in 0..channels {
            for t in latent_mask_len..latent_len {
                noisy[d * latent_len + t] = 0.0;
            }
        }

        // 4. Flow-matching Euler loop.
        let total_step = Tensor::<f32>::from_array(([1_usize], vec![f32::from(steps)]))?;
        let latent_dims = [1_usize, channels, latent_len];
        let mask_dims = [1_usize, 1, latent_len];
        let mut latent = noisy;
        for step in 0..u32::from(steps) {
            let current_step = Tensor::<f32>::from_array(([1_usize], vec![step as f32]))?;
            let noisy_latent = Tensor::<f32>::from_array((latent_dims, latent.clone()))?;
            let latent_mask =
                Tensor::<f32>::from_array((mask_dims, mask_vec(latent_len, latent_mask_len)))?;
            let text_emb_tensor =
                Tensor::<f32>::from_array(([1_usize, text_emb.1, text_emb.2], text_emb.0.clone()))?;
            let inputs: Vec<(&str, SessionInputValue)> = vec![
                ("noisy_latent", (&noisy_latent).into()),
                ("text_emb", (&text_emb_tensor).into()),
                ("style_ttl", (&style_ttl).into()),
                ("latent_mask", (&latent_mask).into()),
                ("text_mask", (&text_mask).into()),
                ("current_step", (&current_step).into()),
                ("total_step", (&total_step).into()),
            ];
            let outputs = sessions.vector_estimator.run(inputs)?;
            latent = extract_tensor(&outputs, "denoised_latent")?.0;
        }

        // 5. Vocoder; truncate to the predicted length.
        let final_latent = Tensor::<f32>::from_array((latent_dims, latent))?;
        let vocoder_inputs: Vec<(&str, SessionInputValue)> =
            vec![("latent", (&final_latent).into())];
        let outputs = sessions.vocoder.run(vocoder_inputs)?;
        let wav = extract_wav(&outputs, "wav_tts")?;
        Ok(wav[..wav_len.min(wav.len())].to_vec())
    }

    /// Runs the full synthesis orchestration shared by
    /// [`TtsEngine::synthesize`].
    fn synthesize_impl(&self, request: &SynthRequest) -> Result<Synthesis, TtsError> {
        let started = Instant::now();
        let (speed, clamped) = Self::clamp_speed(request.speed);
        if clamped {
            // The CLI warns the user from the same clamping rule; this is
            // the engine-side record of the divergence.
            eprintln!(
                "warning: speed {} is outside the voice's 0.7..=2.0 range; using {speed} instead",
                request.speed
            );
        }
        let lang = request.language.as_code();
        let style = self.resolve_style(&request.style)?;

        let segments = if request.markup {
            markup::parse(&request.text)?
        } else {
            markup::plain(&request.text)
        };
        let pieces = markup::for_supertonic(&segments);
        if pieces.is_empty() {
            return Err(TtsError::EmptyText);
        }

        let render_seed = request.seed.unwrap_or_else(time_seed);
        let mut rng = Qrng::new(render_seed);
        let sample_rate = self.geometry.sample_rate;
        let mut samples: Vec<f32> = Vec::new();
        let mut reference_lufs: Option<f64> = None;
        let mut pending_pause_ms: Option<u32> = None;
        let mut previous_chunk: Option<String> = None;

        // A poisoned lock means another thread panicked mid-run; the
        // sessions themselves are still valid, so recover them.
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        for piece in &pieces {
            match piece {
                markup::Piece::Pause(ms) => {
                    let total = pending_pause_ms.map_or(*ms, |prev| prev + *ms);
                    pending_pause_ms = Some(total);
                }
                markup::Piece::Run(text) => {
                    let normalized = expand_numbers(text, request.language);
                    let chunks = chunk_text(&normalized, CHUNK_CHARS);
                    for chunk in &chunks {
                        let jitter = match request.prosody {
                            crate::engine::ProsodyMode::Natural => prosody::rate_jitter(&mut rng),
                            crate::engine::ProsodyMode::Flat => 1.0,
                        };
                        let (effective_speed, _) = Self::clamp_speed(speed * jitter);
                        let mut wav = self.synthesize_chunk(
                            &mut sessions,
                            &style,
                            chunk,
                            lang,
                            request.steps,
                            effective_speed,
                            &mut rng,
                        )?;
                        trim_silence_edges(&mut wav, sample_rate);
                        let mut buffer = AudioBuffer::from_channels(vec![wav], sample_rate)
                            .map_err(|_| TtsError::Inference {
                                detail: format!("invalid sample rate {sample_rate}"),
                            })?;
                        // Loudness-consistent joins: the first chunk sets
                        // the reference, later chunks are gain-matched.
                        match reference_lufs {
                            None => reference_lufs = integrated_lufs(&buffer),
                            Some(target) => prosody::match_loudness(&mut buffer, Some(target)),
                        }
                        // Join pause before this chunk: the exact markup
                        // pause when one is pending, otherwise the prosody
                        // join classified by the previous chunk's ending.
                        if !samples.is_empty() {
                            let pause_ms = join_pause(
                                pending_pause_ms.take(),
                                previous_chunk.as_deref(),
                                request.prosody,
                            );
                            append_silence(&mut samples, pause_ms, sample_rate);
                        } else if let Some(ms) = pending_pause_ms.take() {
                            // A leading [pause] is leading silence.
                            append_silence(&mut samples, ms, sample_rate);
                        }
                        samples.extend_from_slice(buffer.channels()[0].as_slice());
                        previous_chunk = Some(chunk.clone());
                    }
                }
            }
        }
        // A trailing [pause] is trailing silence.
        if let Some(ms) = pending_pause_ms {
            append_silence(&mut samples, ms, sample_rate);
        }

        let mono = AudioBuffer::from_channels(vec![samples], sample_rate).map_err(|_| {
            TtsError::Inference {
                detail: format!("invalid sample rate {sample_rate}"),
            }
        })?;
        let at_native_rate = if request.sr == sample_rate {
            mono
        } else {
            resample(&mono, request.sr).map_err(|error| TtsError::Inference {
                detail: format!("resampling to {} Hz failed: {error}", request.sr),
            })?
        };
        let audio = retarget_channels(&at_native_rate, request.channels)?;
        Ok(Synthesis {
            stats: stats_for(started.elapsed(), &audio),
            audio,
        })
    }
}

impl TtsEngine for SupertonicEngine {
    fn synthesize(&self, request: &SynthRequest) -> Result<Synthesis, TtsError> {
        request.validate()?;
        if request.voice != self.voice.id {
            return Err(TtsError::UnknownVoice {
                id: request.voice.clone(),
            });
        }
        self.synthesize_impl(request)
    }
}

/// The installed path of a voice's style file.
fn style_path(model_dir: &Path, voice: &VoiceEntry) -> PathBuf {
    model_dir
        .join("voice_styles")
        .join(format!("{}.json", voice.style_name))
}

/// A time-based seed for unseeded renders.
fn time_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |since| since.as_nanos() as u64)
}

/// Appends `ms` of silence to `samples`.
fn append_silence(samples: &mut Vec<f32>, ms: u32, sample_rate: u32) {
    let frames = (f64::from(ms) / 1000.0 * f64::from(sample_rate)).round() as usize;
    samples.extend(std::iter::repeat_n(0.0, frames));
}

/// The silence to insert before a joined chunk: the exact markup pause
/// when one is pending, otherwise the prosody join classified by the
/// previous chunk's ending.
fn join_pause(pending: Option<u32>, previous_chunk: Option<&str>, mode: ProsodyMode) -> u32 {
    if let Some(ms) = pending {
        return ms;
    }
    let punctuation =
        previous_chunk.map_or(prosody::Punctuation::None, prosody::Punctuation::at_end_of);
    prosody::join_pause_ms(punctuation, mode)
}

/// Amplitude below which a fully-silent chunk is left untouched
/// (−60 dBFS absolute floor).
const EDGE_SILENCE_THRESHOLD: f32 = 0.001;

/// Edge windows quieter than this fraction of the chunk's peak window
/// RMS count as silence (−45 dB relative). Measured on the pinned
/// bundle: speech onsets ramp through −60…−26 dB over ~150 ms and tails
/// decay through −48…−90 dB over ~130 ms, so the floor sits between the
/// two gradients — breath tails go, phoneme edges stay.
const EDGE_WINDOW_FLOOR: f32 = 0.005_623_413_3;

/// Trims leading and trailing silence from one synthesized chunk so that
/// chunk joins contain exactly the pause the prosody pass inserts.
///
/// The model pads every chunk with its own lead-in and tail (measured:
/// up to ~0.4 s per chunk on the pinned bundle), and the tail is often
/// low-level noise rather than digital zero. Edges are therefore decided
/// on 10 ms RMS windows against the chunk's own peak window — noise at
/// −30 dB relative or quieter is trimmed, speech is kept. A chunk that
/// is silent throughout (peak window below the absolute floor) is left
/// untouched — trimming it to zero length would only hide a broken
/// render.
fn trim_silence_edges(samples: &mut Vec<f32>, sample_rate: u32) {
    let window = (u64::from(sample_rate) * 10 / 1_000).max(1) as usize;
    if samples.len() <= window {
        return;
    }
    let windows: Vec<f32> = samples
        .chunks(window)
        .map(|chunk| {
            let sum: f32 = chunk.iter().map(|x| x * x).sum();
            (sum / chunk.len() as f32).sqrt()
        })
        .collect();
    let Some(peak) = windows.iter().copied().fold(None::<f32>, |a, b| {
        Some(match a {
            Some(current) => current.max(b),
            None => b,
        })
    }) else {
        return;
    };
    if peak < EDGE_SILENCE_THRESHOLD {
        return;
    }
    let floor = peak * EDGE_WINDOW_FLOOR;
    let Some(first) = windows.iter().position(|rms| *rms > floor) else {
        return;
    };
    let Some(last) = windows.iter().rposition(|rms| *rms > floor) else {
        return;
    };
    samples.drain((last + 1) * window..);
    samples.drain(..first * window);
}

/// One mask vector: `ones` leading ones, then zeros to `len`.
fn mask_vec(len: usize, ones: usize) -> Vec<f32> {
    let mut mask = vec![1.0_f32; ones];
    mask.resize(len, 0.0);
    mask
}

/// Extracts a scalar `f32` output (shape `[1]`).
fn extract_scalar(outputs: &ort::session::SessionOutputs<'_>, name: &str) -> Result<f32, TtsError> {
    let value = outputs.get(name).ok_or_else(|| TtsError::Inference {
        detail: format!("the graph produced no {name} output"),
    })?;
    let (_, data) = value.try_extract_tensor::<f32>().map_err(TtsError::Onnx)?;
    data.first().copied().ok_or_else(|| TtsError::Inference {
        detail: format!("the {name} output is empty"),
    })
}

/// Extracts one output tensor as `(flat_data, dim1, dim2)` for 3-D outputs
/// whose first dimension is the batch.
fn extract_tensor(
    outputs: &ort::session::SessionOutputs<'_>,
    name: &str,
) -> Result<(Vec<f32>, usize, usize), TtsError> {
    let value = outputs.get(name).ok_or_else(|| TtsError::Inference {
        detail: format!("the graph produced no {name} output"),
    })?;
    let (shape, data) = value.try_extract_tensor::<f32>().map_err(TtsError::Onnx)?;
    let dims: Vec<usize> = shape.iter().map(|d| *d as usize).collect();
    if dims.len() != 3 {
        return Err(TtsError::Inference {
            detail: format!("the {name} output is not 3-dimensional: {dims:?}"),
        });
    }
    let expected = dims.iter().product::<usize>();
    if data.len() != expected {
        return Err(TtsError::Inference {
            detail: format!(
                "the {name} output has {} values but shape {dims:?} needs {expected}",
                data.len()
            ),
        });
    }
    Ok((data.to_vec(), dims[1], dims[2]))
}

/// Extracts the vocoder waveform output as a flat sample vector.
///
/// The graph emits `[batch, samples]` (measured on the pinned bundle:
/// `[1, 92160]`); a `[batch, 1, samples]` variant is accepted so a
/// re-exported graph with a singleton channel axis still runs.
fn extract_wav(
    outputs: &ort::session::SessionOutputs<'_>,
    name: &str,
) -> Result<Vec<f32>, TtsError> {
    let value = outputs.get(name).ok_or_else(|| TtsError::Inference {
        detail: format!("the graph produced no {name} output"),
    })?;
    let (shape, data) = value.try_extract_tensor::<f32>().map_err(TtsError::Onnx)?;
    let dims: Vec<usize> = shape.iter().map(|d| *d as usize).collect();
    match dims.as_slice() {
        [1, samples] | [1, 1, samples] => {
            if data.len() == *samples {
                Ok(data.to_vec())
            } else {
                Err(TtsError::Inference {
                    detail: format!(
                        "the {name} output has {} values but shape {dims:?} needs {samples}",
                        data.len()
                    ),
                })
            }
        }
        _ => Err(TtsError::Inference {
            detail: format!("the {name} output is not [batch, samples]: {dims:?}"),
        }),
    }
}

/// Draws one standard-normal pair from `rng` (Box-Muller).
fn next_normal(rng: &mut Qrng) -> (f32, f32) {
    // Rejection keeps u1 strictly positive so the logarithm is finite.
    let mut u1 = rng.next_f32();
    while u1 <= f32::EPSILON {
        u1 = rng.next_f32();
    }
    let u2 = rng.next_f32();
    let radius = (-2.0 * u1.ln()).sqrt();
    let angle = 2.0 * std::f32::consts::PI * u2;
    (radius * angle.cos(), radius * angle.sin())
}

/// Verifies every pinned bundle file plus the voice's style file against
/// the catalog digests.
///
/// # Errors
///
/// Returns [`TtsError::ModelDir`] naming the first file that is missing,
/// has the wrong size, or fails its pinned SHA-256.
pub fn verify_bundle(model_dir: &Path, voice: &VoiceEntry) -> Result<(), TtsError> {
    for file in voice.bundle.files {
        let path = model_dir.join(file.rel_path);
        verify_pinned(&path, file.url, file.size_bytes, file.sha256)?;
    }
    verify_style_file(&style_path(model_dir, voice), voice)
}

/// Verifies one file against its pinned size and digest.
fn verify_pinned(path: &Path, url: &str, size: u64, sha256: &str) -> Result<(), TtsError> {
    let bad = |detail: String| TtsError::ModelDir {
        detail: format!("{}: {detail}", path.display()),
    };
    if !path.exists() {
        return Err(bad(format!(
            "missing; run `qinari setup` or point QINARI_SUPERTONIC_DIR at a populated model dir (this file comes from {url})"
        )));
    }
    let meta = std::fs::metadata(path).map_err(|e| bad(format!("could not stat: {e}")))?;
    if meta.len() != size {
        return Err(bad(format!(
            "expected {size} bytes, found {} (the pinned bundle's file differs)",
            meta.len()
        )));
    }
    let digest = file_sha256(path).map_err(|e| bad(format!("could not read: {e}")))?;
    if digest != sha256 {
        return Err(bad(format!(
            "sha256 mismatch: expected {sha256}, found {digest}"
        )));
    }
    Ok(())
}

/// Verifies a voice's style file against its catalog pin.
fn verify_style_file(path: &Path, voice: &VoiceEntry) -> Result<(), TtsError> {
    verify_pinned(
        path,
        voice.style_url,
        voice.style_size_bytes,
        voice.style_sha256,
    )
}

/// Streams `path` through SHA-256 and returns the lowercase hex digest.
fn file_sha256(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Lowercase hex encoding of `bytes`.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_trimming_removes_only_the_silent_edges() {
        // 0.5 s silence + 0.5 s loud block + 1.0 s of quiet noise
        // (−48 dB relative to the block) that must also count as edge.
        let window = 441_usize; // 10 ms at 44.1 kHz
        let mut samples: Vec<f32> = vec![0.0; 5 * window];
        samples.extend(std::iter::repeat_n(0.5, 5 * window));
        samples.extend(std::iter::repeat_n(0.002, 10 * window));
        trim_silence_edges(&mut samples, 44_100);
        assert_eq!(samples.len(), 5 * window, "only the loud block remains");
        assert_eq!(samples.first(), Some(&0.5));
        assert_eq!(samples.last(), Some(&0.5));

        // A fully silent chunk is left untouched instead of emptied.
        let mut silent = vec![0.0_f32; 64];
        trim_silence_edges(&mut silent, 44_100);
        assert_eq!(silent.len(), 64);
    }

    #[test]
    fn speed_clamp_matches_the_model_bounds() {
        assert_eq!(SupertonicEngine::clamp_speed(1.0), (1.0, false));
        assert_eq!(SupertonicEngine::clamp_speed(0.7), (0.7, false));
        assert_eq!(SupertonicEngine::clamp_speed(2.0), (2.0, false));
        assert_eq!(SupertonicEngine::clamp_speed(0.5), (0.7, true));
        assert_eq!(SupertonicEngine::clamp_speed(2.1), (2.0, true));
        // The CLI's wider accepted range maps onto the engine's bounds.
        for accepted in [0.5_f32, 0.6, 0.65] {
            let (clamped, did) = SupertonicEngine::clamp_speed(accepted);
            assert_eq!(clamped, 0.7);
            assert!(did);
        }
    }

    #[test]
    fn box_muller_is_standard_normal_ish() {
        let mut rng = Qrng::new(2024);
        let n = 200_000_usize;
        let mut mean = 0.0_f64;
        let mut var = 0.0_f64;
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for _ in 0..n {
            let (a, b) = next_normal(&mut rng);
            for v in [a, b] {
                mean += f64::from(v);
                var += f64::from(v * v);
                min = min.min(v);
                max = max.max(v);
            }
        }
        let count = f64::from(2 * n as u32);
        mean /= count;
        var = var / count - mean * mean;
        assert!(mean.abs() < 0.02, "mean {mean}");
        assert!((var - 1.0).abs() < 0.05, "variance {var}");
        assert!(min < -3.0 && max > 3.0, "range {min}..{max}");
    }

    #[test]
    fn seeded_latents_reproduce_exactly() {
        fn draw(seed: u64, len: usize) -> Vec<f32> {
            let mut rng = Qrng::new(seed);
            let mut out = Vec::new();
            let mut spare: Option<f32> = None;
            while out.len() < len {
                if let Some(b) = spare.take() {
                    out.push(b);
                } else {
                    let (a, b) = next_normal(&mut rng);
                    out.push(a);
                    spare = Some(b);
                }
            }
            out
        }
        assert_eq!(draw(7, 1000), draw(7, 1000));
        assert_ne!(draw(7, 1000), draw(8, 1000));
    }

    #[test]
    fn silence_appends_round_frames() {
        let mut samples = vec![0.5_f32; 10];
        append_silence(&mut samples, 400, 44_100);
        assert_eq!(samples.len(), 10 + 17_640);
        assert!(samples[10..].iter().all(|s| *s == 0.0));
        // 1 ms at 44100 Hz rounds to 44 frames.
        let mut tiny = Vec::new();
        append_silence(&mut tiny, 1, 44_100);
        assert_eq!(tiny.len(), 44);
    }

    #[test]
    fn mask_vec_covers_then_zeroes() {
        assert_eq!(mask_vec(4, 4), vec![1.0, 1.0, 1.0, 1.0]);
        assert_eq!(mask_vec(4, 2), vec![1.0, 1.0, 0.0, 0.0]);
        assert_eq!(mask_vec(4, 0), vec![0.0; 4]);
    }

    #[test]
    fn hex_is_lowercase() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
    }

    #[test]
    fn missing_model_dir_is_a_named_error() {
        let voice = crate::voices::find_voice("supertonic-M1").expect("catalog voice");
        let error =
            verify_bundle(Path::new("/nonexistent/supertonic"), voice).expect_err("missing dir");
        match &error {
            TtsError::ModelDir { detail } => {
                assert!(detail.contains("missing"), "{detail}");
                assert!(detail.contains("QINARI_SUPERTONIC_DIR"), "{detail}");
            }
            other => panic!("expected ModelDir, got {other:?}"),
        }
    }
}
