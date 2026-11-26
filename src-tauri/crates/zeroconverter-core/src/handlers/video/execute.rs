//! Video conversions. Container changes copy streams whenever the target can
//! hold them; everything is written to a workspace next to the output and
//! renamed into place only on success, like the other handlers.
use crate::handlers::audio::shape::AudioOptions;
use std::path::{Path, PathBuf};

use super::ffmpeg::{self, Ffmpeg};
use super::formats::{target_of, AudioTarget, Container, Target};
use super::media::{self, MediaInfo};
use super::plan;
use super::run::{self, Run, RunError};
use crate::app_error::AppError;
use crate::conversion::traits::ExecContext;
use crate::conversion::types::{ConversionRequest, ConversionResult};

pub const FFMPEG_MISSING: &str = "FFmpeg is needed for this conversion. Download it in ZeroConverter under Settings → Video, or install FFmpeg and put it on PATH.";

fn cancelled() -> AppError {
    AppError::Cancelled
}

fn map_run(e: RunError) -> AppError {
    match e {
        RunError::Cancelled => cancelled(),
        RunError::Failed(m) => AppError::Execute(m),
        RunError::Spawn(m) => AppError::Execute(format!("Could not start FFmpeg: {m}")),
    }
}

fn ok(output: &Path, message: String) -> ConversionResult {
    ConversionResult {
        success: true,
        output_path: Some(output.to_string_lossy().into_owned()),
        message,
        input_integrity: None,
        output_integrity: None,
    }
}

struct Workspace {
    inner: zero_fs::TempWorkspace,
}

impl Workspace {
    fn next_to(output: &Path) -> Result<Self, AppError> {
        let parent = match output.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        zero_fs::TempWorkspace::create(parent)
            .map(|inner| Self { inner })
            .map_err(|e| AppError::Execute(format!("Failed to create video workspace: {e}")))
    }

    fn file(&self, name: &str) -> PathBuf {
        self.inner.path().join(name)
    }
}

fn commit(temporary: &Path, output: &Path, clobber: zero_fs::Clobber) -> Result<(), AppError> {
    zero_fs::commit(temporary, output, clobber)
        .map_err(|e| AppError::Io(format!("Failed to save the video file: {e}")))
}

fn quality_of(request: &ConversionRequest) -> u8 {
    request
        .options
        .get("quality")
        .and_then(|q| q.as_u64())
        .map_or(90, |q| q.clamp(1, 100) as u8)
}

fn require_ffmpeg() -> Result<Ffmpeg, AppError> {
    ffmpeg::locate().ok_or_else(|| AppError::Execute(FFMPEG_MISSING.into()))
}

fn read_media(ff: &Ffmpeg, input: &Path) -> Result<MediaInfo, AppError> {
    media::probe(ff, input).map_err(|e| AppError::Execute(e.to_string()))
}

pub fn execute_video(
    request: &ConversionRequest,
    ctx: &ExecContext,
) -> Result<ConversionResult, AppError> {
    let input = Path::new(&request.input_path);
    let output = Path::new(&request.output_path);
    let target = target_of(&request.output_format.0).ok_or_else(|| {
        AppError::Execute(format!(
            "Unsupported video output format: {}",
            request.output_format.0
        ))
    })?;
    if ctx.is_cancelled() {
        return Err(cancelled());
    }
    match target {
        Target::Container(c) => to_container(
            input,
            output,
            c,
            quality_of(request),
            request.clobber(),
            ctx,
        ),
        Target::Gif => to_gif(input, output, request.clobber(), ctx),
        Target::Audio(a) => extract_audio(
            input,
            output,
            a,
            &AudioOptions::parse(&request.options)?,
            ctx,
        ),
    }
}

// ── container change ────────────────────────────────────────────────────────

