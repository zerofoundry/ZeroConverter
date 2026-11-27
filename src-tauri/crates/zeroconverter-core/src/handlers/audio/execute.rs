use crate::app_error::AppError;
use crate::conversion::traits::ExecContext;
use crate::conversion::types::{ConversionRequest, ConversionResult};
use crate::handlers::audio::formats;
use crate::handlers::audio::shape::AudioOptions;
use std::path::Path;

/// The Symphonia demuxer signals end-of-stream as an unexpected EOF.
/// Keep the legacy helper for its explicit error-classification regression tests.
pub(crate) fn is_end_of_stream(error: &symphonia::core::errors::Error) -> bool {
    matches!(
        error,
        symphonia::core::errors::Error::IoError(io_error)
            if io_error.kind() == std::io::ErrorKind::UnexpectedEof
    )
}

/// All audio output formats commit through a short-lived RAII workspace.
pub(crate) fn write_atomically<F>(
    output: &Path,
    options: &AudioOptions,
    work: F,
) -> Result<(), AppError>
where
    F: FnOnce(&Path) -> Result<(), AppError>,
{
    let parent = match output.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let workspace = zero_fs::TempWorkspace::create(parent)
        .map_err(|e| AppError::Execute(format!("Failed to create audio workspace: {e}")))?;
    let temporary = workspace.path().join("audio.tmp");
    work(&temporary)?;
    zero_fs::commit(
        &temporary,
        output,
        crate::conversion::types::clobber(options.overwrite),
    )
    .map_err(|e| AppError::Io(format!("Failed to save the audio file: {e}")))?;
    Ok(())
}

pub(crate) fn success_result(output: &Path, message: &str) -> ConversionResult {
    ConversionResult {
        success: true,
        output_path: Some(output.to_string_lossy().to_string()),
        message: message.to_owned(),
        input_integrity: None,
        output_integrity: None,
    }
}

pub fn execute_audio(request: &ConversionRequest) -> Result<ConversionResult, AppError> {
    execute_audio_with(request, &ExecContext::none())
}

