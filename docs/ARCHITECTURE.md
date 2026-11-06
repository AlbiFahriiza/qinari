# Qinari architecture — v0.0.5-alpha

## Workspace

```
qinari/
├── crates/
│   ├── qinari-dsp/     # synthesis, filters, effects, analysis primitives
│   ├── qinari-audio/   # containers (wav/mp3), resampling, mastering
│   ├── qinari-sfx/     # 30 procedural recipes + ambient presets
│   ├── qinari-music/   # algorithmic composer, 5 styles
│   ├── qinari-tts/     # local ONNX VITS + OpenAI-compatible cloud
│   └── qinari-cli/     # the `qinari` binary: setup, doctor, commands
├── docs/               # this file, SPEC, RESEARCH-TTS, GOALS, …
├── benchmarks/         # numpy reference for the Python comparison
├── Installer.sh        # verified user-prefix installer
└── .github/workflows/  # CI (check/test/tts-live/deny/build) + release
```

Dependency direction (one-way, no cycles):

```
dsp  <-  audio  <-  sfx / music / tts  <-  cli
```

Only `qinari-cli` touches the network (via curl), the filesystem layout
(XDG), and the terminal. Library crates are pure computation: no I/O, no
printing, no clocks — everything reproducible from a seed. `unsafe` is
forbidden workspace-wide.

## Data flow

All engines render planar `f32` (`qinari-dsp::AudioBuffer`, any sample
rate). The CLI then runs one shared finishing path
(`qinari-cli::render::finish_and_write`): resample (only if rates
differ) → mastering (LUFS normalize + character chain + true-peak
limiter, with a bounded verification loop) → depth finalize (TPDF
dither, noise-shaped for 16-bit) → encode (hound WAV / LAME MP3) →
atomic rename. Details: docs/AUDIO-PIPELINE.md.

## Determinism

`qinari-dsp::rng::Qrng` (xoshiro256++ expanded from the seed by
splitmix64). All randomness in every crate flows through forked Qrngs;
no system RNG reaches the audio path. The mastering dither is seeded
from a fixed string. Same seed + same parameters ⇒ byte-identical file
(asserted by tests).

## Error model and exit codes

Library crates define `thiserror` enums. The CLI maps them to prepared
user-facing messages (what failed, why, what to do next — no internal
identifiers, no raw error chains on screen; full detail goes to the log
file at `~/.local/state/qinari/qinari.log`). Exit codes:

| Code | Meaning |
|------|---------|
| 0 | success |
| 1 | unexpected internal error |
| 2 | CLI usage error (bad arguments) |
| 3 | configuration / setup error |
| 4 | network / download error |
| 5 | render / DSP error |
| 6 | encode / format error (includes unsupported output format) |
| 7 | TTS engine error |
| 8 | `qinari doctor` found problems (scriptable health check) |

## Testing strategy

- Unit tests per DSP module against measurable properties (frequency
  response via Goertzel, energy decay, ENOB/dither statistics,
  determinism).
- Characterization tests: LUFS implementation against ITU/EBU reference
  values (997 Hz sine cases).
- Integration tests per engine (recipes produce non-silent, bounded,
  deterministic audio; CLI end-to-end via `assert_cmd`).
- Cloud TTS engine tested against a local std-only mock server (auth,
  retry-on-5xx, timeout, malformed response).
- Network-dependent tests (model downloads, real synthesis) are
  `#[ignore]`d and run explicitly; CI runs a dedicated job that
  downloads the default English voice and synthesizes.
- Benchmarks: `criterion` (dev) + `qinari bench` (user-facing), results
  in docs/BENCHMARKS.md.

## Security posture

- No telemetry, no auto-update. Downloads only from URLs pinned in the
  binary, verified by SHA-256 and size, installed atomically.
- Non-commercial voice licenses require explicit, recorded acceptance.
- API keys are read from the environment, never written to config or
  logs. Errors never print keys.
- Untrusted input boundaries: CLI args (clap validation), text input for
  TTS (length-capped, chunked), downloaded files (checksummed before
  use), audio decode (symphonia/hound sandboxed parsers).

## Decision log

| # | Decision | Alternatives | Rationale |
|---|----------|--------------|-----------|
| D1 | 6-crate workspace, one-way deps | monolith | engines stay testable and replaceable; cli owns all I/O |
| D2 | local TTS via `ort` + pure-Rust espeak | sherpa-onnx (C++ stack), piper-rs (cmake) | no C toolchain beyond `cc`; pure-Rust phonemization verified |
| D3 | procedural recipes, not samples | sample packs | the differentiator: non-generic, deterministic, tiny |
| D4 | `ureq` (rustls) for cloud HTTP | reqwest (tokio-based) | CLI needs blocking, small dependency tree; no async runtime; runtime TLS is rustls (no OpenSSL linkage in the final binary — build-time OpenSSL appears only in ort-sys's download step, see NON-RUST-DEPENDENCIES.md) |
| D5 | LAME for MP3 | pure-Rust encoders | no mature pure-Rust MP3 encoder as of 2026-11 |
| D6 | mastering always-on by default | opt-in | consistent loudness is part of the product promise |
