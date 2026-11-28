//! The one place the app touches the network: downloading a pinned LGPL
//! FFmpeg build on first video use. The archive must match the pinned SHA-256
//! before anything is extracted; a mismatch deletes it.
//!
//! FFmpeg-Builds prunes old autobuilds, so the pinned file eventually 404s.
//! Then the app falls back to the same LGPL 8.1 build under the project's
//! permanent `latest` tag, checked against the SHA-256 that release publishes
//! in its `checksums.sha256`. Either way nothing is extracted unverified.
//!
//! Only the app builds this module (the `ffmpeg-download` feature); the
//! command line never touches the network. The pin itself is in `pin.rs`.
use std::io::{Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::ffmpeg::{self, Ffmpeg, READY_MARKER};
use crate::app_error::AppError;

pub use super::pin::{Pin, PIN};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Download,
    Verify,
    Extract,
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub phase: Phase,
    pub done: u64,
    pub total: u64,
}

/// Files copied out of the archive. Names are matched on the last path
/// component only, so nothing in the archive can choose where it is written.
const WANTED: &[&str] = &["ffmpeg.exe", "ffprobe.exe", "LICENSE.txt"];

/// Permanent names under FFmpeg-Builds' rolling `latest` release.
const FALLBACK_URL: &str =
    "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/latest/ffmpeg-n8.1-latest-win64-lgpl-8.1.zip";
const FALLBACK_SUMS_URL: &str =
    "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/latest/checksums.sha256";
const FALLBACK_NAME: &str = "ffmpeg-n8.1-latest-win64-lgpl-8.1.zip";
/// The rolling build's size is not known in advance; this only stops a runaway download.
const FALLBACK_MAX_BYTES: u64 = 512 * 1024 * 1024;
const SUMS_MAX_BYTES: u64 = 256 * 1024;
const UNAVAILABLE: &str =
    "FFmpeg is not available for download right now. Try again later, or install FFmpeg and put it on PATH.";

pub fn install_supported() -> bool {
    cfg!(all(windows, target_arch = "x86_64"))
}

fn err(msg: impl Into<String>) -> AppError {
    AppError::Execute(msg.into())
}

pub fn install(
    on_progress: &dyn Fn(Progress),
    cancel: &CancellationToken,
) -> Result<Ffmpeg, AppError> {
    if !install_supported() {
        return Err(err(
            "Automatic FFmpeg download is available on Windows x64 only. Install FFmpeg and put it on PATH.",
        ));
    }
    let dir = ffmpeg::managed_dir().ok_or_else(|| err("App data folder is not ready"))?;
    let parent = dir.parent().ok_or_else(|| err("Bad FFmpeg folder"))?;
    std::fs::create_dir_all(parent).map_err(|e| AppError::Io(e.to_string()))?;
    let staging = parent.join(format!("{}.partial", PIN.id));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| AppError::Io(e.to_string()))?;

    let result = download_and_extract(&staging, on_progress, cancel);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
        return result.map(|_| unreachable!());
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&staging, &dir).map_err(|e| AppError::Io(e.to_string()))?;
    ffmpeg::locate().ok_or_else(|| err("FFmpeg was installed but could not be started"))
}

fn download_and_extract(
    staging: &Path,
    on_progress: &dyn Fn(Progress),
    cancel: &CancellationToken,
) -> Result<(), AppError> {
    let archive = staging.join("ffmpeg.zip");
    let agent = agent();
    let (digest, expected) =
        match download(&agent, PIN.url, PIN.size_bytes, &archive, on_progress, cancel) {
            Ok(digest) => (digest, PIN.sha256.to_string()),
            Err(Fetch::Gone) => {
                // The pinned build was pruned upstream: use the rolling one.
                let expected = fallback_sha256(&agent)?;
                match download(&agent, FALLBACK_URL, FALLBACK_MAX_BYTES, &archive, on_progress, cancel) {
                    Ok(digest) => (digest, expected),
                    Err(Fetch::Gone) => return Err(err(UNAVAILABLE)),
                    Err(Fetch::Failed(e)) => return Err(e),
                }
            }
            Err(Fetch::Failed(e)) => return Err(e),
        };
    on_progress(Progress {
        phase: Phase::Verify,
        done: 1,
        total: 1,
    });
    if !digest.eq_ignore_ascii_case(&expected) {
        return Err(err(format!(
            "The downloaded FFmpeg did not match the expected checksum (got {digest}). It was deleted."
        )));
    }
    extract(&archive, staging, on_progress, cancel)?;
    std::fs::remove_file(&archive).map_err(|e| AppError::Io(e.to_string()))?;
    // The marker is written last: a half-installed folder never looks usable.
    std::fs::write(staging.join(READY_MARKER), expected.to_ascii_lowercase())
        .map_err(|e| AppError::Io(e.to_string()))
}

enum Fetch {
    /// The file is not there (404): try the fallback.
    Gone,
    Failed(AppError),
}

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .https_only(true)
        .user_agent("ZeroConverter")
        .build()
        .into()
}

