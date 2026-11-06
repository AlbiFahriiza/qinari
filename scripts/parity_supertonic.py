#!/usr/bin/env python3
"""Supertonic parity harness: reference SDK pipeline (Python) vs the Rust
port (qinari-tts).

The Python side drives the same four ONNX graphs the archived
supertonic-py SDK drives (core.py `Supertonic.__call__`), reproducing its
preprocessing, tokenizer, mask construction, and flow-matching loop
verbatim. The initial-latent draw mirrors Qinari's own `Qrng`
(xoshiro256++ via splitmix64, f32 Box-Muller) instead of NumPy's RNG, so
both sides start from the *same* latent: durations must agree and the
waveforms must correlate — anything left is ONNX Runtime float-order
noise, which is exactly the documented statistical determinism tier.

The Rust side is `crates/qinari-tts/examples/render.rs` (run through
cargo). Both sides print duration / sample rate / RMS; this script checks
duration agreement within ±5% and non-silence on both.

Usage:

    QINARI_SUPERTONIC_DIR=/path/to/model python3 scripts/parity_supertonic.py

Requirements: python3 with numpy + onnxruntime, a Rust toolchain, and the
model directory (onnx/ + voice_styles/) at QINARI_SUPERTONIC_DIR.
"""

from __future__ import annotations

import json
import subprocess
import sys
import time
from pathlib import Path
from unicodedata import normalize

REPO = Path(__file__).resolve().parent.parent

# The parity corpus: Indonesian and English, no digits (number expansion is
# a Qinari-side feature the SDK does not have), one chunk each.
CASES = [
    ("id", "Halo, nama saya Albi."),
    ("en", "The quick brown fox jumps over the lazy dog."),
]

# What counts as agreement.
DURATION_TOLERANCE = 0.05  # ±5 %
RMS_FLOOR = 0.005  # both sides must be clearly non-silent

# Mirrors `trim_silence_edges` in qinari-tts: edges are decided on 10 ms
# RMS windows; windows below −45 dB relative to the chunk's peak window
# (or below the −60 dBFS absolute floor) are edge silence. Both sides trim
# identically so the duration comparison isolates the port, not the
# post-chain.
EDGE_WINDOW_FLOOR = 0.005_623_413_3
EDGE_ABS_FLOOR = 0.001

# Waveform correlation floor for the same-latent comparison.
CORRELATION_FLOOR = 0.98


class Qrng:
    """xoshiro256++ seeded through splitmix64 — the Python twin of
    qinari-dsp's `Qrng`, so the parity harness draws the exact latent the
    Rust engine draws for the same seed."""

    MASK = (1 << 64) - 1

    def __init__(self, seed: int):
        self._sm = seed & self.MASK
        self.s = [self._splitmix64(), self._splitmix64(), self._splitmix64(), self._splitmix64()]
        if (self.s[0] | self.s[1] | self.s[2] | self.s[3]) == 0:
            self.s[0] = 1

    def _splitmix64(self) -> int:
        self._sm = (self._sm + 0x9E3779B97F4A7C15) & self.MASK
        z = self._sm
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & self.MASK
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & self.MASK
        return z ^ (z >> 31)

    @staticmethod
    def _rotl(x: int, k: int) -> int:
        return ((x << k) | (x >> (64 - k))) & Qrng.MASK

    def next_u64(self) -> int:
        s0, s1, s2, s3 = self.s
        result = self._rotl((s0 + s3) & self.MASK, 23)
        result = (result + s0) & self.MASK
        t = (s1 << 17) & self.MASK
        s2 ^= s0
        s3 ^= s1
        s1 ^= s2
        s0 ^= s3
        s2 ^= t
        s3 = self._rotl(s3, 45)
        self.s = [s0, s1, s2, s3]
        return result

    def next_f32(self):
        import numpy as np

        # (u64 >> 40) as f32 * (1 / 2^24), all in f32 like the Rust code.
        return np.float32(self.next_u64() >> 40) * np.float32(1.0 / 16_777_216.0)


def next_normal(rng: Qrng):
    """f32 Box-Muller pair, mirroring `next_normal` in qinari-tts."""
    import numpy as np

    u1 = rng.next_f32()
    while u1 <= np.float32(np.finfo(np.float32).eps):
        u1 = rng.next_f32()
    u2 = rng.next_f32()
    radius = np.float32(np.sqrt(np.float32(-2.0) * np.log(u1)))
    angle = np.float32(2.0 * np.pi) * u2
    return np.float32(radius * np.cos(angle)), np.float32(radius * np.sin(angle))


def latent_from_qrng(rng: Qrng, channels: int, latent_len: int, latent_mask_len: int):
    """The initial noisy latent exactly as the Rust engine fills it:
    flat channel-major draw (a1, b1, a2, b2, …), then per-channel zero
    masking past `latent_mask_len`."""
    import numpy as np

    wanted = channels * latent_len
    flat: list = []
    spare = None
    while len(flat) < wanted:
        if spare is None:
            value, spare = next_normal(rng)
            flat.append(value)
        else:
            flat.append(spare)
            spare = None
    xt = np.array(flat[:wanted], dtype=np.float32).reshape(1, channels, latent_len)
    xt[0, :, latent_mask_len:] = 0.0
    return xt


