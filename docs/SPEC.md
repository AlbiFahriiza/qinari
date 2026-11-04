# Qinari Specification — v0.0.5-alpha

## Product

Qinari is a CLI audio maker: procedural sound effects, ambient
soundscapes, instrumental music, and Indonesian/English text-to-speech,
in a single static-ish Rust binary, mastered to broadcast loudness
targets. Output: `.wav` and `.mp3` only.

Non-generic by design: audio is synthesized from parameters and a seed —
no sample libraries, no accounts, no telemetry. Same seed ⇒
byte-identical file on every machine.

## Answers and decisions (owner session 2026-11-04)

1. Name/binary: `qinari` (repo + binary + crates `qinari-*`).
2. Rust-first: every non-Rust component is justified and minimized in
   docs/NON-RUST-DEPENDENCIES.md.
3. Linux-first (x86-64); other platforms are roadmap.
4. TTS: local Piper-class ONNX VITS for id+en, cloud via the
   OpenAI-compatible protocol; license-gated downloads with SHA-256
   verification.
5. Downloads: curl (the binary), resumable, never wget.
6. Installer script named `Installer.sh`, user-prefix, no root.
7. Mastering: ITU-R BS.1770-4 loudness, true-peak ceiling, TPDF dither
   with 2nd-order noise shaping for 16-bit.
8. CLI UX: clear help, actionable errors, consistent exit codes, progress
   display; `qinari setup` is idempotent and self-validating.

## Hard constraints (owner-mandated, non-negotiable)

- Rust workspace, crates prefixed `qinari-`.
- `unsafe` forbidden workspace-wide.
- Outputs only `.wav`/`.mp3` (exit 6 otherwise, with an actionable
  message).
- curl as the downloader; `Installer.sh` name; `qinari` binary name.
- Performance claims must be measured (docs/BENCHMARKS.md) — target
  slightly high but real: SFX 10 s < 500 ms, ambient 60 s < 3.5 s,
  music 30 s < 3 s, local TTS RTF <= 0.4 on a 2-vCPU reference box.
- Milestone checkpoints (CHECKPOINT_RULES.md): gates green, then commit,
  then push.

## Scope of 0.0.5-alpha

In scope:

- 6-crate Cargo workspace (dsp, audio, sfx, music, tts, cli)
- 29 procedural SFX recipes, 5 ambient soundscape presets, 5 music styles
- TTS: local VITS engine (ONNX via `ort`) for English and Indonesian;
  cloud engine speaking the OpenAI-compatible protocol; text
  normalization (numbers, decimals, locale separators, chunking) for
  `id` and `en`
- WAV (16/24/32-float) and MP3 (CBR 128–320 kbps) output
- Mastering chain: K-weighted LUFS normalization (ITU-R BS.1770-4),
  true-peak limiting, TPDF dither with 2nd-order noise shaping
- `qinari setup` / `doctor` / `sfx` / `ambient` / `music` / `tts` /
  `master` / `analyze` / `bench` / `info` / `completions` commands
- Criterion benchmarks plus a user-facing `qinari bench`; a numpy
  reference script for the Python comparison
- `Installer.sh`, GitHub Actions CI, release pipeline with checksums
- Full docs: SPEC (this file), RESEARCH-TTS, ARCHITECTURE, GOALS,
  BENCHMARKS, CLI reference, AUDIO-PIPELINE, TTS guide,
  NON-RUST-DEPENDENCIES, CHECKPOINT_RULES, CONTRIBUTING

Out of scope for 0.0.5 (roadmap):

- Voice cloning (consent framework first)
- Singing/vocal synthesis
- GPU inference
- Local commercial-safe Indonesian voice (Supertonic 3 via sherpa-onnx
  is the planned 0.1.0 integration; as of 2026-11 the only local
  Indonesian voices are non-commercial — see RESEARCH-TTS.md)
- Real-time streaming playback; other platforms (macOS/Windows builds)

## Language policy

CLI help, errors, code, comments, commits, and docs are English
(industry convention for open-source tooling). The chat with the owner
is Indonesian.
