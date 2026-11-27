//! Video family. Container changes are stream copies when the target can hold
//! the streams; everything else runs through an FFmpeg that the app downloads
//! on first use (LGPL build, never bundled). See `install` and the README.
pub mod execute;
pub mod ffmpeg;
pub mod formats;
#[cfg(feature = "ffmpeg-download")]
pub mod install;
pub mod media;
pub mod pin;
pub mod plan;
pub mod run;

use std::path::Path;

use crate::app_error::AppError;
use crate::conversion::options::{Control, OptionSpec, OptionValue, Scope};
use crate::conversion::traits::{ConverterHandler, ExecContext};
use crate::conversion::types::{
    find_descriptor, ConversionCapability, ConversionRequest, ConversionResult, FormatDescriptor,
    InputFormat, OutputFormat,
};
pub use formats::VIDEO_FORMATS;

pub struct VideoHandler;

/// Quality of a re-encode (MP4, MKV, MOV, WebM). The bounds are the ones
/// `validate` checks; unset is the same 90 `execute` uses.
pub static QUALITY: OptionSpec = OptionSpec {
    key: "quality",
    control: Control::Slider { min: 1, max: 100 },
    default: OptionValue::Int(90),
    unit: None,
    scope: Scope::Format,
    quick: true,
    show_when: None,
    note: Some("used when the video is re-encoded; a container change that can copy the streams keeps them as they are"),
};

/// Scale a taller video down to this height (MP4, MKV, MOV, WebM). Unset
/// keeps the size; a smaller video is never scaled up.
pub static MAX_HEIGHT: OptionSpec = OptionSpec {
    key: "max_height",
    control: Control::Number {
        min: MAX_HEIGHT_RANGE.0 as i64,
        max: MAX_HEIGHT_RANGE.1 as i64,
        presets: &[2160, 1440, 1080, 720, 480],
    },
    default: OptionValue::Unset,
    unit: Some("p"),
    scope: Scope::Format,
    quick: true,
    show_when: None,
    note: Some("scales a taller video down (re-encodes it); unset keeps the size"),
};

/// Heights `max_height` accepts.
const MAX_HEIGHT_RANGE: (u32, u32) = (144, 4320);

/// Container ids `sniff` can return for a file this handler reads.
fn sniff_matches(extension_id: &str, content: &str) -> bool {
    match extension_id {
        "mp4" | "mov" => matches!(content, "mp4" | "m4a" | "mov"),
        "mkv" | "webm" => matches!(content, "mkv" | "webm"),
        "avi" => content == "avi",
        _ => false,
    }
}

