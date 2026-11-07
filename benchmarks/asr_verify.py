#!/usr/bin/env python3
"""Adversarial ASR verification: word accuracy of qinari TTS renders.

Renders the fixed 30-sentence corpus (15 Indonesian + 15 English) with
the default voice (supertonic-M1), transcribes each clip with
faster-whisper (whisper-small, language forced), and reports word
accuracy per language and overall. Gate: >= 95% (MISSION-0.1.5).
"""
import json
import re
import subprocess
import sys
from pathlib import Path

Q = "/home/z/my-project/qinari/target/release/qinari"
OUT = Path("/tmp/asr-corpus")
OUT.mkdir(exist_ok=True)

ID_SENTENCES = [
    "Selamat pagi, apa kabar hari ini?",
    "Nama saya Budi dan saya tinggal di Jakarta.",
    "Cuaca hari ini sangat cerah sekali.",
    "Saya pergi ke pasar membeli sayur dan buah.",
    "Tolong bantu saya mengangkat kotak ini.",
    "Kereta api akan berangkat pukul tujuh pagi.",
    "Anak itu bermain bola di lapangan.",
    "Saya suka minum kopi saat hujan turun.",
    "Buku ini bercerita tentang petualangan di laut.",
    "Harganya lima puluh ribu rupiah.",
    "Terima kasih atas bantuan kamu semua.",
    "Jalan tol menuju Bandung sedang macet.",
    "Ibu memasak nasi goreng untuk makan malam.",
    "Besok kami akan pergi ke pantai bersama.",
    "Dia belajar bahasa Inggris setiap hari.",
]

EN_SENTENCES = [
    "Good morning, how are you today?",
    "My name is Sarah and I live in Boston.",
    "The weather is very nice today.",
    "I went to the market to buy vegetables.",
    "Please help me carry this box.",
    "The train will depart at seven in the morning.",
    "The children are playing football in the field.",
    "I like drinking coffee when it rains.",
    "This book tells a story about adventures at sea.",
    "The price is fifty thousand rupiah.",
    "Thank you for all your help.",
    "The highway to the mountains is crowded.",
    "Mother is cooking fried rice for dinner.",
    "Tomorrow we will go to the beach together.",
    "She studies English every day.",
]


def normalize(text: str) -> list[str]:
    """Lowercase, strip punctuation, split to words."""
    text = text.lower()
    text = re.sub(r"[^\w\s]", " ", text)
    return [w for w in text.split() if w]


def render(lang: str, index: int, sentence: str) -> Path:
    wav = OUT / f"{lang}-{index:02d}.wav"
    subprocess.run(
        [Q, "tts", "--lang", lang, "--seed", f"asr-{lang}-{index}",
         "-o", str(wav), sentence],
        check=True, capture_output=True,
    )
    return wav


def main() -> int:
    from faster_whisper import WhisperModel

    print("loading whisper-small (int8)…", file=sys.stderr)
    model = WhisperModel("small", device="cpu", compute_type="int8")

    rows = []
    for lang, sentences in (("id", ID_SENTENCES), ("en", EN_SENTENCES)):
        for index, sentence in enumerate(sentences, start=1):
            wav = render(lang, index, sentence)
            segments, _info = model.transcribe(str(wav), language=lang, beam_size=1)
            heard = " ".join(segment.text.strip() for segment in segments)
            ref_words = normalize(sentence)
            hyp_words = normalize(heard)
            # Word accuracy with simple alignment (Levenshtein on words).
            import editdistance  # noqa: PLC0415
            distance = editdistance.eval(ref_words, hyp_words)
            total = max(len(ref_words), 1)
            accuracy = 1.0 - distance / total
            rows.append({
                "lang": lang, "index": index, "reference": sentence,
                "transcript": heard, "words": len(ref_words),
                "errors": distance, "accuracy": round(accuracy, 4),
            })
            print(f"{lang}-{index:02d} acc={accuracy:.3f} ref={sentence!r} heard={heard!r}",
                  file=sys.stderr)

    (OUT / "transcript.json").write_text(json.dumps(rows, indent=1, ensure_ascii=False))

    for lang in ("id", "en", None):
        subset = rows if lang is None else [r for r in rows if r["lang"] == lang]
        words = sum(r["words"] for r in subset)
        errors = sum(r["errors"] for r in subset)
        accuracy = 1.0 - errors / max(words, 1)
        label = lang or "overall"
        print(f"{label}: {len(subset)} sentences, {words} words, "
              f"{errors} errors -> word accuracy {accuracy:.4f}")

    overall = [r for r in rows]
    words = sum(r["words"] for r in overall)
    errors = sum(r["errors"] for r in overall)
    return 0 if (1.0 - errors / max(words, 1)) >= 0.95 else 1


if __name__ == "__main__":
    raise SystemExit(main())
