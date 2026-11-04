# Qinari checkpoint rules (v0.0.5-alpha build)

Milestone discipline for this repository. Every checkpoint is a working
state: it builds, its tests pass, and nothing is left half-wired.

1. **Atomic milestone commits.** One milestone = one commit (plus fixups
   squashed in before the next checkpoint). The commit message names the
   milestone (`feat(dsp): …`, `feat(cli): …`).
2. **Gates before every commit:** `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace` (0 failures), `cargo deny check` (0 errors),
   `cargo bench --workspace --no-run` (0 warnings). No `#[allow]` unless
   the escape is documented in a comment.
3. **Push after every checkpoint.** Local commits are not safe; the work
   is pushed to `origin/main` before the next milestone starts.
4. **No placeholders.** No `todo!()`, no `unimplemented!()`, no stub
   functions that compile but do nothing, no "unused today" fields.
5. **Evidence over claims.** Performance numbers, benchmark medians, and
   acceptance evidence live in docs/BENCHMARKS.md and docs/GOALS.md with
   the measured values and the machine they came from.
6. **Verification is adversarial.** Before release, independent
   verification passes (subagent or fresh reviewer) re-run every gate and
   black-box the CLI against docs/CLI.md. Findings get fixed, then
   re-verified.
