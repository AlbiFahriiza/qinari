//! Qinari speech markup: the `[laugh]` / `[breath]` / `[sigh]` /
//! `[pause:<ms>]` tags every engine understands.
//!
//! Parsing is engine-agnostic: [`parse`] turns text into [`Segment`]s (text
//! runs, expression-tag events, exact pauses), and each engine maps the
//! pieces onto what it can do. Supertonic renders the three expression tags
//! inline as its own `<laugh>` / `<breath>` / `<sigh>` tokens (the only tags
//! probe-verified during the design window; the model's fuller tag list is
//! deliberately not exposed). Engines without expression support strip the
//! tags. An unknown tag is a hard error naming the tag, and
//! `--no-markup` is just "never call [`parse`]": the [`plain`] constructor
//! wraps the raw text as a single run.

use crate::TtsError;

/// One of the verified inline expression tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpressionTag {
    /// Laughter: `[laugh]` → Supertonic `<laugh>`.
    Laugh,
    /// An audible breath: `[breath]` → Supertonic `<breath>`.
    Breath,
    /// A sigh: `[sigh]` → Supertonic `<sigh>`.
    Sigh,
}

impl ExpressionTag {
    /// The Supertonic inline token for this tag.
    pub fn supertonic_token(self) -> &'static str {
        match self {
            ExpressionTag::Laugh => "<laugh>",
            ExpressionTag::Breath => "<breath>",
            ExpressionTag::Sigh => "<sigh>",
        }
    }

    /// The Qinari bracket spelling of this tag.
    pub fn qinari_token(self) -> &'static str {
        match self {
            ExpressionTag::Laugh => "[laugh]",
            ExpressionTag::Breath => "[breath]",
            ExpressionTag::Sigh => "[sigh]",
        }
    }
}

/// One piece of marked-up text.
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    /// A run of plain text (tags and pauses excluded).
    Text(String),
    /// An expression tag event at this position.
    Tag(ExpressionTag),
    /// An exact pause, in milliseconds.
    Pause(u32),
}

/// Parses Qinari markup into segments.
///
/// Recognized tags: `[laugh]`, `[breath]`, `[sigh]`, `[pause:<ms>]` where
/// `<ms>` is 1..=60 000. Tags may carry surrounding spaces; text runs
/// between tags keep their non-space content verbatim (leading/trailing
/// whitespace is trimmed at the tag boundary).
///
/// # Errors
///
/// Returns [`TtsError::Markup`] for an unknown tag (the message names the
/// tag), a `[pause:0]`/out-of-range/negative/non-numeric pause, or an
/// unclosed `[`.
pub fn parse(text: &str) -> Result<Vec<Segment>, TtsError> {
    let mut segments = Vec::new();
    let mut run = String::new();
    let mut rest = text;
    loop {
        let Some(at) = rest.find('[') else {
            run.push_str(rest);
            flush_text(&mut segments, &mut run);
            return Ok(segments);
        };
        run.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest.find(']').ok_or_else(|| TtsError::Markup {
            detail: format!("unclosed '[' in {rest:?}"),
        })?;
        let inner = &rest[1..end];
        match parse_tag(inner)? {
            Segment::Tag(tag) => {
                flush_text(&mut segments, &mut run);
                segments.push(Segment::Tag(tag));
            }
            Segment::Pause(ms) => {
                flush_text(&mut segments, &mut run);
                segments.push(Segment::Pause(ms));
            }
            Segment::Text(_) => unreachable!("parse_tag only returns tags and pauses"),
        }
        rest = &rest[end + 1..];
    }
}

/// The `--no-markup` path: the whole text as one verbatim run.
#[must_use]
pub fn plain(text: &str) -> Vec<Segment> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        Vec::new()
    } else {
        vec![Segment::Text(trimmed.to_string())]
    }
}

