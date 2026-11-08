# Changelog

All notable changes to Qinari are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions
follow semantic versioning.

## [0.2.0] — Unreleased

In development (Fase A–C of `docs/PLAN-0.2.0.md`). This entry records
the claim corrections from the 0.1.5 forensic audit (findings F9, F18,
F20). It corrects what the 0.1.5 entry below says; where the underlying
DSP is unchanged, this is stated, not hidden.

### Fixed

- **Corrected the 0.1.5 "full LFO migration" claim.** The 0.1.5 entry
  below says "every bed recipe and every ambient preset migrated off
  periodic LFOs". That was wrong for ocean: `render_ocean` still drives
  its swell with a periodic `fast_sine(0.055 * t)` sinusoid. Three more
  periodic AM lines also survive the 0.1.5 migration: the engine
  flutter (4.3 Hz), the forcefield throb (0.37 Hz), and the siren alarm
  sweep. The documentation now matches the code; the ocean DSP fix is
  scheduled for 0.3.0 (plan P3-4) and the remaining AM lines stay open
  as finding F20 — none of them is claimed fixed in 0.2.0.
- **Header comment corrections in `qinari-sfx`** (comments only, no
  behavior change): the crate header said 29 recipes — the registry
  holds 30 (`birdsong` was added post-alpha without updating the
  header); the ambient docs said event layers fire at Poisson times —
  the implementation is a Cox (doubly stochastic) process whose rate
  rides the shared latent field.
- **`qinari-graph` and `qinari refine` are RFC, not features.**
  `DESIGN-REALISM-ENGINE.md` presented them as 0.2.0 stages; neither
  exists — no crate in the workspace manifest, no subcommand in the
  CLI. The design doc now marks them RFC with target versions 0.2.x
  (graph, plan P2-1) and 0.3.x (refine, plan P4-1), and the 0.1.5-era
  optimizer sketch is superseded by the plan's in-house deterministic
  optimizer (P4-1, decision D7). `docs/GOALS.md` is rewritten for the
  0.2.0 gate table so no document claims a feature that is absent.

## [0.1.5] — 2026-11-07

One release collapsing the staged 0.1.0/0.2.0 roadmap of
`DESIGN-TTS-EXPRESSIVE.md` and `DESIGN-REALISM-ENGINE.md`: Supertonic
becomes the only bundled local TTS engine, the TTS feature surface is
completed, the CLI reaches production grade, and the core audio stack
carries the realism architecture.

### Changed

- **Local TTS is Supertonic 3** (OpenRAIL-M): a full Rust port of the
  four-graph pipeline over ONNX Runtime, parity-verified against the
  reference Python SDK (waveform correlation 1.0000, duration delta
  0.00%, identical seeded latents). Ten bilingual voices
  (`supertonic-M1..M5`, `F1..F5`) share one 398 MB hash-pinned bundle;
  `setup` downloads resumably, gates on the OpenRAIL-M acceptance flow,
  and validates each voice with a real synthesis (RTF 0.37–0.39).
- **Expression markup and prosody**: `[laugh]`, `[breath]`, `[sigh]`,
  `[pause:<ms>]` (unknown tag = hard error; `--no-markup` speaks
  verbatim); punctuation-aware pausing and bounded seeded rate jitter
  behind `--prosody flat|natural`; `--style`/`--style-file` for timbre;
  seeded byte-identical determinism; `--steps`, `--speed`, `--seed` on
  the CLI.
- **Cloud engines**: ElevenLabs (`eleven_v4`/`eleven_v4_turbo`,
  `QINARI_ELEVENLABS_API_KEY`, tag passthrough, mock-server tested)
  joins the OpenAI-compatible lane; `--engine auto|local|cloud`.
- **Production CLI** (researched in `RESEARCH-CLI-PRODUCTION.md`):
  full `tts` tuning surface, `--json` machine output on renders,
  `qinari interactive` (prompted session; q/Esc/EOF leave cleanly),
  `qinari config get/set` (git-style, eagerly validated), overall
  setup progress for the 398 MB install, exit-code discipline pinned
  by e2e tests.
- **Realism DSP**: `LatentField` beds (two-pass calibrated, tilted
  toward 1/f), Cox/renewal event fields, fractal noise,
  `decorrelate`/`distance`/`early_reflections` — every bed recipe and
  every ambient preset migrated off periodic LFOs and
  interaural-correlation-1.0 beds (see the BENCHMARKS before/after
  table: beds read interaural 0.30–0.47 above 500 Hz, was 0.92–1.00).
- **Physics-shaped families**: explosion (Friedlander body with true
  negative phase, ground taps, air absorption, rumble), thunder
  (tortuosity multi-arrival), modal bank materials
  (wood/metal/ceramic/thud on `impact`/`hit`/`sword`/`footsteps`),
  birdsong v2 (source-filter notes, Podos-constrained rate×bandwidth,
  micro-jitter motif repetitions, Cox phrase field).

### Added

- `qinari-verify` crate: Welch spectrum, band envelopes, modulation
  spectrum, event statistics, loudness, LSD/multi-res STFT distances,
  speech features (F0 σ/range, pauses), and a vendored DNSMOS port
  cross-validated to four decimals against the Python reference.
- `qinari analyze --speech --mos --json` (speech features + DNSMOS MOS
  + machine-readable output); `setup` vendors the DNSMOS models;
  `doctor` checks them.
- Golden TTS gates in live-test form: DNSMOS OVRL ≥ 3.35, F0 σ ≥ 3.5
  st, ≥ 2 pauses, duration floor ±5% on the pinned golden seed
  (measured 3.48 / 4.1 st / 2 / 17.43 s).
- MSRV job in CI: the workspace builds on the declared MSRV
  **1.92** exactly (`rust-version` bumped from the fictional 1.85; the
  real floor is set by `ort` 2.0.0-rc.15 which requires rustc 1.92).

### Removed

- The Piper/VITS local engine, both Piper voices, the PAD sequencer,
  and the eSpeak phonemization path — assets no longer downloaded.

### Fixed

- `qinari tts --file` now wins over the positional text (the argument
  precedence was inverted).
- `qinari config set` accepts negative values (e.g. `-23` LUFS).

## [0.0.5-alpha] — 2026-11-06

First public alpha: six-crate workspace, 30 procedural recipes, 5
ambient presets, 5 music styles, Piper VITS TTS (id+en), OpenAI
cloud TTS, house mastering chain, deterministic seeded renders,
Installer.sh with SHA-256-verified release tarballs.
