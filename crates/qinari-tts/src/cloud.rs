//! The cloud speech engines: the `OpenAI`-compatible protocol (`POST
//! {base}/audio/speech`) and the `ElevenLabs` v4 API.
//!
//! Both engines normalize numbers, split the text into bounded chunks, and
//! request each chunk as a WAV body. Transient server failures (HTTP 5xx)
//! are retried with linear backoff; timeouts and 4xx responses surface
//! immediately. Decoded chunks are joined with inserted silence and
//! converted to the requested channel count. Keys are read from the
//! environment only — never persisted, never logged.
//!
//! # The `OpenAI`-compatible engine
//!
//! [`OpenAiCompatibleEngine`] authenticates with a `Bearer` header and
//! joins chunks with a quarter-second pause.
//!
//! # The `ElevenLabs` engine
//!
//! [`ElevenLabsEngine`] speaks the documented `ElevenLabs` TTS endpoint, `POST
//! {base}/v1/text-to-speech/{voice_id}`, authenticated with the `xi-api-key`
//! header. Configuration lives in [`ElevenLabsConfig`]:
//!
//! - **Key**: [`ELEVENLABS_KEY_ENV`] (`QINARI_ELEVENLABS_API_KEY`), or an
//!   explicit `api_key`. Never persisted, never logged.
//! - **Models**: [`ElevenLabsModel::V4`] (`eleven_v4`, the default — the
//!   most emotive batch model) or [`ElevenLabsModel::V4Turbo`]
//!   (`eleven_v4_turbo`, the ~100 ms real-time variant). The model is
//!   engine configuration, exactly like the `OpenAI` lane's configured
//!   model — not a per-request field. Both models cover 90+ languages,
//!   including Indonesian (`id`) and Javanese (`jv`).
//! - **Voice**: an explicit `voice_id` in the config wins; otherwise the
//!   request's [`SynthRequest::voice`] is read as the service voice id,
//!   either raw (a voice UUID such as `21m00Tcm4TlvDq8ikWAM`, or a
//!   library name such as `Aria`) or with the `elevenlabs-` lane prefix
//!   (`elevenlabs-Aria`), mirroring the `supertonic-` prefix of the local
//!   lane. The resolved id must be URL-safe (letters, digits, `-`, `_`,
//!   `.`, `~`); anything else is rejected before a request is sent, and
//!   unknown ids surface as the service's own error status.
//! - **Output format**: WAV, at the supported `wav_<rate>` nearest the
//!   request's target rate (ties go to the higher rate); the audio plays
//!   at the rate the service sent, like the `OpenAI` lane. Note: per the
//!   API reference, 44.1 kHz WAV output requires a Pro-tier key — name
//!   another rate to avoid that gate.
//! - **Markup**: Qinari `[laugh]` `[breath]` `[sigh]` map onto the
//!   documented v4 audio-tag vocabulary: `[laughs]`, `[exhales]`,
//!   `[sighs]` (there is no literal `[breath]` tag; `[exhales]` is the
//!   documented breath sound). The tags travel inline in the chunk text.
//!   `[pause:<ms>]` inserts exact silence in the join — leading pauses
//!   become leading silence, trailing pauses trailing silence — and joins
//!   without a pending pause use the same quarter-second default as the
//!   `OpenAI` lane. Unknown tags are hard errors (the shared markup parser)
//!   before any request is sent. `speed` travels as `voice_settings.speed`.
//! - **Chunking**: the same 3000-character cap as the `OpenAI` lane, well
//!   under the service's 10 000-character v4 limit.
//!
//! Not supported (deliberate): streaming (WebSocket or chunked playback —
//! requests are blocking, whole-body), voice cloning and voice management
//! endpoints, pronunciation dictionaries, audio effects,
//! `previous_text`/`next_text` stitching, `language_code` forcing, the
//! `seed` parameter, and non-WAV output formats (mp3/opus/pcm/ulaw/alaw).
//! v4 has no SSML; Qinari markup is the whole expressiveness surface.

use crate::engine::{retarget_channels, stats_for, SynthRequest, Synthesis, TtsEngine};
use crate::markup;
use crate::textnorm::{chunk_text, expand_numbers};
use crate::TtsError;
use qinari_dsp::buffer::AudioBuffer;
use std::io::Cursor;
use std::time::{Duration, Instant};

/// Characters of normalized text per cloud request. Far under either
/// service's own limit (`ElevenLabs` v4 accepts 10 000); the local engine
/// chunks at 300 instead because chunk boundaries are prosody joins there.
const CHUNK_CHARS: usize = 3000;

/// Silence inserted between chunk audio when no exact `[pause:<ms>]` is
/// pending, in seconds.
const PAUSE_SECS: f64 = 0.25;

/// [`PAUSE_SECS`] in whole milliseconds, so exact markup pauses and the
/// default join share one code path.
const PAUSE_SECS_MS: u32 = (PAUSE_SECS * 1000.0) as u32;

/// Environment variable checked for the `ElevenLabs` API key.
pub const ELEVENLABS_KEY_ENV: &str = "QINARI_ELEVENLABS_API_KEY";

/// WAV sample rates the `ElevenLabs` `output_format` query parameter
/// accepts (`wav_8000` … `wav_48000`), lowest first.
const WAV_RATES: [u32; 7] = [8_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000];

/// Base delay for 5xx retries; attempt `n` waits `n * RETRY_BACKOFF_MS`.
const RETRY_BACKOFF_MS: u64 = 300;

/// Environment variable checked first for the cloud API key.
pub const API_KEY_ENV: &str = "QINARI_TTS_API_KEY";

/// Environment variable checked second for the cloud API key.
pub const OPENAI_KEY_ENV: &str = "OPENAI_API_KEY";

/// Configuration for the `OpenAI`-compatible cloud engine.
#[derive(Debug, Clone)]
pub struct CloudConfig {
    /// Base URL of the service, e.g. `https://api.openai.com/v1`. The
    /// engine appends `/audio/speech`.
    pub base_url: String,
    /// Model name the service expects, e.g. `gpt-4o-mini-tts`.
    pub model: String,
    /// Voice name the service expects, e.g. `alloy`.
    pub voice: String,
    /// Bearer token. When `None`, [`OpenAiCompatibleEngine::new`] falls
    /// back to the [`API_KEY_ENV`] and then [`OPENAI_KEY_ENV`]
    /// environment variables.
    pub api_key: Option<String>,
    /// Timeout for one whole request (connection, headers, and body).
    pub timeout: Duration,
    /// How many times a 5xx response is retried (2 retries means up to 3
    /// attempts).
    pub retries: u32,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o-mini-tts".to_string(),
            voice: "alloy".to_string(),
            api_key: None,
            timeout: Duration::from_secs(30),
            retries: 2,
        }
    }
}

