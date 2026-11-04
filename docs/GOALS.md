# Qinari Goals — v0.0.5-alpha acceptance criteria

These are the release gates for v0.0.5-alpha. The release does not ship
until every goal is PASS with recorded evidence. "Reference machine" is
the 2-vCPU x86-64 CPU-only Linux box the numbers were produced on — see
docs/BENCHMARKS.md for the environment and the actual measurements.

| # | Goal | Threshold | Status | Evidence |
|---|------|-----------|--------|----------|
| G1 | Clean build | `cargo build --release` with zero warnings; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --check` clean | | BENCHMARKS.md + CI run |
| G2 | Test suite | >= 150 tests passing, 0 failing (network-marked `#[ignore]` tests excluded from the count) | | test run output |
| G3 | SFX engine | >= 18 recipes; heaviest 10 s recipe renders in < 500 ms on the reference machine; same seed => bit-identical WAV; every recipe finite at the 8000 Hz minimum rate | | bench + determinism tests |
| G4 | Ambient engine | >= 4 presets; 60 s soundscape renders in < 3.5 s on the reference machine | | bench |
| G5 | Music engine | >= 4 styles; 30 s track renders in < 3 s; sections are audibly distinct (energy variance between A/B sections asserted in tests); no NaN, peak <= 1.0 | | bench + tests |
| G6 | Local TTS | Piper-class voices synthesize faster than real time: RTF <= 0.4 on the reference machine; output 22.05 kHz resampled to target rate; speed control changes duration | | bench + tests |
| G7 | Cloud TTS | OpenAI-compatible engine passes end-to-end tests against a local mock server: auth, timeout, retry-on-5xx, malformed-response handling | | integration tests |
| G8 | Audio quality | Mastering hits target LUFS within +/- 1.0 LU (997 Hz and speech-like test signals); output peak <= -0.5 dBFS after limiting; 16-bit output dithered (noise floor present, no DC bias); only .wav/.mp3 accepted (exit 6 + actionable message otherwise) | | tests + analyze output |
| G9 | Setup | `qinari setup` is idempotent (second run is a no-op), resumes partial downloads, re-downloads on checksum mismatch, gates NC-licensed voices behind explicit acceptance recorded in the config, supports `--yes` non-interactive mode, and reports actionable errors when offline | | integration tests |
| G10 | Installer | `Installer.sh` passes `bash -n` and shellcheck, verifies SHA-256 of the downloaded artifact, installs to a user prefix without root, supports `--uninstall` and `--version` | | CI/manual run |
| G11 | Determinism | `qinari sfx/music` with identical seed+params produce bit-identical WAV files across runs (byte-compared in tests) | | tests |
| G12 | Benchmarks documented | Every performance claim in README/docs traces to a measured number in BENCHMARKS.md, including a Qinari-vs-numpy comparison for a representative 30 s render | | BENCHMARKS.md |
| G13 | Repo hygiene | No secrets in history (scanned); `cargo deny check licenses advisories` clean; CHECKPOINT_RULES followed (atomic milestone commits, pushed); docs complete and accurate (commands verified against the binary) | | audit output |

## Non-goals for 0.0.5

Voice cloning, singing synthesis, GPU inference, streaming playback,
macOS and Windows binaries, Supertonic/sherpa integration (planned 0.1.0).

## Status legend

PASS with evidence / FAIL with reason. This table is updated at release
time; a goal without recorded evidence is FAIL by definition.
