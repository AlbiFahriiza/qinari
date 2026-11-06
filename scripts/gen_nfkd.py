#!/usr/bin/env python3
"""Regenerates crates/qinari-tts/src/supertonic/nfkd_data.rs.

The Supertonic tokenizer must normalize text exactly like the reference
SDK (`unicodedata.normalize("NFKD", ...)` in supertonic-py's core.py).
Qinari pins no unicode-normalization crate, so the decomposition data is
generated from the Python standard library's UCD snapshot and committed.

Run from the repository root:

    python3 scripts/gen_nfkd.py

The generated file is data only; the algorithm that uses it lives in
crates/qinari-tts/src/supertonic/nfkd.rs.
"""

import unicodedata
from pathlib import Path

DEST = Path("crates/qinari-tts/src/supertonic/nfkd_data.rs")

HEADER = '''//! Generated Unicode NFKD decomposition data (DO NOT EDIT BY HAND).
//!
//! Source of truth: the Unicode Character Database as shipped with Python's
//! `unicodedata` module, version {version}. Regenerate with the snippet in
//! `scripts/gen_nfkd.py` (same table, same format). The table holds the full
//! NFKD decomposition of every assigned code point whose NFKD differs from
//! itself, **except** the Hangul syllable block U+AC00..=U+D7A3, which
//! [`super::nfkd`] decomposes arithmetically. Canonical ordering uses the
//! combining-class table below. Keeping this data in-tree is what lets the
//! Supertonic tokenizer normalize exactly like the reference SDK's
//! `unicodedata.normalize("NFKD", ...)` without a new dependency.

/// Combining classes for code points with a non-zero canonical combining
/// class, sorted by code point (binary-searched).
pub(super) static COMBINING_CLASS: &[(u32, u8)] = &[
'''


def escape(ch: str) -> str:
    """One character as Rust source: literal when reviewable, `\\u{..}` otherwise."""
    cp = ord(ch)
    if 0x20 < cp < 0x7F and ch not in '"\\':
        return ch
    invisible = ("Cc", "Cf", "Cs", "Zs", "Zl", "Zp")
    if (
        cp >= 0xA0
        and unicodedata.combining(ch) == 0
        and unicodedata.category(ch) not in invisible
    ):
        return ch
    return f"\\u{{{cp:x}}}"


def main() -> None:
    out = [HEADER.format(version=unicodedata.unidata_version)]
    for cp in range(0x110000):
        cls = unicodedata.combining(chr(cp))
        if cls:
            out.append(f"    (0x{cp:04X}, {cls}),\n")
    out.append("];\n\n")
    out.append(
        """/// Full NFKD decompositions (Hangul syllables excluded), sorted by code
/// point (binary-searched).
pub(super) static DECOMP: &[(u32, &str)] = &[
"""
    )
    for cp in range(0x110000):
        if 0xAC00 <= cp <= 0xD7A3:  # Hangul SV: decomposed arithmetically
            continue
        ch = chr(cp)
        decomposed = unicodedata.normalize("NFKD", ch)
        if decomposed == ch:
            continue
        value = "".join(escape(c) for c in decomposed)
        out.append(f'    (0x{cp:04X}, "{value}"),\n')
    out.append("];\n")
    DEST.write_text("".join(out), encoding="utf-8")
    print(f"wrote {DEST}")


if __name__ == "__main__":
    main()