impl ConverterHandler for VideoHandler {
    fn format_descriptors(&self) -> &'static [FormatDescriptor] {
        VIDEO_FORMATS
    }

    /// Signature-first, and needs no FFmpeg: the queue must work offline.
    fn probe(&self, path: &Path) -> Result<InputFormat, AppError> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let descriptor = find_descriptor(VIDEO_FORMATS, &ext)
            .filter(|d| d.can_read)
            .ok_or_else(|| AppError::Probe(format!("Unsupported video extension: {ext}")))?;
        let content = crate::sniff::sniff_file(path)
            .ok_or_else(|| AppError::Probe("Not a recognised video container".into()))?;
        if !sniff_matches(descriptor.id, content) {
            return Err(AppError::Probe(format!(
                "File content ({content}) does not match its .{ext} name"
            )));
        }
        Ok(InputFormat(descriptor.id.to_owned()))
    }

    fn capabilities(&self) -> Vec<ConversionCapability> {
        VIDEO_FORMATS
            .iter()
            .filter(|source| source.can_read)
            .map(|source| ConversionCapability {
                input: InputFormat(source.id.to_owned()),
                outputs: VIDEO_FORMATS
                    .iter()
                    .filter(|dest| dest.can_write && dest.id != source.id)
                    .map(|dest| OutputFormat(dest.id.to_owned()))
                    .collect(),
            })
            .collect()
    }

    /// Containers take a quality. Audio targets take the audio family's
    /// options for that format (WAV: sample rate and mono, applied by
    /// FFmpeg); GIF has no setting.
    fn output_options(&self, target: &str) -> Vec<&'static OptionSpec> {
        match formats::target_of(target) {
            Some(formats::Target::Container(_)) => vec![&QUALITY, &MAX_HEIGHT],
            Some(formats::Target::Audio(_)) => crate::handlers::audio::formats::find(target)
                .filter(|module| module.descriptor.can_write)
                .map_or_else(Vec::new, |module| module.options.to_vec()),
            Some(formats::Target::Gif) | None => Vec::new(),
        }
    }

    fn validate(&self, request: &ConversionRequest) -> Result<(), AppError> {
        let input = Path::new(&request.input_path);
        if !input.exists() {
            return Err(AppError::Validate(format!(
                "Input file does not exist: {}",
                request.input_path
            )));
        }
        let ext = input
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let target = request.output_format.0.to_lowercase();
        if let Some(q) = request.options.get("quality").filter(|q| !q.is_null()) {
            if !q.as_u64().is_some_and(|q| (1..=100).contains(&q)) {
                return Err(AppError::Validate(format!(
                    "Quality must be between 1 and 100, got: {q}"
                )));
            }
        }
        if let Some(h) = request.options.get(MAX_HEIGHT.key).filter(|h| !h.is_null()) {
            let (lo, hi) = MAX_HEIGHT_RANGE;
            if !h
                .as_u64()
                .is_some_and(|h| (u64::from(lo)..=u64::from(hi)).contains(&h))
            {
                return Err(AppError::Validate(format!(
                    "Maximum height must be between {lo} and {hi} pixels, got: {h}"
                )));
            }
        }
        if matches!(formats::target_of(&target), Some(formats::Target::Audio(_))) {
            crate::handlers::audio::shape::AudioOptions::parse(&request.options)?;
        }
        let source = find_descriptor(VIDEO_FORMATS, &ext).filter(|d| d.can_read);
        let dest = find_descriptor(VIDEO_FORMATS, &target).filter(|d| d.can_write);
        match (source, dest) {
            (Some(s), Some(d)) if s.id != d.id => Ok(()),
            _ => Err(AppError::Validate(format!(
                "Unsupported video conversion route: {ext} -> {target}"
            ))),
        }
    }

    fn execute(&self, request: &ConversionRequest) -> Result<ConversionResult, AppError> {
        execute::execute_video(request, &ExecContext::none())
    }

    fn execute_with(
        &self,
        request: &ConversionRequest,
        ctx: &ExecContext,
    ) -> Result<ConversionResult, AppError> {
        execute::execute_video(request, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::types::AssetFamily;
    use crate::test_utils::{create_malformed_file, make_request, MalformedMode};

    #[test]
    fn every_target_declares_options_that_validation_agrees_with() {
        // AVI is read only, so every writable target is a route from it.
        let input = create_malformed_file(MalformedMode::Garbage, "avi");
        let handler = VideoHandler;
        for target in VIDEO_FORMATS.iter().filter(|d| d.can_write) {
            let specs = handler.output_options(target.id);
            crate::conversion::options::spec_tests::specs_agree_with_validation(
                target.id,
                &specs,
                &|options| {
                    handler.validate(&make_request(
                        AssetFamily::Video,
                        input.path(),
                        Path::new("out.tmp"),
                        target.id,
                        Some(options),
                    ))
                },
            );
        }
        let keys =
            |t: &str| -> Vec<&str> { handler.output_options(t).iter().map(|s| s.key).collect() };
        assert_eq!(keys("mp4"), ["quality", "max_height"]);
        assert!(keys("gif").is_empty());
        assert_eq!(keys("wav"), ["sample_rate", "channels"]);
        assert_eq!(keys("m4a"), ["bitrate_kbps", "sample_rate", "channels"]);
        assert_eq!(keys("flac"), ["sample_rate", "channels"]);
    }

    #[test]
    fn capabilities_are_every_writable_format_except_the_source() {
        let caps = VideoHandler.capabilities();
        assert_eq!(caps.len(), 5, "mp4, mkv, mov, webm, avi");
        let mp4 = caps.iter().find(|c| c.input.0 == "mp4").unwrap();
        let outs: Vec<&str> = mp4.outputs.iter().map(|o| o.0.as_str()).collect();
        let mut expected = vec![
            "mkv", "mov", "webm", "gif", "wav", "mp3", "flac", "aiff", "opus", "m4a", "aac",
        ];
        if !cfg!(feature = "mp3") {
            expected.retain(|f| *f != "mp3");
        }
        assert_eq!(outs, expected);
        let avi = caps.iter().find(|c| c.input.0 == "avi").unwrap();
        assert!(avi.outputs.iter().any(|o| o.0 == "mp4"));
        assert!(caps.iter().all(|c| c.input.0 != "gif"));
    }

    #[test]
    fn supported_extensions_are_readable_formats_only() {
        let exts = VideoHandler.supported_extensions();
        for e in ["mp4", "m4v", "mkv", "mov", "webm", "avi"] {
            assert!(exts.contains(&e), "{e}");
        }
        for e in ["gif", "mp3", "wav", "m4a"] {
            assert!(!exts.contains(&e), "{e} belongs to another family");
        }
    }

    #[test]
    fn validate_accepts_routes_and_rejects_the_rest() {
        let input = create_malformed_file(MalformedMode::Garbage, "mkv");
        let ok = |target: &str| {
            let req = make_request(
                AssetFamily::Video,
                input.path(),
                Path::new("out"),
                target,
                None,
            );
            VideoHandler.validate(&req)
        };
        assert!(ok("mp4").is_ok());
        assert!(ok("gif").is_ok());
        assert_eq!(ok("mp3").is_ok(), cfg!(feature = "mp3"));
        assert!(ok("mkv").is_err(), "same format");
        assert!(ok("avi").is_err(), "avi is read-only");
        assert!(ok("png").is_err());
    }

    #[test]
    fn probe_checks_the_signature_without_ffmpeg() {
        let mut mp4 = tempfile::Builder::new().suffix(".mp4").tempfile().unwrap();
        std::io::Write::write_all(&mut mp4, b"\0\0\0\x20ftypisom\0\0\x02\0isomiso2").unwrap();
        assert_eq!(VideoHandler.probe(mp4.path()).unwrap().0, "mp4");

        let mut mkv = tempfile::Builder::new().suffix(".webm").tempfile().unwrap();
        std::io::Write::write_all(&mut mkv, b"\x1A\x45\xDF\xA3\x9Fwebm").unwrap();
        assert_eq!(VideoHandler.probe(mkv.path()).unwrap().0, "webm");

        let fake = create_malformed_file(MalformedMode::Garbage, "mp4");
        assert!(VideoHandler.probe(fake.path()).is_err());
    }
}
