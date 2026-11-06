#!/usr/bin/env python3
"""Reference implementation of the `rain` recipe in NumPy/SciPy.

Renders the same DSP structure as `qinari sfx rain` (see
crates/qinari-sfx/src/recipes.rs, `render_rain`):

  * Paul Kellet pink-noise bed (the same six one-pole stages and
    coefficients as `qinari-dsp`'s `NoiseGen`),
  * an RBJ lowpass then highpass shaping the bed (Q = 1/sqrt(2)),
  * a 0.07 Hz gust amplitude modulation,
  * Poisson-distributed droplet pings (heavy and light) with
    T60-calibrated exponential decays,
  * equal-power panning, and 120 ms edge fades.

The output is not bit-identical to qinari's (the RNG streams differ);
this exists to compare a straightforward vectorized NumPy/SciPy render
against the qinari binary on the same task.

Usage:
    python3 benchmarks/numpy_reference.py [duration_secs] [--seed N]

With QINARI_BIN set to the qinari binary, the script also times the CLI
(`qinari sfx rain -d <duration>s --seed 1`) as a full process and
prints the speedup. The comparison deliberately favors NumPy: it
measures only the in-process render, while qinari is timed end to end
(process start, render, mastering, WAV encode, file write).
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import numpy as np
from scipy.io import wavfile
from scipy.signal import lfilter

# Recipe defaults, mirroring RAIN_PARAMS in recipes.rs.
INTENSITY = 0.5
DISTANCE = 0.3
BUTTERWORTH_Q = 1.0 / np.sqrt(2.0)


def kellet_pink(white: np.ndarray) -> np.ndarray:
    """Paul Kellet's economical pink filter, vectorized.

    Each stage is a one-pole IIR (b_i[n] = c_i * b_i[n-1] + a_i * w[n]),
    so the whole chain runs through scipy.signal.lfilter.
    """
    stages = (
        (0.99886, 0.0555179),
        (0.99332, 0.0750759),
        (0.96900, 0.153852),
        (0.86650, 0.3104856),
        (0.55000, 0.5329522),
        (-0.7616, -0.016898),
    )
    out = white * 0.5362
    for c, a in stages:
        out = out + lfilter([a], [1.0, -c], white)
    return out * 0.11


def rbj_biquad(kind: str, sr: float, f0: float, q: float) -> tuple[np.ndarray, np.ndarray]:
    """RBJ audio-EQ-cookbook biquad coefficients (b, a)."""
    w0 = 2.0 * np.pi * f0 / sr
    alpha = np.sin(w0) / (2.0 * q)
    cw = np.cos(w0)
    if kind == "lowpass":
        b = [(1.0 - cw) / 2.0, 1.0 - cw, (1.0 - cw) / 2.0]
    elif kind == "highpass":
        b = [(1.0 + cw) / 2.0, -(1.0 + cw), (1.0 + cw) / 2.0]
    else:
        raise ValueError(f"unsupported kind: {kind}")
    a = [1.0 + alpha, -2.0 * cw, 1.0 - alpha]
    return np.array(b) / a[0], np.array(a) / a[0]


def render_rain(duration_secs: float, sr: int, seed: int) -> tuple[np.ndarray, np.ndarray]:
    """Renders the rain bed and droplets; returns (left, right)."""
    rng = np.random.default_rng(seed)
    frames = int(duration_secs * sr)

    # --- Bed: pink noise -> lowpass -> highpass, gusting at 0.07 Hz. ---
    bed_cutoff = float(np.clip(1500.0 + 6500.0 * INTENSITY - 1200.0 * DISTANCE, 220.0, 9000.0))
    hp_cutoff = float(np.clip(400.0 - 250.0 * DISTANCE, 60.0, 600.0))
    bed_level = 0.06 + 0.14 * INTENSITY

    white = rng.uniform(-1.0, 1.0, frames)
    bed = kellet_pink(white)
    b_lp, a_lp = rbj_biquad("lowpass", sr, bed_cutoff, BUTTERWORTH_Q)
    b_hp, a_hp = rbj_biquad("highpass", sr, hp_cutoff, BUTTERWORTH_Q)
    bed = lfilter(b_hp, a_hp, lfilter(b_lp, a_lp, bed))

    t = np.arange(frames) / sr
    gust = 0.8 + 0.45 * (0.5 + 0.5 * np.sin(2.0 * np.pi * 0.07 * t))
    bed *= bed_level * gust

    left = bed * np.sqrt(0.5)
    right = bed * np.sqrt(0.5)

    # --- Droplets: Poisson pings, heavy 8% of the time. ---
    rate = 18.0 + 95.0 * INTENSITY
    mean_gap = sr / rate
    gaps = -np.log(1.0 - rng.uniform(0.0, 1.0, int(rate * duration_secs * 2) + 16)) * mean_gap
    events = np.cumsum(gaps)
    events = events[events < frames].astype(int)

    attenuation = 1.0 - 0.55 * DISTANCE
    for at in events:
        if rng.uniform() < 0.08:
            freq = 900.0 + rng.uniform() * 700.0
            t60 = 0.1 + rng.uniform() * 0.05
            level = 0.09 + rng.uniform() * 0.08
        else:
            freq = 2200.0 + rng.uniform() * 4500.0
            t60 = 0.035 + rng.uniform() * 0.075
            level = 0.02 + rng.uniform() * 0.07
        pos = rng.uniform(-0.85, 0.85)

        n = max(4, int(np.ceil(1.2 * t60 * sr)))
        idx = np.arange(n)
        phase = rng.uniform(0.0, 2.0 * np.pi)
        ping = np.sin(2.0 * np.pi * freq * idx / sr + phase)
        ping *= np.exp(-idx / (t60 / 6.91 * sr))
        end = min(at + n, frames)
        chunk = ping[: end - at] * level * attenuation
        lg = np.sqrt(0.5 * (1.0 - pos))
        rg = np.sqrt(0.5 * (1.0 + pos))
        left[at:end] += chunk * lg
        right[at:end] += chunk * rg

    # --- Edge fades: 120 ms in and out. ---
    fade = int(120.0 * sr / 1000.0)
    if fade > 0 and frames > 2 * fade:
        ramp_in = np.linspace(0.0, 1.0, fade, endpoint=False)
        ramp_out = np.linspace(1.0, 0.0, fade, endpoint=False)
        left[:fade] *= ramp_in
        right[:fade] *= ramp_in
        left[-fade:] *= ramp_out
        right[-fade:] *= ramp_out

    return left, right


def main() -> int:
    parser = argparse.ArgumentParser(description="NumPy/SciPy reference render of the rain recipe")
    parser.add_argument("duration", nargs="?", type=float, default=10.0, help="seconds (default 10)")
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--sr", type=int, default=44100)
    args = parser.parse_args()

    start = time.perf_counter()
    left, right = render_rain(args.duration, args.sr, args.seed)
    render_secs = time.perf_counter() - start

    out_path = Path(tempfile.gettempdir()) / "qinari-numpy-rain.wav"
    start = time.perf_counter()
    wavfile.write(out_path, args.sr, np.stack([left, right], axis=1).astype(np.float32))
    write_secs = time.perf_counter() - start

    print(f"numpy rain {args.duration:g}s: render {render_secs:.3f}s (+wav {write_secs:.3f}s) -> {out_path}")

    qinari = os.environ.get("QINARI_BIN")
    if qinari:
        cli_out = Path(tempfile.gettempdir()) / "qinari-cli-rain.wav"
        cmd = [qinari, "sfx", "rain", "-d", f"{args.duration:g}s", "--seed", "1", "-o", str(cli_out)]
        start = time.perf_counter()
        result = subprocess.run(cmd, capture_output=True, text=True, check=False)
        cli_secs = time.perf_counter() - start
        if result.returncode != 0:
            print(f"qinari failed (exit {result.returncode}): {result.stderr.strip()}", file=sys.stderr)
            return 1
        print(f"qinari CLI {args.duration:g}s: full process {cli_secs:.3f}s -> {cli_out}")
        ratio = render_secs / cli_secs
        verdict = "faster" if ratio >= 1.0 else "slower"
        print(
            f"comparison: numpy in-process render {render_secs:.3f}s vs qinari full process "
            f"{cli_secs:.3f}s -> qinari is {ratio:.2f}x {verdict} (full process includes process "
            "start, mastering, and WAV encode; numpy timing excludes interpreter startup)"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
