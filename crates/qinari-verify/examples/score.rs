//! DNSMOS scoring binary: prints SIG/BAK/OVRL/P.808 for one audio file.
//!
//! Used by the dev-side cross-validation against the Python reference
//! (Microsoft `speechmos` + librosa) on the same audio. The models come
//! from the directory in `QINARI_DNSMOS_DIR` (default:
//! `/home/z/my-project/assets/dnsmos` on the reference box — for real use,
//! the CLI vendors them under its models directory).
//!
//! ```text
//! QINARI_DNSMOS_DIR=/path/to/models cargo run -p qinari-verify \
//!     --release --example score -- render.wav
//! ```

use std::path::Path;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: score <audio-file>");
        std::process::exit(2);
    };
    let models = std::env::var("QINARI_DNSMOS_DIR")
        .unwrap_or_else(|_| "/home/z/my-project/assets/dnsmos".to_string());
    let buffer = qinari_audio::decode_audio(&path).expect("decode");
    let mut scorer = qinari_verify::DnsmosScorer::load(Path::new(&models)).expect("load models");
    let report = scorer.score_buffer(&buffer).expect("score");
    println!(
        "sig={:.4} bak={:.4} ovrl={:.4} p808={:.4} sr={} duration={:.2}s",
        report.sig,
        report.bak,
        report.ovrl,
        report.p808,
        buffer.sample_rate(),
        buffer.duration()
    );
}
