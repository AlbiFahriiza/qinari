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

Installs the TTS engine and prepares the workspace: checks the
environment (`curl` and the data directories), writes a default config
if none exists, downloads the selected voices (the 398 MB shared
Supertonic bundle plus the tiny per-voice style files, SHA-256 verified,
resumable, with an overall `file n/total` progress bar above the per-file
bars), vendors the two DNSMOS models used by `qinari analyze --mos`, and
validates each installed voice with a real synthesis.

| Option | Effect |
| --- | --- |
| `--list-voices` | List the voice catalog and exit. |
| `--voice <ID>` | Install only this voice (repeatable). Default: all. |
| `--yes` | Non-interactive; skip prompts (licenses are *not* auto-accepted). |
| `--accept-license` | Accept the OpenRAIL-M license of the Supertonic voices. |

Installing a Supertonic voice means accepting its OpenRAIL-M license:
either pass `--accept-license`, confirm interactively, or have accepted
it in an earlier run (acceptance is recorded in the config; one
acceptance covers all ten voices). `--yes` without `--accept-license`
skips the voices with a note instead. Already-installed files are
verified against their pinned digests and skipped, so re-running
`setup` after an interrupted download resumes cheaply.

### `qinari sfx <RECIPE>`

Render a procedural sound effect.

