# Qinari CLI reference

`qinari` renders procedural sound effects, ambient beds, and music, speaks
text through offline or cloud voices, and masters and measures audio —
all in one command. Output is always `.wav` or `.mp3`.

```
qinari [GLOBAL OPTIONS] <COMMAND> [ARGS]
```

## Global options

| Option | Effect |
| --- | --- |
| `--config <FILE>` | Read the config from this file instead of `~/.config/qinari/config.toml`. |
| `-q`, `--quiet` | Suppress informational output; errors still print. |
| `-v`, `--verbose` | More log detail. Repeat for debug level. Details land in `~/.local/state/qinari/qinari.log` (stderr is the fallback when the log file cannot be opened). |

Global options may be given before or after the subcommand. Negative
values (for example `--target-lufs -14`) parse correctly after the
subcommand.

## Commands

### `qinari setup`

Installs TTS voices and prepares the workspace: checks the environment
(`curl` and the data directories), writes a default config if none
exists, extracts the bundled eSpeak NG data offline, downloads the
selected voices (model + config, SHA-256 verified), and validates each
voice with a real synthesis.

| Option | Effect |
| --- | --- |
| `--list-voices` | List the voice catalog and exit. |
| `--voice <ID>` | Install only this voice (repeatable). Default: all. |
| `--yes` | Non-interactive; skip prompts (licenses are *not* auto-accepted). |
| `--accept-license` | Accept the non-commercial license of voices that need it. |

Installing a non-commercial voice means accepting its license: either
pass `--accept-license`, confirm interactively, or have accepted it in
an earlier run (acceptance is recorded in the config). `--yes` without
`--accept-license` skips the voice with a note instead.

### `qinari sfx <RECIPE>`

Render a procedural sound effect.

| Option | Effect |
| --- | --- |
| `--list` | List all recipes (29) and exit. |
| `-o`, `--out <FILE>` | Output path (`.wav`/`.mp3`). Default: `out.wav` (batch runs: `out-000.wav`, `out-001.wav`, …). |
| `-d`, `--duration <T>` | `3` by default. Bare seconds or `ms`/`s`/`m`/`h` units. |
| `--seed <TEXT>` | Any text; same seed = bit-identical output. Default: time-based. |
| `--set NAME=VALUE` | Set a recipe parameter (repeatable). Unknown names get a "did you mean" hint. |
| `--batch <N>` | Render N variations `out-NNN.wav` from one seed. |
| `--channels <N>` | 1 or 2 (default 2). |
| Output overrides | `--sr`, `--depth`, `--bitrate`, `--master`/`--no-master`, `--target-lufs`, `--ceiling`, `--character` — see below. |

Recipes: `rain wind thunder ocean fire stream explosion impact whoosh
sword laser blaster teleport forcefield robot_talk ui_success ui_error
ui_click sonar alarm engine glitch riser hit sub_drop heartbeat
footsteps spell_cast crickets`.

Example:

```
qinari sfx rain -d 30s --seed storm --set intensity=0.9 -o rain.wav
qinari sfx laser --batch 5 --seed laser-set        # out-000.wav … out-004.wav
```

### `qinari ambient <PRESET>`

Render a long soundscape (default 60 s, 1 s to 3600 s). Presets:
`storm campfire ocean spaceship meadow`. Same options as `sfx` minus
`--set`/`--batch` (presets expose no parameters).

### `qinari music <STYLE>`

Compose a music track (default 30 s, 5 s to 600 s).

| Option | Effect |
| --- | --- |
| `--list` | List the styles and exit. |
| `--bpm <N>` | Tempo; out-of-range values are clamped into the style's range. Default: the style's typical tempo. |

Styles: `lofi` (72–90, jazzy and swung), `ambient` (58–72, pads without
drums), `synthwave` (100–118, driving), `cinematic` (76–96, tense),
`chiptune` (128–150, squares). The render may run slightly past the
requested duration to finish the current musical section.

### `qinari tts`

Speak text. The text is a positional argument (`-` reads stdin), or
read with `--file` (a path, or `-`); an explicit `--file` wins over
the positional form.

| Option | Effect |
| --- | --- |
| `--lang <LANG>` | **Required.** `id` (Indonesian) or `en` (English). |
| `--voice <ID>` | Catalog voice id. Default: the language's default voice. |
| `--engine <E>` | `auto` (default), `local`, or `cloud`. |
| `--speed <X>` | 0.5 to 2.0 (default 1.0). |
| `--file <PATH>` | Read text from this file instead of the argument. |
| `--channels <N>` | 1 or 2 (default 1; upmixed to 2 on request). |
| `-o`, `--out <FILE>` | Output path. |

