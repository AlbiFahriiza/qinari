# Non-Rust Dependency Register

Owner constraint: Qinari is Rust-first. Every non-Rust component must be
justified here, minimized, and was approved by the owner's standing
decision ("up to you, as long as it is production grade"). This register
is the complete list — a new non-Rust dependency requires a new entry
and owner sign-off before merging.

| Component | Where | Language | License | Why it cannot be Rust today | Minimization |
|---|---|---|---|---|---|
| LAME 4.0 (vendored) | `mp3lame-encoder` -> `mp3lame-sys`, MP3 encode | C | LGPL (crate LGPL-3.0) | No mature pure-Rust MP3 encoder exists as of 2026-11. LAME is the industry-standard encoder. | Compiled from vendored source by `cc` at build time; no system library, no runtime dependency; used only in the encode step; LGPL-3.0 is compatible with the GPL-3.0 workspace. Watchlist: rusty_mp3 as a future no-C-toolchain option. |
| ONNX Runtime 1.31 | `ort` 2.0.0-rc.15, local TTS inference | C++ | MIT | Neural inference for VITS models at production speed. Pure-Rust runtimes (tract, candle) are significantly slower for these graphs. | Pinned exact version (`=2.0.0-rc.15`), prebuilt static binary downloaded and SHA-256-checked by ort-sys at build time, CPU execution provider only, used only inside `qinari-tts` local engine. The prebuilt archive requires glibc 2.39 and libstdc++ 13.2 (GLIBCXX_3.4.31) symbols — this sets the release binary's platform floor (Ubuntu 24.04+, Debian 13+; the release workflow pins `ubuntu-24.04` accordingly). |
| espeak-ng phonemization data | `espeak-ng` crate (pure-Rust port) + `espeak-ng-data-*` data crates | Rust (port) + data | GPL-3.0-or-later | Not a non-Rust component anymore: the `espeak-ng` crate is a pure-Rust port (bit-identical IPA output, 319 tests). Listed for transparency because it replaces the C library Piper originally used. | Pure Rust; per-language data crates; no C code built. |
| curl (system binary) | `qinari-cli` download layer, runtime | C (the binary we invoke) | curl license (MIT-style) | Owner-mandated downloader (SPEC hard constraint): one transport everywhere, resumable, progress, retry. Invoked as a subprocess, never linked. | Detected by `doctor`; every download SHA-256-verified by Qinari itself; no other system tools required; a missing curl is an actionable error, not a crash. |
| OpenSSL (build-time only) | `ort-sys` build script's download step | C | Apache-2.0-style | ort-sys downloads the prebuilt ONNX Runtime over HTTPS via ureq with its default `tls-native` feature, which pulls `openssl-sys` into the BUILD graph. The final qinari binary does NOT link OpenSSL (runtime cloud HTTP uses rustls, D4). | Build-time only; no runtime footprint; a future switch to ort's `tls-rustls` feature would remove it from the build graph too. |

Deliberately rejected non-Rust paths (with reasons) are recorded in
docs/ARCHITECTURE.md decision log entries D2–D4 (sherpa-onnx C++ stack,
piper-rs cmake/espeak build, reqwest/tokio).
