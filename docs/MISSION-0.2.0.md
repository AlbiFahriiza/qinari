# MISSION — Qinari 0.2.0 (Physical Source & DSP Core)

Status: active build mission. Owner instruction of 2026-11-08: execute
Fase A (Engineering Foundation) in full first, then Fase B and Fase C,
per the master engineering plan `docs/PLAN-0.2.0.md` (v2), which is the
parent document of this mission and supersedes the v1 plan for every
decision it revises. The plan's dependency graph (§7.2), phase gates
(§7.4), recalibrated criteria (§8), baseline policy (§9), production
contract (§10) and Go/No-Go checklist (§11) are binding for this
mission. Task numbers are not execution order; the graph is.

## 1. Scope

### Must land (release blockers)

- **Fase A — P0-1..P0-8 (engineering foundation).**
  - **P0-1** ModalBank rewritten as an exact damped-sinusoid bank:
    excitation projected onto every mode (convolution with the mode
    impulse response), strike-position weighting, global-gain-only
    normalization (relative modal gains preserved), 100+ modes per
    bank, allocation once per render. T60 measured by linear regression
    of the dB envelope; relative error ≤ 10% per mode, ≥ 95% of modes.
  - **P0-2** Verifier with true C/C1/C2: cochlear filterbank,
    modulation filterbanks, C1 cross-channel per modulation band, C2
    complex correlation via analytic-signal frequency doubling, AC
    labeled `AC (Qinari extension)`. Every filterbank parameter
    re-derived from the primary source [S1] and recorded with
    page-level citations in `docs/VERIFICATION-0.2.0.md` before the
    implementation freezes.
  - **P0-3** Stereo-aware analysis: per-channel features plus
    cross-channel features; mono mixdown only for monophonic-by-
    definition features (speech); backward-compatible report.
  - **P0-4** CC0 golden corpus: 10–20 recordings per priority
    category, 3-way split without source leakage, quantile statistics
    (median, P10, P90, mean, sd, CI), full provenance (URL, license,
    access date, duration, sample rate, SHA-256, subset); `qinari
    verify <render> --golden <category>` subcommand; deterministic in
    CI.
  - **P0-5** Permanent regression test: material distance > 3x seed
    distance (4 materials × 6 seeds, render level).
  - **P0-6** Sample-rate invariance: `NoiseGen::fractal` redesigned
    from Hz corners at the actual sample rate; workspace audit for
    hidden rate constants; corners within ±2% at 44.1/48/96 kHz.
  - **P0-7** Documentation and manifest synchronization: GOALS.md
    rewritten for 0.2.0, header comment fixes (Poisson→Cox, 29→30
    recipes), CHANGELOG corrections for ocean/AM claims, graph/refine
    sections marked RFC in DESIGN-REALISM-ENGINE.md; no existing claims
    for features that are absent.
  - **P0-8** Locked 0.1.5 regression baseline: fixed-seed catalog
    renders (30 recipes + 5 ambient × 2 seeds), layer A–B metrics
    committed as JSON, CI comparison with the 4-class policy (§9), the
    three known 0.1.5 regressions recorded as open class-4 issues.
- **Fase B — P1-1..P1-7 + P2-5.** Object Acoustic Profiles per object
  (P1-2) before collision calibration (P1-1); 3 spatialization systems
  (P1-3); source-path split (P1-4); rain as DropletEvent × SurfaceModel
  with Minnaert only on the bubble path (P1-5); 3-layer explosion
  (P1-6); outdoor tail + minimal scene descriptor (P1-7); mastering
  macro-dynamics (P2-5).
- **Fase C — C-1..C-7.** Performance contract with RC-locked numbers;
  runtime safety (offline contract, validation, Result errors);
  fuzz + stress; compatibility (schema_version, migration guide,
  0.1.5 CLI surface intact); cross-platform CI matrix; distribution +
  supply chain; operational documentation (PERFORMANCE.md,
  LIMITATIONS.md).
- **RELEASE.** Workspace version 0.2.0, CHANGELOG, README/CLI/
  ARCHITECTURE/BENCHMARKS updates, tag `v0.2.0` only after Go/No-Go
  9/9 (§11) with committed evidence; adversarial verification pass
  against every gate before the tag.

