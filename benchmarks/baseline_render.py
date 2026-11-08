#!/usr/bin/env python3
"""Locked 0.1.5 regression baseline: render the full catalog, measure it.

Renders every sfx recipe and ambient preset from the LIVE catalog
(`qinari sfx --list` / `qinari ambient --list` — never a hardcoded list)
at two fixed seeds, extracts the layer-A/B metrics of each render with
`qinari analyze --json`, and writes one JSON capture file with a
provenance header.

Determinism is proven, not assumed: the whole catalog is captured TWICE
per invocation and the two JSON documents must be byte-identical, or the
script exits 1 without accepting either capture (a fixed seed that fails
to reproduce itself is a class-1 correctness finding, not noise).

The default output settings are the 0.1.5 config defaults (44.1 kHz,
stereo, 24-bit WAV, mastering chain on: -16 LUFS, -1.0 dBFS ceiling,
transparent). The XDG config/state dirs are isolated to the scratch dir
so a user config on the capture machine cannot leak into the baseline.

Usage:
    python3 benchmarks/baseline_render.py [--qinari PATH] [--out FILE]

Outputs the capture to --out (default benchmarks/baseline-fresh.json).
Exit codes: 0 = capture written and determinism verified, 1 = render
failure or determinism broken.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

# The two fixed seeds. `realism-bench` is the seed the 0.1.5 realism
# migration was measured with (docs/BENCHMARKS.md); `baseline-0.1.5` is
# an independent second draw so the baseline is not a single-sample
# artifact of one stream.
SEEDS = ["realism-bench", "baseline-0.1.5"]
SAMPLE_RATE_HZ = 44_100
CHANNELS = 2
# Modest durations keep the whole double capture in minutes: 4 s per sfx
# recipe (enough for LUFS, the 0-20 Hz gain spectrum, and event
# statistics) and 6 s per ambient bed (ambient renders are ~87x
# realtime; beds need a longer window for their slow motion).
SFX_DURATION_S = 4.0
AMBIENT_DURATION_S = 6.0


def run_qinari(qinari: str, args: list[str], env: dict[str, str]) -> str:
    """Runs the CLI once, returning stdout; any failure is fatal."""
    result = subprocess.run([qinari, *args], capture_output=True, text=True,
                            env=env, check=False)
    if result.returncode != 0:
        sys.stderr.write(
            f"render failed ({result.returncode}): qinari {' '.join(args)}\n"
            f"{result.stderr.strip()}\n")
        raise SystemExit(1)
    return result.stdout


def enumerate_catalog(qinari: str, env: dict[str, str]) -> tuple[list[str], list[str]]:
    """Reads the recipe and preset ids from the live registry."""
    recipes = []
    for line in run_qinari(qinari, ["sfx", "--list"], env).splitlines():
        # Recipe ids sit on their own line; descriptions and parameters
        # are indented.
        if line and not line.startswith((" ", "\t")):
            recipes.append(line.strip())
    presets = []
    for line in run_qinari(qinari, ["ambient", "--list"], env).splitlines():
        # "id — description (N layers)"
        if " — " in line:
            presets.append(line.split(" — ", 1)[0].strip())
    if not recipes or not presets:
        sys.stderr.write(f"catalog enumeration failed: {len(recipes)} recipes, "
                         f"{len(presets)} presets\n")
        raise SystemExit(1)
    return recipes, presets


def capture(qinari: str, scratch: Path, tag: str) -> dict:
    """Renders and measures the whole catalog once; returns the capture.

    `tag` only names the scratch subdirectory (kept out of the capture
    so the two runs remain byte-comparable).
    """
    work = scratch / tag
    work.mkdir(parents=True, exist_ok=True)
    # Isolated XDG dirs: pure config defaults, log file out of the way.
    env = dict(os.environ)
    env["XDG_CONFIG_HOME"] = str(work / "config")
    env["XDG_STATE_HOME"] = str(work / "state")

    recipes, presets = enumerate_catalog(qinari, env)
    jobs = ([("sfx", r, SFX_DURATION_S) for r in recipes]
            + [("ambient", p, AMBIENT_DURATION_S) for p in presets])

    renders = []
    for kind, item, duration in jobs:
        for seed in SEEDS:
            wav = work / f"{item}-{seed}.wav"
            args = ([kind, item, "-o", str(wav), "-d", f"{duration}s",
                     "--seed", seed, "--sr", str(SAMPLE_RATE_HZ),
                     "--channels", str(CHANNELS), "--json"])
            summary = json.loads(run_qinari(qinari, args, env))
            metrics = json.loads(run_qinari(qinari, ["analyze", "--json",
                                                     str(wav)], env))
            # The file path is per-run scratch; everything else is a
            # measurement and stays.
            del metrics["file"]
            renders.append({
                "kind": kind,
                "recipe": item,
                "seed": seed,
                "sha256": summary["sha256"],
                "size_bytes": summary["size_bytes"],
                "mastered": summary["mastered"],
                "metrics": metrics,
            })
            print(f"  {kind} {item} [{seed}]: sha {summary['sha256'][:12]}, "
                  f"peak {metrics['peak_dbfs']:.1f} dBFS, "
                  f"lufs {metrics['lufs']}", file=sys.stderr)
    return {"renders": renders}


def provenance(qinari: str, repo_root: Path) -> dict:
    """The header that makes the capture auditable later."""
    version = run_qinari(qinari, ["--version"], dict(os.environ)).strip()
    code_hash = subprocess.run(["git", "-C", str(repo_root), "rev-parse", "HEAD"],
                               capture_output=True, text=True, check=True
                               ).stdout.strip()
    return {
        # Day granularity on purpose: the determinism proof byte-compares
        # two captures, and a timestamp finer than a day would differ
        # between them by construction.
        "date_utc": datetime.now(timezone.utc).date().isoformat(),
        "code_hash": code_hash,
        "qinari_version": version,
        "machine": {
            "uname": subprocess.run(["uname", "-sr"], capture_output=True,
                                    text=True, check=True).stdout.strip(),
            "cores": os.cpu_count(),
        },
        "seeds": SEEDS,
        "sample_rate_hz": SAMPLE_RATE_HZ,
        "channels": CHANNELS,
        "duration_secs": {"sfx": SFX_DURATION_S, "ambient": AMBIENT_DURATION_S},
        "output": "default chain (24-bit WAV, mastering on: -16 LUFS, "
                  "-1.0 dBFS ceiling, transparent)",
        "tools": {"python": sys.version.split()[0]},
        "determinism": "verified: two full captures byte-identical",
    }


def main() -> int:
    here = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--qinari", default=str(here.parent / "target/release/qinari"),
                        help="qinari binary (default: ./target/release/qinari)")
    parser.add_argument("--out", default=str(here / "baseline-fresh.json"),
                        help="capture file to write")
    args = parser.parse_args()
    qinari = str(Path(args.qinari).resolve())
    if not Path(qinari).exists():
        sys.stderr.write(f"qinari binary not found: {qinari}\n")
        return 1

    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="qinari-baseline-") as tmp:
        scratch = Path(tmp)
        first = capture(qinari, scratch, "run-a")
        second = capture(qinari, scratch, "run-b")
    a = json.dumps(first, indent=1, sort_keys=True)
    b = json.dumps(second, indent=1, sort_keys=True)
    if a != b:
        (here / "baseline-run-a.debug.json").write_text(a)
        (here / "baseline-run-b.debug.json").write_text(b)
        sys.stderr.write(
            "DETERMINISM BROKEN: two captures with the same seeds differ.\n"
            f"Debug dumps: {here}/baseline-run-a.debug.json and "
            f"{here}/baseline-run-b.debug.json\n"
            "This is a class-1 correctness finding; do not average it away.\n")
        return 1

    capture_doc = {"schema": "qinari-baseline/1",
                   "provenance": provenance(qinari, here.parent),
                   **first}
    out = Path(args.out)
    out.write_text(json.dumps(capture_doc, indent=1, sort_keys=True) + "\n")
    print(f"{len(first['renders'])} renders x 2 identical captures in "
          f"{time.monotonic() - started:.0f}s -> {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
