#!/usr/bin/env python3
"""Compare a fresh baseline capture against the committed 0.1.5 baseline.

Implements the four-class regression policy of docs/PLAN-0.2.0.md §9.
The 0.1.5 baseline is a HISTORICAL reference for regression diagnosis,
not a naturalness standard: 0.2.0 changes the generation model on
purpose, so most metric movement is expected — only correctness is
blocked unconditionally.

Classes (§9 table 9b):
  1 correctness   NaN/Inf, determinism not proven, clipping/ceiling,
                  missing or malformed renders, wrong duration/rate.
                  ALWAYS exits 1. No flag can override class 1.
  2 validated     Layer-B/C metrics of categories that passed Test A/B.
                  Exits 1 unless --justified "text"; reopening properly
                  requires new Test A/B evidence (no listening tests
                  exist yet, so the paragraph is the interim).
  3 improvement   Metrics whose improvement direction is validated
                  (golden distance). Passes and is logged.
  4 expected      Model-generation change. Passes ONLY with a mandatory
                  one-paragraph diagnosis: --expected-change "text" or
                  --expected-change-file PATH (e.g.
                  benchmarks/EXPECTED-CHANGE.md in the PR). Logged with
                  the label `expected-change`.

Which metric belongs to which class is the CLASS MAP below. Class 2 and
3 are empty by default on purpose: as of 0.1.5 no category has Test A/B
evidence and the golden corpus (P0-4) does not exist yet, so every
non-correctness metric defaults to class 4. As evidence lands, promote
metrics by overriding the map with --class-map (a JSON file whose
entries replace the defaults below):

  {
    "realism.gain_line_share": {"class": 3, "direction": "down"},
    "lufs": {"class": 2, "abs_tol": 0.05}
  }

Tolerances (abs_tol, rel_tol) decide "changed vs unchanged" per metric;
they absorb cross-machine 1-ulp floating-point drift, not real model
changes. Bit-level determinism is gated separately and on the same
machine by baseline_render.py's double capture.

Usage:
    python3 benchmarks/compare_baseline.py --fresh baseline-fresh.json \
        [--baseline benchmarks/baseline-0.1.5.json] \
        [--expected-change-file benchmarks/EXPECTED-CHANGE.md] \
        [--class-map my-map.json] [--report report.txt]

Exit codes: 0 = no blocking finding, 1 = class-1 violation, unjustified
class-2 change, or class-4 change without the mandatory diagnosis.
"""
from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

# Metric path -> classification. Paths are flattened analyze --json
# keys ("realism.gain_line_share"). abs_tol/rel_tol: a metric changed
# when |fresh - base| > abs_tol + rel_tol * |base|.
#
# Class 2/3 are empty by default (see module docstring); everything
# measurable is class 4 until Test A/B and golden evidence exist.
DEFAULT_CLASS_MAP = {
    "duration_s": {"class": 4, "abs_tol": 0.01},
    "lufs": {"class": 4, "abs_tol": 0.05},
    "peak_dbfs": {"class": 4, "abs_tol": 0.05},
    "rms_dbfs": {"class": 4, "abs_tol": 0.05},
    "speech.voiced_fraction": {"class": 4, "abs_tol": 0.005},
    "speech.f0_mean_hz": {"class": 4, "abs_tol": 1.0},
    "speech.f0_sigma_st": {"class": 4, "abs_tol": 0.05},
    "speech.f0_range_st": {"class": 4, "abs_tol": 0.05},
    "speech.pause_count": {"class": 4, "abs_tol": 1},
    "speech.pause_total_s": {"class": 4, "abs_tol": 0.05},
    "speech.pause_rate_per_min": {"class": 4, "abs_tol": 0.5},
    "realism.gain_peak_mean_ratio": {"class": 4, "abs_tol": 1.0, "rel_tol": 0.02},
    "realism.gain_peak_hz": {"class": 4, "abs_tol": 0.05},
    "realism.gain_line_share": {"class": 4, "abs_tol": 0.005},
    "realism.interaural.b0_500": {"class": 4, "abs_tol": 0.005},
    "realism.interaural.b500_2000": {"class": 4, "abs_tol": 0.005},
    "realism.interaural.b2000_8000": {"class": 4, "abs_tol": 0.005},
    "realism.interaural.b8000_nyquist": {"class": 4, "abs_tol": 0.005},
    "realism.event_count": {"class": 4, "abs_tol": 1},
    "realism.event_rate_hz": {"class": 4, "abs_tol": 0.1},
    "realism.event_interval_cv": {"class": 4, "abs_tol": 0.01},
}

# Class-1 predicates. peak_dbfs above CLIP_DBFS is clipping; above the
# baseline's worst peak by CEILING_MARGIN_DB means the limiter stopped
# holding the 0.1.5 ceiling (transient peaks sit AT the ceiling).
CLIP_DBFS = 0.0
CEILING_MARGIN_DB = 0.5
DURATION_TOLERANCE = 0.01


