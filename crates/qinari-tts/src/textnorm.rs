//! Text normalization for speech: number expansion per locale and
//! length-bounded chunking.
//!
//! Digits are hard for TTS engines to read naturally, so both engines run
//! their input through [`expand_numbers`] first. Long input is split by
//! [`chunk_text`] so no single synthesis call is unbounded.

use std::fmt;

/// The languages Qinari speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    /// English (`en`). Thousands separator `,`, decimal point `.`.
    En,
    /// Indonesian (`id`). Thousands separator `.`, decimal comma `,`.
    Id,
}

impl Language {
    /// Returns the BCP-47 language code: `en` or `id`.
    pub fn as_code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Id => "id",
        }
    }

    /// Parses a language code (`en` or `id`), case-insensitively.
    pub fn from_code(code: &str) -> Option<Self> {
        match code.to_ascii_lowercase().as_str() {
            "en" => Some(Language::En),
            "id" => Some(Language::Id),
            _ => None,
        }
    }

    /// The character that separates thousands groups in this locale.
    fn thousands_sep(self) -> char {
        match self {
            Language::En => ',',
            Language::Id => '.',
        }
    }

    /// The character that introduces the fractional part in this locale.
    fn decimal_sep(self) -> char {
        match self {
            Language::En => '.',
            Language::Id => ',',
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code())
    }
}

const EN_ONES: [&str; 10] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];
const EN_TEENS: [&str; 10] = [
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const EN_TENS: [&str; 8] = [
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];
const EN_SCALES: [&str; 4] = ["", "thousand", "million", "billion"];

const ID_ONES: [&str; 10] = [
    "nol", "satu", "dua", "tiga", "empat", "lima", "enam", "tujuh", "delapan", "sembilan",
];
const ID_SCALES: [&str; 4] = ["", "ribu", "juta", "miliar"];

/// Largest supported number: four digit groups, i.e. up to
/// 999,999,999,999. Larger (or malformed) tokens pass through untouched.
const MAX_GROUPS: usize = 4;

/// Expands the numbers in `text` into words for `language`.
///
/// Recognized tokens follow the locale's conventions:
///
/// - English: `1,234.56` (comma thousands, point decimals, spoken `point`);
/// - Indonesian: `1.234,56` (point thousands, comma decimals, spoken `koma`).
///
/// Integers up to 999,999,999,999 are spoken as full numbers (English
/// tens-and-ones are hyphenated: `thirty-four`; Indonesian uses `tiga puluh
/// empat`). Fractional parts are spoken digit by digit after `point`/`koma`.
/// Anything else — larger numbers, numbers glued to letters (`MP3`, `3D`),
/// tokens with leading zeros, malformed separators — passes through
/// unchanged so the phonemizer can fall back to its own reading.
#[must_use]
pub fn expand_numbers(text: &str, language: Language) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() {
            if chars[i].is_ascii_digit() {
                i += 1;
            } else if (chars[i] == '.' || chars[i] == ',')
                && chars.get(i + 1).is_some_and(char::is_ascii_digit)
            {
                // A separator counts as part of the number only when a digit
                // follows it; otherwise it is sentence punctuation.
                i += 2;
            } else {
                break;
            }
        }
        let token: String = chars[start..i].iter().collect();
        let borders_word = |c: char| c.is_alphanumeric() || c == '_';
        let isolated = chars
            .get(start.wrapping_sub(1))
            .is_none_or(|c| !borders_word(*c))
            && chars.get(i).is_none_or(|c| !borders_word(*c));
        if isolated {
            match expand_token(&token, language) {
                Some(words) => out.push_str(&words),
                None => out.push_str(&token),
            }
        } else {
            out.push_str(&token);
        }
    }
    out
}

