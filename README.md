# Qinari

Procedural audio maker and AI-native text-to-speech in a single Rust
binary: sound effects, ambient soundscapes, instrumental music, and
Indonesian/English speech — every render mastered to a loudness target.
No sample libraries, no accounts: audio from parameters, deterministic
per seed. Output: `.wav` and `.mp3` only.

```
$ qinari sfx rain -d 30s --seed storm --set intensity=0.9
rain.wav — 30.0 s @ 44100 Hz, peak -4.6 dBFS, -25.9 LUFS in / -16.0 LUFS out, 6.3 MiB, mastered

$ qinari tts "Halo, selamat datang di Qinari." --lang id -o halo.wav
halo.wav — 1.7 s @ 22050 Hz, 1 ch, mastered

$ qinari music synthwave -d 60s --seed nightdrive -o drive.mp3
```

## What it does

- **29 sound-effect recipes** — rain, thunder, explosion, laser, sword,
  footsteps, UI blips, and more — each with its own parameters
  (`--set intensity=0.9`), rendered deterministically from a text seed.
- **5 ambient beds** (storm, campfire, ocean, spaceship, meadow) up to
  an hour long.
- **5 composed music styles** (lofi, ambient, synthwave, cinematic,
  chiptune) with chords, motif development, drums, and section layout —
  composed per seed, not looped samples.
- **Offline neural TTS** for Indonesian and English (Piper-class VITS
  voices, ONNX Runtime, no network needed after setup) plus an
  OpenAI-compatible cloud engine — same command, `--engine auto` picks
  local when the voice is installed.
- **House mastering**: every render lands on a loudness target
  (−16 LUFS default, configurable) with a lookahead limiter; `qinari
  master` re-masters existing files, `qinari analyze` measures them.

## Install

Prebuilt Linux x86_64 binary (release page has a SHA-256 sidecar):

```
curl -fsSL https://raw.githubusercontent.com/AlbiFahriiza/qinari/main/Installer.sh | bash
```

Or review then run:

```
curl -fsSL -o Installer.sh https://raw.githubusercontent.com/AlbiFahriiza/qinari/main/Installer.sh
less Installer.sh && bash Installer.sh          # --prefix, --tag, --build, --dev, --uninstall
```

From source (Rust 1.85+, `curl` on PATH for voice setup):

```
git clone https://github.com/AlbiFahriiza/qinari && cd qinari
cargo build --release --bin qinari
./target/release/qinari setup --yes --accept-license   # installs voices (~200 MB)
```

## Quick tour

```
qinari sfx --list                    # all 29 recipes
qinari sfx ui_success -d 500ms --seed chime
qinari ambient storm -d 10m --seed night -o storm.wav
qinari music lofi --bpm 82 -d 45s --seed café
qinari tts "Tiga puluh empat" --lang id --speed 1.2 -o angka.wav
cat story.txt | qinari tts - --lang en -o story.mp3
qinari master old-mix.wav --target-lufs -14 --character warm
qinari analyze storm.wav
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
| Indonesian TTS | — | RTF 0.05 |
| English TTS | — | RTF 0.31 |
| 30 s `rain`, end-to-end vs a NumPy/SciPy port | 0.29 s vs 1.81 s | 6.2× faster |

## How it works

- **[docs/AUDIO-PIPELINE.md](docs/AUDIO-PIPELINE.md)** — from generator
  to mastered file: resampling, the −16 LUFS chain, noise-shaped
  dither, atomic `.wav`/`.mp3` encoding.
- **[docs/TTS.md](docs/TTS.md)** — voices, licenses, offline VITS
  inference, cloud engine, text normalization for `id`/`en`.
- **[docs/BENCHMARKS.md](docs/BENCHMARKS.md)** — every number above
  with its measurement.
- **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** — the six-crate
  workspace (`qinari-dsp`/`-audio`/`-sfx`/`-music`/`-tts`/`-cli`) and
  dependency direction.

## Non-Rust dependencies

`curl` (runtime, for downloads), eSpeak NG phoneme data (bundled via
crates), ONNX Runtime (built with the TTS crate), LAME (mp3 encode).
Full list with roles: **[docs/NON-RUST-DEPENDENCIES.md](docs/NON-RUST-DEPENDENCIES.md)**.

## Project status

v0.0.5-alpha. Linux first; other platforms compile but are untested.
See **[docs/GOALS.md](docs/GOALS.md)** for the goal table and its
evidence, [SPEC.md](docs/SPEC.md) for scope, and
[CONTRIBUTING.md](CONTRIBUTING.md) / [SECURITY.md](SECURITY.md) for
how to help.

License: GPL-3.0-or-later (code). Model weights keep their own
licenses — the Indonesian voice is non-commercial and asks for
explicit acceptance during setup; the English voice is CC BY 4.0.
