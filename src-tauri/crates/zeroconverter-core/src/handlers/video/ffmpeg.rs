//! Finding FFmpeg. ZeroConverter does not ship it: the app downloads a pinned
//! LGPL build into its data folder on first use (see `install`), and reuses an
//! FFmpeg already on the machine (PATH, winget, scoop, chocolatey) when that
//! one runs and has every encoder and muxer the video routes need.
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::pin::PIN;

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Set once at startup to the app data folder.
pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(dir);
}

/// Dev/test override: a folder holding `ffmpeg` and `ffprobe`.
pub const OVERRIDE_ENV: &str = "ZC_FFMPEG_DIR";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Downloaded and hash-verified by ZeroConverter.
    Managed,
    /// Already on the machine (PATH or a package manager's folder); never
    /// modified or deleted by ZeroConverter.
    System,
    /// `ZC_FFMPEG_DIR`.
    Override,
}

#[derive(Debug, Clone)]
pub struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub source: Source,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Where the pinned build lives: `<app data>/ffmpeg/<pin id>/`.
pub fn managed_dir() -> Option<PathBuf> {
    DATA_DIR.get().map(|d| d.join("ffmpeg").join(PIN.id))
}

pub(crate) const READY_MARKER: &str = "verified.sha256";

fn managed() -> Option<Ffmpeg> {
    let dir = managed_dir()?;
    let marker = std::fs::read_to_string(dir.join(READY_MARKER)).ok()?;
    // The marker holds the SHA-256 the archive was verified against: the pin's,
    // or the one published for the fallback build when the pin had gone away.
    let marker = marker.trim();
    if marker.len() != 64 || !marker.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let found = Ffmpeg {
        ffmpeg: dir.join(exe("ffmpeg")),
        ffprobe: dir.join(exe("ffprobe")),
        source: Source::Managed,
    };
    (found.ffmpeg.is_file() && found.ffprobe.is_file()).then_some(found)
}

fn in_dir(dir: &Path, source: Source) -> Option<Ffmpeg> {
    let found = Ffmpeg {
        ffmpeg: dir.join(exe("ffmpeg")),
        ffprobe: dir.join(exe("ffprobe")),
        source,
    };
    (found.ffmpeg.is_file() && found.ffprobe.is_file()).then_some(found)
}

/// Folders that may hold a system FFmpeg, in the order to try them: PATH
/// first, then the few Windows package-manager folders that a GUI app may not
/// see on its PATH (winget links, scoop shims, chocolatey bin). Duplicates and
/// relative entries are dropped. `env` reads an environment variable.
fn system_dirs(env: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = env("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if cfg!(windows) {
        let under = |var: &str, rest: &[&str]| {
            env(var).map(|base| rest.iter().fold(PathBuf::from(base), |p, r| p.join(r)))
        };
        dirs.extend(
            [
                under("LOCALAPPDATA", &["Microsoft", "WinGet", "Links"]),
                under("SCOOP", &["shims"]),
                under("USERPROFILE", &["scoop", "shims"]),
                under("ChocolateyInstall", &["bin"]),
                under("ProgramData", &["chocolatey", "bin"]),
            ]
            .into_iter()
            .flatten(),
        );
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| {
        let key = d
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_string();
        // Windows paths are case-insensitive.
        let key = if cfg!(windows) {
            key.to_lowercase()
        } else {
            key
        };
        d.is_absolute() && seen.insert(key)
    });
    dirs
}

/// Encoders the video routes ask for, besides one H.264 encoder from
/// `H264_ORDER`: AAC (MP4/MOV audio, M4A/AAC), Opus + VP9 (WebM), GIF and PCM
/// (the WAV handed to the audio encoders).
const REQUIRED_ENCODERS: &[&str] = &[
    "aac",
    "libopus",
    "libvpx-vp9",
    "gif",
    "pcm_s16le",
    "pcm_s24le",
];
/// Muxers named with `-f` by the video routes.
const REQUIRED_MUXERS: &[&str] = &[
    "mp4", "matroska", "mov", "webm", "gif", "wav", "ipod", "adts",
];

