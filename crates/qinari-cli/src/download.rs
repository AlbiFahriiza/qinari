//! Verified downloads through a `curl` subprocess.
//!
//! Qinari does no HTTP of its own here: `curl` is spawned with
//! `--fail --location --silent --show-error --retry 3 --continue-at -`,
//! writing to a `.part` sibling that is renamed into place only after the
//! pinned SHA-256 and size match. Voice models carry pinned digests; the
//! tiny `.onnx.json` siblings are only existence-checked (their digests
//! were never pinned by the voice catalog).

use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info, warn};

/// How long the progress poller sleeps between file-size probes.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Errors raised while fetching a file.
#[derive(Debug, Error)]
pub enum DownloadError {
    /// `curl` could not be started or exited non-zero.
    #[error("curl failed (exit {exit}): {stderr}")]
    Curl {
        /// The process exit code, or -1 when curl never started.
        exit: i32,
        /// curl's error line, or why it could not start.
        stderr: String,
    },
    /// The downloaded or existing file does not match its pinned digest
    /// or size.
    #[error("checksum mismatch for {path}: {detail}")]
    Checksum {
        /// The file that failed verification.
        path: String,
        /// What was pinned and what was measured.
        detail: String,
    },
    /// A filesystem failure.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// One verified download.
#[derive(Debug, Clone)]
pub struct DownloadSpec {
    /// URL to fetch.
    pub url: String,
    /// Final resting place of the file.
    pub dest: PathBuf,
    /// Pinned size in bytes, when known.
    pub expected_size: Option<u64>,
    /// Pinned lowercase hex SHA-256, when known.
    pub expected_sha256: Option<String>,
    /// Human-readable name for progress and log lines.
    pub label: String,
}

impl DownloadSpec {
    /// The in-progress sibling curl writes to.
    fn part_path(&self) -> PathBuf {
        PathBuf::from(format!("{}.part", self.dest.display()))
    }
}

/// Fetches `spec`, skipping work when the destination is already valid.
///
/// An existing destination is verified against the pinned digest: a
/// mismatch is an error (the file is left in place for the user to
/// inspect), never a silent re-download. Downloads land in a `.part`
/// sibling and are atomically renamed after verification.
///
/// # Errors
///
/// Returns [`DownloadError::Curl`] when curl cannot run or fails,
/// [`DownloadError::Checksum`] when a digest or size does not match, and
/// [`DownloadError::Io`] for filesystem failures.
pub fn download(spec: &DownloadSpec) -> Result<(), DownloadError> {
    if spec.dest.exists() {
        verify_existing(spec)?;
        info!(label = %spec.label, dest = %spec.dest.display(), "already present");
        return Ok(());
    }
    let part = spec.part_path();
    debug!(url = %spec.url, part = %part.display(), "starting download");
    // curl only creates the output file, never its directory: a voice id
    // like en_US-libritts-high names a directory that must exist first,
    // or curl dies with "write error" on its first body chunk.
    if let Some(parent) = part.parent() {
        fs::create_dir_all(parent)?;
    }

    let total = head_content_length(&spec.url)
        .inspect_err(|error| warn!(url = %spec.url, "HEAD failed: {error}"))
        .ok()
        .or(spec.expected_size);
    if total.is_none() {
        warn!(url = %spec.url, "no content length known; progress is indeterminate");
    }
    let bar = progress_bar(total, &spec.label);

    let mut child = Command::new("curl")
        .arg("--fail")
        .arg("--location")
        .arg("--silent")
        .arg("--show-error")
        .arg("--retry")
        .arg("3")
        .arg("--continue-at")
        .arg("-")
        .arg("--output")
        .arg(&part)
        .arg(&spec.url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| DownloadError::Curl {
            exit: -1,
            stderr: format!("could not start curl: {error}"),
        })?;
    let mut stderr_pipe = child.stderr.take();
    loop {
        if let Some(status) = child.try_wait()? {
            bar.finish_and_clear();
            if !status.success() {
                return Err(DownloadError::Curl {
                    exit: status.code().unwrap_or(-1),
                    stderr: read_stderr(stderr_pipe.as_mut()),
                });
            }
            break;
        }
        if let Ok(meta) = fs::metadata(&part) {
            bar.set_position(meta.len());
        }
        thread::sleep(POLL_INTERVAL);
    }

    verify_part(spec, &part)?;
    fs::rename(&part, &spec.dest)?;
    info!(label = %spec.label, dest = %spec.dest.display(), "download complete");
    Ok(())
}

/// Checks an already-present destination against its pin.
///
/// Digest-pinned destinations must hash to the pin; the JSON siblings
/// without a pin are accepted as-is, because the catalog never pinned a
/// digest for them.
fn verify_existing(spec: &DownloadSpec) -> Result<(), DownloadError> {
    let Some(expected) = &spec.expected_sha256 else {
        return Ok(());
    };
    let actual = file_sha256(&spec.dest)?;
    if &actual == expected {
        return Ok(());
    }
    Err(DownloadError::Checksum {
        path: spec.dest.display().to_string(),
        detail: format!("expected sha256 {expected}, got {actual}"),
    })
}

/// Verifies a finished `.part` file against the pin, removing it on
/// failure so a retry cannot resume from corrupt bytes.
fn verify_part(spec: &DownloadSpec, part: &Path) -> Result<(), DownloadError> {
    let mut failures: Vec<String> = Vec::new();
    let actual_meta = fs::metadata(part)?;
    if let Some(expected_size) = spec.expected_size {
        if actual_meta.len() != expected_size {
            failures.push(format!(
                "expected {expected_size} bytes, got {}",
                actual_meta.len()
            ));
        }
    }
    if let Some(expected) = &spec.expected_sha256 {
        let actual = file_sha256(part)?;
        if &actual != expected {
            failures.push(format!("expected sha256 {expected}, got {actual}"));
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    // A completed download that fails verification would poison the next
    // resume, so it goes away here.
    fs::remove_file(part)?;
    Err(DownloadError::Checksum {
        path: spec.dest.display().to_string(),
        detail: failures.join("; "),
    })
}

/// Streams `path` through SHA-256 and returns the lowercase hex digest.
///
/// # Errors
///
/// Returns the filesystem error when the file cannot be opened or read.
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Lowercase hex encoding of `bytes`.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// Asks the server how large the final hop's body is.
///
/// Follows redirects and reports the **last** `content-length` seen, so
/// redirect chains do not shadow the real length.
fn head_content_length(url: &str) -> Result<u64, String> {
    let output = Command::new("curl")
        .arg("--silent")
        .arg("--location")
        .arg("--head")
        .arg(url)
        .output()
        .map_err(|error| format!("could not start curl: {error}"))?;
    if !output.status.success() {
        return Err(format!("exit code {}", output.status.code().unwrap_or(-1)));
    }
    let headers = String::from_utf8_lossy(&output.stdout);
    let mut length = None;
    for line in headers.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse::<u64>().ok();
        }
    }
    length.ok_or_else(|| "no content-length header".to_string())
}

/// Builds the progress bar for a download of `total` bytes.
fn progress_bar(total: Option<u64>, label: &str) -> ProgressBar {
    let bar = if let Some(len) = total {
        let bar = ProgressBar::new(len);
        bar.set_style(
            ProgressStyle::with_template("{msg}\n  {bar:28} {bytes}/{total_bytes} eta {eta}")
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("=>-"),
        );
        bar
    } else {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template("{spinner} {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_spinner()),
        );
        bar
    };
    bar.set_message(label.to_string());
    bar
}

/// Drains curl's stderr pipe after the process has exited.
fn read_stderr(pipe: Option<&mut std::process::ChildStderr>) -> String {
    let Some(pipe) = pipe else {
        return String::new();
    };
    let mut text = String::new();
    // A read failure here must not mask curl's own exit status.
    let _ = pipe.read_to_string(&mut text);
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serves `body` on 127.0.0.1:0; each connection is answered on its
    /// own accept loop iteration (curl opens one connection per attempt).
    fn serve(body: &'static [u8]) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request);
                let head_only = request.starts_with(b"HEAD");
                let mut response =
                    format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", body.len())
                        .into_bytes();
                if !head_only {
                    response.extend_from_slice(body);
                }
                let _ = stream.write_all(&response);
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        });
        format!("http://{addr}/voice.onnx")
    }

    /// SHA-256 of an in-memory buffer, reusing the module's encoding.
    fn sha256_of(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    fn temp_path(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("qinari-cli-download-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        dir.join(name)
    }

    #[test]
    fn downloads_verifies_and_renames_atomically() {
        let payload: &'static [u8] = b"qinari download payload 0123456789";
        let url = serve(payload);
        let dest = temp_path("model.onnx");
        let _ = fs::remove_file(&dest);
        let spec = DownloadSpec {
            url,
            dest: dest.clone(),
            expected_size: Some(payload.len() as u64),
            expected_sha256: Some(sha256_of(payload)),
            label: "test model".to_string(),
        };
        download(&spec).expect("download succeeds");
        assert_eq!(fs::read(&dest).expect("file renamed into place"), payload);
        assert!(!spec.part_path().exists(), "no .part left behind");
    }

    #[test]
    fn existing_file_with_matching_digest_is_skipped() {
        let dest = temp_path("cached.onnx");
        let payload = b"cached bytes";
        fs::write(&dest, payload).expect("write");
        let spec = DownloadSpec {
            url: "http://127.0.0.1:9/never-asked".to_string(),
            dest: dest.clone(),
            expected_size: Some(payload.len() as u64),
            expected_sha256: Some(file_sha256(&dest).expect("hash")),
            label: "cached".to_string(),
        };
        download(&spec).expect("existing file is accepted");
        assert_eq!(fs::read(&dest).expect("unchanged"), payload);
    }

    #[test]
    fn existing_file_with_wrong_digest_is_an_error() {
        let dest = temp_path("corrupt.onnx");
        fs::write(&dest, b"corrupted").expect("write");
        let spec = DownloadSpec {
            url: "http://127.0.0.1:9/never-asked".to_string(),
            dest: dest.clone(),
            expected_size: None,
            expected_sha256: Some("0".repeat(64)),
            label: "corrupt".to_string(),
        };
        let error = download(&spec).expect_err("mismatch must fail");
        match error {
            DownloadError::Checksum { path, detail } => {
                assert_eq!(path, dest.display().to_string());
                assert!(detail.contains("sha256"), "{detail}");
            }
            other => panic!("expected Checksum, got {other}"),
        }
        // The corrupt file stays for inspection, not silently replaced.
        assert!(dest.exists());
    }

    #[test]
    fn bad_content_is_rejected_and_part_removed() {
        let payload: &'static [u8] = b"not the pinned bytes";
        let url = serve(payload);
        let dest = temp_path("mismatch.onnx");
        let _ = fs::remove_file(&dest);
        let spec = DownloadSpec {
            url,
            dest: dest.clone(),
            expected_size: Some(9999),
            expected_sha256: Some("a".repeat(64)),
            label: "mismatch".to_string(),
        };
        let error = download(&spec).expect_err("bad content must fail");
        assert!(matches!(error, DownloadError::Checksum { .. }), "{error}");
        assert!(!spec.part_path().exists(), "bad .part removed");
        assert!(!dest.exists(), "nothing renamed into place");
    }

    #[test]
    fn unpinned_destinations_are_existence_checked() {
        let dest = temp_path("config.json");
        fs::write(&dest, b"{}").expect("write");
        let spec = DownloadSpec {
            url: "http://127.0.0.1:9/never-asked".to_string(),
            dest: dest.clone(),
            expected_size: None,
            expected_sha256: None,
            label: "config".to_string(),
        };
        download(&spec).expect("existence check only");
        assert_eq!(fs::read(&dest).expect("unchanged"), b"{}");
    }

    #[test]
    fn missing_destination_directory_is_created() {
        // Regression: a voice id names a directory under models/ that does
        // not exist on first install; curl cannot create it itself and
        // failed with exit 23 ("write error") on the first body chunk.
        let payload: &'static [u8] = b"nested destination payload";
        let url = serve(payload);
        let mut dir = std::env::temp_dir();
        dir.push(format!("qinari-cli-nested-{}", std::process::id()));
        let nested = dir.join("en_US-example").join("voice.onnx");
        let _ = fs::remove_dir_all(&dir);
        let spec = DownloadSpec {
            url,
            dest: nested.clone(),
            expected_size: Some(payload.len() as u64),
            expected_sha256: Some(sha256_of(payload)),
            label: "nested".to_string(),
        };
        download(&spec).expect("download creates the destination directory");
        assert_eq!(fs::read(&nested).expect("file written"), payload);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hex_encoding_is_lowercase() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
    }

    #[test]
    fn file_sha256_matches_a_known_vector() {
        // sha256("abc")
        let dir = std::env::temp_dir();
        let path = dir.join(format!("qinari-cli-sha-{}", std::process::id()));
        fs::write(&path, b"abc").expect("write");
        let digest = file_sha256(&path).expect("hash");
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