def load_capture(path: Path) -> dict:
    """Loads one capture file, keyed by (kind, recipe, seed)."""
    doc = json.loads(path.read_text())
    if doc.get("schema") != "qinari-baseline/1":
        sys.stderr.write(f"{path}: unknown schema {doc.get('schema')!r}\n")
        raise SystemExit(1)
    return {(r["kind"], r["recipe"], r["seed"]): r for r in doc["renders"]}


def numeric_leaves(prefix: str, value: object) -> list[tuple[str, object]]:
    """(path, value) for every leaf, descending into dicts."""
    if isinstance(value, dict):
        pairs: list[tuple[str, object]] = []
        for key, inner in value.items():
            pairs.extend(numeric_leaves(f"{prefix}{key}.", inner))
        return pairs
    return [(prefix[:-1], value)]


def changed(base: object, fresh: object, rule: dict) -> bool:
    """Whether one metric moved beyond its tolerance."""
    if isinstance(base, bool) or not isinstance(base, (int, float)):
        # Strings/None: a change is any inequality (e.g. lufs null ->
        # measured). Bools are structural, compared by the caller.
        return base != fresh
    if not isinstance(fresh, (int, float)):
        return True
    tolerance = rule.get("abs_tol", 0.0) + rule.get("rel_tol", 0.0) * abs(base)
    return abs(fresh - base) > tolerance


def classify(rule: dict, base: object, fresh: object) -> int:
    """The class of one changed metric (class 3 only in its direction)."""
    if rule.get("class") == 3 and isinstance(base, (int, float)) \
            and isinstance(fresh, (int, float)) and not isinstance(base, bool):
        direction = rule.get("direction", "down")
        if (direction == "down" and fresh < base) or \
                (direction == "up" and fresh > base):
            return 3
        # Moved against the improvement direction: an expected-change
        # question, not an improvement.
        return 4
    return int(rule.get("class", 4))


def correctness_checks(fresh: dict, base: dict) -> list[str]:
    """Class-1 predicates; returns the list of violations (empty = ok)."""
    failures: list[str] = []
    for key in sorted(set(base) - set(fresh)):
        failures.append(f"missing render: {key[0]}/{key[1]} seed {key[2]}")
    for key, render in sorted(fresh.items()):
        label = f"{key[0]}/{key[1]}[{key[2]}]"
        for path, value in numeric_leaves("", render["metrics"]):
            if isinstance(value, float) and not math.isfinite(value):
                failures.append(f"{label}: {path} is {value}")
        peak = render["metrics"].get("peak_dbfs")
        if isinstance(peak, (int, float)) and peak > CLIP_DBFS:
            failures.append(f"{label}: clipping, peak {peak} dBFS")
        if key in base:
            base_render = base[key]
            base_peak = base_render["metrics"].get("peak_dbfs")
            if isinstance(peak, (int, float)) and isinstance(base_peak, (int, float)) \
                    and peak > base_peak + CEILING_MARGIN_DB:
                failures.append(
                    f"{label}: peak {peak} dBFS breaks the 0.1.5 ceiling "
                    f"(baseline worst {base_peak:.1f} + {CEILING_MARGIN_DB})")
            for prop in ("duration_s", "sample_rate", "channels"):
                b = base_render["metrics"].get(prop)
                f = render["metrics"].get(prop)
                if isinstance(b, (int, float)) and isinstance(f, (int, float)):
                    if abs(f - b) > DURATION_TOLERANCE * max(abs(b), 1.0):
                        failures.append(f"{label}: {prop} {b} -> {f}")
    return failures


def compare_renders(fresh: dict, base: dict, class_map: dict) -> list[dict]:
    """One row per changed metric, with its class and values."""
    rows: list[dict] = []
    for key in sorted(set(fresh) & set(base)):
        base_metrics = base[key]["metrics"]
        fresh_metrics = fresh[key]["metrics"]
        for path, base_value in numeric_leaves("", base_metrics):
            if path not in class_map:
                continue
            fresh_value = fresh_metrics
            for part in path.split("."):
                if not isinstance(fresh_value, dict):
                    break
                fresh_value = fresh_value.get(part)
            rule = class_map[path]
            if changed(base_value, fresh_value, rule):
                rows.append({
                    "render": f"{key[0]}/{key[1]}[{key[2]}]",
                    "metric": path,
                    "class": classify(rule, base_value, fresh_value),
                    "base": base_value,
                    "fresh": fresh_value,
                })
    return rows