/// The cloud speech engine.
pub struct OpenAiCompatibleEngine {
    config: CloudConfig,
    agent: ureq::Agent,
    api_key: String,
}

impl OpenAiCompatibleEngine {
    /// Creates the engine, resolving the API key from `config` or the
    /// environment.
    ///
    /// # Errors
    ///
    /// Returns [`TtsError::NoApiKey`] when neither the configuration nor
    /// the [`API_KEY_ENV`] / [`OPENAI_KEY_ENV`] environment variables
    /// provide a key.
    pub fn new(config: CloudConfig) -> Result<Self, TtsError> {
        let api_key = config
            .api_key
            .clone()
            .or_else(|| std::env::var(API_KEY_ENV).ok())
            .or_else(|| std::env::var(OPENAI_KEY_ENV).ok())
            .ok_or(TtsError::NoApiKey)?;
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(config.timeout))
                .build(),
        );
        Ok(Self {
            config,
            agent,
            api_key,
        })
    }

    /// The configuration this engine was built with.
    pub fn config(&self) -> &CloudConfig {
        &self.config
    }

    /// Requests one chunk of speech, retrying transient server errors.
    fn request_speech(&self, input: &str, speed: f32) -> Result<Vec<u8>, TtsError> {
        let url = format!(
            "{}/audio/speech",
            self.config.base_url.trim_end_matches('/')
        );
        let body = serde_json::json!({
            "model": self.config.model,
            "voice": self.config.voice,
            "input": input,
            "response_format": "wav",
            "speed": speed,
        });
        let mut attempt = 0_u32;
        loop {
            let result = self
                .agent
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .send_json(&body);
            match result {
                Ok(mut response) => {
                    // Read the body as raw bytes: WAV is binary and must
                    // never pass through a lossy UTF-8 conversion.
                    return response.body_mut().read_to_vec().map_err(map_ureq_error);
                }
                // ureq 3.x reports every non-2xx status here, so the 5xx
                // retry logic belongs in this arm (the Ok arm above only
                // ever sees 2xx).
                Err(ureq::Error::StatusCode(status)) => {
                    if status >= 500 && attempt < self.config.retries {
                        attempt += 1;
                        std::thread::sleep(Duration::from_millis(
                            RETRY_BACKOFF_MS * u64::from(attempt),
                        ));
                        continue;
                    }
                    return Err(TtsError::Http { status });
                }
                Err(other) => return Err(map_ureq_error(other)),
            }
        }
    }
}

impl TtsEngine for OpenAiCompatibleEngine {
    /// Synthesizes `request` through the configured service.
    ///
    /// The engine uses its configured model and voice; the request's
    /// language drives number normalization and the returned audio plays
    /// at the sample rate the service sent.
    ///
    /// # Errors
    ///
    /// Returns [`TtsError`] when the request fails validation, the key is
    /// missing, the service answers with an error status after retries
    /// ([`TtsError::Http`]), the request times out ([`TtsError::Timeout`]),
    /// the network fails, or the response is not decodable WAV audio
    /// ([`TtsError::InvalidResponse`]).
    fn synthesize(&self, request: &SynthRequest) -> Result<Synthesis, TtsError> {
        request.validate()?;
        let started = Instant::now();

        let normalized = expand_numbers(&request.text, request.language);
        let chunks = chunk_text(&normalized, CHUNK_CHARS);
        let mut joined: Option<AudioBuffer> = None;
        for chunk in &chunks {
            let bytes = self.request_speech(chunk, request.speed)?;
            let decoded = decode_wav_bytes(&bytes)?;
            if let Some(acc) = joined.as_mut() {
                if decoded.sample_rate() != acc.sample_rate()
                    || decoded.num_channels() != acc.num_channels()
                {
                    return Err(TtsError::InvalidResponse {
                        detail: format!(
                            "the service returned {} Hz / {} ch for one chunk but {} Hz / {} ch for another",
                            decoded.sample_rate(),
                            decoded.num_channels(),
                            acc.sample_rate(),
                            acc.num_channels()
                        ),
                    });
                }
                let pause_frames = (f64::from(acc.sample_rate()) * PAUSE_SECS).round() as usize;
                let pause = AudioBuffer::new(1, pause_frames, acc.sample_rate());
                acc.concat(&pause);
                acc.concat(&decoded);
            } else {
                joined = Some(decoded);
            }
        }

        let audio = retarget_channels(&joined.ok_or(TtsError::EmptyText)?, request.channels)?;
        Ok(Synthesis {
            stats: stats_for(started.elapsed(), &audio),
            audio,
        })
    }
}

/// The `ElevenLabs` model a render runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ElevenLabsModel {
    /// `eleven_v4`: the most emotive batch model (the default).
    #[default]
    V4,
    /// `eleven_v4_turbo`: the real-time v4 variant (~100 ms median
    /// latency).
    V4Turbo,
}

impl ElevenLabsModel {
    /// The API model id this variant sends as `model_id`.
    pub fn as_id(self) -> &'static str {
        match self {
            ElevenLabsModel::V4 => "eleven_v4",
            ElevenLabsModel::V4Turbo => "eleven_v4_turbo",
        }
    }
}

/// Configuration for the `ElevenLabs` cloud engine.
#[derive(Debug, Clone)]
pub struct ElevenLabsConfig {
    /// Base URL of the service. The engine appends
    /// `/v1/text-to-speech/{voice_id}`.
    pub base_url: String,
    /// The model renders run on (see [`ElevenLabsModel`]).
    pub model: ElevenLabsModel,
    /// Service voice id used for every request. When unset, the
    /// request's [`SynthRequest::voice`] is read instead (see the module
    /// docs for the resolution order).
    pub voice_id: Option<String>,
    /// The `xi-api-key` token. When `None`, [`ElevenLabsEngine::new`]
    /// falls back to the [`ELEVENLABS_KEY_ENV`] environment variable.
    pub api_key: Option<String>,
    /// Timeout for one whole request (connection, headers, and body).
    pub timeout: Duration,
    /// How many times a 5xx response is retried (2 retries means up to 3
    /// attempts).
    pub retries: u32,
}

impl Default for ElevenLabsConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.elevenlabs.io".to_string(),
            model: ElevenLabsModel::default(),
            voice_id: None,
            api_key: None,
            timeout: Duration::from_secs(30),
            retries: 2,
        }
    }
}

/// The `ElevenLabs` cloud speech engine.
pub struct ElevenLabsEngine {
    config: ElevenLabsConfig,
    agent: ureq::Agent,
    api_key: String,
}