### Should (valuable, not blocking)

- Corpus growth beyond the six priority categories once their gates
  are green, following the variance-based sizing rule (§6.1).
- Additional impact surface categories in the golden corpus beyond
  the priority set, with the same provenance standard.

### Stretch (lands only after every must is green)

- **GRAPH-1** `qinari-graph` TOML recipes — RFC 0.2.x (D5).
- **REFINE-1** `qinari refine` — RFC 0.3.x; in-house deterministic
  optimizer first (P4-1); no external optimizer dependency without
  owner approval (D7).
- 96 kHz high-rate path (D6) — decision required before RC.

### Out of scope (stand per plan §1.2 and §7.9)

Scene graph, atmospheric ISO propagation filters, geometric reverb,
scene occlusion, full Doppler, the natural source families (0.2.x and
0.3.0), automated parameter refinement, formal MUSHRA harness. No new
dependency of any kind without registry verification and owner
approval (§10.F).

## 2. Team and checkpoint plan

Four subagents (cap), each reading the Panoptes 2.2 skill
(`/home/z/my-project/upload/skill-extracted/disciplined-coding/SKILL.md`)
and `CHECKPOINT_RULES.md` before touching code. Selections and
difficulty/fit are recorded in `.disciplined/delegation.json`; every
subagent's output is gated (check_diff, workspace gates) before merge
and no subagent's claim stands in for evidence. The main agent runs
the workspace gates and owns every commit and push; subagents never
commit. Waves keep parallel work on disjoint files, per the dependency
graph (§7.2); the 0.1.5 baseline (P0-8) is captured from clean HEAD
before any code changes land.

| Wave | Agents | Tracks (disjoint files) |
|---|---|---|
| 1 | S1 P0-2 verifier ∥ S2 P0-1 modal ∥ S3 P0-6 invariance + P0-8 baseline ∥ S4 P0-7 docs + corpus acquisition | qinari-verify ∥ qinari-dsp modal + sfx recipes ∥ qinari-dsp noise + benchmarks/CI ∥ docs/ + manifest |
| 2 | S1 P0-3 stereo ∥ S2 P0-5 ratio test ∥ S4 P0-4 statistics | qinari-verify ∥ qinari-sfx tests ∥ qinari-verify + cli |
| 3 | main agent: integration + release | all |
| 4 | adversarial verification (fresh gates, black-box CLI walk) | all |

Checkpoint = workspace gates green (`cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test
--workspace` 0 failures, `cargo deny check` 0 errors, `cargo bench
--workspace --no-run` 0 warnings) + push to `origin/main`, per
CHECKPOINT_RULES.md. One milestone = one commit, message names the
task code (P0-1, P0-2, …).

## 3. Acceptance evidence (per plan §7.4, §8, §11)

- Fase A exit: §7.4 gates 1–6 green and
  `docs/VERIFICATION-0.2.0.md` committed; T60 relative error ≤ 10% on
  ≥ 95% of modes for all materials; material/seed spectral distance
  ratio > 3 (6 seeds, 4 materials); determinism bit-identical at 44.1
  and 48 kHz; fractal corners within ±2% across sample rates and
  spectra within 0.5 dB (20–10240 Hz); `grep C1` across the repo hits
  only the new definitions; synthetic two-channel signals reproduce
  analytic C/C1/C2 within 0.02; C, C1, C2 stable across ≥ 3 corpus
  recordings per category (CV < 0.3); golden JSON committed for ≥ 6
  categories with full provenance; `qinari verify` deterministic in
  CI; 0.1.5 renders documented against the expected gates (initial
  gap statement).
- Baseline (P0-8): PRs that regress class-1 correctness metrics
  without justification block merge; class-4 expected changes carry a
  one-paragraph diagnosis.
- Release: Go/No-Go 9/9 with committed evidence (CI logs, benchmark
  files, listening reports, audit output) before tag `v0.2.0`;
  features not yet passing their gates stay labeled `experimental`.