/// Parses the inside of one `[...]` construct.
fn parse_tag(inner: &str) -> Result<Segment, TtsError> {
    let inner = inner.trim();
    match inner {
        "laugh" => Ok(Segment::Tag(ExpressionTag::Laugh)),
        "breath" => Ok(Segment::Tag(ExpressionTag::Breath)),
        "sigh" => Ok(Segment::Tag(ExpressionTag::Sigh)),
        _ => {
            if let Some(pause) = inner.strip_prefix("pause:") {
                let ms = parse_pause_ms(pause)?;
                return Ok(Segment::Pause(ms));
            }
            Err(TtsError::Markup {
                detail: format!(
                    "unknown markup tag [{inner}]; the supported tags are [laugh] [breath] [sigh] [pause:<ms>]"
                ),
            })
        }
    }
}

/// Parses a `[pause:<ms>]` value: 1..=60 000 milliseconds.
fn parse_pause_ms(raw: &str) -> Result<u32, TtsError> {
    let raw = raw.trim().trim_end_matches("ms").trim();
    let bad = |reason: &str| TtsError::Markup {
        detail: format!("invalid pause {raw:?} in [pause:{raw}] ({reason})"),
    };
    let value: i64 = raw
        .parse()
        .map_err(|_| bad("the duration must be a whole number of milliseconds"))?;
    if value <= 0 {
        return Err(bad("the pause must be at least 1 ms"));
    }
    if value > 60_000 {
        return Err(bad("the pause must be at most 60000 ms"));
    }
    u32::try_from(value).map_err(|_| bad("the pause must be at most 60000 ms"))
}

/// Commits the pending text run as a `Text` segment (if non-empty).
fn flush_text(segments: &mut Vec<Segment>, run: &mut String) {
    let trimmed = run.trim();
    if !trimmed.is_empty() {
        segments.push(Segment::Text(trimmed.to_string()));
    }
    run.clear();
}

/// One synthesized piece of a marked-up text, in reading order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// A text run (expression tags already inlined as engine tokens).
    Run(String),
    /// An exact `[pause:<ms>]`.
    Pause(u32),
}

/// Renders segments as Supertonic-ready pieces: expression tags become the
/// engine's inline tokens, and `[pause:<ms>]` becomes an exact pause piece
/// between (or around) the runs.
#[must_use]
pub fn for_supertonic(segments: &[Segment]) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    for segment in segments {
        match segment {
            Segment::Text(text) => {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(text);
            }
            Segment::Tag(tag) => {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(tag.supertonic_token());
            }
            Segment::Pause(ms) => {
                push_run(&mut pieces, &mut current);
                pieces.push(Piece::Pause(*ms));
            }
        }
    }
    push_run(&mut pieces, &mut current);
    pieces
}

/// Commits the pending run as a `Run` piece (if non-empty).
fn push_run(pieces: &mut Vec<Piece>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        pieces.push(Piece::Run(trimmed.to_string()));
    }
    current.clear();
}

