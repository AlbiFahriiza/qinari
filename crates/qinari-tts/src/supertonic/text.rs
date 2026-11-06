//! The Supertonic text processor: a faithful port of the reference SDK's
//! `UnicodeProcessor` (supertonic-py `core.py`).
//!
//! [`preprocess`] runs the SDK's ten passes in order (NFKD, emoji removal,
//! symbol normalization, decorative-symbol removal, abbreviation expansion,
//! punctuation-spacing fixes, duplicate-quote collapsing, whitespace
//! cleaning, period addition, language wrapping). [`Tokenizer`] then maps
//! each surviving character through the model's `unicode_indexer.json`
//! codepoint-to-unit table: `text_ids = indexer[ord(c)]`.

use crate::TtsError;
use std::path::Path;

use super::nfkd::nfkd;

/// Whether `c` is one of the emoji/symbol code-point ranges the SDK strips
/// (the `_EMOJI_PATTERN` ranges in `core.py`).
fn is_emoji(c: char) -> bool {
    const RANGES: [(u32, u32); 12] = [
        (0x1F600, 0x1F64F),
        (0x1F300, 0x1F5FF),
        (0x1F680, 0x1F6FF),
        (0x1F700, 0x1F77F),
        (0x1F780, 0x1F7FF),
        (0x1F800, 0x1F8FF),
        (0x1F900, 0x1F9FF),
        (0x1FA00, 0x1FA6F),
        (0x1FA70, 0x1FAFF),
        (0x2600, 0x26FF),
        (0x2700, 0x27BF),
        (0x1F1E6, 0x1F1FF),
    ];
    let cp = u32::from(c);
    RANGES.iter().any(|(lo, hi)| (*lo..=*hi).contains(&cp))
}

/// The SDK's `_SYMBOL_REPLACEMENTS`: punctuation and symbols normalized to
/// plain ASCII forms. Order matters where earlier outputs feed later
/// patterns (e.g. backtick → apostrophe before quote collapsing).
fn normalize_symbols(text: &str) -> String {
    let replacements: [(char, &str); 18] = [
        ('–', "-"), // en dash
        ('‑', "-"), // non-breaking hyphen
        ('—', "-"), // em dash
        ('¯', " "), // macron
        ('_', " "),
        ('“', "\""), // left double quote
        ('”', "\""), // right double quote
        ('‘', "'"),  // left single quote
        ('’', "'"),  // right single quote
        ('´', "'"),  // acute accent
        ('`', "'"),  // grave accent
        ('[', " "),
        (']', " "),
        ('|', " "),
        ('/', " "),
        ('#', " "),
        ('→', " "),
        ('←', " "),
    ];
    let mut out = String::with_capacity(text.len());
    'outer: for c in text.chars() {
        for (from, to) in replacements {
            if c == from {
                out.push_str(to);
                continue 'outer;
            }
        }
        out.push(c);
    }
    out
}

/// Removes the SDK's decorative special symbols (hearts, stars, copyright,
/// backslash).
fn remove_special_chars(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '♥' | '☆' | '♡' | '©' | '\\'))
        .collect()
}

/// The SDK's `_expand_abbreviations` replacements.
fn expand_abbreviations(text: &str) -> String {
    text.replace('@', " at ")
        .replace("e.g.,", "for example, ")
        .replace("i.e.,", "that is, ")
}

/// The SDK's `_fix_punctuation_spacing`: a literal space directly before
/// these marks is removed (the SDK's patterns use `' '`, not `\s`, and
/// each non-overlapping match removes exactly one space).
fn fix_punctuation_spacing(text: &str) -> String {
    const BEFORE: [char; 7] = [',', '.', '!', '?', ';', ':', '\''];
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if c == ' ' {
            // Earlier spaces flush; only the newest one can be swallowed.
            if pending_space {
                out.push(' ');
            }
            pending_space = true;
            continue;
        }
        if BEFORE.contains(&c) && pending_space {
            pending_space = false;
            out.push(c);
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(c);
    }
    if pending_space {
        out.push(' ');
    }
    out
}

