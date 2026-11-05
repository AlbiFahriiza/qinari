//! The engine-agnostic synthesis contract: requests, validation, and
//! results shared by the local VITS engine and the cloud engine.

use crate::TtsError;
use qinari_dsp::buffer::AudioBuffer;
use std::time::Duration;

/// Maximum text length accepted by [`SynthRequest::validate`] (in Unicode
/// characters).
pub const MAX_TEXT_CHARS: usize = 20_000;

/// Lower bound for the speed factor.
pub const MIN_SPEED: f32 = 0.5;

/// Upper bound for the speed factor.
pub const MAX_SPEED: f32 = 2.0;

/// Lowest sample rate a request may name.
pub const MIN_SAMPLE_RATE: u32 = 8_000;

/// Highest sample rate a request may name.
pub const MAX_SAMPLE_RATE: u32 = 192_000;

/// A speech synthesis request.
#[derive(Debug, Clone)]
pub struct SynthRequest {
    /// Text to speak. Validated non-empty and at most
    /// [`MAX_TEXT_CHARS`] characters.
    pub text: String,
    /// Language of the text; drives number normalization and phonemization.
    pub language: crate::textnorm::Language,
    /// Speed factor: `1.0` is the voice's natural pace, `0.5` twice as
    /// slow, `2.0` twice as fast.
    pub speed: f32,
    /// Voice catalog id (see [`crate::voices::voices`]).
    pub voice: String,
    /// Target sample rate of the returned audio. The local VITS engine
    /// only produces its voice's native rate (22 050 Hz for both bundled
    /// voices) and rejects anything else; the cloud engine returns whatever
    /// rate the service sends.
    pub sr: u32,
    /// Target channel count of the returned audio: 1 or 2.
    pub channels: u16,
}

impl SynthRequest {
    /// Validates the request against the engine limits.
    ///
    /// # Errors
    ///
    /// Returns [`TtsError::EmptyText`] for empty or whitespace-only text,
    /// [`TtsError::TextTooLong`] above [`MAX_TEXT_CHARS`] characters,
    /// [`TtsError::SpeedOutOfRange`] outside `0.5..=2.0` (NaN included),
    /// [`TtsError::BadChannels`] for channel counts other than 1 or 2, and
    /// [`TtsError::BadSampleRate`] outside `8000..=192000` Hz.
    pub fn validate(&self) -> Result<(), TtsError> {
        if self.text.trim().is_empty() {
            return Err(TtsError::EmptyText);
        }
        let len = self.text.chars().count();
        if len > MAX_TEXT_CHARS {
            return Err(TtsError::TextTooLong {
                len,
                max: MAX_TEXT_CHARS,
            });
        }
        if !(MIN_SPEED..=MAX_SPEED).contains(&self.speed) {
            return Err(TtsError::SpeedOutOfRange { value: self.speed });
        }
        if self.channels != 1 && self.channels != 2 {
            return Err(TtsError::BadChannels {
                channels: self.channels,
            });
        }
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&self.sr) {
            return Err(TtsError::BadSampleRate { rate: self.sr });
        }
        Ok(())
    }
}

/// Timing statistics for one synthesis run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SynthesisStats {
    /// Wall-clock time spent synthesizing.
    pub elapsed: Duration,
    /// Duration of the produced audio in seconds.
    pub audio_secs: f64,
    /// Real-time factor: `elapsed / audio_secs`. Values below 1.0 mean the
    /// engine synthesizes faster than the audio plays back.
    pub rtf: f64,
}

/// The result of a synthesis: the audio plus how long it took.
#[derive(Debug, Clone)]
pub struct Synthesis {
    /// The synthesized audio at the engine's native sample rate.
    pub audio: AudioBuffer,
    /// Timing statistics.
    pub stats: SynthesisStats,
}

/// A text-to-speech engine.
///
/// Implemented by [`crate::local::LocalVitsEngine`] (offline VITS via ONNX
/// Runtime) and [`crate::cloud::OpenAiCompatibleEngine`] (any
/// OpenAI-compatible speech service).
pub trait TtsEngine {
    /// Synthesizes `request` into speech.
    ///
    /// # Errors
    ///
    /// Returns [`TtsError`] when the request fails validation, the engine
    /// cannot produce the requested voice, or synthesis itself fails.
    fn synthesize(&self, request: &SynthRequest) -> Result<Synthesis, TtsError>;
}

/// Builds the timing statistics for a finished synthesis.
pub(crate) fn stats_for(elapsed: Duration, audio: &AudioBuffer) -> SynthesisStats {
    let audio_secs = audio.duration();
    let rtf = if audio_secs > 0.0 {
        elapsed.as_secs_f64() / audio_secs
    } else {
        0.0
    };
    SynthesisStats {
        elapsed,
        audio_secs,
        rtf,
    }
}