fn to_container(
    input: &Path,
    output: &Path,
    target: Container,
    quality: u8,
    clobber: zero_fs::Clobber,
    ctx: &ExecContext,
) -> Result<ConversionResult, AppError> {
    let ff = require_ffmpeg()?;
    ctx.report(1.0, "Reading streams...");
    let info = read_media(&ff, input)?;
    let work = Workspace::next_to(output)?;
    let temporary = work.file("video.tmp");

    let candidates: Vec<Option<&str>> = if plan::needs_h264(target, &info) {
        let found = ffmpeg::h264_candidates(&ff);
        if found.is_empty() {
            return Err(AppError::Execute(
                "This FFmpeg has no usable H.264 encoder (Media Foundation, NVENC, QSV, AMF or OpenH264).".into(),
            ));
        }
        found.into_iter().map(Some).collect()
    } else {
        vec![None]
    };

    let mut last_error = None;
    let mut chosen = None;
    for encoder in candidates {
        let plan = plan::plan_container(target, &info, encoder, quality);
        let mut args = plan.args.clone();
        args.extend(["-f".into(), target.muxer().into()]);
        let label = if plan.is_pure_copy() {
            "Copying streams..."
        } else {
            "Re-encoding..."
        };
        let attempt = run::run(
            &Run {
                ff: &ff,
                args,
                output: &temporary,
                duration: info.duration,
                lo: 2.0,
                hi: 100.0,
                label,
            },
            ctx,
            input,
        );
        match attempt {
            Ok(()) => {
                chosen = Some(plan);
                break;
            }
            Err(RunError::Cancelled) => return Err(cancelled()),
            // A hardware encoder can be listed yet fail to start: try the next.
            Err(e) => last_error = Some(map_run(e)),
        }
    }
    let Some(plan) = chosen else {
        return Err(last_error.unwrap_or_else(|| AppError::Execute("FFmpeg failed".into())));
    };
    commit(&temporary, output, clobber)?;
    Ok(ok(output, describe(target, &plan)))
}

fn describe(target: Container, plan: &plan::ContainerPlan) -> String {
    let name = match target {
        Container::Mp4 => "MP4",
        Container::Mkv => "MKV",
        Container::Mov => "MOV",
        Container::Webm => "WebM",
    };
    let mut text = if plan.is_pure_copy() {
        format!("Converted to {name} without re-encoding")
    } else {
        let mut parts = Vec::new();
        if let Some(enc) = &plan.video_encoder {
            let codec = if enc == plan::VP9 { "VP9" } else { "H.264" };
            parts.push(format!("video re-encoded to {codec} ({enc})"));
        } else {
            parts.push("video copied".into());
        }
        if plan.audio_reencoded > 0 {
            parts.push(format!(
                "{} audio track(s) re-encoded",
                plan.audio_reencoded
            ));
        }
        format!("Converted to {name}: {}", parts.join(", "))
    };
    if !plan.dropped.is_empty() {
        text.push_str(&format!(". Not carried over: {}", plan.dropped.join(", ")));
    }
    text
}

// ── GIF ─────────────────────────────────────────────────────────────────────

fn to_gif(
    input: &Path,
    output: &Path,
    clobber: zero_fs::Clobber,
    ctx: &ExecContext,
) -> Result<ConversionResult, AppError> {
    let ff = require_ffmpeg()?;
    ctx.report(1.0, "Reading streams...");
    let info = read_media(&ff, input)?;
    if info.primary_video().is_none() {
        return Err(AppError::Execute(
            "This file has no video to turn into a GIF".into(),
        ));
    }
    if info.duration.is_some_and(|d| d > plan::GIF_MAX_SECONDS) {
        return Err(AppError::Execute(format!(
            "GIF output is limited to {} minutes of video. Trim the clip first.",
            plan::GIF_MAX_SECONDS / 60.0
        )));
    }
    let work = Workspace::next_to(output)?;
    let palette = work.file("palette.png");
    let temporary = work.file("video.tmp");

    run::run(
        &Run {
            ff: &ff,
            args: [
                "-an",
                "-vf",
                &format!("{},palettegen=stats_mode=diff", plan::GIF_BASE),
                "-t",
                &plan::GIF_MAX_SECONDS.to_string(),
                "-update",
                "1",
                "-f",
                "image2",
            ]
            .map(String::from)
            .to_vec(),
            output: &palette,
            duration: info.duration,
            lo: 2.0,
            hi: 40.0,
            label: "Building color palette...",
        },
        ctx,
        input,
    )
    .map_err(map_run)?;

    // Second pass reads the source again, with the palette as input 1.
    let args = vec![
        "-i".to_string(),
        palette.to_string_lossy().into_owned(),
        "-an".into(),
        "-lavfi".into(),
        format!(
            "{} [x]; [x][1:v] paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle",
            plan::GIF_BASE
        ),
        // The length check above needs a known duration; this holds anyway.
        "-t".into(),
        plan::GIF_MAX_SECONDS.to_string(),
        "-loop".into(),
        "0".into(),
        "-f".into(),
        "gif".into(),
    ];
    run::run(
        &Run {
            ff: &ff,
            args,
            output: &temporary,
            duration: info.duration,
            lo: 40.0,
            hi: 100.0,
            label: "Writing GIF...",
        },
        ctx,
        input,
    )
    .map_err(map_run)?;
    commit(&temporary, output, clobber)?;
    Ok(ok(
        output,
        "Converted to GIF (15 fps, up to 480 px wide, 256-color palette)".into(),
    ))
}

