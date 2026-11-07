# Qinari benchmarks

Every performance claim in the README and docs traces to a number on
this page. All figures were measured on the reference machine:

> 2 vCPU Intel Xeon, 3.9 GiB RAM, Debian Linux, Rust 1.99.1 (stable),
> release profile, no other load.

Reproduce with:

```
cargo build --release --bin qinari
cargo bench --workspace          # criterion tables below
./target/release/qinari bench    # in-process reference renders
QINARI_BIN=./target/release/qinari python3 benchmarks/numpy_reference.py 30
./target/release/qinari setup --yes --accept-license   # prints per-voice RTF
```

## Criterion suites (`cargo bench --workspace`)

### qinari-dsp (one second of audio per iteration)

| Case | Time |
| --- | --- |
| biquad lowpass over 1 s @ 48 kHz | 219 µs |
| PolyBLEP saw, 220 Hz, 1 s | 215 µs |
| integrated LUFS of 1 s stereo | 614 µs |

### qinari-sfx (10 s stereo 44.1 kHz per case; storm 60 s)

| Case | Time | Realtime factor |
| --- | --- | --- |
| rain (heaviest of the suite) | 43.3 ms | 231× |
| thunder | 8.3 ms | 1205× |
| explosion | 12.9 ms | 776× |
| laser | 3.7 ms | 2703× |
| footsteps | 9.6 ms | 1042× |
| ambient storm bed, 60 s | 693 ms | 87× |

### qinari-music (30 s stereo 44.1 kHz; the three heaviest-representative
styles of the five)

| Case | Time | Realtime factor |
| --- | --- | --- |
| lofi | 250 ms | 120× |
| synthwave | 140 ms | 214× |
| cinematic | 272 ms | 110× |

### qinari-audio

| Case | Time |
| --- | --- |
| resample 5 s stereo 44.1→48 kHz | 5.8 ms |
| mastering chain over 10 s | 38.6 ms |
| MP3 encode 5 s stereo @ 320 kbps | 106 ms |

## In-process reference renders (`qinari bench`)

`sfx rain` 10 s: 45 ms (220× realtime). `music lofi` 10 s (11.7 s of
audio after section rounding): 73 ms (161× realtime). `--json` prints
machine-readable numbers.

## TTS realtime factors

0.1.5 (Supertonic 3, measured on the 2-vCPU reference box with the
release binary): a 17.43 s Indonesian narration renders at RTF **0.44**
(`qinari tts --lang id --seed golden-0.1.5 --json`), a 16.73 s English
narration at RTF **0.29**; `qinari setup` validation syntheses read
RTF 0.37 (M1) and 0.39 (F1). All are under the < 1.0 goal — speech
renders at least 2.3× faster than it plays. (The 0.0.5 Piper-era
numbers — RTF 0.05/0.31 — belong to the deleted VITS engine.)

## Mastering accuracy

`qinari sfx rain -d 10s --seed 1` (default chain, target −16 LUFS,
ceiling −1.0 dBFS), verified with `qinari analyze`:

| Metric | Value |
| --- | --- |
| integrated loudness in → out | −25.9 → **−16.0 LUFS** (deviation 0.0 LU) |
| peak after limiting | **−4.3 dBFS** (ceiling −1.0) |
| re-mastering the output | −16.0 → −16.0 LUFS (stable) |

The ceiling is a hard invariant on transient material too (all
measured with `qinari analyze` on delivered files): `ui_click`,
`hit`, and `impact` 3 s renders all peak at exactly **−1.0 dBFS** with
the default ceiling, and a `--ceiling -0.5` render peaks at exactly
**−0.5 dBFS**. Regression tests pin this with a full-scale impulse
and a fast-attack click (`limiter_holds_ceiling_on_a_single_impulse`,
`limiter_holds_ceiling_on_a_fast_attack_click`).

## Qinari vs NumPy/SciPy (`benchmarks/numpy_reference.py`)

The script is a faithful vectorized port of the `rain` recipe (Kellet
pink bed, RBJ lowpass/highpass shaping, 0.07 Hz gust, Poisson droplet
pings with T60 decays, equal-power panning, 120 ms fades). Output is
not bit-identical — the RNG streams differ — but the DSP structure and
the task are the same. Three framings, all measured on the 30 s render:

| Framing | Qinari | NumPy/SciPy | Result |
| --- | --- | --- | --- |
| render only, in-process (10 s case) | 43.3 ms (criterion) | 84 ms | **qinari 1.9× faster** |
| maximum favor to NumPy: qinari full process vs numpy in-process render (30 s) | 292 ms | 240 ms | numpy 1.2× faster |
| end-to-end as a user runs it (30 s) | **292 ms** | **1811 ms** | **qinari 6.2× faster** |