/// Converts `buffer` to exactly `channels` channels.
///
/// Mono is duplicated into stereo; stereo is averaged into mono. This is
/// how both engines honor [`SynthRequest::channels`] regardless of what the
/// underlying model or service produced.
///
/// # Errors
///
/// Returns [`TtsError::BadChannels`] when the buffer has more than two
/// channels.
pub(crate) fn retarget_channels(
    buffer: &AudioBuffer,
    channels: u16,
) -> Result<AudioBuffer, TtsError> {
    let source = buffer.num_channels();
    if source > 2 {
        return Err(TtsError::BadChannels {
            channels: source as u16,
        });
    }
    if source == usize::from(channels) {
        return Ok(buffer.clone());
    }
    match channels {
        2 => {
            let mono = &buffer.channels()[0];
            AudioBuffer::from_channels(vec![mono.clone(), mono.clone()], buffer.sample_rate())
                .map_err(|_| TtsError::BadChannels { channels })
        }
        1 => {
            let [left, right] = &buffer.channels()[..2] else {
                return Ok(buffer.clone());
            };
            let mono: Vec<f32> = left
                .iter()
                .zip(right.iter())
                .map(|(l, r)| (l + r) * 0.5)
                .collect();
            AudioBuffer::from_channels(vec![mono], buffer.sample_rate())
                .map_err(|_| TtsError::BadChannels { channels })
        }
        _ => Err(TtsError::BadChannels { channels }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::textnorm::Language;

    fn request() -> SynthRequest {
        SynthRequest {
            text: "Hello world.".to_string(),
            language: Language::En,
            speed: 1.0,
            voice: "en_US-libritts-high".to_string(),
            sr: 22_050,
            channels: 1,
        }
    }

    #[test]
    fn valid_request_passes() {
        assert!(request().validate().is_ok());
        let slow = SynthRequest {
            speed: 0.5,
            ..request()
        };
        let fast = SynthRequest {
            speed: 2.0,
            ..request()
        };
        assert!(slow.validate().is_ok());
        assert!(fast.validate().is_ok());
    }

    #[test]
    fn empty_text_is_rejected() {
        let empty = SynthRequest {
            text: String::new(),
            ..request()
        };
        assert!(matches!(empty.validate(), Err(TtsError::EmptyText)));
        let blank = SynthRequest {
            text: "   \n\t ".to_string(),
            ..request()
        };
        assert!(matches!(blank.validate(), Err(TtsError::EmptyText)));
    }

    #[test]
    fn long_text_is_rejected() {
        let long = SynthRequest {
            text: "a".repeat(20_001),
            ..request()
        };
        match long.validate() {
            Err(TtsError::TextTooLong { len, max }) => {
                assert_eq!(len, 20_001);
                assert_eq!(max, 20_000);
            }
            other => panic!("expected TextTooLong, got {other:?}"),
        }
        let exact = SynthRequest {
            text: "a".repeat(20_000),
            ..request()
        };
        assert!(exact.validate().is_ok());
    }

    #[test]
    fn speed_bounds_are_enforced() {
        for speed in [0.0, 0.49, 2.01, 5.0, f32::NAN] {
            let bad = SynthRequest { speed, ..request() };
            match bad.validate() {
                Err(TtsError::SpeedOutOfRange { value }) => {
                    if speed.is_nan() {
                        assert!(value.is_nan());
                    } else {
                        assert_eq!(value, speed);
                    }
                }
                other => panic!("expected SpeedOutOfRange for {speed}, got {other:?}"),
            }
        }
    }

    #[test]
    fn channel_and_rate_bounds_are_enforced() {
        for channels in [0, 3, 8] {
            let bad = SynthRequest {
                channels,
                ..request()
            };
            assert!(matches!(bad.validate(), Err(TtsError::BadChannels { .. })));
        }
        for rate in [0, 7_999, 192_001] {
            let bad = SynthRequest {
                sr: rate,
                ..request()
            };
            assert!(matches!(
                bad.validate(),
                Err(TtsError::BadSampleRate { .. })
            ));
        }
        let edge = SynthRequest {
            sr: 192_000,
            ..request()
        };
        assert!(edge.validate().is_ok());
    }

    #[test]
    fn retarget_channels_duplicates_and_downmixes() {
        let mono = AudioBuffer::from_channels(vec![vec![0.25; 100]], 22_050).expect("valid buffer");
        let stereo = retarget_channels(&mono, 2).expect("stereo");
        assert_eq!(stereo.num_channels(), 2);
        assert_eq!(stereo.channels()[0], stereo.channels()[1]);
        assert_eq!(stereo.channels()[0], mono.channels()[0]);

        let back = retarget_channels(&stereo, 1).expect("mono");
        assert_eq!(back.num_channels(), 1);
        assert_eq!(back.channels()[0], mono.channels()[0]);

        let passthrough = retarget_channels(&mono, 1).expect("mono passthrough");
        assert_eq!(passthrough, mono);
    }
}