/// Expands one number token, or returns `None` when it must pass through.
fn expand_token(token: &str, language: Language) -> Option<String> {
    let (int_digits, frac_digits) = split_token(token, language)?;
    if int_digits.len() > 1 && int_digits.starts_with('0') {
        // Leading zeros belong to identifiers, years, and code numbers.
        return None;
    }
    let groups = split_groups(&int_digits)?;
    if groups.len() > MAX_GROUPS {
        return None;
    }
    let mut words = String::new();
    if groups.iter().all(|group| *group == 0) {
        words.push_str(zero_word(language));
    } else {
        let scale = groups.len() as i32 - 1;
        for (index, group) in groups.iter().enumerate() {
            if *group == 0 {
                continue;
            }
            let group_scale = scale - index as i32;
            if !words.is_empty() {
                words.push(' ');
            }
            // Indonesian contracts "satu ribu" and "seratus".
            if language == Language::Id && *group == 1 && group_scale >= 1 {
                if group_scale == 1 {
                    words.push_str("seribu");
                } else {
                    words.push_str(&under_thousand_id(1));
                    words.push(' ');
                    words.push_str(ID_SCALES[group_scale as usize]);
                }
            } else {
                words.push_str(&under_thousand(*group, language));
                if group_scale >= 1 {
                    words.push(' ');
                    words.push_str(match language {
                        Language::En => EN_SCALES[group_scale as usize],
                        Language::Id => ID_SCALES[group_scale as usize],
                    });
                }
            }
        }
    }
    if let Some(frac) = frac_digits {
        words.push(' ');
        words.push_str(match language {
            Language::En => "point",
            Language::Id => "koma",
        });
        for digit in frac.chars().map(|c| c.to_digit(10).unwrap_or(0)) {
            words.push(' ');
            words.push_str(digit_word(digit, language));
        }
    }
    Some(words)
}

/// Splits a token into integer and fractional digit strings, validating the
/// locale's separator layout. Returns `None` for malformed tokens.
fn split_token(token: &str, language: Language) -> Option<(String, Option<String>)> {
    let (int_part, frac_part) = match token.split_once(language.decimal_sep()) {
        Some((int, frac)) => (int, Some(frac)),
        None => (token, None),
    };
    let digits_only = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let thousands = language.thousands_sep();
    let groups: Vec<&str> = int_part.split(thousands).collect();
    match groups.len() {
        // A single group is a plain integer: any number of digits.
        1 => {
            if !digits_only(groups[0]) {
                return None;
            }
        }
        // With thousands separators, the first group is 1-3 digits and every
        // later group exactly three.
        _ => {
            if groups[0].is_empty() || groups[0].len() > 3 || !digits_only(groups[0]) {
                return None;
            }
            for group in &groups[1..] {
                if group.len() != 3 || !digits_only(group) {
                    return None;
                }
            }
        }
    }
    let frac_digits = match frac_part {
        Some(frac) => {
            if !digits_only(frac) {
                return None;
            }
            Some(frac.to_string())
        }
        None => None,
    };
    let int_digits: String = int_part.chars().filter(|c| *c != thousands).collect();
    Some((int_digits, frac_digits))
}

/// Splits validated integer digits into base-1000 groups, most significant
/// first. `split_token` has already rejected malformed separators, so any
/// remaining separator in the digits is impossible.
fn split_groups(int_digits: &str) -> Option<Vec<u32>> {
    if !int_digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let value: u128 = int_digits.parse().ok()?;
    let mut groups = Vec::new();
    let mut rest = value;
    while rest > 0 {
        groups.push((rest % 1000) as u32);
        rest /= 1000;
    }
    if groups.is_empty() {
        groups.push(0);
    }
    groups.reverse();
    Some(groups)
}

/// Words for a value below 1000 (hundreds included).
fn under_thousand(value: u32, language: Language) -> String {
    match language {
        Language::En => under_thousand_en(value),
        Language::Id => under_thousand_id(value),
    }
}

fn under_thousand_en(value: u32) -> String {
    match value {
        0..=9 => EN_ONES[value as usize].to_string(),
        10..=19 => EN_TEENS[(value - 10) as usize].to_string(),
        20..=99 => {
            let tens = EN_TENS[(value / 10 - 2) as usize];
            let ones = value % 10;
            if ones == 0 {
                tens.to_string()
            } else {
                format!("{tens}-{}", EN_ONES[ones as usize])
            }
        }
        100..=999 => {
            let hundreds = format!("{} hundred", EN_ONES[(value / 100) as usize]);
            let rest = value % 100;
            if rest == 0 {
                hundreds
            } else {
                format!("{hundreds} {}", under_thousand_en(rest))
            }
        }
        _ => String::new(),
    }
}

fn under_thousand_id(value: u32) -> String {
    match value {
        0..=9 => ID_ONES[value as usize].to_string(),
        10 => "sepuluh".to_string(),
        11 => "sebelas".to_string(),
        12..=19 => format!("{} belas", ID_ONES[(value - 10) as usize]),
        20..=99 => {
            let tens = format!("{} puluh", ID_ONES[(value / 10) as usize]);
            let ones = value % 10;
            if ones == 0 {
                tens
            } else {
                format!("{tens} {}", ID_ONES[ones as usize])
            }
        }
        100..=999 => {
            let hundreds = if value / 100 == 1 {
                "seratus".to_string()
            } else {
                format!("{} ratus", ID_ONES[(value / 100) as usize])
            };
            let rest = value % 100;
            if rest == 0 {
                hundreds
            } else {
                format!("{hundreds} {}", under_thousand_id(rest))
            }
        }
        _ => String::new(),
    }
}

