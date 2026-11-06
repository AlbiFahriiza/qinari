# MISSION — Qinari 0.1.5 (Supertonic-Only + Hardened Core)

Status: active build mission. Owner instruction of 2026-11-07: Qinari 0.1.5
is a single release that collapses the staged roadmap of
`DESIGN-TTS-EXPRESSIVE.md` and `DESIGN-REALISM-ENGINE.md` (their 0.1.0 /
0.2.0 split is ignored — everything lands in 0.1.5), makes Supertonic 3
the one and only bundled local TTS engine, completes the TTS feature
surface (custom + cloud API engines, full tuning), upgrades the CLI to
production grade (researched), and upgrades the core audio architecture
per the realism design.

## 1. Scope

### Must land (release blockers)

- **P-1 Piper removal.** The VITS local engine (`local.rs`), both Piper
  voices, the PAD sequencer, and the eSpeak phonemization path are
  deleted. Piper assets are no longer downloaded by `setup`. The
  realism-design stage TTS-1 (PAD fix) is moot: the engine it fixes is
  gone. Supertonic replaces it as the only bundled local engine.
- **TTS-E1 — `SupertonicEngine`.** Full port per
  `DESIGN-TTS-EXPRESSIVE.md` §3.2: four `ort` sessions, unicode-indexer
  tokenizer, style assets, seeded initial-latent (`Qrng`), 44 100 Hz
  native output, resample-on-request, chunking at 300 chars, speed
  mapping 0.7–2.0 (clamped from the CLI's 0.5–2.0 with a warning).
  Voice catalog v2: `supertonic-M1…M5, F1…F5` with pinned hashes
  (bundle + per-voice styles). `setup` downloads the 398 MB bundle once
  (resumable, hash-verified) and gates on the OpenRAIL-M acceptance
  flow.
- **TTS-E2 — markup + prosody + measurement.** `[laugh]` `[breath]`
  `[sigh]` `[pause:<ms>]` markup (unknown tag = hard error,
  `--no-markup`), engine tag maps, punctuation-aware pausing, bounded
  seeded rate jitter, `--prosody flat|natural`. Speech feature set and
  DNSMOS (`--mos`) in the analysis tool; CI golden gates (DNSMOS OVRL
  ≥ 3.35, F0 σ ≥ 3.5 st, ≥ 2 pauses per golden paragraph, duration
  floor ±5%).
- **TTS-E3 — cloud engines.** ElevenLabs (`eleven_v4`/`eleven_v4_turbo`,
  `xi-api-key` via `QINARI_ELEVENLABS_API_KEY`, tag passthrough,
  mock-server tests) joins the existing OpenAI-compatible engine as the
  generic/custom cloud lane. Custom local = user-supplied Supertonic
  model dir + style files. `engine auto|local|cloud` resolution.
- **CLI-1 — production CLI + interactive.** Research doc first
  (`RESEARCH-CLI-PRODUCTION.md`, web-verified), then: interactive mode,
  full `tts` tuning surface (`--voice --lang --speed --steps --seed
  --style --style-file --prosody --engine --format …`), `--json`
  machine output, shell completions, exit-code discipline, config
  get/set, progress UX for the 398 MB setup.
- **INFRA-1 — MSRV truth.** `rust-version = 1.87` + CI job building on
  1.87 exactly (realism design §5).
- **VERIFY-1 — `qinari-verify`.** Feature extraction (Welch spectrum,
  band envelopes, modulation spectrum, event stats, loudness), LSD /
  multi-res STFT / texture distances, speech features (F0 σ/range,
  pauses) + DNSMOS (vendored, hash-pinned), golden-target machinery,
  `analyze` surface.
- **REALISM-1 — DSP for natural statistics.** `LatentField` (two-pass
  realized-std calibration), attack/decay/`Segment` envelopes,
  renewal/Cox events, fractal + band-limited noise, `decorrelate` /
  `distance` / `early_reflections`; migration of the bed recipes (rain,
  wind, fire, stream, thunder, storm, meadow) with before/after tables
  in `docs/BENCHMARKS.md`.
- **PHYSICAL-1 — physics-shaped families.** Explosion (shock front,
  Friedlander body with negative phase, ground reflections, air
  absorption, rumble), thunder (multi-arrival far field), modal bank
  (wood/metal/ceramic/thud → impact/hit/sword/footstep), birdsong v2
  (source-filter notes, Podos tradeoff, micro-jitter motifs, Cox phrase
  field).
- **RELEASE.** Workspace version 0.1.5, CHANGELOG, README, CLI.md /
  TTS.md / ARCHITECTURE.md / NON-RUST-DEPENDENCIES.md updates (model
  attribution: Supertonic 3 OpenRAIL-M, DNSMOS CC BY 4.0), tag
  `v0.1.5`, adversarial verification pass against every gate.

### Stretch (lands only after every must is green)

- **GRAPH-1** — `qinari-graph` TOML recipes (`--recipe-file`).
- **REFINE-1** — `qinari refine` authoring-time optimizer.
- **Challenger bench** (pocket-tts-id, qwen3-tts-id, chatterbox-id).

### Out of scope (design non-goals stand)

Voice training, streaming playback, SSML, GPU execution provider,
bundled cloud keys, neural SFX synthesis, GUI, reference recordings.

## 2. Team and checkpoint plan

Five subagents (cap), every one reads the Panoptes skill
(`/home/z/my-project/panoptes-skill/disciplined-coding/SKILL.md`) and
`CHECKPOINT_RULES.md` before touching code. The main agent runs the
workspace gates and owns every commit and push; subagents never commit.
Waves keep parallel work on disjoint crates.

| Wave | Agents | Crates touched |
|---|---|---|
| 1 | A1 Supertonic port ∥ A4 verify crate | qinari-tts (+mechanical CLI) ∥ qinari-verify (new) |
| 2 | A2 cloud & config | qinari-tts |
| 3 | A5 realism core ∥ A3 CLI | dsp/audio/sfx/music ∥ qinari-cli |
| 4 | main agent: integration + release | all |
| 5 | main agent: adversarial verification (fresh gates, black-box CLI, ASR + DNSMOS on renders) | all |

Checkpoint = workspace gates green (fmt, clippy `-D warnings`, tests 0
fail, deny, bench `--no-run`) + push to `origin/main`.

## 3. Acceptance evidence

- `qinari tts --voice supertonic-M1` renders id + en; RTF < 1.0 on the
  2-vCPU box (measured, committed to BENCHMARKS.md).
- Golden TTS gates green in CI form (tolerance tier, statistical
  determinism documented).
- ASR word accuracy ≥ 95% on the 30-sentence id+en corpus (dev-side,
  pre-release, transcript committed).
- Every migrated recipe: before/after statistics table committed; no
  exactly-periodic gain line; interaural correlation < 0.9 above
  500 Hz on beds.
- Black-box CLI walk of `docs/CLI.md` passes on the release binary.