/// The SHA-256 FFmpeg-Builds publishes for the rolling LGPL 8.1 zip.
fn fallback_sha256(agent: &ureq::Agent) -> Result<String, AppError> {
    let mut text = String::new();
    agent
        .get(FALLBACK_SUMS_URL)
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => err(UNAVAILABLE),
            e => err(format!("FFmpeg download failed: {e}")),
        })?
        .into_body()
        .into_reader()
        .take(SUMS_MAX_BYTES)
        .read_to_string(&mut text)
        .map_err(|e| err(format!("FFmpeg download failed: {e}")))?;
    sha256_for(&text, FALLBACK_NAME).ok_or_else(|| err(UNAVAILABLE))
}

/// Finds `name` in a `sha256sum` listing (`<hex>  <name>` or `<hex> *<name>`).
fn sha256_for(listing: &str, name: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        let ok = file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        ok.then(|| hash.to_ascii_lowercase())
    })
}

fn download(
    agent: &ureq::Agent,
    url: &str,
    max_bytes: u64,
    to: &Path,
    on_progress: &dyn Fn(Progress),
    cancel: &CancellationToken,
) -> Result<String, Fetch> {
    let fail = Fetch::Failed;
    let response = agent.get(url).call().map_err(|e| match e {
        ureq::Error::StatusCode(404) => Fetch::Gone,
        e => fail(err(format!("FFmpeg download failed: {e}"))),
    })?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(max_bytes);
    let mut reader = response.into_body().into_reader();
    let mut file = std::fs::File::create(to).map_err(|e| fail(AppError::Io(e.to_string())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    let mut last_report = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(fail(AppError::Cancelled));
        }
        let n = reader
            .read(&mut buf)
            .map_err(|e| fail(err(format!("FFmpeg download was interrupted: {e}"))))?;
        if n == 0 {
            break;
        }
        // A pinned file has a known size; anything longer is not it.
        if done + n as u64 > max_bytes {
            return Err(fail(err("FFmpeg download is larger than expected")));
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| fail(AppError::Io(e.to_string())))?;
        done += n as u64;
        if done - last_report >= 512 * 1024 {
            last_report = done;
            on_progress(Progress {
                phase: Phase::Download,
                done,
                total,
            });
        }
    }
    file.flush().map_err(|e| fail(AppError::Io(e.to_string())))?;
    on_progress(Progress {
        phase: Phase::Download,
        done,
        total: done,
    });
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn extract(
    archive: &Path,
    into: &Path,
    on_progress: &dyn Fn(Progress),
    cancel: &CancellationToken,
) -> Result<(), AppError> {
    let file = std::fs::File::open(archive).map_err(|e| AppError::Io(e.to_string()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| err(format!("FFmpeg archive is unreadable: {e}")))?;
    let mut copied = 0u64;
    for i in 0..zip.len() {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let mut entry = zip
            .by_index(i)
            .map_err(|e| err(format!("FFmpeg archive is unreadable: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        let Some(name) = entry.name().rsplit('/').next().map(str::to_string) else {
            continue;
        };
        if !WANTED.contains(&name.as_str()) {
            continue;
        }
        let mut out =
            std::fs::File::create(into.join(&name)).map_err(|e| AppError::Io(e.to_string()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| AppError::Io(e.to_string()))?;
        copied += 1;
        on_progress(Progress {
            phase: Phase::Extract,
            done: copied,
            total: WANTED.len() as u64,
        });
    }
    if copied < WANTED.len() as u64 {
        return Err(err("The FFmpeg archive is missing expected files"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_is_the_lgpl_static_build_and_its_hash_is_found() {
        assert!(FALLBACK_URL.ends_with(FALLBACK_NAME));
        assert!(FALLBACK_NAME.contains("win64-lgpl-") && !FALLBACK_NAME.contains("shared"));
        let h = "84cde51e0012b9cddbefbeef7bd124267c98fb2d46fd8ca4ae69aa39dc38fc00";
        let listing = format!(
            "{}  ffmpeg-n8.1-latest-win64-lgpl-shared-8.1.zip\n{h}  {FALLBACK_NAME}\n",
            "a".repeat(64)
        );
        assert_eq!(sha256_for(&listing, FALLBACK_NAME).as_deref(), Some(h));
        assert_eq!(sha256_for(&format!("{h} *{FALLBACK_NAME}"), FALLBACK_NAME).as_deref(), Some(h));
        assert_eq!(sha256_for("nothash  ffmpeg-n8.1-latest-win64-lgpl-8.1.zip", FALLBACK_NAME), None);
        assert_eq!(sha256_for(&listing, "other.zip"), None);
    }

    #[test]
    fn pin_is_https_lgpl_and_hash_shaped() {
        assert!(PIN.url.starts_with("https://github.com/"));
        assert!(PIN.url.contains("win64-lgpl"));
        assert!(!PIN.url.contains("-gpl-") && !PIN.url.contains("gpl.zip"));
        assert_eq!(PIN.sha256.len(), 64);
        assert!(PIN.sha256.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn extracts_only_wanted_files_by_basename() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            for name in [
                "pkg/bin/ffmpeg.exe",
                "pkg/bin/ffprobe.exe",
                "pkg/bin/ffplay.exe",
                "pkg/LICENSE.txt",
                "../evil/ffmpeg.exe.dll",
            ] {
                w.start_file(name, opts).unwrap();
                w.write_all(b"x").unwrap();
            }
            w.finish().unwrap();
        }
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        extract(&archive, &out, &|_| {}, &CancellationToken::new()).unwrap();
        let mut names: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["LICENSE.txt", "ffmpeg.exe", "ffprobe.exe"]);
    }
}