fn zero_word(language: Language) -> &'static str {
    match language {
        Language::En => "zero",
        Language::Id => "nol",
    }
}

fn digit_word(digit: u32, language: Language) -> &'static str {
    match language {
        Language::En => EN_ONES[digit as usize],
        Language::Id => ID_ONES[digit as usize],
    }
}

/// Sentence terminators used by [`chunk_text`].
fn is_sentence_terminator(c: char) -> bool {
    c == '.' || c == '!' || c == '?' || c == '\n'
}

/// Splits `text` into chunks of at most `max_chars` characters (Unicode
/// characters, not bytes).
///
/// Splitting prefers sentence boundaries (`.`, `!`, `?`, line breaks), then
/// spaces; a decimal point between digits is never treated as a boundary.
/// A single word longer than `max_chars` becomes its own (over-limit)
/// chunk. Whitespace runs between chunks are collapsed to one space.
#[must_use]
pub fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.chars().count() <= max_chars {
        return vec![trimmed.to_string()];
    }

    let mut pieces: Vec<String> = Vec::new();
    for sentence in split_sentences(trimmed) {
        if sentence.chars().count() <= max_chars {
            pieces.push(sentence);
        } else {
            pieces.extend(split_long_sentence(&sentence, max_chars));
        }
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for piece in pieces {
        let current_len = current.chars().count();
        let piece_len = piece.chars().count();
        if !current.is_empty() && current_len + 1 + piece_len > max_chars {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&piece);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Splits after sentence terminators, keeping them attached. A `.` between
/// two digits (a decimal point) does not terminate a sentence.
fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut sentences = Vec::new();
    let mut current = String::new();
    for (index, c) in chars.iter().copied().enumerate() {
        current.push(c);
        let decimal_point = c == '.'
            && index > 0
            && chars[index - 1].is_ascii_digit()
            && chars.get(index + 1).is_some_and(char::is_ascii_digit);
        if is_sentence_terminator(c) && !decimal_point {
            sentences.push(std::mem::take(&mut current).trim().to_string());
        }
    }
    let tail = current.trim();
    if !tail.is_empty() {
        sentences.push(tail.to_string());
    }
    sentences.retain(|sentence| !sentence.is_empty());
    sentences
}

/// Splits one over-long sentence at spaces, greedily packing words up to
/// `max_chars` per piece.
fn split_long_sentence(sentence: &str, max_chars: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    for word in sentence.split_whitespace() {
        let word_len = word.chars().count();
        let current_len = current.chars().count();
        if !current.is_empty() && current_len + 1 + word_len > max_chars {
            pieces.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_numbers() {
        assert_eq!(expand_numbers("34", Language::En), "thirty-four");
        assert_eq!(expand_numbers("12", Language::En), "twelve");
        assert_eq!(expand_numbers("7", Language::En), "seven");
        assert_eq!(expand_numbers("0", Language::En), "zero");
        assert_eq!(expand_numbers("100", Language::En), "one hundred");
        assert_eq!(expand_numbers("105", Language::En), "one hundred five");
        assert_eq!(expand_numbers("340", Language::En), "three hundred forty");
        assert_eq!(
            expand_numbers("1,234", Language::En),
            "one thousand two hundred thirty-four"
        );
        assert_eq!(
            expand_numbers("1,234,567", Language::En),
            "one million two hundred thirty-four thousand five hundred sixty-seven"
        );
        assert_eq!(expand_numbers("2,000,000,000", Language::En), "two billion");
        assert_eq!(
            expand_numbers("12345", Language::En),
            "twelve thousand three hundred forty-five"
        );
    }

    #[test]
    fn english_decimals() {
        assert_eq!(
            expand_numbers("1,234.56", Language::En),
            "one thousand two hundred thirty-four point five six"
        );
        assert_eq!(expand_numbers("0.5", Language::En), "zero point five");
        assert_eq!(
            expand_numbers("1.234", Language::En),
            "one point two three four"
        );
    }

    #[test]
    fn indonesian_numbers() {
        assert_eq!(expand_numbers("34", Language::Id), "tiga puluh empat");
        assert_eq!(expand_numbers("11", Language::Id), "sebelas");
        assert_eq!(expand_numbers("19", Language::Id), "sembilan belas");
        assert_eq!(expand_numbers("100", Language::Id), "seratus");
        assert_eq!(expand_numbers("200", Language::Id), "dua ratus");
        assert_eq!(expand_numbers("1000", Language::Id), "seribu");
        assert_eq!(expand_numbers("2000", Language::Id), "dua ribu");
        assert_eq!(
            expand_numbers("1.234", Language::Id),
            "seribu dua ratus tiga puluh empat"
        );
        assert_eq!(
            expand_numbers("12.345", Language::Id),
            "dua belas ribu tiga ratus empat puluh lima"
        );
        assert_eq!(expand_numbers("1.000.000.000", Language::Id), "satu miliar");
    }

    #[test]
    fn indonesian_decimals() {
        assert_eq!(
            expand_numbers("1.234,56", Language::Id),
            "seribu dua ratus tiga puluh empat koma lima enam"
        );
        assert_eq!(expand_numbers("0,5", Language::Id), "nol koma lima");
        assert_eq!(
            expand_numbers("12,75", Language::Id),
            "dua belas koma tujuh lima"
        );
    }

    #[test]
    fn numbers_in_context_keep_surrounding_text() {
        assert_eq!(
            expand_numbers("Ada 12 kata di sini.", Language::Id),
            "Ada dua belas kata di sini."
        );
        assert_eq!(
            expand_numbers("Chapter 7 begins.", Language::En),
            "Chapter seven begins."
        );
        assert_eq!(
            expand_numbers("Harga 1.250.000 rupiah", Language::Id),
            "Harga satu juta dua ratus lima puluh ribu rupiah"
        );
    }

    #[test]
    fn huge_and_malformed_numbers_pass_through() {
        for language in [Language::En, Language::Id] {
            assert_eq!(
                expand_numbers("12345678901234567890", language),
                "12345678901234567890"
            );
            // One group past the supported range (trillions).
            assert_eq!(
                expand_numbers("1,000,000,000,000", language),
                "1,000,000,000,000"
            );
            // Glued to letters: not a number token.
            assert_eq!(expand_numbers("MP3 and 3D", language), "MP3 and 3D");
            assert_eq!(expand_numbers("room42", language), "room42");
            // Leading zeros are identifiers, not quantities.
            assert_eq!(expand_numbers("007", language), "007");
            // Wrong-layout separators for the locale pass through.
            assert_eq!(expand_numbers("1,23", Language::En), "1,23");
            assert_eq!(expand_numbers("1.23", Language::Id), "1.23");
        }
        // A trailing period stays sentence punctuation.
        assert_eq!(
            expand_numbers("It costs 5.", Language::En),
            "It costs five."
        );
    }

    #[test]
    fn chunking_short_text_is_one_piece() {
        assert_eq!(chunk_text("Hello there.", 3000), vec!["Hello there."]);
        assert!(chunk_text("   ", 3000).is_empty());
    }

    #[test]
    fn chunking_splits_on_sentence_boundaries() {
        let text = "One sentence here. Another one follows! A third? Yes.";
        let chunks = chunk_text(text, 30);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 30, "chunk too long: {chunk}");
        }
        assert_eq!(
            chunks.join(" "),
            "One sentence here. Another one follows! A third? Yes."
        );
    }

    #[test]
    fn chunking_splits_long_sentences_at_spaces() {
        let text = "word ".repeat(600).trim_end().to_string();
        let chunks = chunk_text(&text, 100);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 100, "chunk too long: {chunk}");
        }
        let rejoined = chunks.join(" ");
        assert_eq!(rejoined.split_whitespace().count(), 600);
    }

    #[test]
    fn chunking_never_breaks_decimals() {
        // A decimal point between digits is not a sentence boundary.
        let text = format!("{} 3.14 {}", "a".repeat(60), "b".repeat(60));
        let chunks = chunk_text(&text, 80);
        assert!(chunks.len() >= 2);
        assert!(
            chunks.iter().any(|chunk| chunk.contains("3.14")),
            "decimal split across chunks: {chunks:?}"
        );
    }

    #[test]
    fn chunking_single_word_longer_than_limit_is_kept_whole() {
        let chunks = chunk_text("abcdefghijklmno", 5);
        assert_eq!(chunks, vec!["abcdefghijklmno"]);
    }
}