impl ElevenLabsEngine {
    /// Creates the engine, resolving the API key from `config` or the
    /// [`ELEVENLABS_KEY_ENV`] environment variable.
    ///
    /// # Errors
    ///
    /// Returns [`TtsError::NoApiKey`] when neither the configuration nor
    /// the environment provides a key.
    pub fn new(config: ElevenLabsConfig) -> Result<Self, TtsError> {
        let api_key = config
            .api_key
            .clone()
            .or_else(|| std::env::var(ELEVENLABS_KEY_ENV).ok())
            .ok_or(TtsError::NoApiKey)?;
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(config.timeout))
                .build(),
        );
        Ok(Self {
            config,
            agent,
            api_key,
        })
    }

    /// The configuration this engine was built with.
    pub fn config(&self) -> &ElevenLabsConfig {
        &self.config
    }

    /// Resolves the voice id for one request: an explicit
    /// [`ElevenLabsConfig::voice_id`] wins, otherwise the voice named by
    /// the request is read with the `elevenlabs-` lane prefix stripped.
    /// The result must be URL-safe — it travels unencoded in the request
    /// path.
    ///
    /// # Errors
    ///
    /// Returns [`TtsError::UnknownVoice`] when the resolved id is empty
    /// or contains characters outside the URL-unreserved set.
    fn resolve_voice_id(&self, request: &SynthRequest) -> Result<String, TtsError> {
        // A configured id (blank counts as unset) wins; the request voice
        // is the fallback.
        let configured = self
            .config
            .voice_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty());
        let raw = configured.unwrap_or(request.voice.trim());
        let id = raw.strip_prefix("elevenlabs-").unwrap_or(raw);
        if !is_url_safe(id) {
            return Err(TtsError::UnknownVoice {
                id: request.voice.clone(),
            });
        }
        Ok(id.to_string())
    }

    /// Requests one chunk of speech, retrying transient server errors.
    fn request_speech(
        &self,
        voice_id: &str,
        input: &str,
        speed: f32,
        output_format: &str,
    ) -> Result<Vec<u8>, TtsError> {
        let url = format!(
            "{}/v1/text-to-speech/{}?output_format={}",
            self.config.base_url.trim_end_matches('/'),
            voice_id,
            output_format
        );
        let body = serde_json::json!({
            "text": input,
            "model_id": self.config.model.as_id(),
            "voice_settings": { "speed": speed },
        });
        let mut attempt = 0_u32;
        loop {
            let result = self
                .agent
                .post(&url)
                .header("xi-api-key", self.api_key.as_str())
                .send_json(&body);
            match result {
                Ok(mut response) => {
                    // Read the body as raw bytes: WAV is binary and must
                    // never pass through a lossy UTF-8 conversion.
                    return response.body_mut().read_to_vec().map_err(map_ureq_error);
                }
                // ureq 3.x reports every non-2xx status here, so the 5xx
                // retry logic belongs in this arm (the Ok arm above only
                // ever sees 2xx).
                Err(ureq::Error::StatusCode(status)) => {
                    if status >= 500 && attempt < self.config.retries {
                        attempt += 1;
                        std::thread::sleep(Duration::from_millis(
                            RETRY_BACKOFF_MS * u64::from(attempt),
                        ));
                        continue;
                    }
                    return Err(TtsError::Http { status });
                }
                Err(other) => return Err(map_ureq_error(other)),
            }
        }
    }
}

impl TtsEngine for ElevenLabsEngine {
    /// Synthesizes `request` through the `ElevenLabs` service.
    ///
    /// The engine uses its configured model and the request's voice id
    /// (see the module docs); the request's language drives number
    /// normalization, markup tags travel inline in the chunk text, and
    /// the returned audio plays at the `wav_<rate>` the service sent.
    ///
    /// # Errors
    ///
    /// Returns [`TtsError`] when the request fails validation, markup
    /// does not parse, no voice id resolves, the key is missing, the
    /// service answers with an error status after retries
    /// ([`TtsError::Http`]), the request times out ([`TtsError::Timeout`]),
    /// the network fails, or the response is not decodable WAV audio
    /// ([`TtsError::InvalidResponse`]).
    fn synthesize(&self, request: &SynthRequest) -> Result<Synthesis, TtsError> {
        request.validate()?;
        let started = Instant::now();

        // Markup is parsed before anything else: an unknown tag must fail
        // before a single request leaves.
        let segments = if request.markup {
            markup::parse(&request.text)?
        } else {
            markup::plain(&request.text)
        };
        let pieces = elevenlabs_pieces(&segments);
        if pieces.is_empty() {
            return Err(TtsError::EmptyText);
        }

        let voice_id = self.resolve_voice_id(request)?;
        let output_format = format!("wav_{}", nearest_wav_rate(request.sr));
        let joined = self.render_pieces(&pieces, request, &voice_id, &output_format)?;

        let audio = retarget_channels(&joined.ok_or(TtsError::EmptyText)?, request.channels)?;
        Ok(Synthesis {
            stats: stats_for(started.elapsed(), &audio),
            audio,
        })
    }
}

impl ElevenLabsEngine {
    /// Walks the markup pieces: every run's chunks are requested from the
    /// service and joined onto the accumulator, with pauses inserted
    /// exactly. Returns the joined audio, or `None` when no audio was
    /// ever produced.
    ///
    /// # Errors
    ///
    /// Surfaces the service and decoding errors of
    /// [`ElevenLabsEngine::request_speech`] and `decode_wav_bytes`, plus
    /// the join's rate-mismatch error.
    fn render_pieces(
        &self,
        pieces: &[markup::Piece],
        request: &SynthRequest,
        voice_id: &str,
        output_format: &str,
    ) -> Result<Option<AudioBuffer>, TtsError> {
        let mut joined: Option<AudioBuffer> = None;
        let mut pending_pause_ms: u32 = 0;
        for piece in pieces {
            match piece {
                // Consecutive pauses add up, like the local engine.
                markup::Piece::Pause(ms) => pending_pause_ms += ms,
                markup::Piece::Run(text) => {
                    let normalized = expand_numbers(text, request.language);
                    for chunk in chunk_text(&normalized, CHUNK_CHARS) {
                        let bytes =
                            self.request_speech(voice_id, &chunk, request.speed, output_format)?;
                        let decoded = decode_wav_bytes(&bytes)?;
                        joined = join_chunk(joined, decoded, pending_pause_ms)?;
                        pending_pause_ms = 0;
                    }
                }
            }
        }
        // A trailing [pause] is trailing silence; a text of only pauses
        // renders as exactly that silence.
        if pending_pause_ms > 0 {
            joined = Some(append_trailing_pause(
                joined,
                pending_pause_ms,
                nearest_wav_rate(request.sr),
            ));
        }
        Ok(joined)
    }
}

