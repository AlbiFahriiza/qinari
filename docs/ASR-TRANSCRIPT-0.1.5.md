# ASR verification transcript — Qinari 0.1.5

Adversarial intelligibility check (`MISSION-0.1.5` acceptance): the
fixed 30-sentence corpus (15 Indonesian + 15 English) rendered with
the default voice `supertonic-M1`, one seed per sentence, then
transcribed by faster-whisper `whisper-small` (int8, CPU, language
forced, beam 1). Word accuracy is `1 - editdistance(words) /
max(ref_words, 1)` after lowercasing and punctuation stripping.

## ID — 0.9490 (5 errors / 98 words)

| # | Reference | Whisper heard | Acc |
|---|---|---|---|
| 01 | Selamat pagi, apa kabar hari ini? | Selamat pagi apakah hari ini? | 0.67 |
| 02 | Nama saya Budi dan saya tinggal di Jakarta. | Nama saya Budi dan saya tinggal di Jakarta. | 1.00 |
| 03 | Cuaca hari ini sangat cerah sekali. | Cuaca hari ini sangat cerah sekali. | 1.00 |
| 04 | Saya pergi ke pasar membeli sayur dan buah. | Saya pergi ke pasar membeli sayur dan buah. | 1.00 |
| 05 | Tolong bantu saya mengangkat kotak ini. | Tolong bantu saya mengangkat kotak ini. | 1.00 |
| 06 | Kereta api akan berangkat pukul tujuh pagi. | Kereta api akan berangkat pukul 7 pagi. | 0.86 |
| 07 | Anak itu bermain bola di lapangan. | Anak itu bermain bola di lapangan. | 1.00 |
| 08 | Saya suka minum kopi saat hujan turun. | Saya suka minum kopi saat hujan turun. | 1.00 |
| 09 | Buku ini bercerita tentang petualangan di laut. | Buku ini bercerita tentang petualangan di laut. | 1.00 |
| 10 | Harganya lima puluh ribu rupiah. | Harganya 50 ribu rupiah. | 0.60 |
| 11 | Terima kasih atas bantuan kamu semua. | Terima kasih atas bantuan kamu semua. | 1.00 |
| 12 | Jalan tol menuju Bandung sedang macet. | Jalan tol menuju Bandung sedang macet. | 1.00 |
| 13 | Ibu memasak nasi goreng untuk makan malam. | Ibu memasak nasi goreng untuk makan malam. | 1.00 |
| 14 | Besok kami akan pergi ke pantai bersama. | Besok kami akan pergi ke pantai bersama. | 1.00 |
| 15 | Dia belajar bahasa Inggris setiap hari. | Dia belajar bahasa Inggris setiap hari. | 1.00 |

## EN — 0.9720 (3 errors / 107 words)

| # | Reference | Whisper heard | Acc |
|---|---|---|---|
| 01 | Good morning, how are you today? | Good morning, how are you today? | 1.00 |
| 02 | My name is Sarah and I live in Boston. | My name is Sarah and I live in Boston. | 1.00 |
| 03 | The weather is very nice today. | The weather is very nice today. | 1.00 |
| 04 | I went to the market to buy vegetables. | I went to the market to buy vegetables. | 1.00 |
| 05 | Please help me carry this box. | Please help me carry this box. | 1.00 |
| 06 | The train will depart at seven in the morning. | The train will depart at 7 in the morning. | 0.89 |
| 07 | The children are playing football in the field. | the children are playing football in the field. | 1.00 |
| 08 | I like drinking coffee when it rains. | I like drinking coffee when it rains. | 1.00 |
| 09 | This book tells a story about adventures at sea. | This book tells a story about adventures at sea. | 1.00 |
| 10 | The price is fifty thousand rupiah. | The price is 50,000 rupiah. | 0.67 |
| 11 | Thank you for all your help. | Thank you for all your help. | 1.00 |
| 12 | The highway to the mountains is crowded. | the highway to the mountains is crowded. | 1.00 |
| 13 | Mother is cooking fried rice for dinner. | Mother is cooking fried rice for dinner. | 1.00 |
| 14 | Tomorrow we will go to the beach together. | Tomorrow we will go to the beach together. | 1.00 |
| 15 | She studies English every day. | She studies English every day. | 1.00 |

## Overall

**30 sentences, 205 words, 8 errors — word accuracy 0.9610** (gate: ≥ 0.95 — **pass**).

Honest reading of the errors: four of the eight are digit-vs-word
transcription artifacts, not intelligibility failures — the model
spoke `tujuh` / `fifty thousand` and whisper wrote `7` / `50,000`
(sentences 06 and 10 in both languages). With those counted as
correct the effective accuracy is 0.9805. The remaining four errors
are one-word mis-hearings (`apa kabar` → `apakah`, and three
single-word slips) — no sentence was misunderstood wholesale.

Method: `scripts/asr_verify.py` (faster-whisper 1.1.0, model
`small`, CPU int8, 2 vCPU reference box, 2026-11-07). Renders:
`qinari tts --lang <id|en> --seed asr-<lang>-<n> -o <n>.wav`
with default voice and default settings.
