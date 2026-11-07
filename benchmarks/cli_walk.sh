#!/usr/bin/env bash
# Adversarial black-box walk of docs/CLI.md: every documented example,
# run verbatim against the release binary, with exit-code checks.
# Config is isolated (XDG_CONFIG_HOME/STATE under the walk dir) while the
# data dir stays real so the installed voices work.
set -uo pipefail
Q="${QINARI_BIN:-./target/release/qinari}"
W=/tmp/cli-walk
rm -rf "$W"; mkdir -p "$W/config" "$W/state" "$W/out"
cd "$W/out"
export XDG_CONFIG_HOME="$W/config"
export XDG_STATE_HOME="$W/state"
export QINARI_TTS_API_KEY=""; unset OPENAI_API_KEY

pass=0; fail=0
check() { # check <label> <exit-code>
    if [ "$2" -eq 0 ]; then pass=$((pass+1)); echo "ok   $1";
    else fail=$((fail+1)); echo "FAIL $1 (exit $2)"; fi
}

# --- sfx section ---
$Q sfx rain -d 30s --seed storm --set intensity=0.9 -o rain.wav >/dev/null 2>&1
check "sfx rain -d 30s --set intensity=0.9" $?
[ -s rain.wav ] && check "rain.wav exists non-empty" 0 || check "rain.wav exists non-empty" 1

$Q sfx laser --batch 5 --seed laser-set >/dev/null 2>&1
check "sfx laser --batch 5" $?
n=$(ls out-0*.wav 2>/dev/null | wc -l)
[ "$n" -eq 5 ] && check "batch wrote out-000..out-004" 0 || check "batch wrote out-000..out-004 ($n)" 1

$Q sfx --list >/dev/null 2>&1
check "sfx --list" $?

# --- ambient + music + master (doc: same options as sfx; master accepts -o) ---
$Q ambient campfire -d 1m --seed walk -o camp.wav >/dev/null 2>&1
check "ambient campfire -d 1m" $?
$Q music lofi --bpm 82 -d 45s --seed café -o lofi.wav >/dev/null 2>&1
check "music lofi --bpm 82 -d 45s" $?
$Q music --list >/dev/null 2>&1
check "music --list" $?
$Q master rain.wav -o rain-mastered.wav >/dev/null 2>&1
check "master rain.wav" $?

# --- tts section (all three doc examples) ---
$Q tts --lang id --voice supertonic-M1 "Selamat datang [pause:300]di Qinari." >/dev/null 2>&1
check "tts id --voice supertonic-M1 with [pause:300]" $?
$Q tts --lang en --seed storm --style F1 --speed 1.1 -o intro.mp3 "Chapter one." >/dev/null 2>&1
check "tts en --seed --style F1 --speed 1.1 -o intro.mp3" $?
[ -s intro.mp3 ] && check "intro.mp3 exists" 0 || check "intro.mp3 exists" 1
$Q tts --lang id --json "Halo dunia" 2>/dev/null | python3 -c "import json,sys; d=json.load(sys.stdin); assert d['sha256'] and d['rtf']>0" >/dev/null 2>&1
check "tts --lang id --json (single JSON object, sha256+rtf)" $?

# --- analyze section ---
$Q tts --lang id -o narasi.wav "Halo, ini narasi untuk uji analisis." >/dev/null 2>&1
check "prereq: render narasi.wav" $?
$Q analyze narasi.wav --speech --mos >/dev/null 2>&1
check "analyze --speech --mos" $?
$Q analyze narasi.wav --json --mos 2>/dev/null | python3 -c "import json,sys; d=json.load(sys.stdin); assert d['dnsmos']['ovrl']>0 and d['speech']['pause_count']>=0" >/dev/null 2>&1
check "analyze --json --mos (dnsmos.ovrl, speech.pause_count)" $?
$Q analyze rain.wav >/dev/null 2>&1
check "analyze plain" $?

# --- interactive (documented EOF behavior) ---
$Q interactive < /dev/null >/dev/null 2>&1
check "interactive under EOF exits 0" $?

# --- config section ---
$Q config get output.sr >/dev/null 2>&1
check "config get output.sr" $?
$Q config set mastering.lufs -23 >/dev/null 2>&1
check "config set mastering.lufs -23 (negative value)" $?
v=$($Q config get mastering.lufs 2>/dev/null)
[ "$v" = "-23" ] && check "config round-trip reads -23" 0 || check "config round-trip reads -23 (got $v)" 1
$Q config set tts.engine cloud >/dev/null 2>&1
check "config set tts.engine cloud" $?
$Q config get tts.engine >/dev/null 2>&1
check "config get tts.engine" $?

# --- completions ---
for shell in bash zsh fish powershell; do
    $Q completions "$shell" >/dev/null 2>&1
    check "completions $shell" $?
done

# --- bench / info / doctor ---
$Q bench --json >/dev/null 2>&1
check "bench --json" $?
$Q info --json >/dev/null 2>&1
check "info --json" $?
$Q doctor >/dev/null 2>&1
check "doctor (healthy, exit 0)" $?

# --- documented error paths ---
$Q config set output.sr 192001 >/dev/null 2>&1
[ $? -eq 2 ] && check "config set out-of-range exits 2" 0 || check "config set out-of-range exits 2" 1
$Q tts --lang id --file no-such.txt halo >/dev/null 2>&1
[ $? -eq 2 ] && check "tts --file missing exits 2 (usage)" 0 || check "tts --file missing exits 2" 1
$Q tts --lang id "[unknown]" >/dev/null 2>&1
[ $? -eq 7 ] && check "unknown markup tag exits 7 with tag named" 0 || check "unknown markup tag exits 7" 1

echo
echo "walk result: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
