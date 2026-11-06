# DESIGN — Realism Engine (Qinari 0.1.0 / 0.2.0)

Status: proposed architecture, evidence-verified. Every load-bearing claim
cites either `RESEARCH-AUDIO-REALISM.md` (§R) — where the evidence, the
experiment, or the source lives — or a `file:line` in this repository.
Written after the mandated 60-minute design window; premises were
re-checked against code and measurements during that window (see §12).

## 1. Problem statement (what "hardened" must fix)

Four verified defects define the gap between Qinari 0.0.5-alpha and a
realism engine:

1. **TTS prosody is broken by our sequencer.** `map_symbols` omits
   Piper's PAD interleaving; the same sentence synthesizes 63–70% too
   fast on the Indonesian voice and 33–38% on the English voice (pad-probe
   experiment, §R 3.2). Both bundled voices sound "aneh" for this reason
   first — the models are secondary (§R 3.3: Piper's own sample shares
   the id voice's ~1.6 kHz bandwidth ceiling; that part is not our bug).
2. **SFX are statistically synthetic.** 14 structural tells (§R 1.2):
   independent exactly-periodic LFOs, memoryless Poisson event fields,
   independent-uniform parameter draws, interaural correlation 1.0 beds,
   zero-attack one-shots, anechoic events, no blast negative phase —
   each violates the statistics listeners use to judge naturalness
   (§R 1.1: cross-channel correlation + envelope autocorrelation are the
   decisive ones).
3. **The engine is blind.** Analysis is Goertzel + ZCR (`analysis.rs:9-57`);
   nothing measures spectra, envelopes, modulation, or event statistics,
   so every recipe is tuned blind — the exact advantage a "tuned Python
   pipeline" holds (§R 1.3).
4. **The MSRV promise is fiction.** `rust-version = "1.85"` but locked
   rubato 5.0.1 requires 1.87 in its manifest; no CI job tests MSRV
   (§R 4.5).

## 2. Non-negotiable constraints

- **Determinism**: `(recipe params, seed) → byte-identical file` (G11
  today; every stage below preserves it — all new randomness flows
  through tagged `Qrng` streams; optimizers run at authoring time only
  and write found values as data).
- **`#![forbid(unsafe_code)]`** in every Qinari-owned crate (dependency
  crates are outside the lint's scope — Cargo book semantics, §R 4.4).
- **GPL-3.0-or-later**, offline-first, `curl` the only runtime download
  path, MSRV declared truthfully and CI-enforced.
- **Performance budget**: today's gates hold (heaviest 10 s recipe
  43 ms; 60 s storm 693 ms; 30 s music 272 ms — `docs/BENCHMARKS.md`).
  New per-sample DSP must stay O(1)/sample; verification runs in CI and
  authoring, never in the render path.
- **CLI compatibility**: existing subcommands and flags keep working;
  new capability arrives as new subcommands and additive params.

## 3. Architecture overview

Target crate graph (0.2.0 end-state; staged in §11) — layers list their
true dependencies:

```
qinari-dsp      primitives (osc, filters, noise, env, fx, rng, lufs, …)
                → depends on: nothing but std/thiserror

qinari-graph    NEW: scene graph — events → layers → tracks (buses),
                declarative variation (playlists, scatterers),
                TOML recipe schema, statistical target blocks
                → depends on: dsp

qinari-verify   NEW: feature extraction (STFT via realfft), distances
                (LSD, multi-res STFT, texture statistics), golden
                regression targets, refine optimizers
                → depends on: dsp (+ audio, decode-only, for file input)

qinari-sfx      recipes (data-driven, migrated from fn tables)
qinari-music    styles (unchanged interface)     → both depend on: dsp, graph
qinari-tts      VITS engine (PAD fix, text normalization, per-sentence)
                                                    → depends on: dsp
qinari-audio    resample → master → dither → encode (unchanged path)
                                                    → depends on: dsp
qinari-cli      surface: existing + `analyze` + `verify` + `refine`
                                                    → depends on: all above
```

Dependency direction stays one-way and DSP stays I/O-free. The closed
loop that defines the engine:

```
author (TOML recipe + target statistics or reference file)
   → render (pure, seeded, fast)
   → analyze (features: spectrum, band envelopes, modulation, event rate)
   → distance vs target
   → refine (Nelder-Mead ≤10 params / CMA-ES 10–40 params, seeded)
   → write params back into the recipe   → shipped renders stay pure
```

This generalizes the pattern already proven in `master.rs:148-174`
(measure LUFS → correct → bounded loop) from one scalar to a feature
vector, and matches the FMOD-endorsed offline-baking niche (§R 4.1).

## 4. Stage TTS-1 — fix the sequencer (0.1.0, first change)

**Change.** `map_symbols` (`crates/qinari-tts/src/local.rs:282-293`)
interleaves PAD exactly as Piper does: PAD id after `^` and after every
mapped symbol; `$` unpadded (verified against `piper-phonemize
src/phoneme_ids.cpp`, §R 3.2). The unit test that currently locks the
unpadded shape (`local.rs:340`) is updated to the padded shape
(`^ _ a _ ␣ _ b _ $` → `[1,0,5,0,3,0,6,0,2]` with the test map). The
"matching Piper" doc comment becomes true.

**Effect (measured in advance).** The failing sentence goes from
1.83–1.89 s to 3.09–3.12 s (id voice) — natural pace instead of rushed
(§R 3.2). Live tests are floors and relative comparisons only
(`tests/live.rs:110,175`) — unaffected; RTF stays far inside budget
(current 0.05–0.33 against a < 1.5 gate, even though wall time also
rises with the longer id sequence).

**Same stage, smaller items.**
- Filter espeak `(lang)` switch markers before symbol mapping (upstream
  behavior; §R 3.4-1) with a unit test using a switch-triggering phrase.
- Text normalization hooks before phonemization: acronym expansion
  (`GLM` → `ge el em` is espeak's letter-name fallback — acceptable once
  prosody is fixed, but a `--text-mode raw|normalized` choice and a
  Piper-style `[[ … ]]` raw-phoneme escape give authors control
  (§R 3.3 phonemization, §R 3.5 checklist).
- Document the id voice ceiling honestly in `docs/TTS.md`: it is the
  only Indonesian voice in the catalog (§R 3.3); no upgrade exists
  today; muffling below ~1.6 kHz rolloff95 is the voice, not Qinari.

**Acceptance.** Padded sequence shape locked by unit test; pad-probe
numbers reproduced post-fix (unpadded path no longer reachable);
sentence duration for the reference sentence ∈ [2.8, 3.4] s; existing
suite green; `qinari doctor` healthy.

## 5. Stage INFRA-1 — make the MSRV true (0.1.0)

Decision: **raise MSRV to 1.87** and pin nothing. Rationale: rubato
5.0.0 and 5.0.1 both require 1.87 (§R 4.5); downgrading to 4.2.1 (no
declared MSRV) loses the 5.x sinc-resampler line the mastering chain
already validated, to serve a toolchain margin only; 1.87 remains
conservative for Debian-13-era distributions. Add a CI job building the
workspace with `1.87` exactly (the MSRV claim becomes tested, not
declared), and a line in `docs/NON-RUST-DEPENDENCIES.md`.

## 6. Stage VERIFY-1 — the engine grows eyes (0.1.0)

New crate `qinari-verify`, deps: `qinari-dsp` + `realfft` 3.5.1 (already
in tree via rubato; MSRV 1.61 ≤ 1.87 — §R 4.4).

Features (all pure functions over `AudioBuffer`):
- `spectrum`: Welch power spectrum, `rolloff95`, band energies.
- `band_envelopes`: N-band (≈24) Hilbert-ish envelopes via STFT
  magnitudes; moments (mean, variance, sparsity/kurtosis).
- `modulation_spectrum`: envelope spectra per band + cross-band
  correlation matrix (the McDermott C1/AC statistics — §R 1.1).
- `event_stats`: onset detection by envelope derivative (pure Rust,
  ~50 lines — aubio rejected, §R 4.4): rate, inter-onset CV, burstiness.
- `loudness`: reuse `qinari-dsp::lufs`.

Distances: log-spectral distance (LSD), multi-resolution STFT loss
(Yamamoto-style, windows 512/1024/2048 — §R 4.3), texture-statistic
distance (weighted: spectrum, envelope moments, modulation, event
stats).

Surfaces:
- `qinari analyze <input.wav> --json` — feature report.
- `qinari verify sfx <recipe>` — renders the seeded golden seed set and
  checks stored targets (tolerances per feature).
- CI gate: for every recipe, golden targets (committed as
  `tests/golden/<recipe>.json`, generated once by `qinari analyze` and
  reviewed) must hold within tolerance; plus the existing null-test/
  byte-determinism and LUFS gates.

Why golden files instead of reference recordings: Qinari ships no
recordings (license discipline); targets are authored statistics plus
regression deltas — the TexStat direction of using the statistic set
itself as the validation metric (§R 4.2).

**Acceptance.** Null test + LSD/multi-res STFT + texture stats run in CI
on the full recipe set; a deliberately-broken recipe (cutoff halved in a
scratch branch) fails the gate — the gate must be demonstrated to fire
before it is trusted.

## 7. Stage REALISM-1 — DSP for natural statistics (0.1.0)

All in `qinari-dsp`, each primitive unit-tested against its statistical
intent:

- **`LatentField`**: one slow stochastic process per render — white noise
  from a tagged `Qrng` stream → chained one-pole LPs (corner 0.05–0.5
  Hz) → affine map to a controlled range. **Calibration caveat found
  during the design window (numerical check, scripts/
  design_premise_checks.py): the infinite-limit analytic variance
  a/(2−a) does NOT predict the realized amplitude on 20–60 s renders
  at low corners (measured ratio 0.63–1.10× across fc 0.05–0.5 Hz) —
  the process has not reached steady state within the window. Design
  consequence: calibrate by two passes — draw the noise first, then
  normalize by its realized std over the actual render buffer before
  the affine map (deterministic, still a pure function of the seed);
  for renders ≤60 s, corners below ~0.1 Hz are avoided.** Optional 1/f
  tilt. This replaces per-layer sinusoid LFOs. Everything that used to
  breathe on its own oscillator (bed gain, cutoffs, gust depth, event
  rate) reads from the same field → cross-band correlation and
  broadband modulation exist by construction (§R 2.1).
- **Envelopes**: `Percussive` gains an attack ramp (0.2–5 ms) and a
  two-stage decay (τ₁ fast → τ₂ slow); new `Segment` envelope for
  multi-stage shapes (§R 2.2). `noise_burst` gains the same attack.
- **Event processes**: `poisson_events` keeps existing behavior (high-
  rate beds); new `renewal_events` (gamma intervals, CV parameter) and
  `cox_events` (rate = f(LatentField)) (§R 2.4). Each with statistical
  tests: measured inter-onset CV within tolerance of the parameter.
- **Noise palette**: `NoiseGen::fractal(alpha)` — 1/f^α between white
  and brown (single parameter; Voss-style or filtered construction
  chosen by test), plus band-limited Gaussian (§R 2.5).
- **Spatial**: `decorrelate` (independent HF noise per channel above a
  corner, correlated below — kills the interaural-1.0 tell, §R 1.2-5),
  `distance` (delay ∝ d, gain ∝ 1/d, one-pole HF rolloff ∝ d),
  `early_reflections` (1–3 taps using the existing `Delay`).

**Migration** (recipes, 0.1.0 scope — highest-impact beds first): rain,
wind, fire, stream, thunder, storm/meadow ambients switch to
`LatentField` + new envelopes + renewal/Cox events + bed decorrelation.
Each migrated recipe gets its golden targets re-authored and a
before/after `qinari analyze` table committed to `docs/BENCHMARKS.md` —
the anti-slop claim becomes numbers (envelope correlation, modulation
bandwidth, inter-onset CV), not adjectives.

**Measured baseline before migration (design-window check, 30 s rain,
seed `design-baseline`)**: interaural correlation 0.999 / 0.949 / 0.917
/ 0.985 in the 0–500 / 500–2000 / 2000–8000 / 8000–11025 Hz bands (all
fail the < 0.9-above-500 Hz target); bed modulation-spectrum peak/mean
32.7 with the peak at 0.067 Hz — the recipe's hard-coded 0.07 Hz gust
sinusoid (`recipes.rs:298`) is a measurable line spike, exactly the
signature §R 1.2-1 predicts. These are the "before" numbers the
migration must beat.

**Acceptance.** For each migrated recipe: no exactly-periodic gain
component (modulation-spectrum peak-to-mean ratio below a calibrated
threshold — measured separation during the design window: sinusoid AM
57.0 vs latent AM 9.5, a 6× gap; exact threshold calibrated in VERIFY-1
against the migrated set), bed interaural correlation < 0.9 above
500 Hz, event CV within tolerance, determinism byte-test still green,
perf within budget.

## 8. Stage PHYSICAL-1 — physics-shaped families (0.1.0: explosion +
thunder; 0.2.0: modal + birdsong v2)

**Explosion family** (§R 2.3, all shapes from verified sources):
1. Shock front: sub-ms broadband click (shaped noise, hard onset).
2. Friedlander body: envelope `P(t) = Ps·e^(−t/t*)·(1 − t/t*)` —
   includes the rarefaction (negative) phase; implemented as a bipolar
   envelope shaping a filtered-noise body. `t*` grows with `distance`.
3. Ground reflections: 2–4 taps at distance-scaled delays (Mach-stem
   sum) via `early_reflections`.
4. Air absorption: distance-dependent LP corner (superlinear in
   frequency — one-pole cascade approximates the standard curve; the
   exact ISO table is unverified, so the design pins only the
   qualitative law, §R 2.3).
5. Rumble: fractal noise bed (α ≈ 2), LP 30–300 Hz, onset locked to the
   body, tail driven by `LatentField`.
   `size` maps to Ps and low-frequency proportion; `distance` maps to
   t*, tap delays, and absorption. Both keep their current ranges so
   CLI calls do not change meaning, only fidelity.

**Thunder** = same family's far-field limit with a tortuosity
parameter: 4–8 crack arrivals over 1–5 s from a distributed source
(Roy 1980 model), clap energy < 300 Hz emphasized, each arrival
distance-attenuated (§R 2.3).

**Modal bank (0.2.0)**: `ModalBank { [(f_i, Q_i, t60_i)] }` over
existing biquads; material presets wood/metal/ceramic/thud; applied to
impact/hit/sword/footstep (§R 2.3).

**Birdsong v2 (0.2.0)**: source-filter notes (buzz/harmonic source +
2–3 swept SVF formants — Nowicki's helium result, §R 2.3); trill rate
× bandwidth drawn from a joint distribution honoring the Podos tradeoff
(no fast-and-wide songs); motifs repeat with micro-jitter (±3–8%
timing, ±1–3% pitch) through a small transition matrix; phrase field
becomes a Cox process on `LatentField`.

**Acceptance.** Explosion: negative-phase lobe visible in the waveform
regression target; ground taps at predicted delays; distance sweep
monotonically lowers HF corner. Thunder: ≥4 detected onsets spread
>1 s. Birdsong v2: no note violates the rate/bandwidth constraint
(enforced in test); motif repetitions differ (auto-correlation of
phrase audio < 1 within tolerance).

## 9. Stage GRAPH-1 — scenes as data (0.2.0)

New crate `qinari-graph`; `qinari-sfx` recipes migrate from `static
RECIPES: [Recipe; N]` fn tables (`recipes.rs:1901`) to TOML documents
compiled to a typed graph — FMOD's proven object model mapped offline
(§R 4.1):

```toml
[recipe.birdsong]
id = "birdsong"
duration_s = 20.0

[[recipe.birdsong.params]]       # stays CLI-settable via --set
name = "activity"
kind = "float"
default = 0.75
min = 0.0
max = 1.0
doc = "Phrase-field intensity"

[recipe.birdsong.source]          # source-filter note generator
kind = "birdsong-v2"
[recipe.birdsong.source.note]
rate_bandwidth = "podos"          # joint distribution, not two uniforms

[[recipe.birdsong.layers]]
kind = "event-field"
process = "cox"                   # rate driven by the latent below
intensity = 1.0
spawn = { min_s = 1.2, max_s = 6.0 }     # scatterer semantics
variation = { pitch_st = 1.5, gain_db = 3.0 }  # per-instance ranges

[[recipe.birdsong.layers]]
kind = "bed"
noise = { kind = "fractal", alpha = 1.7, lp_hz = 8000 }

[recipe.birdsong.latent]
corner_hz = 0.2
depth = 0.35

[recipe.birdsong.target]          # verified by qinari verify
rolloff95_hz = { min = 5000, max = 9500 }
event_cv = { min = 0.4, max = 1.6 }
```

- Selection semantics: `shuffle` (no immediate repeat, FMOD default),
  `randomize`, `sequential` — implemented over tagged `Qrng`.
- The fn-pointer registry remains during migration (both load paths
  valid); recipes migrate one per PR with golden targets re-authored.
- CLI: `--recipe-file path.toml` additive flag; built-ins resolve from
  an embedded include of the same TOML (one source of truth).

## 10. Stage REFINE-1 — authoring-time optimization (0.2.0)

`qinari refine sfx <recipe> --target <ref.wav|target.toml> --params
a,b,c --seed x --budget 300`:
- renders in-memory (43 ms/10 s measured → 300 renders ≈ 13 s, §R 4.2),
- Nelder-Mead (`argmin` 0.11.0) for ≤10 params; CMA-ES (`cmaes` 0.2.2,
  seeded — determinism of the search recorded in the output metadata:
  optimizer, version, seed, budget) for 10–40,
- prints the found values and writes them into the recipe TOML on
  confirmation; shipped renders never run the optimizer.
- Overfitting guard (the FAD literature's own warning, §R 4.3): refine
  against multi-feature distance, never a single scalar; human listening
  stays a release step (§11 protocol).

## 11. Verification strategy and release protocol

CI (every PR): fmt, clippy `-D warnings`, tests, **MSRV 1.87 build**,
golden verification (§6), byte-determinism, LUFS/ceiling invariants,
perf budget benchmarks (existing gates).

Release (manual, local): FAD audit optional (out-of-CI — needs
embeddings, §R 4.3); **listening protocol** — the owner listens to the
golden seed set per recipe family through the A/B harness
(`/download/pad_probe_*.wav` style pairs), because no metric replaces
ears for the final word; loudness-matched comparisons only (§R 3.5-7).

## 12. Design-window verification log (what was re-checked while designing)

1. `map_symbols` and its test re-read post-research; fix shape matches
   `phoneme_ids.cpp` exactly (pad after BOS and each symbol, EOS bare).
2. `tests/live.rs` assertions re-read: floors and ratios only — the
   duration increase from the PAD fix cannot break them; RTF gate
   re-checked (< 1.5 vs current 0.05–0.33 — ample headroom even though
   wall time also rises with the longer id sequence).
3. `docs/BENCHMARKS.md` numbers re-read to set the perf budget lines.
   Three design premises verified numerically (scripts/
   design_premise_checks.py): the Friedlander negative lobe is exactly
   −exp(−2) ≈ −0.135·Ps at t = 2t* (design constant confirmed); the
   modulation-spectrum discriminator separates current-pattern AM from
   latent AM by 6× (57.0 vs 9.5 peak/mean) — the gate is measurable;
   and the LatentField analytic-variance assumption FAILED at low
   corners on finite windows (ratio 0.63–1.10) — design amended to
   two-pass realized-std calibration (§7), which is exactly the class
   of flaw the design window exists to catch.
4. MSRV options re-checked against the two rubato tags (both 1.87) —
   decision recorded with rationale (§5).
5. Explosion/thunder/birdsong parameter names re-checked against
   `recipes.rs` current specs so ranges stay CLI-compatible.
6. Every §R citation re-opened where load-bearing; three agent-sourced
   claims were rejected or downgraded during verification (the
   nonexistent "Parker 2024"; the crates.io `rust_version` null vs
   manifest truth; Wwise specifics — kept out of this design entirely
   because the docs were unreachable, §R 4.1).
7. The §6 gate principle demonstrated on the current binary: the same
   seed with two params pushed to extremes (`distance=1.0,
   intensity=0.15` vs defaults) moves rolloff95 5965→4942 Hz, shifts
   0–500 Hz band energy 11.1%→43.3%, and measures **LSD 13.0 dB**
   against the golden render — far above a 6 dB regression threshold.
   The metric family the verifier will use discriminates a broken
   recipe today, before the crate exists.

## 13. Roadmap and scope split

**0.1.0 — "Hearing and Honest"**: TTS-1 (§4), INFRA-1 (§5), VERIFY-1
(§6), REALISM-1 (§7), PHYSICAL-1 explosion + thunder (§8). Exit
criteria: the failing sentence renders at natural pace; every migrated
recipe has committed golden targets and a before/after statistics table;
MSRV job green on 1.87; all existing gates green.

**0.2.0 — "Composed and Tuned"**: GRAPH-1 (§9), REFINE-1 (§10), modal
bank + birdsong v2 (§8), full recipe migration. Exit criteria: all
built-in recipes are TOML; `qinari refine` demonstrably closes ≥50% of
the texture-stat distance to a reference for one bed recipe (target
measured before/after in the release notes).

Risks and mitigations: metric overfitting (multi-feature + ears, §10);
optimizer non-determinism (seeded + recorded, §10); recipe migration
regressions (golden gates must fail first on a scratch branch, §6);
latent-field calibration drift (analytic steady-state + unit tests,
§7); perf creep (budget benchmarks stay CI gates, §11).

## 14. Non-goals

No neural SFX synthesis, no voice training or fine-tuning, no real-time
rendering, no GUI, no bundled reference recordings (license), no
Wwise-specific container semantics (unverifiable sources, §12-6). The TTS
scope note that used to live here ("no change to the cloud-TTS path beyond
sharing the text-normalization hooks") is superseded by
`DESIGN-TTS-EXPRESSIVE.md` (owner instruction of 2026-11-06: expressive,
narrator-class TTS).
