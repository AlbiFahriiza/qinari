//! Unicode NFKD normalization over the generated tables in
//! [`nfkd_data`].
//!
//! The reference SDK normalizes text with Python's
//! `unicodedata.normalize("NFKD", ...)` before tokenizing; Qinari pins no
//! unicode-normalization crate, so this module reproduces the same
//! normalization from committed data: the full NFKD mapping (minus the
//! Hangul syllable block, decomposed arithmetically) plus canonical
//! ordering of combining marks.

use super::nfkd_data;

/// Decomposes Hangul syllables (U+AC00..=U+D7A3) into their jamo, the one
/// block the generated table leaves to arithmetic.
fn push_hangul_decomposed(cp: u32, out: &mut String) {
    const SYLLABLE_BASE: u32 = 0xAC00;
    const LEAD_BASE: u32 = 0x1100;
    const VOWEL_BASE: u32 = 0x1161;
    const TRAIL_BASE: u32 = 0x11A7;
    const VOWEL_COUNT: u32 = 21;
    const TRAIL_COUNT: u32 = 28;
    let syllable = cp - SYLLABLE_BASE;
    let lead = LEAD_BASE + syllable / (VOWEL_COUNT * TRAIL_COUNT);
    let vowel = VOWEL_BASE + (syllable % (VOWEL_COUNT * TRAIL_COUNT)) / TRAIL_COUNT;
    let trail = syllable % TRAIL_COUNT;
    // SAFETY-free: every value below is inside the assigned jamo blocks.
    if let (Some(l), Some(v), Some(t)) = (
        char::from_u32(lead),
        char::from_u32(vowel),
        char::from_u32(TRAIL_BASE + trail),
    ) {
        out.push(l);
        out.push(v);
        if trail > 0 {
            out.push(t);
        }
    }
}

/// The canonical combining class of `c` (0 when none).
pub(crate) fn combining_class(c: char) -> u8 {
    nfkd_data::COMBINING_CLASS
        .binary_search_by_key(&(c as u32), |entry| entry.0)
        .map_or(0, |at| nfkd_data::COMBINING_CLASS[at].1)
}

/// Appends the full NFKD decomposition of `c` to `out`.
fn push_decomposed(c: char, out: &mut String) {
    let cp = u32::from(c);
    if (0xAC00..=0xD7A3).contains(&cp) {
        push_hangul_decomposed(cp, out);
        return;
    }
    match nfkd_data::DECOMP.binary_search_by_key(&cp, |entry| entry.0) {
        Ok(at) => out.push_str(nfkd_data::DECOMP[at].1),
        Err(_) => out.push(c),
    }
}

/// Normalizes `text` to NFKD: full decomposition, then canonical ordering
/// of combining marks within each combining sequence.
#[must_use]
pub(crate) fn nfkd(text: &str) -> String {
    let mut decomposed = String::with_capacity(text.len());
    for c in text.chars() {
        push_decomposed(c, &mut decomposed);
    }
    canonical_order(&mut decomposed);
    decomposed
}

/// Reorders combining marks canonically: within every maximal run of
/// non-starter characters, a stable insertion sort by combining class
/// (D109/D110 in the Unicode Standard).
fn canonical_order(text: &mut String) {
    let chars: Vec<(char, u8)> = text
        .chars()
        .map(|c| (c, combining_class(c)))
        .collect::<Vec<_>>();
    let mut ordered = chars;
    let mut start = 0;
    for at in 0..=ordered.len() {
        let begins_new_sequence = at == ordered.len() || ordered[at].1 == 0;
        if begins_new_sequence {
            if at > start + 1 {
                ordered[start..at].sort_by_key(|entry| entry.1);
            }
            start = at + 1;
        }
    }
    if ordered.iter().map(|entry| entry.0).eq(text.chars()) {
        return;
    }
    text.clear();
    for (c, _) in ordered {
        text.push(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference vectors produced with Python's `unicodedata` (15.0.0).
    #[test]
    fn matches_python_nfkd_reference_vectors() {
        // Accented Latin decomposes to base letter + combining mark.
        assert_eq!(nfkd("é"), "e\u{0301}");
        assert_eq!(nfkd("Ünter"), "U\u{0308}nter");
        // Hangul syllables decompose arithmetically.
        assert_eq!(nfkd("한"), "\u{1112}\u{1161}\u{11AB}");
        // Compatibility mappings.
        assert_eq!(nfkd("ﬁ"), "fi");
        assert_eq!(nfkd("…"), "...");
        assert_eq!(nfkd("½"), "1⁄2");
        assert_eq!(nfkd("㎡"), "m2");
        assert_eq!(nfkd("㎝"), "cm");
        assert_eq!(nfkd("Ⅳ"), "IV");
        assert_eq!(nfkd("Ａ"), "A");
        assert_eq!(nfkd("ﾊ"), "ハ");
        // Untouched text stays untouched.
        assert_eq!(nfkd("Halo, nama saya Albi."), "Halo, nama saya Albi.");
        assert_eq!(nfkd(""), "");
    }

    #[test]
    fn combining_marks_are_canonically_ordered() {
        // U+031B (ccc 216) must sort before U+0301 (ccc 230).
        assert_eq!(nfkd("a\u{0301}\u{031B}"), "a\u{031B}\u{0301}");
        // Already-ordered sequences do not move.
        assert_eq!(nfkd("a\u{031B}\u{0301}"), "a\u{031B}\u{0301}");
    }

    #[test]
    fn decomposed_output_of_real_sentences_is_stable() {
        // The em dash has no decomposition (the SDK maps it in a later,
        // separate step), the accents do.
        let out = nfkd("Café naïve — résumé.");
        assert_eq!(out, "Cafe\u{0301} nai\u{0308}ve — re\u{0301}sume\u{0301}.");
    }
}