pub fn execute_audio_with(
    request: &ConversionRequest,
    ctx: &ExecContext,
) -> Result<ConversionResult, AppError> {
    let input = Path::new(&request.input_path);
    let output = Path::new(&request.output_path);
    let target = request.output_format.0.to_lowercase();
    let module = formats::find(&target)
        .ok_or_else(|| AppError::Execute(format!("Unsupported audio output format: {target}")))?;
    if module.via_ffmpeg() {
        return crate::handlers::video::execute::audio_via_ffmpeg(request, ctx);
    }
    let encode = module.encoder.ok_or_else(|| {
        AppError::Execute(format!("Unsupported output format: {target} (read-only)"))
    })?;
    encode(input, output, &AudioOptions::from_json(&request.options))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::types::AssetFamily;
    use crate::handlers::audio::pcm;
    use crate::handlers::audio::probe::probe_audio;
    use crate::test_utils::{create_malformed_file, create_test_wav, make_request, MalformedMode};
    use hound::{WavSpec, WavWriter};

    /// A stereo WAV at `bits` whose samples include both extremes of the range.
    fn wav_with_extremes(bits: u16) -> (tempfile::NamedTempFile, Vec<i32>) {
        let max = (1i32 << (bits - 1)) - 1;
        let min = -(1i32 << (bits - 1));
        let mut samples = vec![min, max, 0, 1, -1, max - 1, min + 1, 12_345 % max];
        samples.extend((0..4000).map(|i| ((i * 7919) % (2 * max)) - max));
        let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let spec = WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: bits,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = WavWriter::create(file.path(), spec).unwrap();
        for &s in &samples {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
        (file, samples)
    }

    /// Converts and decodes back to integers at `bits`.
    fn round_trip(input: &Path, ext: &str, bits: u32) -> (Vec<i32>, u32) {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join(format!("out.{ext}"));
        let req = make_request(AssetFamily::Audio, input, &out, ext, None);
        execute_audio(&req).unwrap();
        let mut back = Vec::new();
        pcm::stream_pcm(&out, |s, _| {
            back.extend(s.iter().map(|&x| pcm::to_int(x, bits)));
            Ok(())
        })
        .unwrap();
        (back, probe_audio(&out).unwrap().bits_per_sample)
    }

    #[test]
    fn lossless_outputs_keep_24_bit_samples_exactly() {
        let (input, samples) = wav_with_extremes(24);
        for ext in ["wav", "flac", "aiff"] {
            let (back, bits) = round_trip(input.path(), ext, 24);
            assert_eq!(bits, 24, "{ext} must stay 24-bit");
            assert!(back == samples, "{ext} changed 24-bit samples");
        }
    }

    #[test]
    fn lossless_outputs_keep_16_bit_samples_exactly() {
        let (input, samples) = wav_with_extremes(16);
        for ext in ["wav", "flac", "aiff"] {
            let (back, bits) = round_trip(input.path(), ext, 16);
            assert_eq!(bits, 16, "{ext} must stay 16-bit");
            assert!(back == samples, "{ext} changed 16-bit samples");
        }
    }

    /// Assert no TempWorkspace debris (zero_tmp_* dirs) exists in a directory.
    fn assert_no_temp_workspace_debris(dir: &std::path::Path) {
        let debris: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("zero_tmp_"))
            .collect();
        assert!(
            debris.is_empty(),
            "Expected no zero_tmp_ debris in {:?}, found: {:?}",
            dir,
            debris.iter().map(|e| e.file_name()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn execute_audio_converts_wav_to_mp3_successfully() {
        let input = create_test_wav(2, 44100, 100);
        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("result.mp3");

        let req = make_request(AssetFamily::Audio, input.path(), &output_path, "mp3", None);

        let result = execute_audio(&req);
        assert!(result.is_ok(), "Execution failed: {:?}", result.err());

        let res = result.unwrap();
        assert!(res.success);
        assert!(output_path.exists(), "Output file should exist");
        assert!(
            output_path.metadata().unwrap().len() > 0,
            "Output file should not be empty"
        );

        // Lightweight probe check: Ensure the output is at least a valid audio file
        let probe_res = probe_audio(&output_path);
        assert!(
            probe_res.is_ok(),
            "Output MP3 should be probeable: {:?}",
            probe_res.err()
        );

        assert_no_temp_workspace_debris(output_dir.path());
    }

    /// With FFmpeg: WAV -> M4A (AAC encode) -> AAC (stream copy). Without it:
    /// the typed "needs FFmpeg" error the rename flow turns into the download
    /// prompt, and nothing on disk.
    #[test]
    fn wav_becomes_m4a_and_aac_through_ffmpeg() {
        use crate::handlers::video::execute::FFMPEG_MISSING;
        let input = create_test_wav(2, 44100, 500);
        let dir = tempfile::tempdir().unwrap();
        let m4a = dir.path().join("song.m4a");
        let result = execute_audio(&make_request(
            AssetFamily::Audio,
            input.path(),
            &m4a,
            "m4a",
            None,
        ));
        if crate::handlers::video::ffmpeg::locate().is_none() {
            assert!(
                matches!(&result, Err(AppError::Execute(m)) if m == FFMPEG_MISSING),
                "{result:?}"
            );
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
            return;
        }
        result.unwrap();
        let head = std::fs::read(&m4a).unwrap();
        assert_eq!(&head[4..8], b"ftyp", "an MP4 container");
        probe_audio(&m4a).unwrap();

        let aac = dir.path().join("song.aac");
        execute_audio(&make_request(AssetFamily::Audio, &m4a, &aac, "aac", None)).unwrap();
        let head = std::fs::read(&aac).unwrap();
        assert!(head[0] == 0xFF && head[1] & 0xF0 == 0xF0, "an ADTS stream");
        probe_audio(&aac).unwrap();
        assert_no_temp_workspace_debris(dir.path());
    }

    /// The settings asked for reach FFmpeg: 64 kbps mono at 48 kHz.
    #[test]
    fn m4a_honours_bitrate_sample_rate_and_mono() {
        if crate::handlers::video::ffmpeg::locate().is_none() {
            return; // covered by the missing-FFmpeg case above
        }
        let input = create_test_wav(2, 44100, 1000);
        let dir = tempfile::tempdir().unwrap();
        let m4a = dir.path().join("song.m4a");
        execute_audio(&make_request(
            AssetFamily::Audio,
            input.path(),
            &m4a,
            "m4a",
            Some(serde_json::json!({ "bitrate_kbps": 64, "sample_rate": 48000, "channels": 1 })),
        ))
        .unwrap();
        let spec = pcm::inspect(&m4a).unwrap();
        assert_eq!((spec.channels, spec.sample_rate), (1, 48000));
        assert_no_temp_workspace_debris(dir.path());
    }

    #[test]
    fn execute_audio_converts_mono_wav_to_mp3_at_full_length() {
        // 1 s of mono. Fed to LAME as interleaved stereo it came out half as long.
        let input = create_test_wav(1, 44100, 1000);
        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("mono.mp3");
        let req = make_request(AssetFamily::Audio, input.path(), &output_path, "mp3", None);
        execute_audio(&req).unwrap();
        let meta = probe_audio(&output_path).unwrap();
        assert!(
            (meta.duration_seconds - 1.0).abs() < 0.15,
            "duration {}",
            meta.duration_seconds
        );
        assert_eq!(meta.channels, 1);
    }

    #[test]
    fn execute_audio_converts_wav_to_flac_successfully() {
        let input = create_test_wav(2, 44100, 100);
        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("result.flac");

        let req = make_request(AssetFamily::Audio, input.path(), &output_path, "flac", None);

        let result = execute_audio(&req);
        assert!(result.is_ok(), "Execution failed: {:?}", result.err());

        let res = result.unwrap();
        assert!(res.success);
        assert!(output_path.exists(), "Output file should exist");
        assert!(
            output_path.metadata().unwrap().len() > 0,
            "Output file should not be empty"
        );

        let probe_res = probe_audio(&output_path);
        assert!(
            probe_res.is_ok(),
            "Output FLAC should be probeable: {:?}",
            probe_res.err()
        );

        assert_no_temp_workspace_debris(output_dir.path());
    }
    #[test]
    fn execute_audio_converts_ogg_to_wav_successfully() {
        let input_path = std::path::Path::new("tests/fixtures/minimal.ogg");
        assert!(
            input_path.exists(),
            "Required OGG test fixture missing: {:?}",
            input_path
        );

        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("result.wav");

        let req = make_request(AssetFamily::Audio, input_path, &output_path, "wav", None);

        let result = execute_audio(&req);
        assert!(
            result.is_ok(),
            "Execution failed for OGG to WAV: {:?}",
            result.err()
        );

        let res = result.unwrap();
        assert!(res.success);
        assert!(output_path.exists(), "Output file should exist");
        assert!(
            output_path.metadata().unwrap().len() > 0,
            "Output file should not be empty"
        );

        let probe_res = probe_audio(&output_path);
        assert!(
            probe_res.is_ok(),
            "Output WAV should be probeable: {:?}",
            probe_res.err()
        );

        assert_no_temp_workspace_debris(output_dir.path());
    }

    #[test]
    fn execute_audio_converts_ogg_to_mp3_successfully() {
        let input_path = std::path::Path::new("tests/fixtures/minimal.ogg");
        assert!(
            input_path.exists(),
            "Required OGG test fixture missing: {:?}",
            input_path
        );

        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("result.mp3");

        let req = make_request(AssetFamily::Audio, input_path, &output_path, "mp3", None);

        let result = execute_audio(&req);
        assert!(
            result.is_ok(),
            "Execution failed for OGG to MP3: {:?}",
            result.err()
        );

        let res = result.unwrap();
        assert!(res.success);
        assert!(output_path.exists());

        assert_no_temp_workspace_debris(output_dir.path());
    }

    #[test]
    fn execute_audio_converts_ogg_to_flac_successfully() {
        let input_path = std::path::Path::new("tests/fixtures/minimal.ogg");
        assert!(
            input_path.exists(),
            "Required OGG test fixture missing: {:?}",
            input_path
        );

        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("result.flac");

        let req = make_request(AssetFamily::Audio, input_path, &output_path, "flac", None);

        let result = execute_audio(&req);
        assert!(
            result.is_ok(),
            "Execution failed for OGG to FLAC: {:?}",
            result.err()
        );

        let res = result.unwrap();
        assert!(res.success);
        assert!(output_path.exists());

        assert_no_temp_workspace_debris(output_dir.path());
    }

    #[test]
    fn execute_audio_converts_flac_to_wav_and_mp3() {
        // Step 1: Create a FLAC file from WAV
        let input_wav = create_test_wav(2, 44100, 100);
        let output_dir = tempfile::tempdir().unwrap();
        let flac_path = output_dir.path().join("intermediate.flac");

        let req_flac = make_request(
            AssetFamily::Audio,
            input_wav.path(),
            &flac_path,
            "flac",
            None,
        );
        assert!(execute_audio(&req_flac).is_ok());
        assert!(flac_path.exists());

        // Step 2: Convert FLAC to WAV
        let wav_path = output_dir.path().join("result.wav");
        let req_wav = make_request(AssetFamily::Audio, &flac_path, &wav_path, "wav", None);
        let result_wav = execute_audio(&req_wav);
        assert!(result_wav.is_ok());
        assert!(probe_audio(&wav_path).is_ok());

        // Step 3: Convert FLAC to MP3
        let mp3_path = output_dir.path().join("result.mp3");
        let req_mp3 = make_request(AssetFamily::Audio, &flac_path, &mp3_path, "mp3", None);
        let result_mp3 = execute_audio(&req_mp3);
        assert!(result_mp3.is_ok());
        assert!(probe_audio(&mp3_path).is_ok());

        assert_no_temp_workspace_debris(output_dir.path());
    }

    #[test]
    fn execute_audio_converts_mp3_to_flac() {
        // Step 1: Create an MP3 file from WAV
        let input_wav = create_test_wav(2, 44100, 100);
        let output_dir = tempfile::tempdir().unwrap();
        let mp3_path = output_dir.path().join("intermediate.mp3");

        let req_mp3 = make_request(AssetFamily::Audio, input_wav.path(), &mp3_path, "mp3", None);
        assert!(execute_audio(&req_mp3).is_ok());
        assert!(mp3_path.exists());

        // Step 2: Convert MP3 to FLAC
        let flac_path = output_dir.path().join("result.flac");
        let req_flac = make_request(AssetFamily::Audio, &mp3_path, &flac_path, "flac", None);
        let result_flac = execute_audio(&req_flac);
        assert!(result_flac.is_ok());
        assert!(probe_audio(&flac_path).is_ok());

        assert_no_temp_workspace_debris(output_dir.path());
    }

    #[test]
    fn execute_audio_fails_on_unsupported_format() {
        let input = create_test_wav(2, 44100, 100);
        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("result.ogg");

        let req = make_request(AssetFamily::Audio, input.path(), &output_path, "ogg", None);

        let result = execute_audio(&req);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unsupported output format"));
    }

    #[test]
    fn execute_audio_fails_on_malformed_input() {
        let input = create_malformed_file(MalformedMode::Garbage, "wav");
        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("result.mp3");

        let req = make_request(AssetFamily::Audio, input.path(), &output_path, "mp3", None);

        let result = execute_audio(&req);
        assert!(result.is_err());
    }

    #[test]
    fn execute_audio_fails_on_malformed_ogg_input() {
        let input = create_malformed_file(MalformedMode::Garbage, "ogg");
        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("result.mp3");

        let req = make_request(AssetFamily::Audio, input.path(), &output_path, "mp3", None);

        let result = execute_audio(&req);
        assert!(result.is_err());
        assert!(!output_path.exists());
    }

    #[test]
    fn test_audio_wav_to_mp3_streaming_outputs_valid_mp3() {
        let input = create_test_wav(2, 44100, 100);
        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("result.mp3");

        let req = make_request(AssetFamily::Audio, input.path(), &output_path, "mp3", None);

        let result = execute_audio(&req);
        assert!(result.is_ok(), "Execution failed: {:?}", result.err());

        let res = result.unwrap();
        assert!(res.success);
        assert!(output_path.exists(), "Output file should exist");
        assert!(
            output_path.metadata().unwrap().len() > 0,
            "Output file should not be empty"
        );

        let probe_res = probe_audio(&output_path);
        assert!(
            probe_res.is_ok(),
            "Output MP3 should be probeable: {:?}",
            probe_res.err()
        );

        assert_no_temp_workspace_debris(output_dir.path());
    }

    #[test]
    fn test_audio_execute_leaves_no_artifacts_on_malformed_input() {
        let input = create_malformed_file(MalformedMode::Garbage, "wav");
        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("error_result.mp3");

        let req = make_request(AssetFamily::Audio, input.path(), &output_path, "mp3", None);

        let result = execute_audio(&req);
        assert!(result.is_err(), "Execution should fail on malformed input");

        assert!(
            !output_path.exists(),
            "Output file should not exist on error"
        );

        assert_no_temp_workspace_debris(output_dir.path());
    }

    #[test]
    fn without_overwrite_an_existing_file_is_left_alone() {
        let input = create_test_wav(2, 44100, 100);
        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("keep.mp3");
        std::fs::write(&output_path, b"someone else's file").unwrap();

        let req = make_request(AssetFamily::Audio, input.path(), &output_path, "mp3", None);
        assert!(execute_audio(&req).is_err());
        assert_eq!(std::fs::read(&output_path).unwrap(), b"someone else's file");
        assert_no_temp_workspace_debris(output_dir.path());
    }

    #[test]
    fn test_audio_execute_overwrite_true_replaces_existing_file() {
        let input = create_test_wav(2, 44100, 100);
        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("overwrite_test.mp3");

        let dummy_content = b"dummy old audio bytes";
        std::fs::write(&output_path, dummy_content).unwrap();

        // The user ticked "Overwrite existing files".
        let overwrite = Some(serde_json::json!({ "overwrite": true }));
        let req = make_request(
            AssetFamily::Audio,
            input.path(),
            &output_path,
            "mp3",
            overwrite,
        );

        let result = execute_audio(&req);
        assert!(
            result.is_ok(),
            "Conversion should succeed: {:?}",
            result.err()
        );
        assert!(output_path.exists(), "Output file must exist");

        let content = std::fs::read(&output_path).unwrap();
        assert_ne!(
            content, dummy_content,
            "Output must not be the old dummy bytes"
        );
        assert!(!content.is_empty(), "Output must not be empty");

        let probe_res = probe_audio(&output_path);
        assert!(
            probe_res.is_ok(),
            "Output should be probeable: {:?}",
            probe_res.err()
        );

        assert_no_temp_workspace_debris(output_dir.path());
    }

    #[test]
    fn test_audio_execute_fails_and_preserves_original_on_malformed_input() {
        let input = create_malformed_file(MalformedMode::Garbage, "wav");
        let output_dir = tempfile::tempdir().unwrap();
        let output_path = output_dir.path().join("preserve_test.mp3");

        let dummy_content = b"original audio bytes for safe preserve test";
        std::fs::write(&output_path, dummy_content).unwrap();

        let req = make_request(AssetFamily::Audio, input.path(), &output_path, "mp3", None);

        let result = execute_audio(&req);
        assert!(result.is_err(), "Conversion should fail on malformed input");
        assert!(
            output_path.exists(),
            "Existing output file must still exist"
        );

        let content = std::fs::read(&output_path).unwrap();
        assert_eq!(
            content, dummy_content,
            "Existing output bytes must remain exactly unchanged"
        );

        assert_no_temp_workspace_debris(output_dir.path());
    }

    #[test]
    fn test_audio_execute_fails_when_output_path_is_directory() {
        let input = create_test_wav(2, 44100, 100);
        // Controlled parent so we don't scan the global temp directory
        let parent_dir = tempfile::tempdir().unwrap();
        let output_path = parent_dir.path().join("target_dir");
        std::fs::create_dir(&output_path).unwrap();

        let req = make_request(AssetFamily::Audio, input.path(), &output_path, "mp3", None);

        let result = execute_audio(&req);
        assert!(result.is_err(), "Should fail when output is a directory");

        assert_no_temp_workspace_debris(parent_dir.path());
    }

    // ═══════════════════════════════════════════
    // EOF Classifier Unit Tests
    // ═══════════════════════════════════════════

    #[test]
    fn test_is_end_of_stream_accepts_unexpected_eof() {
        let err = symphonia::core::errors::Error::IoError(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "end of stream",
        ));
        assert!(is_end_of_stream(&err));
    }

    #[test]
    fn test_is_end_of_stream_rejects_other_io_error() {
        let err = symphonia::core::errors::Error::IoError(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permission denied",
        ));
        assert!(!is_end_of_stream(&err));
    }

    #[test]
    fn test_is_end_of_stream_rejects_decode_error() {
        let err = symphonia::core::errors::Error::DecodeError("corrupt stream data");
        assert!(!is_end_of_stream(&err));
    }

    #[test]
    fn test_is_end_of_stream_rejects_unsupported() {
        let err = symphonia::core::errors::Error::Unsupported("unsupported codec feature");
        assert!(!is_end_of_stream(&err));
    }

    #[test]
    fn test_is_end_of_stream_rejects_reset_required() {
        let err = symphonia::core::errors::Error::ResetRequired;
        assert!(!is_end_of_stream(&err));
    }

    #[test]
    fn test_is_end_of_stream_rejects_limit_error() {
        let err = symphonia::core::errors::Error::LimitError("stream limit exceeded");
        assert!(!is_end_of_stream(&err));
    }
}
