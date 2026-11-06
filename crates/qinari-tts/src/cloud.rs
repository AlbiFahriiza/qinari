//! The OpenAI-compatible cloud engine: any service that speaks
//! `POST {base}/audio/speech`.
//!
//! Synthesis normalizes numbers, splits the text into bounded chunks, and
//! requests each chunk as a WAV body with bearer authentication. Transient
//! server failures (HTTP 5xx) are retried with linear backoff; timeouts
//! and 4xx responses surface immediately. Decoded chunks are joined with a
//! quarter-second pause and converted to the requested channel count.

use crate::engine::{retarget_channels, stats_for, SynthRequest, Synthesis, TtsEngine};
use crate::textnorm::{chunk_text, expand_numbers};
use crate::TtsError;
use qinari_dsp::buffer::AudioBuffer;
use std::io::Cursor;
use std::time::{Duration, Instant};

/// Characters of normalized text per request, matching the local engine.
const CHUNK_CHARS: usize = 3000;

/// Silence inserted between chunk audio, in seconds.
const PAUSE_SECS: f64 = 0.25;

/// Base delay for 5xx retries; attempt `n` waits `n * RETRY_BACKOFF_MS`.
const RETRY_BACKOFF_MS: u64 = 300;

/// Environment variable checked first for the cloud API key.
pub const API_KEY_ENV: &str = "QINARI_TTS_API_KEY";

/// Environment variable checked second for the cloud API key.
pub const OPENAI_KEY_ENV: &str = "OPENAI_API_KEY";

/// Configuration for the OpenAI-compatible cloud engine.
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
}
