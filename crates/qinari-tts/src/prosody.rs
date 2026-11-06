//! The engine-agnostic prosody post-chain: punctuation-aware pausing,
//! bounded seeded rate jitter, and loudness-consistent chunk joins.
//!
//! The Supertonic model already renders sentence-internal prosody (commas,
//! questions) as speech; what a synthesizer cannot do for itself is decide
//! how long to be silent between synthesis chunks, or let one render of the
//! same text breathe slightly differently from the next. This module owns
//! those decisions:
//!
//! - [`join_pause_ms`] classifies the punctuation a chunk ends with and
//!   maps it to a pause (comma 160 ms, colon/ellipsis 260 ms, sentence end
//!   400 ms in [`ProsodyMode::Natural`]; the reference SDK's fixed 300 ms
//!   in [`ProsodyMode::Flat`]).
//! - [`rate_jitter`] draws a bounded ±4 % multiplier from the render's
//!   seeded [`Qrng`] stream, so natural-mode narration varies per sentence
//!   without breaking seed reproducibility.
//! - [`match_loudness`] gain-matches each chunk to the first chunk's
//!   integrated loudness, keeping joins from pumping.

use crate::engine::ProsodyMode;
use qinari_dsp::buffer::AudioBuffer;
use qinari_dsp::lufs::integrated_lufs;
use qinari_dsp::rng::Qrng;

/// Pause after a chunk ending in a comma, in milliseconds.
pub const COMMA_PAUSE_MS: u32 = 160;

/// Pause after a chunk ending in a colon or ellipsis, in milliseconds.
pub const COLON_PAUSE_MS: u32 = 260;

/// Pause after a chunk ending with sentence-final punctuation, in
/// milliseconds.
pub const SENTENCE_PAUSE_MS: u32 = 400;

/// The reference SDK's fixed inter-chunk silence, in milliseconds.
pub const FLAT_PAUSE_MS: u32 = 300;

/// Maximum absolute rate jitter in natural mode (4 %).
pub const JITTER_FRACTION: f32 = 0.04;

/// How much one chunk may be gain-corrected for a loudness-consistent
/// join, in dB. Chunks from the same voice and style rarely differ by
/// more than a couple of dB; a bigger gap means something else changed.
const MATCH_LIMIT_DB: f32 = 6.0;

/// The punctuation class governing one join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Punctuation {
    /// The chunk ends with `,` or `;` — a clause boundary.
    Comma,
    /// The chunk ends with `:` or `…` — a suspended boundary.
    Colon,
    /// The chunk ends with `.`, `!`, or `?` — a sentence boundary.
    SentenceEnd,
    /// The chunk ends without boundary punctuation (a length-forced
    /// split); the flat default applies.
    None,
}

impl Punctuation {
    /// Classifies the punctuation a chunk of text ends with.
    #[must_use]
    pub fn at_end_of(text: &str) -> Self {
        let Some(last) = text.trim_end().chars().next_back() else {
            return Punctuation::None;
        };
        match last {
            '.' | '!' | '?' => Punctuation::SentenceEnd,
            ',' | ';' => Punctuation::Comma,
            ':' | '…' => Punctuation::Colon,
            _ => Punctuation::None,
        }
    }
}

/// The pause, in milliseconds, to insert after a chunk that ends with
/// `punctuation`, under `mode`.
#[must_use]
pub fn join_pause_ms(punctuation: Punctuation, mode: ProsodyMode) -> u32 {
    match mode {
        ProsodyMode::Flat => FLAT_PAUSE_MS,
        ProsodyMode::Natural => match punctuation {
            Punctuation::Comma => COMMA_PAUSE_MS,
            Punctuation::Colon => COLON_PAUSE_MS,
            Punctuation::SentenceEnd => SENTENCE_PAUSE_MS,
            Punctuation::None => FLAT_PAUSE_MS,
        },
    }
}

/// Draws one bounded rate multiplier in `1 ± 4 %` from `rng`.
///
/// Flat mode callers simply use `1.0`; natural mode draws one multiplier
/// per sentence from the render's seeded stream, so the same seed always
/// produces the same narration rhythm.
#[must_use]
pub fn rate_jitter(rng: &mut Qrng) -> f32 {
    let unit = rng.next_f32();
    // Map the uniform draw onto [-1, 1] before scaling: the bounds are
    // exact and the mean stays 1.0.
    1.0 + (unit * 2.0 - 1.0) * JITTER_FRACTION
}

