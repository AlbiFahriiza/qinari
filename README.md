# Qinari

Procedural audio maker and AI-native text-to-speech in a single Rust
binary: sound effects, ambient soundscapes, instrumental music, and
Indonesian/English speech — every render mastered to a loudness target.
No sample libraries, no accounts: audio from parameters, deterministic
per seed. Output: `.wav` and `.mp3` only.

```
$ qinari sfx rain -d 30s --seed storm --set intensity=0.9 -o rain.wav
rain.wav — 30.0 s @ 44100 Hz, peak -4.6 dBFS, -23.0 LUFS in / -16.0 LUFS out, 7.6 MiB, mastered

$ qinari tts "Halo, selamat datang di Qinari." --lang id -o halo.wav
halo.wav — 1.3 s @ 44100 Hz, peak -1.6 dBFS, -26.0 LUFS in / -16.0 LUFS out, 168.1 KiB, mastered

$ qinari music synthwave -d 60s --seed nightdrive -o drive.mp3
```

## What it does

- **30 sound-effect recipes** — rain, thunder, explosion, birdsong, laser,
  sword, footsteps, UI blips, and more — each with its own parameters
  (`--set intensity=0.9`), rendered deterministically from a text seed.
- **5 ambient beds** (storm, campfire, ocean, spaceship, meadow) up to
  an hour long.
- **5 composed music styles** (lofi, ambient, synthwave, cinematic,
  chiptune) with chords, motif development, drums, and section layout —
  composed per seed, not looped samples.
- **Offline neural TTS** for Indonesian and English (Supertonic 3, ten
  voices, ONNX Runtime, no network needed after setup) with expression
  markup (`[laugh]` `[breath]` `[sigh]` `[pause:400]`), seeded
  determinism, and two cloud engines (any OpenAI-compatible endpoint,
  ElevenLabs) — same command, `--engine auto` picks local when the
  voice is installed.
- **House mastering**: every render lands on a loudness target
  (−16 LUFS default, configurable) with a lookahead limiter; `qinari
  master` re-masters existing files, `qinari analyze` measures them.

## Install

Prebuilt Linux x86_64 binary (needs glibc 2.39+ and libstdc++ 13.2+ —
Ubuntu 24.04+, Debian 13+, Fedora 40+; the release page has a SHA-256
sidecar; older distributions can build from source with
`Installer.sh --build`):

```
curl -fsSL https://raw.githubusercontent.com/AlbiFahriiza/qinari/main/Installer.sh | bash
```

Or review then run (the script is plain bash; it downloads the release
tarball with curl, verifies its SHA-256 sidecar, and installs to
`~/.local` without root):

```
curl -fsSL -o Installer.sh https://raw.githubusercontent.com/AlbiFahriiza/qinari/main/Installer.sh
less Installer.sh && bash Installer.sh          # --prefix, --tag, --build, --dev, --uninstall
```

From source (Rust 1.92+, `curl` on PATH for voice setup):

```
git clone https://github.com/AlbiFahriiza/qinari && cd qinari
cargo build --release --bin qinari
./target/release/qinari setup --yes --accept-license   # installs voices (~400 MB)
```

## Quick tour

```
qinari sfx --list                    # all 30 recipes
qinari sfx impact --set material=1 --seed anvil   # modal materials: wood/metal/ceramic/thud
qinari ambient storm -d 10m --seed night -o storm.wav
qinari music lofi --bpm 82 -d 45s --seed café
qinari tts "Tiga puluh empat" --lang id --speed 1.2 -o angka.wav
qinari tts --lang id --seed storm --style F1 "Bab satu. [pause:400]Badai dimulai." -o bab1.wav
qinari analyze angka.wav --speech --mos    # F0 stats, pauses, DNSMOS
qinari master old-mix.wav --target-lufs -14 --character warm
qinari interactive                     # prompted renders in a loop
qinari doctor                        # exit 8 = not healthy (scriptable)
```

Every command, flag, config key, environment variable, and exit code:
**[docs/CLI.md](docs/CLI.md)**.

## Performance

Measured on a 2 vCPU reference machine (full tables and reproduce
commands in **[docs/BENCHMARKS.md](docs/BENCHMARKS.md)**):

| Task | Time | Realtime |
| --- | --- | --- |
| heaviest 10 s sound effect | 43 ms | 231× |
| 60 s ambient bed | 0.69 s | 87× |
| 30 s music track | 0.27 s | 110× |
| Indonesian TTS (17 s narration) | RTF 0.44 | 2.3× |
| English TTS (17 s narration) | RTF 0.29 | 3.4× |
| 30 s `rain`, end-to-end vs a NumPy/SciPy port | 0.29 s vs 1.81 s | 6.2× faster |
| mastered output peak | at the ceiling (−1.0 dBFS default), enforced on transients | brickwall |

## How it works

- **[docs/AUDIO-PIPELINE.md](docs/AUDIO-PIPELINE.md)** — from generator
  to mastered file: resampling, the −16 LUFS chain, noise-shaped
  dither, atomic `.wav`/`.mp3` encoding.
- **[docs/TTS.md](docs/TTS.md)** — voices, licenses, the Supertonic
  engine, expression markup, cloud engines, text normalization for
  `id`/`en`, DNSMOS measurement.
- **[docs/BENCHMARKS.md](docs/BENCHMARKS.md)** — every number above
  with its measurement, plus the realism before/after tables.
- **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** — the seven-crate
  workspace (`qinari-dsp`/`-audio`/`-sfx`/`-music`/`-tts`/`-verify`/`-cli`)
  and dependency direction.
- **[docs/RESEARCH-AUDIO-REALISM.md](docs/RESEARCH-AUDIO-REALISM.md)** —
  verified research groundwork for the 0.1.0 realism engine: texture
  statistics, physical modeling, the proven TTS PAD bug, voice ceilings,
  objective quality metrics, Rust ecosystem audit.
- **[docs/DESIGN-REALISM-ENGINE.md](docs/DESIGN-REALISM-ENGINE.md)** —
  the staged architecture built on that research: TTS sequencer fix,
  latent-modulation DSP, physics-shaped families, verification crate,
  data-driven scenes, authoring-time refinement (0.1.0/0.2.0 roadmap
  with acceptance criteria).

## Non-Rust dependencies

`curl` (runtime, for downloads), ONNX Runtime (built with the TTS
crate), LAME (mp3 encode). Model weights are downloaded by `setup`,
hash-pinned: Supertonic 3 (OpenRAIL-M) and DNSMOS (CC BY 4.0).
Full list with roles: **[docs/NON-RUST-DEPENDENCIES.md](docs/NON-RUST-DEPENDENCIES.md)**.

## Project status

v0.1.5. Linux first; other platforms compile but are untested.
See **[docs/GOALS.md](docs/GOALS.md)** for the goal table and its
evidence, [SPEC.md](docs/SPEC.md) for scope, and
[CONTRIBUTING.md](CONTRIBUTING.md) / [SECURITY.md](SECURITY.md) for
how to help.

License: GPL-3.0-or-later (code). Model weights keep their own
licenses — the Supertonic voices are OpenRAIL-M and ask for explicit
acceptance during setup; the DNSMOS models are CC BY 4.0.