/// What a system FFmpeg lacks for our routes; empty means usable.
pub fn missing_features(encoders: &[String], muxers: &[String]) -> Vec<String> {
    let has = |list: &[String], name: &str| list.iter().any(|n| n == name);
    let mut missing: Vec<String> = REQUIRED_ENCODERS
        .iter()
        .filter(|e| !has(encoders, e))
        .map(|e| format!("encoder {e}"))
        .collect();
    if !H264_ORDER.iter().any(|e| has(encoders, e)) {
        missing.push("an LGPL-safe H.264 encoder".into());
    }
    missing.extend(
        REQUIRED_MUXERS
            .iter()
            .filter(|m| !has(muxers, m))
            .map(|m| format!("muxer {m}")),
    );
    missing
}

/// Deadline for `-version` on a candidate: a broken or blocked binary must
/// not hold up the status check.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs, answers `-version`, and has what our routes need.
fn usable(ff: &Ffmpeg) -> bool {
    let runs = |exe: &Path| {
        let mut cmd = command(exe);
        cmd.args(["-hide_banner", "-version"]);
        output_limited(cmd, PROBE_TIMEOUT, 64 * 1024).is_ok_and(|o| o.status.success())
    };
    if !runs(&ff.ffmpeg) || !runs(&ff.ffprobe) {
        return false;
    }
    let mut cmd = command(&ff.ffmpeg);
    cmd.args(["-hide_banner", "-muxers"]);
    let muxers = output_limited(cmd, TOOL_TIMEOUT, 1024 * 1024)
        .map(|o| parse_muxer_list(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    missing_features(&encoder_names(ff), &muxers).is_empty()
}

/// Whether this build can fetch its own FFmpeg (the CLI is built without it).
fn can_download() -> bool {
    #[cfg(feature = "ffmpeg-download")]
    return super::install::install_supported();
    #[cfg(not(feature = "ffmpeg-download"))]
    false
}

/// The first usable system FFmpeg, looked up once per process. Without an
/// automatic download (off Windows x64) a system FFmpeg that runs but lacks
/// a feature is still taken, as before: only the routes needing it fail.
fn system() -> Option<Ffmpeg> {
    static FOUND: OnceLock<Option<Ffmpeg>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let found: Vec<Ffmpeg> = system_dirs(|v| std::env::var_os(v))
                .iter()
                .filter_map(|d| in_dir(d, Source::System))
                .collect();
            found.iter().find(|f| usable(f)).cloned().or_else(|| {
                (!can_download())
                    .then(|| found.into_iter().find(|f| version_line(f).is_some()))
                    .flatten()
            })
        })
        .clone()
}

/// The override when set; else our own downloaded copy (known license and
/// version); else a usable system FFmpeg. `None` means: offer the download.
pub fn locate() -> Option<Ffmpeg> {
    if let Some(dir) = std::env::var_os(OVERRIDE_ENV) {
        return in_dir(Path::new(&dir), Source::Override);
    }
    managed().or_else(system)
}

/// A child-process command that never flashes a console window.
pub fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null());
    cmd
}

/// What a short helper run (ffprobe, `-version`, `-encoders`) printed.
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Keeps the first `max` bytes of a pipe and drains the rest, so the child
/// never blocks on a full pipe and a flood of output cannot fill memory.
fn read_capped(mut pipe: impl Read, max: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let _ = (&mut pipe).take(max as u64).read_to_end(&mut kept);
    let _ = std::io::copy(&mut pipe, &mut std::io::sink());
    kept
}