/// Gain-matches `buffer` to `target_lufs` (the first chunk's integrated
/// loudness). The correction is capped at [`MATCH_LIMIT_DB`]: a chunk that
/// sits further from the target still receives the full allowed
/// correction, never more (and never zero — a bounded corrector applies a
/// bounded step, it does not skip).
///
/// Chunks that cannot be measured (too short or silent) pass through
/// unchanged; so does an on-target chunk.
pub fn match_loudness(buffer: &mut AudioBuffer, target_lufs: Option<f64>) {
    let Some(target) = target_lufs else {
        return;
    };
    let Some(actual) = integrated_lufs(buffer) else {
        return;
    };
    // Compare in f64: an f32 cast can round 5.999999 up past the limit.
    let limit = f64::from(MATCH_LIMIT_DB);
    let delta = (target - actual).clamp(-limit, limit);
    if delta.abs() > 0.1 {
        buffer.apply_gain_db(delta as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_classification() {
        assert_eq!(
            Punctuation::at_end_of("Hello, world."),
            Punctuation::SentenceEnd
        );
        assert_eq!(Punctuation::at_end_of("Really?!"), Punctuation::SentenceEnd);
        assert_eq!(Punctuation::at_end_of("yes."), Punctuation::SentenceEnd);
        assert_eq!(Punctuation::at_end_of("first,"), Punctuation::Comma);
        assert_eq!(Punctuation::at_end_of("next;"), Punctuation::Comma);
        assert_eq!(Punctuation::at_end_of("as follows:"), Punctuation::Colon);
        assert_eq!(Punctuation::at_end_of("and then…"), Punctuation::Colon);
        assert_eq!(Punctuation::at_end_of("no punct"), Punctuation::None);
        assert_eq!(Punctuation::at_end_of("   "), Punctuation::None);
        // Trailing whitespace after the punctuation does not hide it.
        assert_eq!(Punctuation::at_end_of("end. "), Punctuation::SentenceEnd);
    }

    #[test]
    fn pause_math_per_mode() {
        use crate::engine::ProsodyMode::{Flat, Natural};
        // Natural mode: the design constants.
        assert_eq!(join_pause_ms(Punctuation::Comma, Natural), 160);
        assert_eq!(join_pause_ms(Punctuation::Colon, Natural), 260);
        assert_eq!(join_pause_ms(Punctuation::SentenceEnd, Natural), 400);
        assert_eq!(join_pause_ms(Punctuation::None, Natural), 300);
        // Flat mode: the reference SDK's fixed silence everywhere.
        for class in [
            Punctuation::Comma,
            Punctuation::Colon,
            Punctuation::SentenceEnd,
            Punctuation::None,
        ] {
            assert_eq!(join_pause_ms(class, Flat), 300);
        }
    }

    #[test]
    fn rate_jitter_is_bounded_and_seeded() {
        let mut rng = Qrng::new(99);
        for _ in 0..1000 {
            let jitter = rate_jitter(&mut rng);
            assert!(
                (0.96..=1.04).contains(&jitter),
                "jitter {jitter} outside ±4%"
            );
        }
        // Same seed, same rhythm.
        let mut left = Qrng::new(7);
        let mut right = Qrng::new(7);
        let a: Vec<f32> = (0..32).map(|_| rate_jitter(&mut left)).collect();
        let b: Vec<f32> = (0..32).map(|_| rate_jitter(&mut right)).collect();
        assert_eq!(a, b);
    }

    #[test]
    fn loudness_matching_pulls_chunks_to_the_target() {
        let frames = 44_100_usize;
        let loud: Vec<f32> = (0..frames)
            .map(|n| 0.5 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / 44_100.0).sin())
            .collect();
        let loud_buffer = AudioBuffer::from_channels(vec![loud], 44_100).expect("buffer");
        let target = integrated_lufs(&loud_buffer).expect("the loud chunk is measurable");

        let mut quiet = loud_buffer.clone();
        quiet.apply_gain_db(-6.0);
        let before = integrated_lufs(&quiet).expect("the quiet chunk is measurable");
        assert!(
            target - before > 4.0,
            "fixture must start clearly below the target ({target} vs {before})"
        );

        match_loudness(&mut quiet, Some(target));
        let after = integrated_lufs(&quiet).expect("measurable");
        assert!(
            (after - target).abs() < 0.5,
            "expected the chunk pulled to {target}, got {after:.2}"
        );

        // Unmeasurable targets pass through untouched.
        let mut untouched = loud_buffer.clone();
        match_loudness(&mut untouched, None);
        assert_eq!(untouched, loud_buffer);
    }
}