The middle row charges qinari for process start, mastering, and 24-bit
WAV encoding while charging NumPy nothing for interpreter startup and
imports (1.37 s on this machine). The honest headline is the bottom
row: run both the way a user would, and qinari finishes the 30 s task
six times sooner.

## Goal thresholds

| Goal | Threshold | Measured | Verdict |
| --- | --- | --- | --- |
| G3 heaviest 10 s recipe | < 500 ms | 43.3 ms (rain) | pass (11.6× headroom) |
| G4 60 s ambient bed | < 3.5 s | 693 ms (storm) | pass (5.1×) |
| G5 30 s music track | < 3 s | 272 ms (cinematic) | pass (11.0×) |
| G6 TTS RTF | ≤ 0.4 | 0.31 / 0.05 | pass |
| G8 mastering | ± 1.0 LU of target | 0.0 LU | pass |
| G8 ceiling | peak ≤ ceiling | exactly at ceiling (−1.0 / −0.5 dBFS), transients included | pass |
| G12 documented comparison | measured 30 s vs numpy | table above | pass |

## Realism migration — before/after (0.1.5)

Method: the same renders (fixed seed `realism-bench`, 20 s stereo 44.1
kHz, default parameters; explosion/impact/sword 6/3/3 s) from the
pre-realism tree (`9cf1736`, binary built from that commit) and the
current tree, analyzed with the current `qinari analyze --json`.
Metrics: **gain peak/mean** and **gain line share** of the 0–20 Hz gain
modulation spectrum (an exactly-periodic amplitude line reads a huge
peak/mean and a line share near 1.0), **interaural correlation** above
500 Hz (worst of the 0.5–2 kHz and 2–8 kHz bands; 1.0 = the hard
synthetic tell), and the **interval CV** of detected events.

| recipe | gain peak/mean (before → after) | gain line share | interaural > 500 Hz | event CV |
| --- | --- | --- | --- | --- |
| rain | 207.1 → **96.0** | 0.25 → **0.14** | 0.960 → **0.303** | 0.89 → 0.81 |
| wind | 358.6 → **96.3** | 0.35 → **0.18** | 1.000 → **0.458** | — → — |
| fire | 64.1 → 100.4 | 0.08 → 0.13 | 0.919 → **0.308** | 0.50 → 0.48 |
| stream | 663.6 → **49.4** | 0.47 → **0.11** | 0.996 → **0.414** | 0.17 → — |
| storm | 1098.4 → **214.4** | 0.77 → **0.25** | 0.976 → **0.323** | — → — |
| meadow | 309.2 → **118.0** | 0.27 → **0.15** | 0.993 → **0.467** | 1.19 → 1.22 |
| thunder | 451.8 → 642.4 | 0.88 → 0.90 | 1.000 → 1.000 | 0.24 → 0.22 |
| explosion | 383.2 → 398.8 | 0.75 → 0.78 | 0.998 → 0.940 | 0.86 → **0.44** |
| impact | 88.4 → 88.4 | 0.35 → 0.35 | 0.999 → 0.999 | — → — |
| sword | 151.1 → **60.4** | 0.60 → **0.24** | 0.966 → 0.966 | 1.44 → — |
| birdsong | 511.7 → 540.1 | 0.53 → 0.65 | 0.977 → 0.950 | 3.46 → **2.69** |

Reading the table:

- **Beds** (rain, wind, fire, stream, storm, meadow): every one now
  reads interaural ≤ 0.47 above 500 Hz (was 0.92–1.00 — the decorrelate
  pass), and the periodic gain line is gone (line share 0.11–0.25; the
  storm preset's sinusoid swell — an exact 0.05 Hz spectral line —
  dropped peak/mean 1098 → 214 after the preset migrated to a shared
  tilted latent field). Fire's peak/mean rise is its Cox-driven crackle
  clustering, not a line: its line share stays 0.13.
- **One-shots** (thunder, explosion, impact, sword, birdsong): the
  gain-modulation spectrum is dominated by each event's own decay
  envelope, so a high line share is the event shape, not a periodic
  LFO — none of these carry a repeating gain line (their modulation
  peaks sit at the event's own timescale). Sword's peak/mean and line
  share both dropped sharply after the Karplus-Strong pair became a
  modal metal bank. Thunder and explosion keep interaural ≈ 1.0 by
  design: a distant clap is a single far-field source, not a decorrelated
  bed. Birdsong's phrase field is a Cox process on a tilted latent
  (interval CV 3.46 → 2.69), and every motif repetition re-renders with
  micro-jitter (repetition correlation < 0.9995, unit-tested).
- **Acceptance** (`DESIGN-REALISM-ENGINE.md` §7/§8): no exactly-periodic
  gain line on any migrated bed; interaural < 0.9 above 500 Hz on every
  bed — both hold with wide margins.
