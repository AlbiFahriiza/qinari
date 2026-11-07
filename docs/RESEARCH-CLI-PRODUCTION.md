# Production CLI research — interactive, machine output, exit codes (2026-11-07)

Scope: the evidence base for MISSION-0.1.5 stage **CLI-1** ("production CLI +
interactive"). Every design decision in the implementation maps to a finding
here: the interactive REPL, `--json` machine output on render commands,
exit-code discipline, shell completions, `config get/set`, and progress UX for
the 398 MB Supertonic bundle download.

Method note: sources were fetched and read this window. Where a project's
canonical docs are a JS-rendered page (docs.rs source views for `dialoguer`,
cli.github.com category pages), the exact crate tarball was downloaded from
crates.io and the claim read from the actual compiled-in source instead — a
stronger citation than rendered docs. `ffmpeg.org` and `gnu.org` were
unreachable from this sandbox (connection timeouts); every claim that would
have come from them was either re-verified by running the installed binary
(git 2.47.3, ffmpeg 7.x, curl) or is marked UNVERIFIED. The mission's
"NCGA conventions" is read as the general usage-error convention (distinct
exit code for bad invocation vs. runtime failure); it is supported here
through measured clap/git behavior rather than the unreachable standard.

---

## 1. Interactive prompts in production CLIs

### 1.1 What the tools do

- **git** is interactive only in narrow places (`git add -p`, credential
  prompts, `init` hints) and always reads the answer from the terminal.
  Non-interactive contexts get an explicit opt-out flag instead of a hang:
  `git config --get` on a missing key prints nothing and exits 1 (measured
  this window with git 2.47.3), and scripted Git tells you to use
  `GIT_TERMINAL_PROMPT=0` when a prompt would block
  (<https://git-scm.com/docs/git-config>, behavior measured locally).
- **curl** prompts for a password when `-u` names a user and no password
  (<https://raw.githubusercontent.com/curl/curl/master/docs/cmdline-opts/user.md>:
  "If you specify only the username, curl prompts for a password"), and reads
  config from stdin when the filename is `-`
  (<https://raw.githubusercontent.com/curl/curl/master/docs/cmdline-opts/config.md>).
- **ffmpeg** asks "File already exists. Overwrite?" on a terminal, and offers
  the two non-interactive answers as first-class flags: `-y overwrite output
  files`, `-n never overwrite output files` (measured this window from the
  installed ffmpeg's help text; the prompt itself was observed blocking a
  pty-attached run). The lesson taken: interactive modes need explicit
  non-interactive escapes, which Qinari already has (`setup --yes`).
- **gh** exposes structured output (`--json`) alongside its prompts so
  scripts never need the interactive path at all (§2.2).

### 1.2 dialoguer 0.12.0 — verified from the crate source

Qinari already depends on `dialoguer 0.12.0` (used by `setup`'s license
confirmation). Facts read from
<https://crates.io/crates/dialoguer/0.12.0> (tarball, `src/prompts/*.rs`):

- **Prompts refuse non-terminals.** Every prompt begins with the same guard
  (`select.rs:213`, `confirm.rs:154`, `input.rs:297/641`):
  `if !term.is_term() { return Err(io::Error::new(io::ErrorKind::NotConnected, "not a terminal").into()) }`.
  So a piped stdin/stderr never blocks: the call errors immediately. A
  session loop that treats prompt errors as "quit" is therefore safe under
  `qinari interactive < /dev/null` — it exits instead of hanging.
- **Select has a built-in quit affordance.** `Select::interact_opt()` returns
  `Ok(None)` when the user presses Escape or `q` (`select.rs`: `Key::Escape |
  Key::Char('q') => … return Ok(None)`). `interact()` (the variant without
  `_opt`) instead errors with "Quit not allowed in this case". A REPL should
  use `interact_opt()` and treat `None` as leave.
- **Input supports defaults and text entry.** `Input::default(...)` shows the
  default and accepts Enter for it; `interact_text()` is the line-editing
  variant (`input.rs:291`). **History exists but is not a default feature**
  (`Cargo.toml [features]`: `history = []`, default = `editor, password`) —
  Qinari does not enable it, and this work does not add it (no new features
  without need; the doc records that the knob exists).
- **Ctrl-C restores the terminal, then re-raises SIGINT.** Verified in
  console 0.15.8 (`src/unix_term.rs`, the terminal crate dialoguer delegates
  to): on reading `\x03` it restores the original termios and then
  `libc::raise(libc::SIGINT)` — i.e. the process dies by signal (shell
  reports 130) with a clean terminal, *not* with an in-language error a
  program could map to exit 0. Catching SIGINT to force exit 0 would need a
  signal handler (unsafe code or a new dependency; both out of bounds for
  this repo: `#![forbid(unsafe_code)]`, no new deps). Decision: EOF,
  non-terminal, Escape, and `q` quit with exit 0; Ctrl-C leaves via SIGINT
  with the terminal restored — the same terminal-safety contract git and gh
  users already know, documented honestly in CLI.md.

### 1.3 The REPL shape chosen

- One `Select` for the domain (`tts` / `sfx` / `music` / `quit`) — the
  mission's own wording ("pick domain … loop") — then per-domain prompted
  parameters with defaults shown (dialoguer's `Input::default`), then the
  render reuses the exact same handler functions the CLI subcommands use.
- Render failures inside the session print the message and return to the
  domain picker (an interactive session must not die on one bad render);
  leaving the session always exits 0.

## 2. Machine-readable output (`--json`)

### 2.1 ripgrep: JSON Lines for streaming results

`rg --json` "Enable[s] printing results in a JSON Lines format … a sequence
of messages, each encoded as a JSON object", with typed messages (`begin`,
`end`, `match`, `context`, `summary`) — one JSON object **per line**, because
ripgrep streams many results
(<https://man.archlinux.org/man/rg.1.en.html>; format reference
<https://docs.rs/grep-printer/*/grep_printer/struct.JSON.html>). ripgrep
also shows the discipline rule: flags that emit other output formats are
rejected in combination with `--json` rather than silently mixing.

### 2.2 gh: one JSON document per invocation, fields selectable

`gh issue list` documents `--json <fields>` "Output JSON with the specified
fields" and `-q, --jq <expression>` "Filter JSON output using a jq expression"
(<https://cli.github.com/manual/gh_issue_list>). gh emits one JSON document
per invocation (an array of records), not a stream — the right shape when a
command produces a result *set*, and the caller selects and post-processes
fields.

### 2.3 Decision for Qinari

A render command produces **one result**, so the gh shape fits: `--json`
prints **a single JSON object** to stdout, and everything human (summary
line, the speed-clamp warning) goes to stderr — the curl body-vs-progress
separation. stdout stays parseable: `qinari tts … --json | jq .sha256` works,
and `--quiet`/warnings never corrupt the object. This matches the existing
in-repo precedent (`bench --json`, `info --json` already print one object to
stdout). JSON Lines is deliberately *not* used: batch renders emit one
object per line in `--batch` runs (one object per invocation is per-file;
`--batch` prints one object per rendered file, line-delimited, like ripgrep's
streaming case).

Fields a render pipeline should report (assembled from what the tools above
expose and what a pipeline consumer needs to verify a build artifact):
`path`, `duration_s`, `sample_rate`, `channels`, `bits`, `sha256` of the
written file (integrity), `size_bytes`, `format`, `mastered`, plus
TTS-specific `engine` and `voice`, and `rtf` (render time / audio duration)
for performance tracking — the mission's own acceptance evidence is RTF
based, so the flag that feeds CI needs the number in the artifact.

## 3. Exit-code discipline

### 3.1 sysexits.h (the BSD standard)

<https://www.freebsd.org/cgi/man.cgi?query=sysexits&sektion=3> — the
canonical table: `EX_USAGE (64)` "The command was used incorrectly, e.g.,
with the wrong number of arguments, a bad flag, a bad syntax in a parameter";
`EX_DATAERR (65)` "The input data was incorrect in some way"; `EX_NOINPUT
(66)` "An input file … did not exist or was not readable"; `EX_UNAVAILABLE
(69)`; `EX_SOFTWARE (70)` "internal software error".

### 3.2 What the tools actually do

- **clap 4** (Qinari's parser): usage errors print to stderr and the process
  exits **2**; help/version print to stdout and exit 0 — verified in the
  clap_builder 4.6.7 source (`error/mod.rs`: `USAGE_CODE: i32 = 2`,
  `exit_code()` returns it for every stderr-kind error;
  <https://docs.rs/clap/latest/clap/error/struct.Error.html#method.exit>).
- **git** (measured, 2.47.3): usage errors like an unknown subcommand exit
  **1**; `git config --get <missing>` exits 1 silently; a fatally bad stored
  value under `--type=int` exits 128. GNU-family tools did not adopt
  sysexits; clap-family Rust tools standardized on 2 + 1 + specific codes.

### 3.3 Qinari's existing mapping (kept, not changed)

Qinari's `xcode.rs` table is a *domain* extension of the sysexits idea with
clap-compatible usage errors: 0 ok, 1 internal (≈ EX_SOFTWARE), 2 usage
(clap-compatible ≈ EX_USAGE), 3 setup/config, 4 network, 5 render, 6 encode,
7 tts engine, 8 doctor. Notably Qinari already made the deliberate choice
the sysexits table suggests against: a **missing input file is exit 2**
(usage), not 66 — because the file *argument* was wrong (EX_NOINPUT's own
rationale covers "not readable"), and 2 keeps compatibility with clap's
parser errors. New paths follow the existing convention: unknown/invalid
`config` keys and values are usage errors (2); write failures of the config
file are setup errors (3).

## 4. Shell completions via clap_complete

`clap_complete` generates shell completions from the live `clap::Command`
tree at runtime (the "aot" API: `clap_complete::aot::generate(generator,
cmd, name, &mut io::stdout())`), and ships a `Shell` value-enum "for each
natively-supported shell type" — bash, zsh, fish, PowerShell, Elvish (plus
Nushell) in the 4.6 line (<https://docs.rs/clap_complete/4.6.11/clap_complete/>).
The enum plugs straight into clap as `#[arg(value_enum)]`, which is how the
existing `qinari completions <SHELL>` subcommand is built (in-repo tests
already assert every shell parses and `tcsh` is rejected). This is **static
generation at runtime**: one subcommand, no dynamic completion registration,
no extra process. Qinari's implementation predates this mission and already
matches the researched shape; this stage's job is only to keep it green as
the command tree grows and document it.

## 5. Config get/set UX

### 5.1 git config (the reference model)

- Keys are `<section>.<name>` — "You can query/set/replace/unset options
  with this command. The name is actually the section and the key separated
  by a dot" (<https://git-scm.com/docs/git-config>).
- Measured behavior (git 2.47.3, this window): `git config --get user.name`
  with no stored value prints nothing, **exit 1**; `git config user.email
  a@b.c` stores silently, **exit 0**; `git config core.compression banana`
  is *accepted at set time* (exit 0) — git validates lazily, and a typed
  read (`git config --type=int --get core.compression`) fails later with
  `fatal: bad numeric config value 'banana' …: invalid unit`, **exit 128**.

### 5.2 What a TOML-backed config should validate

git's lazy validation works because every consumer tolerates re-reading.
Qinari's consumers fail at render time with a hard exit-3 setup error naming
the config file (`render.rs` already validates `config [output] bit` etc. on
every render), so a `config set` that accepted anything would let users
store values that break the very next render. Decision: **validate eagerly
on `set`** with the same bounds the render path enforces (sampled from the
existing validators): `output.sr` 8000..=192000, `output.bit` ∈ {16, 24,
32f}, `output.mp3` ∈ {128, 192, 256, 320}, `output.master` bool,
`mastering.lufs` −30.0..=−5.0, `mastering.ceiling` −3.0..=0.0 (the render
path's enforced range — the mission draft said −6..=0, but
`validate_ceiling` and CLI.md both say −3.0..=0.0, and a set-only range
wider than the render range would store values every render rejects),
`mastering.character` ∈ {transparent, warm, punchy}, `tts.engine` ∈
{auto, local, cloud}, `tts.base` a non-empty http(s) URL, `tts.model` /
`tts.voice` non-empty. Unknown keys are refused with the list of valid keys
(the "did you mean" discipline the CLI already uses for recipes and voices);
`tts.accepted_licenses` is refused for `set` because it is written by the
license-acceptance flow (`qinari setup`), like git refusing to write
reserved sections by convention. `get` prints just the value (git style);
`set` is silent on success (git style).

## 6. Progress UX for large downloads (indicatif)

Verified from the indicatif 0.18.6 crate source
(<https://crates.io/crates/indicatif/0.18.6>):

- **Bars draw to stderr and hide themselves off-terminal.**
  `ProgressDrawTarget::stderr()` builds on `term(Term::buffered_stderr(), 20)`,
  and `term()` says: "if the terminal is not user attended the entire
  progress bar will be hidden. This is done so that piping to a file will
  not produce useless escape codes in that file. Progress bars will also be
  hidden if `TERM` is unset/`dumb`." (`draw_target.rs`). So piped/CI runs
  stay clean with zero extra code; bars do not need a manual tty check.
- **Multi-file progress is a first-class concept.** `MultiProgress` stacks
  bars: "Adds a progress bar … the draw target changed to a remote draw
  target that is intercepted by the multi progress object" (`multi.rs`) —
  the per-file byte bar and the overall items bar compose instead of
  clobbering each other.
- **Bytes vs items vs ETA are template keys**, not custom code: `{bytes}`,
  `{total_bytes}`, `{bytes_per_sec}`, `{eta}`, `{pos}`, `{len}`, `{msg}`
  (`style.rs`). The existing per-file bar already uses `{bytes}/{total_bytes}
  eta {eta}`; an items bar needs only `{pos}/{len}`.
- Decision: keep the per-file byte bars; add **one overall items bar**
  spanning the whole `setup` ("file 3/16" — 6 shared bundle files + 10 voice
  styles for the default all-voices install) nested above them in a
  `MultiProgress`. The overall bar counts only files that actually need
  fetching (pre-scan), so a re-run with everything installed shows no
  misleading partial bar; `--quiet` suppresses all bars (passing
  `ProgressBar::hidden()` semantics down), and non-tty stderr is hidden by
  indicatif itself.

## 7. stdin/stdout binary safety

- **curl treats `-` as stdin/stdout and guards the terminal.**
  `--config -` "make[s] curl read the file from stdin"
  (<https://raw.githubusercontent.com/curl/curl/master/docs/cmdline-opts/config.md>);
  for output, "Specifying the output as '-' (a single dash) passes the output
  to stdout", and `-o -` is also how you "force the output to stdout, **to
  override curl's internal binary output in terminal prevention**"
  (<https://raw.githubusercontent.com/curl/curl/master/docs/cmdline-opts/output.md>)
  — i.e. a production CLI actively *prevents* spewing binary at a terminal
  unless the user explicitly demands it.
- **Qinari's invariant (already true, now documented): audio is never
  written to stdout.** Renders always land in files via the
  `.qinari-tmp` → rename dance; `-` means stdin for *text* input only
  (`tts --file -`, positional `-`), the curl `--config -` convention. The
  `--json` split (§2.3) keeps stdout text-only so a terminal never receives
  audio bytes and a pipe never receives progress noise.

## 8. Decisions (finding → implementation)

| # | Finding | Decision in CLI-1 |
|---|---|---|
| D1 | dialoguer prompts error `NotConnected` off-terminal (`select.rs:213`) | `qinari interactive` under piped stdin/stderr maps prompt errors to a clean exit 0; e2e test asserts it |
| D2 | `Select::interact_opt()` returns `Ok(None)` on Escape/q | Domain picker uses `interact_opt()`; `None` = leave session (exit 0) |
| D3 | Ctrl-C: console restores termios then re-raises SIGINT (console `unix_term.rs`) | Ctrl-C leaves with the terminal restored (SIGINT/130); EOF/Escape/q exit 0; documented in CLI.md; no signal handler (unsafe/new-dep, out of bounds) |
| D4 | gh: one JSON document per invocation, selectable fields | `--json` on render commands prints one JSON object to stdout; human text on stderr; `--batch` = one object per line |
| D5 | ripgrep rejects flag combinations that would mix output formats | `--json` replaces the human summary line instead of printing both on stdout |
| D6 | clap exits 2 on usage errors; Qinari maps missing input to 2 | New error paths (unknown config key, invalid config value, missing `--file`) are usage errors (2); config write failures are setup (3); assert_cmd tests pin the codes |
| D7 | clap_complete `Shell` enum + `aot::generate` at runtime | Existing `qinari completions <shell>` matches; keep green, document |
| D8 | git config: dotted keys, `--get` silent exit 1 on missing, `set` silent on success | `qinari config get <dotted.key>` prints the value (unknown key = usage error listing keys); `config set` validates eagerly and is silent on success |
| D9 | git validates lazily; qinari renders hard-fail on bad config | `config set` enforces the same bounds as the render path (incl. ceiling −3..=0); every branch unit-tested |
| D10 | indicatif hides bars off-terminal; `MultiProgress` nests bars; `{pos}/{len}` = items | Setup gains an overall "file n/total" bar nested over the per-file byte bars; `--quiet` hides all bars |
| D11 | curl: `-` = stdin; binary-to-terminal prevention | `-` stays text-stdin only; audio never goes to stdout; `--json` keeps stdout textual |
| D12 | dialoguer `history` is a non-default feature | Not enabled; recorded here as a known knob |

## Sources

- dialoguer 0.12.0 source tarball — <https://crates.io/crates/dialoguer/0.12.0>
  (read: `src/prompts/select.rs`, `src/prompts/input.rs`,
  `src/prompts/confirm.rs`, `src/error.rs`, `Cargo.toml`)
- console 0.15.8 source tarball — <https://crates.io/crates/console/0.15.8>
  (read: `src/unix_term.rs`)
- indicatif 0.18.6 source tarball — <https://crates.io/crates/indicatif/0.18.6>
  (read: `src/draw_target.rs`, `src/multi.rs`, `src/style.rs`)
- clap_builder 4.6.7 source tarball — <https://crates.io/crates/clap_builder/4.6.7>
  (read: `src/error/mod.rs`, `src/util/mod.rs`)
- clap_complete 4.6.11 docs — <https://docs.rs/clap_complete/4.6.11/clap_complete/>
- ripgrep(1) manual — <https://man.archlinux.org/man/rg.1.en.html>;
  JSON schema reference — <https://docs.rs/grep-printer/*/grep_printer/struct.JSON.html>
- gh manual, `gh issue list` — <https://cli.github.com/manual/gh_issue_list>
- sysexits(3) (FreeBSD) —
  <https://www.freebsd.org/cgi/man.cgi?query=sysexits&sektion=3>
- git-config(1) — <https://git-scm.com/docs/git-config>; git 2.47.3 exit
  codes and `--type=int` failure measured locally this window
- curl `--output` documentation —
  <https://raw.githubusercontent.com/curl/curl/master/docs/cmdline-opts/output.md>
- curl `--config` documentation —
  <https://raw.githubusercontent.com/curl/curl/master/docs/cmdline-opts/config.md>
- curl `--user` documentation —
  <https://raw.githubusercontent.com/curl/curl/master/docs/cmdline-opts/user.md>
- ffmpeg 7.x `-y`/`-n` help text — measured from the installed binary this
  window; canonical page <https://ffmpeg.org/ffmpeg.html> was unreachable
  from this sandbox (marked UNVERIFIED above where it mattered)

## Unverified (explicit list)

- ffmpeg.org and gnu.org pages: connection timeouts from this sandbox; all
  ffmpeg claims above come from the installed binary's own help and an
  observed pty run, not the website.
- gh's *runtime* prompt behavior in non-tty contexts: the manual documents
  flags only; no gh binary is installed in this sandbox to measure.
- dialoguer Input **history** behavior: feature exists in 0.12 but is not
  compiled into Qinari (non-default feature); behavior not exercised.