/// Collapses runs of the same quote character (`""` → `"`, `'''` → `'`).
fn remove_duplicate_quotes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous_quote: Option<char> = None;
    for c in text.chars() {
        if c == '"' || c == '\'' {
            if previous_quote == Some(c) {
                continue;
            }
            previous_quote = Some(c);
        } else {
            previous_quote = None;
        }
        out.push(c);
    }
    out
}

/// Collapses all whitespace runs to single spaces and trims the ends.
fn clean_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(c);
        }
    }
    out
}

/// Whether `c` is in the SDK's ending-punctuation set: text may end with
/// these without gaining a period.
fn is_ending_punctuation(c: char) -> bool {
    matches!(
        c,
        '.' | '!'
            | '?'
            | ';'
            | ':'
            | ','
            | '\''
            | '"'
            | ')'
            | ']'
            | '}'
            | '…'
            | '。'
            | '」'
            | '』'
            | '】'
            | '〉'
            | '》'
            | '›'
            | '»'
    )
}

/// Adds a final period when the text does not already end with (quote or
/// closing bracket or) punctuation.
fn add_period_if_needed(text: &str) -> String {
    let mut text = text.to_string();
    if !text.chars().next_back().is_some_and(is_ending_punctuation) {
        text.push('.');
    }
    text
}

/// The SDK's `_preprocess_text`, verbatim in pass order, ending with the
/// `<lang>…</lang>` wrap. `lang` must be a code the model knows (`en`,
/// `id`, …); the caller owns that check.
#[must_use]
pub fn preprocess(text: &str, lang: &str) -> String {
    let text = nfkd(text);
    let text: String = text.chars().filter(|c| !is_emoji(*c)).collect();
    let text = normalize_symbols(&text);
    let text = remove_special_chars(&text);
    let text = expand_abbreviations(&text);
    let text = fix_punctuation_spacing(&text);
    let text = remove_duplicate_quotes(&text);
    let text = clean_whitespace(&text);
    let text = add_period_if_needed(&text);
    format!("<{lang}>{text}</{lang}>")
}

/// The codepoint-to-unit tokenizer over `unicode_indexer.json`.
pub struct Tokenizer {
    indexer: Vec<i32>,
}

impl Tokenizer {
    /// Loads the indexer table from `path` (a JSON array of ints, `-1` for
    /// unsupported code points).
    ///
    /// # Errors
    ///
    /// Returns [`TtsError::ModelDir`] when the file is missing or is not a
    /// non-empty JSON array of integers.
    pub fn load(path: &Path) -> Result<Self, TtsError> {
        let text = std::fs::read_to_string(path).map_err(|error| TtsError::ModelDir {
            detail: format!("could not read {}: {error}", path.display()),
        })?;
        let indexer: Vec<i32> =
            serde_json::from_str(&text).map_err(|error| TtsError::ModelDir {
                detail: format!(
                    "{:?} is not a unicode indexer table: {error}",
                    path.display()
                ),
            })?;
        if indexer.is_empty() {
            return Err(TtsError::ModelDir {
                detail: format!("{} is empty", path.display()),
            });
        }
        Ok(Self { indexer })
    }

    /// Builds a tokenizer from an existing table (tests and custom
    /// bundles).
    #[must_use]
    pub fn from_table(indexer: Vec<i32>) -> Self {
        Self { indexer }
    }

    /// The unit id for one code point: the table entry, or `None` when the
    /// code point is out of the table or marked unsupported (`-1`).
    #[must_use]
    pub fn unit_for(&self, c: char) -> Option<i64> {
        let cp = u32::from(c) as usize;
        if cp >= self.indexer.len() {
            return None;
        }
        let unit = self.indexer[cp];
        (unit >= 0).then_some(i64::from(unit))
    }

