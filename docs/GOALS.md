# Qinari Goals — v0.2.0 release gates

These are the release gates for v0.2.0 (Physical Source & DSP Core). The
release does not ship until every gate is verified with committed
evidence — a CI log, a benchmark file, a test report, or an audit
output, never a statement. The parent document is
`docs/PLAN-0.2.0.md` (v2): this file tracks the same gates as the
plan's §7.4 (phase gates), §8 (recalibrated criteria), and §11 (Go/No-Go
checklist); where they disagree, the plan wins.

Status markers are honest about what exists:

- `open` — not verified yet; the phase that will close it is named.
- `in verification` — work merged, evidence not yet committed.
- `verified` — evidence committed and cited in the row.

A gate without recorded evidence is `open` by definition. Nothing here
is marked by promise, and no category threshold may be quoted as a
target before it is born from the committed golden corpus (plan §8
reading rules, below).

## Release gates (plan §11 — 9 of 9 before the `v0.2.0` tag)

| # | Gate | Accepted evidence | Status |
|---|------|--------------------|--------|
| G1 | Correctness | T60 per mode within the plan-§8 criteria (relative error ≤ 10% on ≥ 95% of modes, measured by linear regression of the dB envelope); excitation projected onto every mode; 0 NaN/Inf; 0 uncontrolled clipping; sample-rate invariance; seed determinism proven on supported configurations | open — Fase A (P0-1, P0-6) |
| G2 | Perceptual | every category claimed stable passes its golden gates (layers B/C) and listening Test A/B; Test C real-vs-synthetic on the hold-out measured and recorded | open — Fase B for the source work; listening tests per plan §6.4 |
| G3 | Determinism | seed reproducibility proven at 44.1/48 kHz for the full catalog; bit-identical | open — holds on the 0.1.5 baseline (byte-compare tests); must be re-proven on the 0.2.0 catalog |
| G4 | Performance | RTF, CPU per voice, memory, and concurrent load meet `docs/PERFORMANCE.md` (numbers locked at RC) on the reference hardware (see docs/BENCHMARKS.md) | open — Fase C (C-1); PERFORMANCE.md does not exist yet |
| G5 | Reliability | 10k-event stress, 10-minute renders, parser fuzz, malformed input, recovery and error handling — all with regression tests | open — Fase C (C-3) |
| G6 | Compatibility | public API, profile schema (`schema_version` + migrator), the 0.1.5 CLI surface still works, 0.1.5 → 0.2.0 migration guide published | open — Fase C (C-4) |
| G7 | Licensing | corpus provenance complete (URL, license, access date, SHA-256, split); dependency license audit green (cargo-deny); no NC-licensed asset on the product path | open — Fase A (P0-4): acquisition done (104 CC0 files, 8 categories, provenance manifest committed); statistics extraction later in Fase A; cargo-deny re-run at RC |
| G8 | Distribution | cross-OS CI matrix green; release binaries + verified checksums; CHANGELOG and release notes complete; `Cargo.lock` locked | open — Fase C (C-5, C-6) |
| G9 | Release review | no open P0/P1 blockers (including the completed P0-2 re-derivation); every feature labeled stable has acceptance evidence; features not yet passing their gates stay `experimental` | open — continuous |

Rule: nine of nine before the tag. Sign-off is not granted because
"Fase A and B are done" — only because the gates above are proven
(plan §11).

## Phase exit gates (plan §7.4 — all green before the next phase starts)

1. `cargo fmt --check`
2. `cargo clippy --workspace -- -D warnings`
3. `cargo test --workspace`, including the new render tests
4. Layer A–B–C regression benchmarks do not worsen against the locked
   0.1.5 baseline (P0-8) under the four-class policy of plan §9 — CI
   classifies each change, it does not blind-block
5. Listening Test A/B passes for every tuning change
6. One atomic commit per task; the message names the task code
   (P0-1, P1-3, …)