/// `Command::output` with a deadline and an output cap. A hostile file that
/// makes ffprobe hang or print without end is killed instead of waited on.
pub fn output_limited(
    mut cmd: Command,
    timeout: Duration,
    max_bytes: usize,
) -> std::io::Result<Output> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let out = child
        .stdout
        .take()
        .map(|p| std::thread::spawn(move || read_capped(p, max_bytes)));
    let err = child
        .stderr
        .take()
        .map(|p| std::thread::spawn(move || read_capped(p, 64 * 1024)));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("gave no answer within {} seconds", timeout.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let join = |h: Option<std::thread::JoinHandle<Vec<u8>>>| {
        h.and_then(|h| h.join().ok()).unwrap_or_default()
    };
    Ok(Output {
        status,
        stdout: join(out),
        stderr: join(err),
    })
}

/// Deadline for quick questions to the FFmpeg binary itself.
const TOOL_TIMEOUT: Duration = Duration::from_secs(15);

/// First line of `ffmpeg -version`, e.g. `ffmpeg version n8.1.3-35-...`.
pub fn version_line(ff: &Ffmpeg) -> Option<String> {
    let mut cmd = command(&ff.ffmpeg);
    cmd.args(["-hide_banner", "-version"]);
    let out = output_limited(cmd, TOOL_TIMEOUT, 64 * 1024).ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    })
}

/// H.264 encoders worth trying, best first. All are available in LGPL builds;
/// libx264 (GPL) is never used. Hardware ones may still fail at run time on
/// machines without the hardware, so the caller walks the list.
const H264_ORDER: &[&str] = &[
    "h264_nvenc",
    "h264_qsv",
    "h264_amf",
    "h264_mf",
    "libopenh264",
];

type EncoderCache = Mutex<HashMap<PathBuf, Arc<Vec<String>>>>;

fn encoder_names(ff: &Ffmpeg) -> Arc<Vec<String>> {
    static CACHE: OnceLock<EncoderCache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap().get(&ff.ffmpeg) {
        return Arc::clone(hit);
    }
    let mut cmd = command(&ff.ffmpeg);
    cmd.args(["-hide_banner", "-encoders"]);
    let names: Vec<String> = output_limited(cmd, TOOL_TIMEOUT, 1024 * 1024)
        .map(|o| parse_encoder_list(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    let names = Arc::new(names);
    cache
        .lock()
        .unwrap()
        .insert(ff.ffmpeg.clone(), Arc::clone(&names));
    names
}

pub fn parse_encoder_list(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let flags = parts.next()?;
            // Encoder rows start with a flag column such as `V.....` / `A....D`.
            let name = parts.next()?;
            (flags.len() == 6
                && name != "="
                && flags.chars().all(|c| c.is_ascii_alphabetic() || c == '.'))
            .then(|| name.to_string())
        })
        .collect()
}

/// Muxer names from `ffmpeg -muxers`. Rows look like `  E  mp4   MP4 ...`
/// (older builds: ` E mp4 ...` or ` DE ...`); the flag column holds only
/// `D`, `E`, `d` and `.`.
pub fn parse_muxer_list(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let flags = parts.next()?;
            let name = parts.next()?;
            (name != "="
                && flags.contains('E')
                && flags.chars().all(|c| matches!(c, 'D' | 'E' | 'd' | '.')))
            .then_some(name)
        })
        .flat_map(|name| name.split(',').map(str::to_string))
        .collect()
}

pub fn has_encoder(ff: &Ffmpeg, name: &str) -> bool {
    encoder_names(ff).iter().any(|n| n == name)
}

