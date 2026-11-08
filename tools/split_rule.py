#!/usr/bin/env python3
"""Split rule of record for the Qinari 0.2.0 golden corpus (plan P0-4).

The corpus manifest
(`crates/qinari-verify/tests/golden/corpus-manifest.json`) assigns every
file one of three subsets: train / validation / hold-out. The assignment
is NOT a judgment call per file — it is a fixed, documented, reproducible
function of the uploader, so that statistics tooling and CI can recompute
it at any time and detect drift:

    digest = sha256("qinari-corpus-split-v1|<seed>|<uploader>")  (utf-8)
    u      = int(digest[:16], 16) / 2**64        # uniform in [0, 1)
    subset = "train"       if u < 0.60
             "validation"  if u < 0.80
             "hold-out"    otherwise

The seed is recorded in the manifest (`split_rule.seed`,
"corpus-split-v1" as of 2026-11-08) and never changes after recording.

The unit of assignment is the UPLOADER, globally across categories — not
(uploader, category) — because the no-source-leakage rule (plan 6.1
rule 2: one recorder/session never in two subsets) must hold corpus-wide:
several uploaders contribute to two categories, and one source
(fs_477342) is deliberately listed under both rain and wind. Hashing the
uploader alone pins every file of a source — including copies of the
same recording listed under two categories — into exactly one subset.

This script is the verifier: it recomputes the split from the manifest
and fails (exit 1) on any deviation. Stdlib only; no repository runtime
dependency.

Usage (from the repo root):

    python3 tools/split_rule.py
    python3 tools/split_rule.py --corpus-root /path/to/corpus  # + disk check
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from collections import Counter
from pathlib import Path

ALGO_TAG = "qinari-corpus-split-v1"
DEFAULT_MANIFEST = Path("crates/qinari-verify/tests/golden/"
                        "corpus-manifest.json")
TRAIN_MAX = 0.60  # u < 0.60 -> train
VALIDATION_MAX = 0.80  # 0.60 <= u < 0.80 -> validation; else hold-out


def assign_subset(uploader: str, seed: str) -> str:
    """The rule of record. Must stay byte-identical to the manifest."""
    digest = hashlib.sha256(
        f"{ALGO_TAG}|{seed}|{uploader}".encode("utf-8")).hexdigest()
    u = int(digest[:16], 16) / 2 ** 64
    if u < TRAIN_MAX:
        return "train"
    if u < VALIDATION_MAX:
        return "validation"
    return "hold-out"


SUBSETS = ("train", "validation", "hold-out")


def check_assignment(files: list[dict], seed: str) -> list[str]:
    """Every row's recorded subset matches the recomputed assignment."""
    errors = []
    for f in files:
        want = assign_subset(f["uploader"], seed)
        if f["subset"] == want:
            continue
        errors.append(f"{f['file']}: subset {f['subset']!r} != {want!r}")
    return errors


def category_counts(files: list[dict], category: str) -> Counter:
    """Subset counts for one category."""
    return Counter(f["subset"] for f in files if f["category"] == category)


def check_no_leakage(files: list[dict]) -> list[str]:
    """No uploader (source) spans two subsets."""
    errors = []
    seen: dict[str, str] = {}
    for f in files:
        prev = seen.setdefault(f["uploader"], f["subset"])
        if prev == f["subset"]:
            continue
        errors.append(f"source leakage: {f['uploader']!r} in {prev!r} and {f['subset']!r}")
    return errors


def check_summary(manifest: dict, files: list[dict]) -> list[str]:
    """The manifest's summary block matches the recomputed counts."""
    errors = []
    overall = Counter(f["subset"] for f in files)
    per_cat = manifest["split_summary"]["per_category"]
    for cat, summary in per_cat.items():
        actual = category_counts(files, cat)
        for subset in SUBSETS:
            got = actual.get(subset, 0)
            if summary[subset] == got:
                continue
            errors.append(f"{cat}: {subset} {summary[subset]} != {got}")
    overall_recorded = manifest["split_summary"]["overall"]
    for subset in SUBSETS:
        got = overall.get(subset, 0)
        if overall_recorded[subset] == got:
            continue
        errors.append(f"overall: {subset} {overall_recorded[subset]} != {got}")
    return errors


def check_disk(files: list[dict], corpus_root: Path) -> list[str]:
    """Optional: verify the audio files on disk against the manifest."""
    errors = []
    for f in files:
        path = corpus_root / f["file"]
        if not path.exists():
            errors.append(f"{f['file']}: missing under {corpus_root}")
            continue
        if path.stat().st_size != f["bytes"]:
            errors.append(f"{f['file']}: size on disk != manifest")
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if digest != f["sha256"]:
            errors.append(f"{f['file']}: SHA-256 on disk != manifest")
    return errors


def print_summary(manifest_path: Path, seed: str, files: list[dict]) -> None:
    n = len(files)
    overall = Counter(f["subset"] for f in files)
    print(f"manifest: {manifest_path} ({n} files, seed {seed!r})")
    for cat in sorted({f["category"] for f in files}):
        c = category_counts(files, cat)
        counts = " ".join(f"{s}={c.get(s, 0):>2}" for s in SUBSETS)
        print(f"  {cat:<10} {counts}")
    pct = "/".join(f"{100 * overall.get(s, 0) / n:.0f}%" for s in SUBSETS)
    counts = " ".join(f"{s}={overall.get(s, 0):>2}" for s in SUBSETS)
    print(f"  {'TOTAL':<10} {counts}  ({pct}, target 60/20/20)")


def verify(manifest_path: Path, corpus_root: Path | None) -> int:
    manifest = json.loads(manifest_path.read_text())
    seed = manifest["split_rule"]["seed"]
    files = manifest["files"]

    errors = (check_assignment(files, seed)
              + check_no_leakage(files)
              + check_summary(manifest, files))
    disk_checked = corpus_root is not None
    if disk_checked:
        errors += check_disk(files, corpus_root)

    print_summary(manifest_path, seed, files)
    if errors:
        for e in errors:
            print(f"FAIL: {e}", file=sys.stderr)
        return 1
    print("split rule verified: no deviation, no source leakage"
          + (", disk bytes + SHA-256 all match" if disk_checked else ""))
    return 0


def main(argv: list[str]) -> int:
    here = Path(__file__).resolve()
    repo_root = here.parent.parent
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--manifest", type=Path,
        default=repo_root / DEFAULT_MANIFEST,
        help="path to corpus-manifest.json")
    parser.add_argument(
        "--corpus-root", type=Path, default=None,
        help="optional corpus root to also verify file bytes/SHA-256")
    args = parser.parse_args(argv)
    if not args.manifest.is_absolute():
        args.manifest = repo_root / args.manifest
    return verify(args.manifest, args.corpus_root)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