// ── audio extraction ────────────────────────────────────────────────────────

/// An audio file to a target only FFmpeg writes (M4A, AAC): the same path as
/// a video's audio track, stream copy when the source is already AAC.
pub fn audio_via_ffmpeg(
    request: &ConversionRequest,
    ctx: &ExecContext,
) -> Result<ConversionResult, AppError> {
    let Some(Target::Audio(target)) = target_of(&request.output_format.0) else {
        return Err(AppError::Execute(format!(
            "Unsupported audio output format: {}",
            request.output_format.0
        )));
    };
    if ctx.is_cancelled() {
        return Err(cancelled());
    }
    extract_audio(
        Path::new(&request.input_path),
        Path::new(&request.output_path),
        target,
        &AudioOptions::parse(&request.options)?,
        ctx,
    )
    .map(|mut done| {
        // An audio file is converted, not extracted from a video.
        done.message = format!("Converted to {}", audio_id(target).to_uppercase());
        done
    })
}

fn audio_id(a: AudioTarget) -> &'static str {
    match a {
        AudioTarget::Wav => "wav",
        AudioTarget::Mp3 => "mp3",
        AudioTarget::Flac => "flac",
        AudioTarget::Aiff => "aiff",
        AudioTarget::Opus => "opus",
        AudioTarget::M4a => "m4a",
        AudioTarget::Aac => "aac",
    }
}

fn extract_audio(
    input: &Path,
    output: &Path,
    target: AudioTarget,
    options: &AudioOptions,
    ctx: &ExecContext,
) -> Result<ConversionResult, AppError> {
    let clobber = crate::conversion::types::clobber(options.overwrite);
    let encoder = crate::handlers::audio::formats::find(audio_id(target)).and_then(|m| m.encoder);
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // MP4/MOV with an AAC/MP3/... track: the pure-Rust decoder handles it, so
    // no FFmpeg (and no download) is needed. Anything it cannot read falls
    // through to FFmpeg.
    if let (Some(encode), "mp4" | "m4v" | "mov") = (encoder, ext.as_str()) {
        ctx.report(5.0, "Decoding audio...");
        if let Ok(done) = encode(input, output, options) {
            return Ok(done);
        }
        if ctx.is_cancelled() {
            return Err(cancelled());
        }
    }

    let ff = require_ffmpeg()?;
    ctx.report(1.0, "Reading streams...");
    let info = read_media(&ff, input)?;
    let no_audio = || AppError::Execute("This video has no audio track".into());
    let work = Workspace::next_to(output)?;
    let temporary = work.file("audio.tmp");

    if let Some(args) = plan::plan_audio_direct(target, &info) {
        run::run(
            &Run {
                ff: &ff,
                args,
                output: &temporary,
                duration: info.duration,
                lo: 2.0,
                hi: 100.0,
                label: "Extracting audio...",
            },
            ctx,
            input,
        )
        .map_err(map_run)?;
        commit(&temporary, output, clobber)?;
        return Ok(ok(
            output,
            format!("Extracted audio to {}", audio_id(target).to_uppercase()),
        ));
    }

    let args = plan::plan_audio_wav(target, &info).ok_or_else(no_audio)?;
    if target == AudioTarget::Wav {
        run::run(
            &Run {
                ff: &ff,
                args,
                output: &temporary,
                duration: info.duration,
                lo: 2.0,
                hi: 100.0,
                label: "Extracting audio...",
            },
            ctx,
            input,
        )
        .map_err(map_run)?;
        commit(&temporary, output, clobber)?;
        return Ok(ok(output, "Extracted audio to WAV".into()));
    }

    let wav = work.file("audio.wav");
    run::run(
        &Run {
            ff: &ff,
            args,
            output: &wav,
            duration: info.duration,
            lo: 2.0,
            hi: 70.0,
            label: "Extracting audio...",
        },
        ctx,
        input,
    )
    .map_err(map_run)?;
    if ctx.is_cancelled() {
        return Err(cancelled());
    }
    ctx.report(75.0, "Encoding audio...");
    let encode = encoder.ok_or_else(|| {
        AppError::Execute(format!(
            "No encoder for {}",
            audio_id(target).to_uppercase()
        ))
    })?;
    encode(&wav, output, options)?;
    Ok(ok(
        output,
        format!("Extracted audio to {}", audio_id(target).to_uppercase()),
    ))
}
