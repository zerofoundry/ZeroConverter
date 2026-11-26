use crate::app_error::AppError;
use crate::conversion::types::ConversionRequest;
use std::path::Path;

pub fn validate_audio(request: &ConversionRequest) -> Result<(), AppError> {
    let input_path = Path::new(&request.input_path);
    if !input_path.exists() {
        return Err(AppError::Validate(format!(
            "Input file does not exist: {}",
            request.input_path
        )));
    }

    super::shape::AudioOptions::parse(&request.options)?;

    let input_ext = input_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let output_format_ext = request.output_format.0.to_lowercase();

    let source = super::formats::find(&input_ext);
    let destination = super::formats::find(&output_format_ext);
    if !matches!(source, Some(module) if module.descriptor.can_read)
        || !matches!(destination, Some(module) if module.descriptor.can_write)
        || source.map(|module| module.descriptor.id)
            == destination.map(|module| module.descriptor.id)
    {
        return Err(AppError::Validate(format!(
            "Unsupported audio conversion route: {} -> {}",
            input_ext, output_format_ext
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::types::AssetFamily;
    use crate::test_utils::{create_malformed_file, make_request, MalformedMode};
    use std::path::Path;

    // ═══════════════════════════════════════════
    // Valid Conversion Routes
    // ═══════════════════════════════════════════

    #[test]
    fn validate_accepts_wav_to_mp3() {
        let input = create_malformed_file(MalformedMode::Garbage, "wav");
        let output = Path::new("output.mp3");
        let req = make_request(AssetFamily::Audio, input.path(), output, "mp3", None);
        assert!(validate_audio(&req).is_ok());
    }

    #[test]
    fn validate_accepts_mp3_to_wav() {
        let input = create_malformed_file(MalformedMode::Garbage, "mp3");
        let output = Path::new("output.wav");
        let req = make_request(AssetFamily::Audio, input.path(), output, "wav", None);
        assert!(validate_audio(&req).is_ok());
    }

    #[test]
    fn validate_accepts_wav_to_flac() {
        let input = create_malformed_file(MalformedMode::Garbage, "wav");
        let output = Path::new("output.flac");
        let req = make_request(AssetFamily::Audio, input.path(), output, "flac", None);
        assert!(validate_audio(&req).is_ok());
    }

    #[test]
    fn validate_accepts_flac_to_wav() {
        let input = create_malformed_file(MalformedMode::Garbage, "flac");
        let output = Path::new("output.wav");
        let req = make_request(AssetFamily::Audio, input.path(), output, "wav", None);
        assert!(validate_audio(&req).is_ok());
    }

    #[test]
    fn validate_accepts_mp3_to_flac() {
        let input = create_malformed_file(MalformedMode::Garbage, "mp3");
        let output = Path::new("output.flac");
        let req = make_request(AssetFamily::Audio, input.path(), output, "flac", None);
        assert!(validate_audio(&req).is_ok());
    }

    #[test]
    fn validate_accepts_flac_to_mp3() {
        let input = create_malformed_file(MalformedMode::Garbage, "flac");
        let output = Path::new("output.mp3");
        let req = make_request(AssetFamily::Audio, input.path(), output, "mp3", None);
        assert!(validate_audio(&req).is_ok());
    }

    #[test]
    fn validate_accepts_ogg_to_wav() {
        let input = create_malformed_file(MalformedMode::Garbage, "ogg");
        let output = Path::new("output.wav");
        let req = make_request(AssetFamily::Audio, input.path(), output, "wav", None);
        assert!(validate_audio(&req).is_ok());
    }

    #[test]
    fn validate_accepts_ogg_to_mp3() {
        let input = create_malformed_file(MalformedMode::Garbage, "ogg");
        let output = Path::new("output.mp3");
        let req = make_request(AssetFamily::Audio, input.path(), output, "mp3", None);
        assert!(validate_audio(&req).is_ok());
    }

    #[test]
    fn validate_accepts_ogg_to_flac() {
        let input = create_malformed_file(MalformedMode::Garbage, "ogg");
        let output = Path::new("output.flac");
        let req = make_request(AssetFamily::Audio, input.path(), output, "flac", None);
        assert!(validate_audio(&req).is_ok());
    }

    // ═══════════════════════════════════════════
    // Unsupported Conversion Routes
    // ═══════════════════════════════════════════

    #[test]
    fn validate_rejects_wav_to_wav_same_format() {
        let input = create_malformed_file(MalformedMode::Garbage, "wav");
        let output = Path::new("output.wav");
        let req = make_request(AssetFamily::Audio, input.path(), output, "wav", None);
        let result = validate_audio(&req);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported audio conversion route"));
    }

    #[test]
    fn validate_rejects_mp3_to_mp3_same_format() {
        let input = create_malformed_file(MalformedMode::Garbage, "mp3");
        let output = Path::new("output.mp3");
        let req = make_request(AssetFamily::Audio, input.path(), output, "mp3", None);
        assert!(validate_audio(&req).is_err());
    }

    #[test]
    fn validate_rejects_flac_to_flac_same_format() {
        let input = create_malformed_file(MalformedMode::Garbage, "flac");
        let output = Path::new("output.flac");
        let req = make_request(AssetFamily::Audio, input.path(), output, "flac", None);
        assert!(validate_audio(&req).is_err());
    }

    #[test]
    fn validate_rejects_ogg_to_ogg_same_format() {
        let input = create_malformed_file(MalformedMode::Garbage, "ogg");
        let output = Path::new("output.ogg");
        let req = make_request(AssetFamily::Audio, input.path(), output, "ogg", None);
        assert!(validate_audio(&req).is_err());
    }

    #[test]
    fn validate_rejects_wav_to_ogg_unsupported() {
        let input = create_malformed_file(MalformedMode::Garbage, "wav");
        let output = Path::new("output.ogg");
        let req = make_request(AssetFamily::Audio, input.path(), output, "ogg", None);
        assert!(validate_audio(&req).is_err());
    }

    #[test]
    fn validate_rejects_mp3_to_ogg_unsupported() {
        let input = create_malformed_file(MalformedMode::Garbage, "mp3");
        let output = Path::new("output.ogg");
        let req = make_request(AssetFamily::Audio, input.path(), output, "ogg", None);
        assert!(validate_audio(&req).is_err());
    }

    #[test]
    fn validate_rejects_flac_to_ogg_unsupported() {
        let input = create_malformed_file(MalformedMode::Garbage, "flac");
        let output = Path::new("output.ogg");
        let req = make_request(AssetFamily::Audio, input.path(), output, "ogg", None);
        assert!(validate_audio(&req).is_err());
    }

    #[test]
    fn validate_accepts_mp3_to_m4a_and_ogg_to_aac() {
        for (source, target) in [("mp3", "m4a"), ("ogg", "aac"), ("m4a", "aac")] {
            let input = create_malformed_file(MalformedMode::Garbage, source);
            let output = Path::new("output.tmp");
            let req = make_request(AssetFamily::Audio, input.path(), output, target, None);
            assert!(validate_audio(&req).is_ok(), "{source} -> {target}");
        }
    }

    #[test]
    fn validate_accepts_aac_input() {
        let input = create_malformed_file(MalformedMode::Garbage, "aac");
        let output = Path::new("output.mp3");
        let req = make_request(AssetFamily::Audio, input.path(), output, "mp3", None);
        assert!(validate_audio(&req).is_ok());
    }

    // ═══════════════════════════════════════════
    // Input File Existence
    // ═══════════════════════════════════════════

    #[test]
    fn validate_rejects_nonexistent_input_file() {
        let req = make_request(
            AssetFamily::Audio,
            Path::new("phantom.wav"),
            Path::new("output.mp3"),
            "mp3",
            None,
        );
        let result = validate_audio(&req);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("does not exist"));
    }

    // ═══════════════════════════════════════════
    // Edge Cases: No Extension
    // ═══════════════════════════════════════════

    #[test]
    fn validate_rejects_input_with_no_extension() {
        let input = create_malformed_file(MalformedMode::Empty, "tmp");
        let output = Path::new("output.mp3");
        let req = make_request(AssetFamily::Audio, input.path(), output, "mp3", None);
        assert!(validate_audio(&req).is_err());
    }

    // ═══════════════════════════════════════════
    // Case Sensitivity
    // ═══════════════════════════════════════════

    #[test]
    fn validate_handles_case_insensitive_output_format() {
        let input = create_malformed_file(MalformedMode::Garbage, "wav");
        let output = Path::new("output.mp3");
        let req = make_request(AssetFamily::Audio, input.path(), output, "MP3", None);
        assert!(validate_audio(&req).is_ok());
    }

    // ═══════════════════════════════════════════
    // Drift Prevention: Route Pairs vs Descriptors
    // ═══════════════════════════════════════════

    #[test]
    fn route_pair_formats_reference_declared_descriptors() {
        use crate::conversion::types::find_descriptor;
        use crate::handlers::audio::all_descriptors;

        // Every format that appears in a route pair must be a registered module
        let route_formats = ["wav", "mp3", "flac", "ogg", "aiff", "opus", "aac", "m4a"];
        for fmt in &route_formats {
            assert!(
                find_descriptor(all_descriptors(), fmt).is_some(),
                "Route pair format '{}' is not a declared audio format",
                fmt
            );
        }
    }

    #[test]
    fn every_writable_descriptor_in_at_least_one_route_output() {
        use crate::handlers::audio::all_descriptors;

        let allowed_outputs: Vec<&str> = vec!["mp3", "wav", "flac", "aiff", "opus", "m4a", "aac"]; // outputs from route pairs
        for desc in all_descriptors().iter().filter(|d| d.can_write) {
            assert!(
                allowed_outputs.contains(&desc.id),
                "Writable descriptor '{}' is not an allowed output in any route pair",
                desc.id
            );
        }
    }
    #[test]
    fn new_format_routes_are_descriptor_driven() {
        let cases = [
            ("wav", "aiff"),
            ("wav", "opus"),
            ("aiff", "wav"),
            ("aif", "mp3"),
            ("opus", "flac"),
            ("aac", "wav"),
            ("m4a", "flac"),
        ];
        for (source, target) in cases {
            let input = create_malformed_file(MalformedMode::Garbage, source);
            let request = make_request(
                AssetFamily::Audio,
                input.path(),
                Path::new("out.tmp"),
                target,
                None,
            );
            assert!(
                validate_audio(&request).is_ok(),
                "expected advertised route {source} -> {target}"
            );
        }
    }

    #[test]
    fn only_registered_writable_targets_are_allowed() {
        let input = create_malformed_file(MalformedMode::Garbage, "aiff");
        for target in ["ogg", "unknown", "aiff"] {
            let request = make_request(
                AssetFamily::Audio,
                input.path(),
                Path::new("out.tmp"),
                target,
                None,
            );
            assert!(
                validate_audio(&request).is_err(),
                "{target} must be rejected"
            );
        }
    }
}