/// H.264 encoders this FFmpeg has, in the order to try them.
pub fn h264_candidates(ff: &Ffmpeg) -> Vec<&'static str> {
    let names = encoder_names(ff);
    H264_ORDER
        .iter()
        .copied()
        .filter(|c| names.iter().any(|n| n == c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn a_process_that_hangs_is_killed_at_the_deadline() {
        let mut cmd = Command::new("ping");
        cmd.args(["-n", "30", "127.0.0.1"]).stdin(Stdio::null());
        let started = Instant::now();
        let err = output_limited(cmd, Duration::from_millis(300), 1024)
            .err()
            .expect("must time out");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(windows)]
    #[test]
    fn a_flood_of_output_is_capped_and_the_process_still_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.txt");
        std::fs::write(&big, vec![b'x'; 2 * 1024 * 1024]).unwrap();
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg("type").arg(&big).stdin(Stdio::null());
        let out = output_limited(cmd, Duration::from_secs(20), 1000).unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), 1000);
    }

    #[test]
    fn parses_encoder_rows_and_skips_the_legend() {
        let text = "Encoders:\n V..... = Video\n ------\n V....D libvpx-vp9           libvpx VP9\n A....D aac                  AAC\n V.S... h264_mf              H264 via MediaFoundation\n";
        let names = parse_encoder_list(text);
        assert_eq!(names, ["libvpx-vp9", "aac", "h264_mf"]);
    }

    #[test]
    fn parses_muxer_rows_old_and_new_layouts() {
        let new = "Formats:
 D.. = Demuxing supported
 .E. = Muxing supported
 ..d = Is a device
 ---
  E  mp4             MP4 (MPEG-4 Part 14)
  E  matroska        Matroska
 D   aac             raw ADTS
";
        assert_eq!(parse_muxer_list(new), ["mp4", "matroska"]);
        let old = "File formats:
 D. = Demuxing supported
 .E = Muxing supported
 --
  E webm            WebM
 DE wav             WAV
";
        assert_eq!(parse_muxer_list(old), ["webm", "wav"]);
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_complete_build_misses_nothing() {
        let mut enc = names(REQUIRED_ENCODERS);
        enc.push("h264_mf".into());
        assert!(missing_features(&enc, &names(REQUIRED_MUXERS)).is_empty());
    }

    #[test]
    fn a_build_with_only_x264_or_without_opus_is_not_usable() {
        let mut enc = names(REQUIRED_ENCODERS);
        enc.push("libx264".into());
        let missing = missing_features(&enc, &names(REQUIRED_MUXERS));
        assert_eq!(missing, ["an LGPL-safe H.264 encoder"]);

        let enc: Vec<String> = names(&[
            "aac",
            "libvpx-vp9",
            "gif",
            "pcm_s16le",
            "pcm_s24le",
            "h264_nvenc",
        ]);
        let missing = missing_features(
            &enc,
            &names(&["mp4", "matroska", "mov", "gif", "wav", "ipod", "adts"]),
        );
        assert_eq!(missing, ["encoder libopus", "muxer webm"]);
    }

    #[test]
    fn system_dirs_keep_path_order_then_package_folders_without_duplicates() {
        let root = std::env::temp_dir();
        let a = root.join("a");
        let b = root.join("b");
        let path =
            std::env::join_paths([a.clone(), PathBuf::from("relative"), b.clone(), a.clone()])
                .unwrap();
        let local = root.join("local");
        let env = |var: &str| match var {
            "PATH" => Some(path.clone()),
            "LOCALAPPDATA" => Some(local.clone().into_os_string()),
            "USERPROFILE" => Some(root.join("me").into_os_string()),
            _ => None,
        };
        let dirs = system_dirs(env);
        let mut want = vec![a.clone(), b.clone()];
        if cfg!(windows) {
            want.push(local.join("Microsoft").join("WinGet").join("Links"));
            want.push(root.join("me").join("scoop").join("shims"));
        }
        assert_eq!(dirs, want);
    }

    #[test]
    fn no_environment_means_no_candidates() {
        assert!(system_dirs(|_| None).is_empty());
    }

    #[test]
    fn h264_order_never_includes_gpl_encoders() {
        assert!(H264_ORDER.iter().all(|e| !e.contains("x264")));
        assert!(H264_ORDER.iter().all(|e| !e.contains("x265")));
    }
}