/// Joins one decoded chunk onto the accumulator, inserting the pause that
/// belongs before it: the exact pending markup pause when one is open,
/// the lane's quarter-second default between chunks of one run, and
/// nothing before the very first chunk — where an open pause instead
/// becomes leading silence.
///
/// # Errors
///
/// Returns [`TtsError::InvalidResponse`] when the chunk's rate or channel
/// count differs from what was joined before.
fn join_chunk(
    joined: Option<AudioBuffer>,
    decoded: AudioBuffer,
    pending_pause_ms: u32,
) -> Result<Option<AudioBuffer>, TtsError> {
    let pause_ms = if pending_pause_ms > 0 {
        pending_pause_ms
    } else if joined.is_some() {
        PAUSE_SECS_MS
    } else {
        0
    };
    let Some(mut acc) = joined else {
        // Before the first chunk an open pause is leading silence.
        let start = if pause_ms == 0 {
            decoded
        } else {
            let mut lead = silence_buffer(pause_ms, decoded.sample_rate());
            lead.concat(&decoded);
            lead
        };
        return Ok(Some(start));
    };
    if decoded.sample_rate() != acc.sample_rate() || decoded.num_channels() != acc.num_channels() {
        return Err(TtsError::InvalidResponse {
            detail: format!(
                "the service returned {} Hz / {} ch for one chunk but {} Hz / {} ch for another",
                decoded.sample_rate(),
                decoded.num_channels(),
                acc.sample_rate(),
                acc.num_channels()
            ),
        });
    }
    if pause_ms > 0 {
        let pause = silence_buffer(pause_ms, acc.sample_rate());
        acc.concat(&pause);
    }
    acc.concat(&decoded);
    Ok(Some(acc))
}

/// Appends a trailing pause as trailing silence; with no audio joined it
/// is the whole (silent) output, at `fallback_rate`.
fn append_trailing_pause(joined: Option<AudioBuffer>, ms: u32, fallback_rate: u32) -> AudioBuffer {
    let rate = joined
        .as_ref()
        .map_or(fallback_rate, AudioBuffer::sample_rate);
    let tail = silence_buffer(ms, rate);
    match joined {
        Some(mut acc) => {
            acc.concat(&tail);
            acc
        }
        None => tail,
    }
}

/// The `ElevenLabs` token for one expression tag, from the documented v4
/// audio-tag vocabulary: `[laughs]`, `[exhales]`, `[sighs]` (there is no
/// literal `[breath]` tag; `[exhales]` is the documented breath sound).
fn elevenlabs_token(tag: markup::ExpressionTag) -> &'static str {
    match tag {
        markup::ExpressionTag::Laugh => "[laughs]",
        markup::ExpressionTag::Breath => "[exhales]",
        markup::ExpressionTag::Sigh => "[sighs]",
    }
}

/// Renders parsed segments as `ElevenLabs`-ready pieces: expression tags
/// become the service's bracket tokens inline, and `[pause:<ms>]` becomes
/// an exact pause piece. The same shape as [`markup::for_supertonic`] with
/// the cloud vocabulary inlined (the tag map lives here because the
/// markup module stays engine-neutral).
fn elevenlabs_pieces(segments: &[markup::Segment]) -> Vec<markup::Piece> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    for segment in segments {
        match segment {
            markup::Segment::Text(text) => {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(text);
            }
            markup::Segment::Tag(tag) => {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(elevenlabs_token(*tag));
            }
            markup::Segment::Pause(ms) => {
                commit_run(&mut pieces, &mut current);
                pieces.push(markup::Piece::Pause(*ms));
            }
        }
    }
    commit_run(&mut pieces, &mut current);
    pieces
}

/// Commits the pending run as a `Run` piece (if non-empty).
fn commit_run(pieces: &mut Vec<markup::Piece>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        pieces.push(markup::Piece::Run(trimmed.to_string()));
    }
    current.clear();
}

/// A silence buffer of exactly `ms` milliseconds at `rate`.
fn silence_buffer(ms: u32, rate: u32) -> AudioBuffer {
    let frames = (f64::from(rate) * f64::from(ms) / 1000.0).round() as usize;
    AudioBuffer::new(1, frames, rate)
}

/// The supported WAV rate closest to `sr`; ties go to the higher rate so
/// unsupported targets never lose bandwidth unnecessarily.
fn nearest_wav_rate(sr: u32) -> u32 {
    WAV_RATES.iter().copied().fold(WAV_RATES[0], |best, rate| {
        let to_best = i64::from(best) - i64::from(sr);
        let to_rate = i64::from(rate) - i64::from(sr);
        if to_rate.abs() < to_best.abs() || (to_rate.abs() == to_best.abs() && rate > best) {
            rate
        } else {
            best
        }
    })
}

/// Whether `id` is non-empty and URL-unreserved only, so it can travel
/// unencoded in the request path (the service's voice ids are UUIDs and
/// short alphanumeric library ids).
fn is_url_safe(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
}

/// Maps a `ureq` error onto the public error type.
fn map_ureq_error(error: ureq::Error) -> TtsError {
    match error {
        ureq::Error::StatusCode(status) => TtsError::Http { status },
        ureq::Error::Timeout(_) => TtsError::Timeout,
        other => TtsError::Network {
            detail: other.to_string(),
        },
    }
}

