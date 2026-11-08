# Baseline 0.1.5 — open class-4 issues

The committed `baseline-0.1.5.json` is a **historical regression
baseline**, not a naturalness standard (docs/PLAN-0.2.0.md §9). These
are the three metrics that got *worse* during the 0.1.5 realism
migration (history in docs/BENCHMARKS.md, "Realism migration —
before/after"). They are recorded as **OPEN class-4 issues**: the
movement came with the generation-model change, is not a correctness
failure, and must not silently disappear — every entry keeps a
one-paragraph diagnosis and moves to class 2 (protected) once its
category passes Test A/B listening evidence (§9, last rule).

## 1. thunder — gain peak/mean 451.8 → 642.4 (OPEN, class 4)

Diagnosis: thunder is a one-shot whose 0–20 Hz gain-modulation spectrum
is dominated by the event's own decay envelope, so peak/mean measures
the envelope shape, not a periodic AM line. The 0.1.5 realism migration
replaced the old rumble construction with a fractal (1/f²) rumble bank
plus a rolling-echo tail; the longer, smoother envelope raises the
spectrum's peak-to-mean ratio without adding any repeating gain line —
the modulation peak stays at the event's own timescale, and thunder
correctly keeps interaural ≈ 1.0 (a distant clap is a single far-field
source, not a decorrelated bed). Revisit when the impact/thunder
category has Test A/B evidence or when F8 (explosion/thunder physical
layering, Phase B P1-6) lands.

## 2. birdsong — gain line share 0.53 → 0.65 (OPEN, class 4)

Diagnosis: the 0.1.5 birdsong v2 moved phrase timing to a Cox process
on a tilted latent field (event interval CV 3.46 → 2.69) and re-renders
every motif repetition with micro-jitter (repetition correlation
< 0.9995). A higher 0–20 Hz line share here reflects the *phrase
rhythm* of the (now stochastic) motif field concentrating modulation
energy at the phrase rate — a quasi-periodic musical structure real
birds also have — rather than the exactly-periodic sample-pack LFO the
line-share metric was built to catch (the pre-realism 0.53 was the
metronomic version; 0.65 is not a return of the periodic tell, since
the repetition correlation gate rules that out). Revisit with Test A/B
on the birdsong category, or if line share approaches the ≥ 0.9
hard-line region.

## 3. fire — gain peak/mean 64.1 → 100.4 (OPEN, class 4)

Diagnosis: fire's crackle ticks moved from a memoryless Poisson stream
to Cox-driven clustering (crackles cluster on the latent field's gusts,
as real fire does); clustering concentrates the gain spectrum's energy
in fewer modulation bins, raising peak/mean while the line share stays
low (0.08 → 0.13 — nowhere near a spectral line). The interaural
picture improved in the same migration (0.919 → 0.308), so the net
perceptual direction is positive; the peak/mean rise is the measured
signature of clustering, not of a periodic AM. Revisit with Test A/B on
the fire category or when P1-5's droplet/surface model reworks event
timing.

## Policy reminder (§9)

- Class-4 movement is expected across 0.2.0 (model generation changes);
  CI blocks it only until a one-paragraph diagnosis is supplied
  (`--expected-change-file benchmarks/EXPECTED-CHANGE.md`).
- None of the three entries above may be quoted as a naturalness
  standard; they are diagnosis anchors.
- A class-4 label must never be used to hide a class-1 (correctness)
  regression; reviewers reject that.