## Recalibrated completion criteria (plan §8, in force)

| Task | Criterion in force (v2 — supersedes the v1 numbers) |
|------|------------------------------------------------------|
| P0-1 modal bank | T60 relative error ≤ 10% per mode (a time quantity; the v1 "< 1 dB" was a unit error); slope per mode from linear regression of the dB envelope on the `0..t60` window with an explicit noise floor; ≥ 95% of modes conform; the envelope fit error is reported separately in dB |
| P0-1 separation | spectral distance between materials > 3× the distance between seeds (4 materials × 6 seeds) |
| P0-2 verifier | synthetic two-channel signals reproduce analytic C/C1/C2 within 0.02; C/C1/C2 stable across ≥ 3 corpus recordings per category (CV < 0.3); every filterbank parameter carries a primary-source citation in `docs/VERIFICATION-0.2.0.md` before implementation freezes |
| P0-3 stereo | per-channel features detect asymmetric L/R test signals; the report keeps the old fields as aggregates (backward compatible) |
| P0-4 corpus | golden JSON stores quantiles (median, P10, P90, mean, sd, CI) — min–max only as a supplement; 3-way split with no source leakage; ≥ 6 categories with full provenance |
| P0-6 invariance | fractal corners stay within ±2% at 44.1/48/96 kHz; spectra within 0.5 dB over 20–10240 Hz across rates |

Reading rules (plan §8):

1. Every category threshold (IACC, flatness, class ratios, modulation
   spectra) must be born from the committed golden corpus; example
   numbers from the plan may not be copied into tests.
2. If corpus measurement shows the 10% T60 bound is too strict for
   noisy weak modes, the bound is revised in the plan with data — never
   silently in a test.

## What exists today (committed 0.1.5 baseline, audit-verified)

Honest inventory at the start of 0.2.0 (plan §1c, verified on HEAD):

- Workspace: 7 crates (dsp, audio, sfx, music, tts, verify, cli). No
  `qinari-graph` crate — it is RFC 0.2.x (plan P2-1); no `refine`
  subcommand — it is RFC 0.3.x (plan P4-1). Neither exists in the
  workspace manifest or the CLI.
- CLI: 13 subcommands; no `verify` yet. `qinari verify` is planned for
  0.2.0 (P0-4); until it lands with deterministic CI behavior it is not
  claimed anywhere.
- Engine: 30 recipes, 5 ambient presets, 5 music styles; 486 `#[test]`
  attributes on the 0.1.5 baseline; determinism byte-compared in tests.
- Golden corpus: acquisition complete (P0-4 part 1) — 104 CC0
  recordings over 8 categories (rain, wind, fire, stream, ocean,
  thunder, impact, footsteps), stored outside the repository, with the
  provenance manifest
  `crates/qinari-verify/tests/golden/corpus-manifest.json`
  (per-file URL, license, access date, duration, sample rate, channels,
  SHA-256, microphone context, 3-way split by uploader with no source
  leakage). Statistics extraction, golden statistics JSON, and the
  `verify` subcommand land later in Fase A; no golden statistics JSON
  exists yet.
- Known 0.1.5 defects carried as open findings (plan §3): the ocean
  swell is still periodic (`fast_sine(0.055·t)`, F9); three more
  periodic AM lines survive (engine flutter, forcefield throb, siren
  sweep — F20); C1/C2 are not implemented as defined (F13); 25 of 30
  recipes render fully dry (F10).

## Non-goals for 0.2.0 (plan §1.2)

No scene graph, no ISO atmospheric propagation filters, no geometric
room reverb, no scene occlusion, no full Doppler, not the complete set
of natural source families, no automated parameter refinement, no
formal ABX/MUSHRA harness. Those are 0.2.x and 0.3.x work; the release
name is only as wide as its evidence.

## Status legend

`open` / `in verification` / `verified` with cited evidence. This table
is updated at checkpoint time; a gate without recorded evidence is
`open` by definition.