def trim_edges(samples: np.ndarray, sr: int) -> np.ndarray:
    """Window-aligned edge trim, the Python twin of the Rust helper."""
    import numpy as np

    window = max(1, sr * 10 // 1000)
    if len(samples) <= window:
        return samples
    n_windows = (len(samples) + window - 1) // window
    rms = np.empty(n_windows, dtype=np.float64)
    for i in range(n_windows):
        chunk = samples[i * window : (i + 1) * window]
        rms[i] = np.sqrt(np.mean(chunk.astype(np.float64) ** 2))
    peak = rms.max()
    if peak < EDGE_ABS_FLOOR:
        return samples
    floor = peak * EDGE_WINDOW_FLOOR
    loud = np.nonzero(rms > floor)[0]
    if len(loud) == 0:
        return samples
    return samples[loud[0] * window : (loud[-1] + 1) * window]


def load_parts(model_dir: Path):
    cfg = json.loads((model_dir / "onnx/tts.json").read_text())
    indexer = json.loads((model_dir / "onnx/unicode_indexer.json").read_text())
    style = json.loads((model_dir / "voice_styles/M1.json").read_text())
    ttl = reshape(style["style_ttl"])
    dp = reshape(style["style_dp"])
    sessions = {
        name: __import__("onnxruntime").InferenceSession(
            str(model_dir / "onnx" / f"{name}.onnx"),
            providers=["CPUExecutionProvider"],
        )
        for name in (
            "duration_predictor",
            "text_encoder",
            "vector_estimator",
            "vocoder",
        )
    }
    return cfg, indexer, ttl, dp, sessions


def reshape(component):
    import numpy as np

    return np.array(component["data"], dtype=np.float32).reshape(component["dims"])


def preprocess(text: str, lang: str) -> str:
    """The SDK's `UnicodeProcessor._preprocess_text` (core.py), verbatim."""
    text = normalize("NFKD", text)
    for old, new in {
        "\u2013": "-",
        "\u2011": "-",
        "\u2014": "-",
        "\u00af": " ",
        "_": " ",
        "\u201c": '"',
        "\u201d": '"',
        "\u2018": "'",
        "\u2019": "'",
        "\u00b4": "'",
        "`": "'",
        "[": " ",
        "]": " ",
        "|": " ",
        "/": " ",
        "#": " ",
        "\u2192": " ",
        "\u2190": " ",
    }.items():
        text = text.replace(old, new)
    for symbol in "\u2665\u2606\u2661\u00a9\\":
        text = text.replace(symbol, "")
    for old, new in {
        "@": " at ",
        "e.g.,": "for example, ",
        "i.e.,": "that is, ",
    }.items():
        text = text.replace(old, new)
    for old, new in [
        (" ,", ","),
        (" .", "."),
        (" !", "!"),
        (" ?", "?"),
        (" ;", ";"),
        (" :", ":"),
        (" '", "'"),
    ]:
        text = text.replace(old, new)
    import re

    text = re.sub(r'(["\'`])\1+', r"\1", text)
    text = re.sub(r"\s+", " ", text).strip()
    if text and not re.search(
        r"[.!?;:,'\")\]}。．」』】〉》›»]$", text
    ):
        text += "."
    return f"<{lang}>{text}</{lang}>"


def synth(model_dir: Path, parts, text: str, lang: str, steps: int, speed: float, seed: int):
    """One chunk through the four graphs, mirroring core.py exactly."""
    import numpy as np

    cfg, indexer, ttl, dp, sessions = parts
    sr = cfg["ae"]["sample_rate"]
    bcs = cfg["ae"]["base_chunk_size"]
    ccf = cfg["ttl"]["chunk_compress_factor"]
    ldim = cfg["ttl"]["latent_dim"]

    preprocessed = preprocess(text, lang)
    ids = np.array(
        [[indexer[ord(c)] for c in preprocessed if ord(c) < len(indexer)]],
        dtype=np.int64,
    )
    mask = np.ones((1, 1, ids.shape[1]), dtype=np.float32)

    dur = sessions["duration_predictor"].run(
        None, {"text_ids": ids, "style_dp": dp, "text_mask": mask}
    )[0] / speed
    emb = sessions["text_encoder"].run(
        None, {"text_ids": ids, "style_ttl": ttl, "text_mask": mask}
    )[0]

    wav_len = int(dur[0] * sr)
    chunk = bcs * ccf
    latent_len = int((dur[0] * sr + chunk - 1) / chunk)
    latent_len = max(latent_len, 1)
    latent_mask_len = max(1, min((wav_len + chunk - 1) // chunk, latent_len))

    rng = Qrng(seed)
    channels = ldim * ccf
    xt = latent_from_qrng(rng, channels, latent_len, latent_mask_len)
    lm = np.zeros((1, 1, latent_len), dtype=np.float32)
    lm[0, 0, :latent_mask_len] = 1.0

    total_step = np.array([steps], dtype=np.float32)
    for step in range(steps):
        current = np.array([step], dtype=np.float32)
        xt = sessions["vector_estimator"].run(
            None,
            {
                "noisy_latent": xt,
                "text_emb": emb,
                "style_ttl": ttl,
                "latent_mask": lm,
                "text_mask": mask,
                "current_step": current,
                "total_step": total_step,
            },
        )[0]
    wav = sessions["vocoder"].run(None, {"latent": xt})[0]
    kept = wav[0][: min(wav_len, wav[0].shape[0])]
    trimmed = trim_edges(kept, sr)
    return {
        "duration": len(trimmed) / sr,
        "kept_seconds": len(kept) / sr,
        "sr": int(sr),
        "rms": float(np.sqrt(np.mean(trimmed**2))),
        "preprocessed": preprocessed,
        "samples": np.asarray(trimmed, dtype=np.float32),
    }


def read_wav_f32(path: Path):
    """Reads a 32-bit float RIFF/WAVE file (what the example binary writes
    via hound) into a float32 array."""
    import struct

    data = path.read_bytes()
    idx = data.find(b"data")
    if idx < 0:
        raise ValueError(f"no data chunk in {path}")
    size = struct.unpack("<I", data[idx + 4 : idx + 8])[0]
    payload = data[idx + 8 : idx + 8 + size]
    import numpy as np

    return np.array(struct.unpack(f"<{len(payload) // 4}f", payload), dtype=np.float32)


def render_rust(model_dir: Path, text: str, lang: str, steps: int, seed: int) -> dict:
    """Runs the Rust example binary, parses its report line, and loads the
    waveform it wrote for the correlation check."""
    import tempfile

    with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as handle:
        out = Path(handle.name)
    command = [
        "cargo",
        "run",
        "--release",
        "--quiet",
        "-p",
        "qinari-tts",
        "--example",
        "render",
        "--",
        "--lang",
        lang,
        "--steps",
        str(steps),
        "--seed",
        str(seed),
        "--speed",
        "1.0",
        "--text",
        text,
        "--out",
        str(out),
    ]
    started = time.time()
    result = subprocess.run(
        command, cwd=REPO, capture_output=True, text=True, check=True
    )
    wall = time.time() - started
    fields = {}
    for token in result.stdout.split():
        if "=" in token:
            key, value = token.split("=", 1)
            fields[key] = value
    return {
        "duration": float(fields["duration"]),
        "sr": int(fields["sr"]),
        "rms": float(fields["rms"]),
        "rtf": float(fields["rtf"]),
        "wall_including_build": wall,
        "samples": read_wav_f32(out),
    }


def main() -> int:
    model_dir = Path(__import__("os").environ.get("QINARI_SUPERTONIC_DIR", "."))
    if not (model_dir / "onnx/tts.json").exists():
        print(f"model dir not found at {model_dir}", file=sys.stderr)
        return 2
    parts = load_parts(model_dir)
    steps = 8
    seed = 1234
    failures = 0
    print(f"model dir: {model_dir}  steps={steps} seed={seed} speed=1.0 voice=M1")
    print(f"{'case':<4} {'side':<7} {'duration':>9} {'sr':>7} {'rms':>9}")
    for lang, text in CASES:
        py = synth(model_dir, parts, text, lang, steps, 1.0, seed)
        rs = render_rust(model_dir, text, lang, steps, seed)
        for side, report in (("python", py), ("rust", rs)):
            print(
                f"{lang:<4} {side:<7} {report['duration']:>9.3f} "
                f"{report['sr']:>7} {report['rms']:>9.5f}"
            )
        delta = abs(py["duration"] - rs["duration"]) / py["duration"]
        # Same latent on both sides: the waveforms themselves must line up
        # (up to ORT float-order noise, the documented statistical tier).
        import numpy as np

        common = min(len(py["samples"]), len(rs["samples"]))
        a = py["samples"][:common].astype(np.float64)
        b = rs["samples"][:common].astype(np.float64)
        corr = (
            float(np.corrcoef(a, b)[0, 1])
            if common > 1 and a.std() > 0 and b.std() > 0
            else 0.0
        )
        verdict = "ok" if delta <= DURATION_TOLERANCE else "FAIL"
        if delta > DURATION_TOLERANCE:
            failures += 1
        if corr < CORRELATION_FLOOR:
            verdict = f"FAIL (corr {corr:.3f})"
            failures += 1
        if py["rms"] < RMS_FLOOR or rs["rms"] < RMS_FLOOR:
            verdict = "FAIL (silent)"
            failures += 1
        print(
            f"     -> duration delta {delta*100:.2f}% ({verdict}), "
            f"waveform corr {corr:.4f}, rust RTF {rs['rtf']:.3f}"
        )
        if py["preprocessed"] != f"<{lang}>{text if text.endswith('.') else text + '.'}</{lang}>":
            print(f"     note: preprocessed = {py['preprocessed']!r}")
    print("parity: " + ("PASS" if failures == 0 else f"{failures} FAILURE(S)"))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
