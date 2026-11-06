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

Measured by `qinari setup` validation on the reference machine (full
synthesis of a real sentence through the ONNX engine, 22.05 kHz):

| Voice | Audio | Render | RTF |
| --- | --- | --- | --- |
| en_US-libritts-high | 1.58 s | 0.50 s | **0.31** |
| id_ID-news_tts-medium | 1.66 s | 0.08 s | **0.05** |

Both are far under the 0.4 goal — speech renders at least 3× faster
than it plays.

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