/// Decodes a WAV file from raw bytes: 16/24-bit integer or 32-bit float,
/// any channel count.
fn decode_wav_bytes(bytes: &[u8]) -> Result<AudioBuffer, TtsError> {
    let invalid = |detail: String| TtsError::InvalidResponse { detail };
    let mut reader = hound::WavReader::new(Cursor::new(bytes))
        .map_err(|error| invalid(format!("response body is not WAV audio: {error}")))?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels);
    if channels == 0 {
        return Err(invalid("WAV audio with zero channels".to_string()));
    }
    let read_samples = |samples: Result<Vec<f32>, hound::Error>| {
        samples.map_err(|error| invalid(format!("WAV sample data is corrupt: {error}")))
    };
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => read_samples(
            reader
                .samples::<i16>()
                .map(|sample| sample.map(|value| f32::from(value) / 32_768.0))
                .collect(),
        )?,
        (hound::SampleFormat::Int, 24) => read_samples(
            reader
                .samples::<i32>()
                .map(|sample| sample.map(|value| value as f32 / 8_388_608.0))
                .collect(),
        )?,
        (hound::SampleFormat::Float, 32) => read_samples(reader.samples::<f32>().collect())?,
        (sample_format, bits) => {
            return Err(invalid(format!(
                "unsupported WAV depth: {bits}-bit {}",
                if sample_format == hound::SampleFormat::Float {
                    "float"
                } else {
                    "int"
                }
            )))
        }
    };
    if samples.len() % channels != 0 {
        return Err(invalid(
            "WAV sample data is not a whole number of frames".to_string(),
        ));
    }
    let deinterleaved: Vec<Vec<f32>> = (0..channels)
        .map(|channel| {
            samples
                .iter()
                .skip(channel)
                .step_by(channels)
                .copied()
                .collect()
        })
        .collect();
    AudioBuffer::from_channels(deinterleaved, spec.sample_rate)
        .map_err(|_| invalid("WAV audio with an invalid sample rate".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::textnorm::Language;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    /// What the mock server answers for one request.
    #[derive(Clone)]
    enum Scripted {
        /// A full HTTP response with this status and body.
        Respond { status: u16, body: Arc<Vec<u8>> },
        /// Accept the request, then hold the connection for this long
        /// without answering.
        Stall { seconds: u64 },
    }

    /// One request as the mock server saw it. Header names are lowercased
    /// because clients (ureq included) are free to change case.
    #[derive(Debug)]
    struct Recorded {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    /// Starts a thread-per-connection HTTP/1.1 mock on `127.0.0.1:0`.
    ///
    /// Requests are recorded in order; request `i` is answered with
    /// `responses[i]` (the last entry repeats for extra requests).
    fn spawn_mock(responses: Vec<Scripted>) -> (String, Arc<Mutex<Vec<Recorded>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let addr = listener.local_addr().expect("mock address");
        let log: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
        let log_for_accept = Arc::clone(&log);
        std::thread::spawn(move || {
            for (next, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { break };
                let response = responses
                    .get(next)
                    .or_else(|| responses.last())
                    .expect("at least one scripted response")
                    .clone();
                let log_for_conn = Arc::clone(&log_for_accept);
                std::thread::spawn(move || handle_connection(&mut stream, response, &log_for_conn));
            }
        });
        (format!("http://{addr}"), log)
    }

    fn handle_connection(
        stream: &mut TcpStream,
        response: Scripted,
        log: &Arc<Mutex<Vec<Recorded>>>,
    ) {
        if let Some(recorded) = read_request(stream) {
            log.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(recorded);
        }
        match response {
            Scripted::Respond { status, body } => {
                let reason = match status {
                    200 => "OK",
                    500 => "Internal Server Error",
                    503 => "Service Unavailable",
                    _ => "Status",
                };
                let head = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
            Scripted::Stall { seconds } => {
                std::thread::sleep(Duration::from_secs(seconds));
            }
        }
    }

    /// Reads one HTTP request (head + `Content-Length` body). Header names
    /// are stored lowercased so the assertions match regardless of the
    /// case the client used.
    fn read_request(stream: &mut TcpStream) -> Option<Recorded> {
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            match stream.read(&mut byte) {
                Ok(0) | Err(_) => return None,
                Ok(_) => {
                    head.push(byte[0]);
                    if head.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
            }
        }
        let head = String::from_utf8_lossy(&head);
        let mut lines = head.split("\r\n");
        let request_line = lines.next()?.to_string();
        let mut parts = request_line.split_whitespace();
        let method = parts.next()?.to_string();
        let path = parts.next()?.to_string();
        let mut headers = HashMap::new();
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }
        let length: usize = headers
            .get("content-length")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0_u8; length];
        let mut read = 0;
        while read < length {
            match stream.read(&mut body[read..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => read += n,
            }
        }
        Some(Recorded {
            method,
            path,
            headers,
            body,
        })
    }

    /// A short sine-tone WAV (16-bit mono) as raw bytes.
    fn wav_bytes(sample_rate: u32, seconds: f32) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec).expect("wav writer");
            let frames = (f64::from(sample_rate) * f64::from(seconds)) as usize;
            for frame in 0..frames {
                let phase = frame as f32 * 440.0 / sample_rate as f32;
                let sample = (phase * std::f32::consts::PI * 2.0).sin() * 0.5;
                writer
                    .write_sample((sample * 32_767.0) as i16)
                    .expect("write");
            }
            writer.finalize().expect("finalize");
        }
        cursor.into_inner()
    }

    fn engine(base: &str, retries: u32, timeout: Duration) -> OpenAiCompatibleEngine {
        OpenAiCompatibleEngine::new(CloudConfig {
            base_url: base.to_string(),
            model: "test-model".to_string(),
            voice: "test-voice".to_string(),
            api_key: Some("test-key".to_string()),
            timeout,
            retries,
        })
        .expect("engine")
    }

    fn request(text: &str) -> SynthRequest {
        SynthRequest {
            text: text.to_string(),
            language: Language::En,
            speed: 1.0,
            voice: "cloud".to_string(),
            sr: 22_050,
            channels: 1,
            steps: crate::engine::DEFAULT_STEPS,
            seed: None,
            prosody: crate::engine::ProsodyMode::Natural,
            style: crate::engine::StyleSpec::Default,
            markup: true,
        }
    }

    fn ok(status: u16, body: Vec<u8>) -> Scripted {
        Scripted::Respond {
            status,
            body: Arc::new(body),
        }
    }

    #[test]
    fn sends_bearer_auth_and_decodes_wav() {
        let wav = wav_bytes(22_050, 0.3);
        let (base, log) = spawn_mock(vec![ok(200, wav)]);
        let engine = engine(&base, 0, Duration::from_secs(10));

        let synthesis = engine
            .synthesize(&request("Chapter 42 is here."))
            .expect("synthesis");
        assert_eq!(synthesis.audio.sample_rate(), 22_050);
        assert_eq!(synthesis.audio.num_channels(), 1);
        assert!(synthesis.audio.peak() > 0.05, "audio should not be silent");
        assert!(synthesis.audio.duration() > 0.2);

        let log = log.lock().expect("log");
        assert_eq!(log.len(), 1);
        let recorded = &log[0];
        assert_eq!(recorded.method, "POST");
        assert_eq!(recorded.path, "/audio/speech");
        assert_eq!(
            recorded.headers.get("authorization").map(String::as_str),
            Some("Bearer test-key"),
            "case-insensitive header lookup: {recorded:?}"
        );
        let body: serde_json::Value = serde_json::from_slice(&recorded.body).expect("json body");
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["voice"], "test-voice");
        assert_eq!(body["response_format"], "wav");
        assert_eq!(body["speed"], 1.0);
        // Numbers were expanded before the request left.
        assert_eq!(body["input"], "Chapter forty-two is here.");
    }

    #[test]
    fn retries_5xx_then_succeeds() {
        let wav = wav_bytes(22_050, 0.2);
        let (base, log) = spawn_mock(vec![ok(500, Vec::new()), ok(503, Vec::new()), ok(200, wav)]);
        let engine = engine(&base, 2, Duration::from_secs(10));

        let synthesis = engine
            .synthesize(&request("Hello there."))
            .expect("synthesis");
        assert!(synthesis.audio.frames() > 0);
        assert_eq!(log.lock().expect("log").len(), 3);
    }

    #[test]
    fn persistent_503_fails_with_http_status() {
        let (base, log) = spawn_mock(vec![ok(503, Vec::new())]);
        let engine = engine(&base, 2, Duration::from_secs(10));

        let error = engine
            .synthesize(&request("Hello there."))
            .expect_err("should fail");
        match &error {
            TtsError::Http { status } => assert_eq!(*status, 503),
            other => panic!("expected Http, got {other:?}"),
        }
        assert!(
            error.to_string().contains("HTTP 503"),
            "message should name the status: {error}"
        );
        assert_eq!(log.lock().expect("log").len(), 3);
    }

    #[test]
    fn stalled_response_times_out() {
        let (base, _log) = spawn_mock(vec![Scripted::Stall { seconds: 3 }]);
        let engine = engine(&base, 2, Duration::from_millis(400));

        let started = Instant::now();
        let error = engine
            .synthesize(&request("Hello there."))
            .expect_err("should fail");
        let elapsed = started.elapsed();
        assert!(
            matches!(error, TtsError::Timeout),
            "expected Timeout, got {error:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "timeout must fire before the server answers: {elapsed:?}"
        );
    }

    #[test]
    fn non_wav_body_is_a_clean_response_error() {
        let (base, _log) = spawn_mock(vec![ok(200, b"this is definitely not audio".to_vec())]);
        let engine = engine(&base, 0, Duration::from_secs(10));

        let error = engine
            .synthesize(&request("Hello there."))
            .expect_err("should fail");
        match &error {
            TtsError::InvalidResponse { detail } => {
                assert!(
                    detail.contains("WAV") || detail.contains("audio"),
                    "error should say what was wrong: {detail}"
                );
            }
            other => panic!("expected InvalidResponse, got {other:?}"),
        }
        assert!(
            !error.to_string().contains("panicked"),
            "error must be a clean message: {error}"
        );
    }

    #[test]
    fn joins_chunks_with_a_quarter_second_pause() {
        // Long enough to force a second chunk.
        let text = format!("{} {}", "word".repeat(400), "word".repeat(400));
        let (base, log) = spawn_mock(vec![
            ok(200, wav_bytes(22_050, 0.2)),
            ok(200, wav_bytes(22_050, 0.2)),
        ]);
        let engine = engine(&base, 0, Duration::from_secs(10));

        let synthesis = engine.synthesize(&request(&text)).expect("synthesis");
        assert_eq!(log.lock().expect("log").len(), 2);
        let expected = 0.2 + 0.25 + 0.2;
        let actual = synthesis.audio.duration();
        assert!(
            (actual - expected).abs() < 0.05,
            "expected about {expected:.2}s of audio (two chunks plus a 0.25s pause), got {actual:.2}s"
        );
    }

    #[test]
    fn stereo_is_duplicated_when_requested() {
        let wav = wav_bytes(22_050, 0.2);
        let (base, _log) = spawn_mock(vec![ok(200, wav)]);
        let engine = engine(&base, 0, Duration::from_secs(10));

        let stereo_request = SynthRequest {
            channels: 2,
            ..request("Hello there.")
        };
        let synthesis = engine.synthesize(&stereo_request).expect("synthesis");
        assert_eq!(synthesis.audio.num_channels(), 2);
        assert_eq!(synthesis.audio.channels()[0], synthesis.audio.channels()[1]);
    }

    #[test]
    fn missing_api_key_is_rejected_and_env_fallback_order_holds() {
        // No other test in this module constructs an engine without an
        // explicit key, so the env reads below have no concurrent reader.
        std::env::remove_var(API_KEY_ENV);
        std::env::remove_var(OPENAI_KEY_ENV);
        let error = OpenAiCompatibleEngine::new(CloudConfig {
            api_key: None,
            ..CloudConfig::default()
        })
        .err()
        .expect("no key");
        assert!(matches!(error, TtsError::NoApiKey));

        std::env::set_var(OPENAI_KEY_ENV, "openai-key");
        std::env::set_var(API_KEY_ENV, "qinari-key");
        let engine = OpenAiCompatibleEngine::new(CloudConfig::default()).expect("env key");
        assert_eq!(engine.api_key, "qinari-key");

        std::env::remove_var(API_KEY_ENV);
        let engine = OpenAiCompatibleEngine::new(CloudConfig::default()).expect("openai key");
        assert_eq!(engine.api_key, "openai-key");

        std::env::remove_var(OPENAI_KEY_ENV);
    }

    /// Builds an `ElevenLabs` engine against `base` with an explicit key, so
    /// no test but the env-fallback one reads the environment.
    fn eleven_engine(
        base: &str,
        model: ElevenLabsModel,
        retries: u32,
        timeout: Duration,
        voice_id: Option<&str>,
    ) -> ElevenLabsEngine {
        ElevenLabsEngine::new(ElevenLabsConfig {
            base_url: base.to_string(),
            model,
            voice_id: voice_id.map(str::to_string),
            api_key: Some("test-key".to_string()),
            timeout,
            retries,
        })
        .expect("engine")
    }

    /// A request for the `ElevenLabs` lane: English, 22 050 Hz, markup on,
    /// voice `elevenlabs-test-voice` (the lane prefix form).
    fn eleven_request(text: &str) -> SynthRequest {
        SynthRequest {
            text: text.to_string(),
            language: Language::En,
            speed: 1.0,
            voice: "elevenlabs-test-voice".to_string(),
            sr: 22_050,
            channels: 1,
            steps: crate::engine::DEFAULT_STEPS,
            seed: None,
            prosody: crate::engine::ProsodyMode::Natural,
            style: crate::engine::StyleSpec::Default,
            markup: true,
        }
    }

    #[test]
    fn eleven_sends_xi_api_key_and_decodes_wav() {
        let wav = wav_bytes(22_050, 0.3);
        let (base, log) = spawn_mock(vec![ok(200, wav)]);
        let engine = eleven_engine(&base, ElevenLabsModel::V4, 0, Duration::from_secs(10), None);

        let synthesis = engine
            .synthesize(&eleven_request("Chapter 42 is here."))
            .expect("synthesis");
        assert_eq!(synthesis.audio.sample_rate(), 22_050);
        assert_eq!(synthesis.audio.num_channels(), 1);
        assert!(synthesis.audio.peak() > 0.05, "audio should not be silent");
        assert!(synthesis.audio.duration() > 0.2);

        let log = log.lock().expect("log");
        assert_eq!(log.len(), 1);
        let recorded = &log[0];
        assert_eq!(recorded.method, "POST");
        // The voice id and output format travel in the path and query, not
        // the body, and the lane prefix is stripped on the way.
        assert_eq!(
            recorded.path, "/v1/text-to-speech/test-voice?output_format=wav_22050",
            "path: {recorded:?}"
        );
        assert_eq!(
            recorded.headers.get("xi-api-key").map(String::as_str),
            Some("test-key")
        );
        assert!(
            !recorded.headers.contains_key("authorization"),
            "the `ElevenLabs` lane must not send a Bearer header: {recorded:?}"
        );
        let body: serde_json::Value = serde_json::from_slice(&recorded.body).expect("json body");
        assert_eq!(body["model_id"], "eleven_v4");
        assert_eq!(body["voice_settings"]["speed"], 1.0);
        // Numbers were expanded before the request left.
        assert_eq!(body["text"], "Chapter forty-two is here.");
    }

    #[test]
    fn eleven_retries_5xx_then_succeeds() {
        let wav = wav_bytes(22_050, 0.2);
        let (base, log) = spawn_mock(vec![ok(500, Vec::new()), ok(503, Vec::new()), ok(200, wav)]);
        let engine = eleven_engine(&base, ElevenLabsModel::V4, 2, Duration::from_secs(10), None);

        let synthesis = engine
            .synthesize(&eleven_request("Hello there."))
            .expect("synthesis");
        assert!(synthesis.audio.frames() > 0);
        assert_eq!(log.lock().expect("log").len(), 3);
    }

    #[test]
    fn eleven_persistent_500_fails_with_http_status() {
        let (base, log) = spawn_mock(vec![ok(500, Vec::new())]);
        let engine = eleven_engine(&base, ElevenLabsModel::V4, 2, Duration::from_secs(10), None);

        let error = engine
            .synthesize(&eleven_request("Hello there."))
            .expect_err("should fail");
        match &error {
            TtsError::Http { status } => assert_eq!(*status, 500),
            other => panic!("expected Http, got {other:?}"),
        }
        assert!(
            error.to_string().contains("HTTP 500"),
            "message should name the status: {error}"
        );
        assert_eq!(log.lock().expect("log").len(), 3);
    }

    #[test]
    fn eleven_non_wav_body_is_a_clean_response_error() {
        let (base, _log) = spawn_mock(vec![ok(200, b"this is definitely not audio".to_vec())]);
        let engine = eleven_engine(&base, ElevenLabsModel::V4, 0, Duration::from_secs(10), None);

        let error = engine
            .synthesize(&eleven_request("Hello there."))
            .expect_err("should fail");
        match &error {
            TtsError::InvalidResponse { detail } => {
                assert!(
                    detail.contains("WAV") || detail.contains("audio"),
                    "error should say what was wrong: {detail}"
                );
            }
            other => panic!("expected InvalidResponse, got {other:?}"),
        }
    }

    #[test]
    fn eleven_stalled_response_times_out() {
        let (base, _log) = spawn_mock(vec![Scripted::Stall { seconds: 3 }]);
        let engine = eleven_engine(
            &base,
            ElevenLabsModel::V4,
            2,
            Duration::from_millis(400),
            None,
        );

        let started = Instant::now();
        let error = engine
            .synthesize(&eleven_request("Hello there."))
            .expect_err("should fail");
        let elapsed = started.elapsed();
        assert!(
            matches!(error, TtsError::Timeout),
            "expected Timeout, got {error:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "timeout must fire before the server answers: {elapsed:?}"
        );
    }

    #[test]
    fn eleven_output_format_snaps_to_the_nearest_supported_rate() {
        // Exact matches pass through; anything else snaps to the nearest
        // supported rate, ties going up.
        assert_eq!(nearest_wav_rate(8_000), 8_000);
        assert_eq!(nearest_wav_rate(22_050), 22_050);
        assert_eq!(nearest_wav_rate(44_100), 44_100);
        assert_eq!(nearest_wav_rate(12_000), 16_000);
        assert_eq!(nearest_wav_rate(192_000), 48_000);
        assert_eq!(nearest_wav_rate(1), 8_000);

        // On the wire: an unsupported target asks for the nearest rate.
        let (base, log) = spawn_mock(vec![ok(200, wav_bytes(22_050, 0.2))]);
        let engine = eleven_engine(&base, ElevenLabsModel::V4, 0, Duration::from_secs(10), None);
        let request = SynthRequest {
            sr: 192_000,
            ..eleven_request("Hello there.")
        };
        engine.synthesize(&request).expect("synthesis");
        let path = &log.lock().expect("log")[0].path;
        assert!(
            path.ends_with("output_format=wav_48000"),
            "192 kHz should request wav_48000, got {path}"
        );
    }

    #[test]
    fn eleven_pause_inserts_exact_silence_in_the_join() {
        let (base, log) = spawn_mock(vec![
            ok(200, wav_bytes(22_050, 0.2)),
            ok(200, wav_bytes(22_050, 0.2)),
        ]);
        let engine = eleven_engine(&base, ElevenLabsModel::V4, 0, Duration::from_secs(10), None);

        let synthesis = engine
            .synthesize(&eleven_request("Hello [pause:300] there."))
            .expect("synthesis");
        assert_eq!(log.lock().expect("log").len(), 2);
        let expected = 0.2 + 0.3 + 0.2;
        let actual = synthesis.audio.duration();
        assert!(
            (actual - expected).abs() < 0.05,
            "expected about {expected:.2}s of audio (two chunks plus an exact 300 ms pause), got {actual:.2}s"
        );
    }

    #[test]
    fn eleven_leading_and_trailing_pauses_are_silence() {
        let (base, log) = spawn_mock(vec![ok(200, wav_bytes(22_050, 0.2))]);
        let engine = eleven_engine(&base, ElevenLabsModel::V4, 0, Duration::from_secs(10), None);

        let synthesis = engine
            .synthesize(&eleven_request("[pause:100] Hello [pause:200]"))
            .expect("synthesis");
        assert_eq!(log.lock().expect("log").len(), 1);
        let expected = 0.1 + 0.2 + 0.2;
        let actual = synthesis.audio.duration();
        assert!(
            (actual - expected).abs() < 0.05,
            "expected about {expected:.2}s (leading and trailing silence around one chunk), got {actual:.2}s"
        );
    }

    #[test]
    fn eleven_pause_only_text_renders_exact_silence() {
        let (base, log) = spawn_mock(vec![]);
        let engine = eleven_engine(&base, ElevenLabsModel::V4, 0, Duration::from_secs(10), None);

        let synthesis = engine
            .synthesize(&eleven_request("[pause:400]"))
            .expect("synthesis");
        assert_eq!(log.lock().expect("log").len(), 0, "no request needed");
        let actual = synthesis.audio.duration();
        assert!(
            (actual - 0.4).abs() < 0.01,
            "expected 0.4s of pure silence, got {actual:.2}s"
        );
        assert!(
            synthesis.audio.peak() < 1e-9,
            "the render must be silent, peak {}",
            synthesis.audio.peak()
        );
    }

    #[test]
    fn eleven_joins_long_runs_with_the_default_pause() {
        // Long enough to force a second chunk within one run; the join
        // between chunks of a run uses the 0.25 s default.
        let text = format!("{} {}", "word".repeat(400), "word".repeat(400));
        let (base, log) = spawn_mock(vec![
            ok(200, wav_bytes(22_050, 0.2)),
            ok(200, wav_bytes(22_050, 0.2)),
        ]);
        let engine = eleven_engine(&base, ElevenLabsModel::V4, 0, Duration::from_secs(10), None);

        let synthesis = engine
            .synthesize(&eleven_request(&text))
            .expect("synthesis");
        assert_eq!(log.lock().expect("log").len(), 2);
        let expected = 0.2 + 0.25 + 0.2;
        let actual = synthesis.audio.duration();
        assert!(
            (actual - expected).abs() < 0.05,
            "expected about {expected:.2}s of audio (two chunks plus the default pause), got {actual:.2}s"
        );
    }

    #[test]
    fn eleven_tag_map_follows_the_documented_vocabulary() {
        // Unit level: the pieces mirror the shape of the Supertonic
        // mapping with the `ElevenLabs` tokens inline.
        let segments =
            markup::parse("Pada zaman dahulu... [pause:400ms] hiduplah [laugh] seorang anak.")
                .expect("valid markup");
        assert_eq!(
            elevenlabs_pieces(&segments),
            vec![
                markup::Piece::Run("Pada zaman dahulu...".to_string()),
                markup::Piece::Pause(400),
                markup::Piece::Run("hiduplah [laughs] seorang anak.".to_string()),
            ]
        );
        let segments = markup::parse("Dia [laugh] lalu [breath] menghela [sigh]").expect("valid");
        assert_eq!(
            elevenlabs_pieces(&segments),
            vec![markup::Piece::Run(
                "Dia [laughs] lalu [exhales] menghela [sighs]".to_string()
            )]
        );
    }

    #[test]
    fn eleven_tags_travel_inline_in_the_request_text() {
        let (base, log) = spawn_mock(vec![ok(200, wav_bytes(22_050, 0.2))]);
        let engine = eleven_engine(&base, ElevenLabsModel::V4, 0, Duration::from_secs(10), None);
        engine
            .synthesize(&eleven_request(
                "Dia tertawa [laugh] lalu menghela [breath]",
            ))
            .expect("synthesis");
        let body: serde_json::Value =
            serde_json::from_slice(&log.lock().expect("log")[0].body).expect("json body");
        assert_eq!(body["text"], "Dia tertawa [laughs] lalu menghela [exhales]");
    }

    #[test]
    fn eleven_unknown_tag_is_a_hard_error_before_any_request() {
        let (base, log) = spawn_mock(vec![ok(200, wav_bytes(22_050, 0.2))]);
        let engine = eleven_engine(&base, ElevenLabsModel::V4, 0, Duration::from_secs(10), None);

        let error = engine
            .synthesize(&eleven_request("dia [cough] keras"))
            .expect_err("unknown tag");
        match &error {
            TtsError::Markup { detail } => {
                assert!(detail.contains("[cough]"), "{detail}");
            }
            other => panic!("expected Markup, got {other:?}"),
        }
        assert_eq!(
            log.lock().expect("log").len(),
            0,
            "the markup error must fire before any request is sent"
        );
    }

    #[test]
    fn eleven_model_ids_default_to_v4_and_turbo_is_selectable() {
        assert_eq!(ElevenLabsConfig::default().model, ElevenLabsModel::V4);
        assert_eq!(ElevenLabsModel::V4.as_id(), "eleven_v4");
        assert_eq!(ElevenLabsModel::V4Turbo.as_id(), "eleven_v4_turbo");

        // On the wire: the turbo engine names its model.
        let (base, log) = spawn_mock(vec![ok(200, wav_bytes(22_050, 0.2))]);
        let engine = eleven_engine(
            &base,
            ElevenLabsModel::V4Turbo,
            0,
            Duration::from_secs(10),
            None,
        );
        engine
            .synthesize(&eleven_request("Hello there."))
            .expect("synthesis");
        let body: serde_json::Value =
            serde_json::from_slice(&log.lock().expect("log")[0].body).expect("json body");
        assert_eq!(body["model_id"], "eleven_v4_turbo");
    }

    #[test]
    fn eleven_voice_resolution_prefix_raw_config_override_and_url_safety() {
        // The dummy base is never contacted: resolution is pure.
        let engine = eleven_engine(
            "http://127.0.0.1:1",
            ElevenLabsModel::V4,
            0,
            Duration::from_secs(10),
            None,
        );
        let resolve = |voice: &str| engine.resolve_voice_id(&eleven_request_with_voice(voice));

        // The lane prefix is stripped; raw ids pass through.
        assert_eq!(resolve("elevenlabs-Aria").ok().as_deref(), Some("Aria"));
        assert_eq!(resolve("  elevenlabs-Aria  ").ok().as_deref(), Some("Aria"));
        assert_eq!(
            resolve("21m00Tcm4TlvDq8ikWAM").ok().as_deref(),
            Some("21m00Tcm4TlvDq8ikWAM")
        );

        // Not URL-safe or empty after stripping: a clean UnknownVoice.
        for bad in ["elevenlabs-", "bad/id", "has space", ""] {
            match resolve(bad) {
                Err(TtsError::UnknownVoice { id }) => assert_eq!(id, bad, "{bad:?}"),
                other => panic!("expected UnknownVoice for {bad:?}, got {other:?}"),
            }
        }

        // A configured id wins over the request voice.
        let engine = eleven_engine(
            "http://127.0.0.1:1",
            ElevenLabsModel::V4,
            0,
            Duration::from_secs(10),
            Some("  config-voice  "),
        );
        assert_eq!(
            engine
                .resolve_voice_id(&eleven_request_with_voice("elevenlabs-Aria"))
                .ok()
                .as_deref(),
            Some("config-voice")
        );
    }

    /// [`eleven_request`] with a specific `voice` field, for resolution
    /// tests (the base URL is never contacted).
    fn eleven_request_with_voice(voice: &str) -> SynthRequest {
        SynthRequest {
            voice: voice.to_string(),
            ..eleven_request("Hello there.")
        }
    }

    #[test]
    fn eleven_missing_api_key_is_rejected_and_env_fallback_holds() {
        // No other `ElevenLabs` test constructs an engine without an explicit
        // key, so the env reads below have no concurrent reader.
        std::env::remove_var(ELEVENLABS_KEY_ENV);
        let error = ElevenLabsEngine::new(ElevenLabsConfig {
            api_key: None,
            ..ElevenLabsConfig::default()
        })
        .err()
        .expect("no key");
        assert!(matches!(error, TtsError::NoApiKey));

        std::env::set_var(ELEVENLABS_KEY_ENV, "xi-key");
        let engine = ElevenLabsEngine::new(ElevenLabsConfig::default()).expect("env key");
        assert_eq!(engine.api_key, "xi-key");

        std::env::remove_var(ELEVENLABS_KEY_ENV);
    }
}