Engine choice: `auto` uses the local offline engine when the language's
voice is installed (installing a voice implies its license was
accepted), otherwise the cloud service. Numbers in the text are spoken
as words in the active language (Indonesian: `tiga puluh empat`;
English: `thirty-four`); long texts are split at sentence boundaries
into chunks of at most 3000 characters.

Cloud mode talks to an OpenAI-compatible `/audio/speech` endpoint and
needs an API key from `QINARI_TTS_API_KEY` (or `OPENAI_API_KEY`).

### `qinari master <INPUT>`

Re-master an existing `.wav`/`.mp3` file with the house chain. Accepts
`-o` and the output overrides. Prints the input/output loudness report.

### `qinari analyze <FILE>`

Print duration, integrated loudness (LUFS), sample peak, and RMS of a
`.wav`/`.mp3` file. A missing or undecodable file is a usage error.

### `qinari bench`

Times the two reference renders (10 s `rain`, 10 s `lofi`) in-process
and prints the realtime factors. `--json` prints machine-readable
output.

### `qinari info`

Show version, filesystem layout, effective config, and installed
voices. `--json` for machine-readable output.

### `qinari completions <SHELL>`

Print a completion script for `bash`, `zsh`, `fish`, `elvish`, or
`powershell`.

### `qinari doctor`

Scriptable health check. Healthy = `curl` on PATH, eSpeak NG data for
both languages present, and at least one voice installed. Exits `8`
when unhealthy, `0` when healthy; a missing config file is only a note.

## Output overrides (all render commands)

| Option | Values | Default (config) |
| --- | --- | --- |
| `--sr` | 8000–192000 Hz | 44100 |
| `--depth` | `16`, `24`, `32f` | `24` |
| `--bitrate` | 128, 192, 256, 320 kbps (mp3) | 320 |
| `--master` / `--no-master` | run or skip the mastering chain | master |
| `--target-lufs` | −30.0 to −5.0 (enforced) | −16.0 |
| `--ceiling` | −3.0 to 0.0 dBFS (enforced) | −1.0 |
| `--character` | `transparent`, `warm`, `punchy` | `transparent` |

Renders land in a `.qinari-tmp` sibling first and are renamed into
place, so a failed encode never leaves a half-written output. 16-bit
output uses noise-shaped TPDF dither; 24-bit uses flat TPDF dither;
32f passes through untouched.

## Configuration file

`~/.config/qinari/config.toml` (override with `--config`). Every field
has a default; a missing file behaves like the defaults. A corrupt file
is a hard error naming the path.

```toml
version = 1

[output]
sr = 44100
bit = "24"        # 16 | 24 | 32f
mp3 = 320         # 128 | 192 | 256 | 320
master = true

[mastering]
lufs = -16.0
ceiling = -1.0
character = "transparent"   # transparent | warm | punchy

[tts]
engine = "auto"   # auto | local | cloud
base = "https://api.openai.com/v1"
model = "gpt-4o-mini-tts"
voice = "alloy"
accepted_licenses = []
```

Layout: config `~/.config/qinari/config.toml`, voices
`~/.local/share/qinari/models/<voice-id>/`, eSpeak NG data
`~/.local/share/qinari/models/espeak-data/`, log
`~/.local/state/qinari/qinari.log`. On platforms without a state
convention the log lives under the data tree.

## Environment

| Variable | Effect |
| --- | --- |
| `QINARI_TTS_API_KEY` | API key for the cloud TTS service (preferred). |
| `OPENAI_API_KEY` | Fallback when `QINARI_TTS_API_KEY` is unset. |
| `RUST_LOG` | Overrides the `-v` log level (e.g. `qinari=debug`). |

API keys are read from the environment only; they are never written to
the config file or the log.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 1 | Unexpected internal error |
| 2 | Usage error: bad arguments, unknown recipe/preset/style (with a "did you mean" hint), unusable input file, out-of-range `--target-lufs`/`--ceiling` |
| 3 | Configuration or setup error |
| 4 | Network or download error |
| 5 | Render/DSP error |
| 6 | Encode error, including a non-`.wav`/`.mp3` output path |
| 7 | TTS engine error, including an unknown voice (with a "did you mean" hint) |
| 8 | `qinari doctor` found problems |

## Determinism

Every procedural render is a pure function of its inputs: same recipe,
parameters, duration, seed, sample rate, and channel count render
bit-identical bytes. Seeds are text: `--seed storm` and `--seed 1234`
are both fine, and `--batch` derives variation N from the seed plus the
run index.