/// Renders segments as plain text with expression tags stripped: the
/// mapping for engines that cannot speak them.
#[must_use]
pub fn strip_tags(segments: &[Segment]) -> String {
    let mut out = String::new();
    for segment in segments {
        match segment {
            Segment::Text(text) => {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(text);
            }
            // Both non-text kinds vanish: tags are unspeakable for these
            // engines and pauses are reinserted by the caller's own join.
            Segment::Tag(_) | Segment::Pause(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_three_expression_tags() {
        let segments = parse("Dia tertawa [laugh] lalu [breath] menghela napas [sigh].")
            .expect("valid markup");
        assert_eq!(
            segments,
            vec![
                Segment::Text("Dia tertawa".to_string()),
                Segment::Tag(ExpressionTag::Laugh),
                Segment::Text("lalu".to_string()),
                Segment::Tag(ExpressionTag::Breath),
                Segment::Text("menghela napas".to_string()),
                Segment::Tag(ExpressionTag::Sigh),
                Segment::Text(".".to_string()),
            ]
        );
    }

    #[test]
    fn parses_pauses_with_and_without_unit() {
        let segments = parse("Pada zaman dahulu... [pause:400ms] hiduplah seorang anak.")
            .expect("valid markup");
        assert_eq!(segments[1], Segment::Pause(400));
        let segments = parse("a [pause: 250 ] b").expect("spaced pause");
        assert_eq!(segments[1], Segment::Pause(250));
        assert_eq!(segments[2], Segment::Text("b".to_string()));
    }

    #[test]
    fn plain_text_has_no_segments_to_strip() {
        assert!(parse("Halo, nama saya Albi.").is_ok());
        let segments = parse("Halo, nama saya Albi.").expect("no tags");
        assert_eq!(
            segments,
            vec![Segment::Text("Halo, nama saya Albi.".to_string())]
        );
    }

    #[test]
    fn unknown_tag_is_a_hard_error_naming_the_tag() {
        let error = parse("dia [cough] keras").expect_err("unknown tag");
        match &error {
            TtsError::Markup { detail } => {
                assert!(detail.contains("[cough]"), "{detail}");
                assert!(detail.contains("unknown markup tag"), "{detail}");
            }
            other => panic!("expected Markup, got {other:?}"),
        }
        // The full 7-tag engine list is not exposed: only our three parse.
        for hidden in ["[sing]", "[whisper]", "[shout]", "[emo:happy]"] {
            assert!(parse(hidden).is_err(), "{hidden} must not parse");
        }
    }

    #[test]
    fn bad_pause_values_are_rejected() {
        for bad in [
            "[pause:0]",
            "[pause:-5]",
            "[pause:abc]",
            "[pause:]",
            "[pause:99000]",
        ] {
            assert!(parse(bad).is_err(), "{bad} must be rejected");
        }
        let error = parse("[pause:0]").expect_err("zero pause");
        assert!(error.to_string().contains("at least 1 ms"), "{error}");
    }

    #[test]
    fn unclosed_bracket_is_an_error() {
        let error = parse("boo [laugh").expect_err("unclosed");
        assert!(error.to_string().contains("unclosed"), "{error}");
    }

    #[test]
    fn the_no_markup_path_is_verbatim() {
        let segments = plain("Take [laugh] literally, [pause:400] and all.");
        assert_eq!(
            segments,
            vec![Segment::Text(
                "Take [laugh] literally, [pause:400] and all.".to_string()
            )]
        );
        assert_eq!(plain("   "), Vec::new());
    }

    #[test]
    fn supertonic_mapping_inlines_tokens_and_splits_runs() {
        let segments = parse("Pada zaman dahulu... [pause:400ms] hiduplah [laugh] seorang anak.")
            .expect("valid markup");
        let pieces = for_supertonic(&segments);
        assert_eq!(
            pieces,
            vec![
                Piece::Run("Pada zaman dahulu...".to_string()),
                Piece::Pause(400),
                Piece::Run("hiduplah <laugh> seorang anak.".to_string()),
            ]
        );
    }

    #[test]
    fn leading_and_trailing_pauses_produce_edge_pieces() {
        let segments = parse("[pause:100] hello [pause:200]").expect("valid");
        let pieces = for_supertonic(&segments);
        assert_eq!(
            pieces,
            vec![
                Piece::Pause(100),
                Piece::Run("hello".to_string()),
                Piece::Pause(200),
            ]
        );
        // Adjacent pauses merge into their runs' boundaries naturally: each
        // is its own piece, the engine sums consecutive pauses.
        let segments = parse("a [pause:50][pause:60] b").expect("valid");
        assert_eq!(
            for_supertonic(&segments),
            vec![
                Piece::Run("a".to_string()),
                Piece::Pause(50),
                Piece::Pause(60),
                Piece::Run("b".to_string()),
            ]
        );
    }

    #[test]
    fn strip_tags_removes_everything_but_text() {
        let segments = parse("a [laugh] b [pause:100] c").expect("valid");
        assert_eq!(strip_tags(&segments), "a b c");
    }
}