| Option | Effect |
| --- | --- |
| `--list` | List all recipes (30) and exit. |
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
footsteps spell_cast crickets birdsong`.

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

Speak text offline (Supertonic) or through a cloud service. The text
is a positional argument (`-` reads stdin), or read with `--file` (a
path, or `-`); an explicit `--file` wins over the positional form.

| Option | Effect |
| --- | --- |
| `--lang <LANG>` | **Required.** `id` (Indonesian) or `en` (English). |
| `--voice <ID>` | Catalog voice id (`supertonic-M1`…`M5`, `F1`…`F5`); all voices are bilingual. Default: `supertonic-M1`. |
| `--engine <E>` | `auto` (default), `local`, or `cloud`. |
| `--speed <X>` | 0.5 to 2.0 (default 1.0). Local voices clamp to 0.7–2.0 with a warning. |
| `--steps <N>` | Denoising steps 1–100 (default 8). More steps trade speed for quality. |
| `--seed <TEXT>` | Same seed = the same audio, byte-identical. Default: time-based. |
| `--style <NAME>` | Style preset (`M1`…`M5`, `F1`…`F5`) or full voice id — speak in another installed voice's timbre. |
| `--style-file <F>` | Custom Supertonic voice-style JSON instead of a preset. |
| `--prosody <P>` | `natural` (default: punctuation-aware pauses + bounded rate jitter) or `flat` (raw SDK pacing). |
| `--no-markup` | Speak markup tags verbatim instead of parsing them. |
| `--format <F>` | Output container `wav` or `mp3` (overrides the `-o` extension). |
| `--file <PATH>` | Read text from this file instead of the argument. |
| `--channels <N>` | 1 or 2 (default 1; upmixed to 2 on request). |
| `--json` | Print the render report as one JSON object on stdout. |
| `-o`, `--out <FILE>` | Output path. |

Expression markup (parsed unless `--no-markup`): `[laugh]`, `[breath]`,
`[sigh]`, and `[pause:<ms>]` for an exact pause, e.g.
`Halo[pause:400]apa kabar?`. An unknown tag is a hard error naming the
offending tag.

Engine choice: `auto` uses the local offline engine when the voice's
files are installed (installing a voice implies its license was
accepted), otherwise the cloud service. Numbers in the text are spoken
as words in the active language (Indonesian: `tiga puluh empat`;
English: `thirty-four`); long texts are split at sentence boundaries
(chunks of at most 300 characters locally, 3000 in the cloud lane) and
cap out at 20 000 characters per render.

Cloud mode talks to an OpenAI-compatible `/audio/speech` endpoint and
needs an API key from `QINARI_TTS_API_KEY` (or `OPENAI_API_KEY`).

```
qinari tts --lang id --voice supertonic-M1 "Selamat datang [pause:300]di Qinari."
qinari tts --lang en --seed storm --style F1 --speed 1.1 -o intro.mp3 "Chapter one."
qinari tts --lang id --json "Halo dunia" | jq .sha256
```

### `qinari master <INPUT>`

Re-master an existing `.wav`/`.mp3` file with the house chain. Accepts
`-o` and the output overrides. Prints the input/output loudness report.

### `qinari analyze <FILE>`

Print duration, integrated loudness (LUFS), sample peak, and RMS of a
`.wav`/`.mp3` file. A missing or undecodable file is a usage error.

| Option | Effect |
| --- | --- |
| `--speech` | Add the speech section: voiced fraction, F0 mean/sigma (semitones)/range, pause count and rate. |
| `--mos` | Add DNSMOS MOS estimates (`sig`, `bak`, `ovrl`, `p808`) scored with the models `setup` vendors. A missing model pair is a setup error pointing at `qinari setup`. |
| `--json` | Print one JSON object (always including the `speech` section; `dnsmos` appears with `--mos`). |

```
qinari analyze narasi.wav --speech --mos
qinari analyze narasi.wav --json --mos | jq .dnsmos.ovrl
```

### `qinari bench`

Times the two reference renders (10 s `rain`, 10 s `lofi`) in-process
and prints the realtime factors. `--json` prints machine-readable
output.

### `qinari info`

Show version, filesystem layout, effective config, and installed
voices. `--json` for machine-readable output.

### `qinari interactive`

A prompted render session: pick a domain (`sfx`, `music`, `tts`,
`quit`), answer the parameter prompts (defaults shown — Enter accepts
them), and the render runs through the very same handlers the
subcommands use. A failed render prints its message and returns to the
picker; the session always exits 0. Leave with the `quit` item, `q`/Esc
at a picker, or EOF; Ctrl-C restores the terminal and exits via SIGINT
(130) like any terminal program. Under piped stdin/stderr the first
prompt fails cleanly and the session exits 0 instead of hanging.

```
qinari interactive
```

### `qinari config <get|set>`

Read and write the configuration file git-style, with dotted keys.
`get` prints just the value; `set` validates eagerly against the same
bounds the render path enforces and is silent on success. Unknown keys
are usage errors listing every valid key.

```
qinari config get output.sr
qinari config set mastering.lufs -23
qinari config set tts.engine cloud
```

Keys: `output.sr` (8000–192000), `output.bit` (16/24/32f),
`output.mp3` (128/192/256/320), `output.master` (bool),
`mastering.lufs` (−30..=−5), `mastering.ceiling` (−3..=0),
`mastering.character` (transparent/warm/punchy), `tts.engine`
(auto/local/cloud), `tts.base` (http(s) URL), `tts.model`, `tts.voice`.
`tts.accepted_licenses` is read-only (managed by `qinari setup`).

### `qinari completions <SHELL>`

Print a completion script for `bash`, `zsh`, `fish`, `elvish`, or
`powershell`.

```
qinari completions bash > /usr/share/bash-completion/completions/qinari
```

### `qinari doctor`

Scriptable health check. Healthy = `curl` on PATH, the Supertonic
engine bundle installed, the DNSMOS model pair installed, and at least
one voice installed. Exits `8` when unhealthy, `0` when healthy; a
missing config file is only a note.

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

## Machine output (`--json`)

Every render command (`tts`, `sfx`, `ambient`, `music`) takes `--json`:
one JSON object per render goes to **stdout** (human text moves to
stderr), so `qinari tts … --json | jq .sha256` works and `--batch`
prints one object per line. The object carries `path`, `format`,
`duration_s`, `sample_rate`, `channels`, `bits`, `sha256`, `size_bytes`,
`mastered`, and `rtf` (render seconds over audio seconds); tts renders
add `engine` and `voice`. `bench`, `info`, and `analyze` have their own
`--json` shapes.

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

Layout: config `~/.config/qinari/config.toml`, Supertonic engine
`~/.local/share/qinari/models/supertonic/` (`onnx/` + `voice_styles/`),
DNSMOS models `~/.local/share/qinari/models/dnsmos/`, log
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