def resolve_blocking(args, rows: list[dict]) -> tuple[list[dict], str | None]:
    """The rows that block the PR, and the resolved class-4 diagnosis.

    Class 2 blocks without --justified; class 4 blocks without the
    mandatory one-paragraph diagnosis.
    """
    blocking = [r for r in rows if r["class"] == 2 and not args.justified]
    needs_diagnosis = [r for r in rows if r["class"] == 4]
    diagnosis = args.expected_change
    if args.expected_change_file:
        # The PR mechanism: benchmarks/EXPECTED-CHANGE.md is added by the
        # author only when their change moves metrics; absent file means
        # "no diagnosis supplied", not an error.
        path = Path(args.expected_change_file)
        if path.exists():
            diagnosis = path.read_text().strip()
    if needs_diagnosis and not diagnosis:
        blocking = blocking + [{"render": r["render"], "metric": r["metric"],
                                "class": 4, "base": r["base"], "fresh": r["fresh"]}
                               for r in needs_diagnosis]
    return blocking, diagnosis


def report_lines(header: list[str], rows: list[dict], failures: list[str],
                 bit_changes: list[str], diagnosis: str | None,
                 justified: str | None) -> list[str]:
    """The human-readable comparison report: `header`, then the detail."""
    lines = list(header)
    if diagnosis:
        lines.append("expected-change diagnosis: " + diagnosis)
    if justified:
        lines.append("class-2 justification (interim, Test A/B still owed): "
                     + justified)
    for row in sorted(rows, key=lambda r: (r["class"], r["render"], r["metric"])):
        lines.append(f"  class-{row['class']} {row['render']}: {row['metric']} "
                     f"{row['base']} -> {row['fresh']}")
    for failure in failures:
        lines.append(f"  class-1 {failure}")
    if bit_changes:
        lines.append("  informational (not gated): sha256 changed for " +
                     ", ".join(bit_changes[:10]) +
                     ("..." if len(bit_changes) > 10 else ""))
    return lines


def main() -> int:
    here = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--fresh", required=True,
                        help="capture from baseline_render.py (the PR's code)")
    parser.add_argument("--baseline", default=str(here / "baseline-0.1.5.json"),
                        help="committed baseline (default: benchmarks/baseline-0.1.5.json)")
    parser.add_argument("--expected-change", default=None,
                        help="mandatory one-paragraph diagnosis for class-4 changes")
    parser.add_argument("--expected-change-file", default=None,
                        help="read the class-4 diagnosis from this file")
    parser.add_argument("--justified", default=None,
                        help="justification paragraph for class-2 changes (interim)")
    parser.add_argument("--class-map", default=None,
                        help="JSON file overriding entries of the built-in class map")
    parser.add_argument("--report", default=None,
                        help="write the full comparison here (default: <fresh>.comparison.txt)")
    args = parser.parse_args()

    class_map = dict(DEFAULT_CLASS_MAP)
    if args.class_map:
        class_map.update(json.loads(Path(args.class_map).read_text()))

    fresh_doc = json.loads(Path(args.fresh).read_text())
    provenance = fresh_doc.get("provenance", {})
    # startswith, not substring: "not verified" must not pass.
    if not str(provenance.get("determinism", "")).startswith("verified"):
        sys.stderr.write(
            "class-1: the fresh capture did not prove same-machine "
            "determinism (run baseline_render.py, which captures twice "
            "and byte-compares)\n")
        return 1

    fresh = load_capture(Path(args.fresh))
    base = load_capture(Path(args.baseline))
    failures = correctness_checks(fresh, base)
    rows = compare_renders(fresh, base, class_map)

    # Audio bytes that differ while every metric holds still: recorded,
    # not gated — cross-machine libm can shift a sample by 1 ulp and
    # flip the digest; metric tolerances are the gate.
    bit_changes = [f"{key[0]}/{key[1]}[{key[2]}]" for key in sorted(fresh)
                   if key in base and fresh[key]["sha256"] != base[key]["sha256"]]

    blocking, diagnosis = resolve_blocking(args, rows)

    header = [f"baseline: {args.baseline}",
              f"fresh:    {args.fresh} "
              f"(code {provenance.get('code_hash', '?')[:12]}, "
              f"qinari {provenance.get('qinari_version', '?')})",
              f"renders compared: {len(set(fresh) & set(base))} "
              f"(baseline {len(base)}, fresh {len(fresh)})",
              f"changed metrics: {len(rows)} " +
              "(by class: " + ", ".join(f"{c} -> {sum(1 for r in rows if r['class'] == c)}"
                                        for c in (1, 2, 3, 4)) + ")",
              f"audio bytes changed: {len(bit_changes)} renders"]
    lines = report_lines(header, rows, failures, bit_changes, diagnosis,
                         args.justified)
    report = Path(args.report) if args.report else Path(args.fresh).with_suffix(
        "").with_name(Path(args.fresh).stem + ".comparison.txt")
    report.write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"report: {report}")

    if failures:
        print("BLOCK: class-1 correctness violations are not overridable (§9).")
        return 1
    if blocking:
        print("BLOCK: class-2 change without --justified, or class-4 change "
              "without the mandatory --expected-change/-file diagnosis (§9).")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