    /// Preprocesses `text` for `lang` and maps it to unit ids.
    ///
    /// Characters the model does not support are skipped (the SDK instead
    /// rejects the whole text up front; preprocessing already strips the
    /// common offenders, and skipping keeps the remaining rare ones from
    /// poisoning the sequence). The mask length equals the id count: every
    /// id emitted is a real unit.
    ///
    /// # Errors
    ///
    /// Returns [`TtsError::ModelDir`] when the preprocessed text contains
    /// no supported characters at all (nothing to speak).
    pub fn encode(&self, text: &str, lang: &str) -> Result<Vec<i64>, TtsError> {
        let preprocessed = preprocess(text, lang);
        let mut ids = Vec::with_capacity(preprocessed.chars().count());
        for c in preprocessed.chars() {
            if let Some(id) = self.unit_for(c) {
                ids.push(id);
            }
        }
        if ids.is_empty() {
            return Err(TtsError::ModelDir {
                detail: format!("the text contains no characters the voice can speak: {text:?}"),
            });
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny synthetic indexer: letters and the tokens the wrapper needs,
    /// `-1` for everything else.
    fn tokenizer() -> Tokenizer {
        let mut table = vec![-1_i32; 65_536];
        for (unit, c) in "abcdefghijklmnopqrstuvwxyz<>/".chars().enumerate() {
            table[c as usize] = unit as i32;
        }
        for (unit, c) in " .,!?".chars().enumerate() {
            table[c as usize] = 30 + unit as i32;
        }
        // Combining acute is supported (post-NFKD accents survive).
        table[0x0301] = 40;
        Tokenizer::from_table(table)
    }

    #[test]
    fn preprocess_matches_the_sdk_passes() {
        // NFKD + symbol normalization + spacing + period + wrap.
        assert_eq!(
            preprocess("Halo, nama saya Albi", "id"),
            "<id>Halo, nama saya Albi.</id>"
        );
        // Dashes normalize, brackets become spaces, then whitespace cleans.
        assert_eq!(preprocess("a—b [c] d", "en"), "<en>a-b c d.</en>");
        // e.g. expands (the abbreviation pattern needs its comma attached),
        // spacing before punctuation is fixed.
        assert_eq!(
            preprocess("e.g., cats", "en"),
            "<en>for example, cats.</en>"
        );
        assert_eq!(preprocess("a , b . c", "en"), "<en>a, b. c.</en>");
        // @ becomes " at ".
        assert_eq!(preprocess("a@b", "en"), "<en>a at b.</en>");
        // Curly quotes straighten; duplicate quotes collapse; a closing
        // quote counts as ending punctuation (no extra period).
        assert_eq!(
            preprocess("he said \"\"hi\"\"", "en"),
            "<en>he said \"hi\"</en>"
        );
        // Emoji are removed.
        assert_eq!(preprocess("hi 🎉 there", "en"), "<en>hi there.</en>");
        // Ending punctuation suppresses the period.
        assert_eq!(preprocess("done?", "en"), "<en>done?</en>");
        // Accented input decomposes (é → e + U+0301).
        assert_eq!(preprocess("café", "en"), "<en>cafe\u{0301}.</en>");
    }

    #[test]
    fn encode_skips_unsupported_and_wraps_with_language() {
        let tok = tokenizer();
        let ids = tok.encode("hi", "en").expect("encodable");
        // < e n > h i < / e n > with the period added.
        assert_eq!(ids.len(), "<en>hi.</en>".len());
        // Every id is a real unit (the table's -1 entries never appear).
        assert!(ids.iter().all(|id| *id >= 0));
        // The Cyrillic letter is unsupported in the synthetic table: its
        // decomposition is dropped, and the space where it stood survives
        // between `hi` and the period.
        let mixed = tok.encode("hi й", "en").expect("encodable");
        assert_eq!(mixed.len(), ids.len() + 1);
        // A table where even the wrapper is unsupported has nothing to
        // say: that is the hard error.
        let mute = Tokenizer::from_table(vec![-1]);
        assert!(mute.encode("anything", "en").is_err());
    }

    #[test]
    fn unit_for_respects_the_table() {
        let tok = tokenizer();
        assert_eq!(tok.unit_for('a'), Some(0));
        assert_eq!(tok.unit_for('.'), Some(31));
        assert_eq!(tok.unit_for('й'), None);
        assert_eq!(tok.unit_for('\u{1F600}'), None); // out of table
        let small = Tokenizer::from_table(vec![5, -1]);
        assert_eq!(small.unit_for('\0'), Some(5));
        assert_eq!(small.unit_for('\u{1}'), None);
    }
}
