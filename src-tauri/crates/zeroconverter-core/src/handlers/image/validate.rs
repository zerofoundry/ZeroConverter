use super::options::{
    compute_resize_target, ImageOptions, ResizeMode, DDS_FORMATS, MAX_RESIZE_DIMENSION,
};
use crate::app_error::AppError;
use crate::conversion::types::{find_descriptor, ConversionRequest, InputFormat};
use std::path::Path;

pub(super) fn validate_static_gif(path: &Path) -> Result<(), AppError> {
    use image::AnimationDecoder;
    let file = std::fs::File::open(path)
        .map_err(|e| AppError::Validate(format!("Failed to open GIF: {}", e)))?;
    let buffered = std::io::BufReader::new(file);
    let decoder = image::codecs::gif::GifDecoder::new(buffered)
        .map_err(|e| AppError::Validate(format!("Malformed GIF: {}", e)))?;
    // The header gives the canvas size; every frame is decoded at that size,
    // so check the budget before any frame is.
    let (w, h) = image::ImageDecoder::dimensions(&decoder);
    crate::image_limits::check_pixel_budget(&crate::image_limits::dimensions_from_pair(w, h))?;
    let mut frames = decoder.into_frames();

    // First frame must exist and be valid
    match frames.next() {
        Some(Ok(_)) => {}
        Some(Err(e)) => return Err(AppError::Validate(format!("Malformed GIF frame: {}", e))),
        None => {
            return Err(AppError::Validate(
                "Malformed GIF: no frames found".to_string(),
            ))
        }
    }

    // Second frame must NOT exist
    match frames.next() {
        Some(Ok(_)) => Err(AppError::Validate(
            "Animated GIF conversion is not supported yet. Please provide a static GIF."
                .to_string(),
        )),
        Some(Err(e)) => Err(AppError::Validate(format!("Malformed GIF frame: {}", e))),
        None => Ok(()), // Exactly one frame, valid static GIF
    }
}

const MAX_ICO_DIMENSION: u32 = 256;

pub fn validate_image_conversion(
    request: &ConversionRequest,
    _input_format: &InputFormat,
) -> Result<(), AppError> {
    let input_path = Path::new(&request.input_path);
    if !input_path.exists() {
        return Err(AppError::Validate(format!(
            "Input file does not exist: {}",
            request.input_path
        )));
    }

    if _input_format.0 == "gif" {
        validate_static_gif(input_path)?;
    }

    let output_format = &request.output_format.0;
    match find_descriptor(super::formats::all_descriptors(), output_format) {
        Some(d) if d.can_write => {}
        _ => {
            return Err(AppError::Validate(format!(
                "Unsupported output format: {}",
                output_format
            )));
        }
    }

    let options: ImageOptions = serde_json::from_value(request.options.clone())
        .map_err(|e| AppError::Validate(format!("Invalid options: {}", e)))?;

    if options.quality == 0 || options.quality > 100 {
        return Err(AppError::Validate(format!(
            "Quality must be between 1 and 100, got: {}",
            options.quality
        )));
    }

    if let Some(format) = options
        .dds_format
        .as_deref()
        .filter(|_| output_format.eq_ignore_ascii_case("dds"))
    {
        if !DDS_FORMATS.iter().any(|f| f.eq_ignore_ascii_case(format)) {
            return Err(AppError::Validate(format!(
                "Unknown DDS compression \"{format}\"; use one of {}",
                DDS_FORMATS.join(", ")
            )));
        }
    }

    if let Some(mode) = options
        .webp_mode
        .as_deref()
        .filter(|_| output_format.eq_ignore_ascii_case("webp"))
    {
        if !["lossless", "lossy"]
            .iter()
            .any(|m| m.eq_ignore_ascii_case(mode))
        {
            return Err(AppError::Validate(format!(
                "Unknown WebP mode \"{mode}\"; use lossless or lossy"
            )));
        }
    }

    let validate_dim = |dim: Option<u32>, name: &str| -> Result<(), AppError> {
        let val = dim.ok_or_else(|| {
            AppError::Validate(format!("{} is required for this resize mode", name))
        })?;
        if val == 0 {
            return Err(AppError::Validate(format!(
                "{} must be greater than 0",
                name
            )));
        }
        if val > MAX_RESIZE_DIMENSION {
            return Err(AppError::Validate(format!(
                "{} must not exceed {}",
                name, MAX_RESIZE_DIMENSION
            )));
        }
        Ok(())
    };

    match options.resize_mode {
        ResizeMode::None => {}
        ResizeMode::MaxWidth => validate_dim(options.resize_width, "resize_width")?,
        ResizeMode::MaxHeight => validate_dim(options.resize_height, "resize_height")?,
        ResizeMode::FitBox => {
            validate_dim(options.resize_width, "resize_width")?;
            validate_dim(options.resize_height, "resize_height")?;
        }
    }

    // Check image dimensions against memory safety limit.
    // Route by resolved input format: SVG uses its own parser, raster uses image crate.
    let dims = if _input_format.0 == "svg" {
        super::svg::validate_svg_dimensions(input_path)?
    } else {
        crate::image_limits::validate_image_dimensions(input_path)?
    };

    // ICO output: enforce 256x256 max and reject same-format ICO → ICO
    if output_format == "ico" {
        if _input_format.0.to_lowercase() == "ico" {
            return Err(AppError::Validate(
                "ICO to ICO conversion is not supported. Convert to a raster format instead."
                    .to_string(),
            ));
        }

        let (final_w, final_h) = match compute_resize_target(dims.width, dims.height, &options) {
            Some((w, h)) => (w, h),
            None => (dims.width, dims.height),
        };

        if final_w > MAX_ICO_DIMENSION || final_h > MAX_ICO_DIMENSION {
            return Err(AppError::IcoTooLarge {
                width: final_w,
                height: final_h,
                max: MAX_ICO_DIMENSION,
            });
        }
    }

    // Reject SVG → SVG (SVG output is not supported)
    if output_format == "svg" {
        return Err(AppError::Validate(
            "SVG output is not supported.".to_string(),
        ));
    }

    // PDF accepts every registered readable raster input, including image
    // format modules. SVG is not part of the raster-to-PDF contract.
    if find_descriptor(super::SPECIAL_FORMATS, output_format).is_some_and(|d| d.id == "pdf") {
        let can_render = find_descriptor(super::formats::all_descriptors(), &_input_format.0)
            .is_some_and(|d| d.can_read && d.id != "svg");
        if !can_render {
            return Err(AppError::Validate(format!(
                "{} to PDF conversion is not supported (requires a readable raster input).",
                _input_format.0.to_uppercase()
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::types::{AssetFamily, OutputFormat};
    use serde_json::json;

    fn make_real_png() -> tempfile::NamedTempFile {
        use image::{ImageBuffer, RgbImage};
        let img: RgbImage = ImageBuffer::from_fn(2, 2, |_, _| image::Rgb([128, 128, 128]));
        let f = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        img.save(f.path()).unwrap();
        f
    }

    fn make_request(
        input: &str,
        output: &str,
        format: &str,
        quality: u8,
        overwrite: bool,
    ) -> ConversionRequest {
        ConversionRequest {
            family: AssetFamily::Image,
            input_path: input.to_string(),
            output_path: output.to_string(),
            output_root: "/".to_string(),
            output_format: OutputFormat(format.to_string()),
            options: json!({ "quality": quality, "overwrite": overwrite }),
        }
    }

    // ═══════════════════════════════════════════
    // Happy Path
    // ═══════════════════════════════════════════

    #[test]
    fn validate_accepts_valid_png_to_webp() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output_test.webp",
            "webp",
            80,
            false,
        );
        let input_format = InputFormat("png".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_accepts_valid_png_to_jpg() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output_test.jpg",
            "jpg",
            90,
            false,
        );
        let input_format = InputFormat("png".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_accepts_uppercase_output_format() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output_test.png",
            "PNG",
            80,
            false,
        );
        let input_format = InputFormat("png".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_accepts_ico_input() {
        let input = make_real_png(); // Use dummy file to pass exists check
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output_test.png",
            "png",
            80,
            false,
        );
        let input_format = InputFormat("ico".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    // ═══════════════════════════════════════════
    // Input Validation
    // ═══════════════════════════════════════════

    #[test]
    fn validate_rejects_nonexistent_input_file() {
        let req = make_request(
            "/nonexistent/phantom_file.png",
            "/tmp/output.webp",
            "webp",
            80,
            false,
        );
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("does not exist"),
            "Expected 'does not exist', got: {}",
            err
        );
    }

    // ═══════════════════════════════════════════
    // Output Format Validation
    // ═══════════════════════════════════════════

    #[test]
    fn validate_rejects_unsupported_output_format_gif() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.gif",
            "gif",
            80,
            false,
        );
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported output format"));
    }

    #[test]
    fn validate_accepts_output_format_bmp() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.bmp",
            "bmp",
            80,
            false,
        );
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_accepts_output_format_tiff() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.tiff",
            "tiff",
            80,
            false,
        );
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_accepts_output_format_tif_alias() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.tiff",
            "tif", // Alias
            80,
            false,
        );
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_accepts_png_to_ico_with_valid_size() {
        // 2x2 PNG fits within 256x256 — ICO output must accept
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.ico",
            "ico",
            80,
            false,
        );
        let input_format = InputFormat("png".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_rejects_ico_to_ico_same_format() {
        let input = make_real_png(); // dummy file for exists check
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.ico",
            "ico",
            80,
            false,
        );
        let input_format = InputFormat("ico".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("ICO to ICO conversion is not supported"));
    }

    #[test]
    fn validate_rejects_oversized_ico_output_without_resize() {
        // Create a 500x500 PNG — exceeds 256x256 ICO limit
        use image::{ImageBuffer, RgbImage};
        let img: RgbImage = ImageBuffer::from_fn(500, 500, |_, _| image::Rgb([128, 128, 128]));
        let f = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        img.save(f.path()).unwrap();

        let req = make_request(
            f.path().to_str().unwrap(),
            "/tmp/output.ico",
            "ico",
            80,
            false,
        );
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("ICO output requires dimensions up to 256x256"));
    }

    #[test]
    fn validate_accepts_oversized_input_with_fit_box_256() {
        // 500x500 PNG with fit-box 256x256 → planned output 256x256 → accepted
        use image::{ImageBuffer, RgbImage};
        let img: RgbImage = ImageBuffer::from_fn(500, 500, |_, _| image::Rgb([128, 128, 128]));
        let f = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        img.save(f.path()).unwrap();

        let mut req = make_request(
            f.path().to_str().unwrap(),
            "/tmp/output.ico",
            "ico",
            80,
            false,
        );
        req.options = json!({
            "quality": 80,
            "overwrite": false,
            "resize_mode": "fit_box",
            "resize_width": 256,
            "resize_height": 256
        });
        let input_format = InputFormat("png".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_rejects_resize_above_256_for_ico() {
        // 500x500 PNG with resize 512x512 → planned output 500x500 (not downsized) → reject
        // Actually compute_resize_target won't downscale if already <= target.
        // Use 500x500 input + fit_box 512x512 → no resize (500 <= 512) → 500x500 > 256 → reject
        use image::{ImageBuffer, RgbImage};
        let img: RgbImage = ImageBuffer::from_fn(500, 500, |_, _| image::Rgb([128, 128, 128]));
        let f = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        img.save(f.path()).unwrap();

        let mut req = make_request(
            f.path().to_str().unwrap(),
            "/tmp/output.ico",
            "ico",
            80,
            false,
        );
        req.options = json!({
            "quality": 80,
            "overwrite": false,
            "resize_mode": "fit_box",
            "resize_width": 512,
            "resize_height": 512
        });
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("ICO output requires dimensions up to 256x256"));
    }

    #[test]
    fn validate_accepts_exactly_256x256_for_ico() {
        use image::{ImageBuffer, RgbImage};
        let img: RgbImage = ImageBuffer::from_fn(256, 256, |_, _| image::Rgb([128, 128, 128]));
        let f = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        img.save(f.path()).unwrap();

        let req = make_request(
            f.path().to_str().unwrap(),
            "/tmp/output.ico",
            "ico",
            80,
            false,
        );
        let input_format = InputFormat("png".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_rejects_unknown_output_format() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.xyz",
            "xyz",
            80,
            false,
        );
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
    }

    #[test]
    fn validate_rejects_unsupported_output_format_heif() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.heic",
            "heif",
            80,
            false,
        );
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
    }

    #[test]
    fn validate_rejects_empty_output_format() {
        let input = make_real_png();
        let req = make_request(input.path().to_str().unwrap(), "/tmp/output", "", 80, false);
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
    }

    // ═══════════════════════════════════════════
    // Options Validation
    // ═══════════════════════════════════════════

    #[test]
    fn validate_rejects_malformed_options_json() {
        let input = make_real_png();
        let req = ConversionRequest {
            family: AssetFamily::Image,
            input_path: input.path().to_str().unwrap().to_string(),
            output_path: "/tmp/output.webp".to_string(),
            output_root: "/tmp".to_string(),
            output_format: OutputFormat("webp".to_string()),
            options: json!({ "quality": "not_a_number" }),
        };
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Invalid options"),
            "Expected options error, got: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_quality_zero() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.webp",
            "webp",
            0,
            false,
        );
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("between 1 and 100"));
    }

    #[test]
    fn validate_rejects_quality_above_100() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.jpg",
            "jpg",
            101,
            false,
        );
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("between 1 and 100"));

        let req_max = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.jpg",
            "jpg",
            255,
            false,
        );
        let result_max = validate_image_conversion(&req_max, &input_format);
        assert!(result_max.is_err());
        assert!(result_max
            .unwrap_err()
            .to_string()
            .contains("between 1 and 100"));
    }

    #[test]
    fn validate_accepts_quality_bounds() {
        let input = make_real_png();
        let req_min = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.jpg",
            "jpg",
            1,
            false,
        );
        let input_format = InputFormat("png".to_string());
        assert!(validate_image_conversion(&req_min, &input_format).is_ok());

        let req_max = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.jpg",
            "jpg",
            100,
            false,
        );
        assert!(validate_image_conversion(&req_max, &input_format).is_ok());
    }

    // ═══════════════════════════════════════════
    // Resize Validation Tests
    // ═══════════════════════════════════════════

    #[test]
    fn validate_rejects_max_width_without_width() {
        let input = make_real_png();
        let mut req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.png",
            "png",
            80,
            false,
        );
        req.options = json!({ "quality": 80, "overwrite": false, "resize_mode": "max_width" });
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("resize_width is required"));
    }

    #[test]
    fn validate_rejects_max_height_without_height() {
        let input = make_real_png();
        let mut req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.png",
            "png",
            80,
            false,
        );
        req.options = json!({ "quality": 80, "overwrite": false, "resize_mode": "max_height" });
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("resize_height is required"));
    }

    #[test]
    fn validate_rejects_fit_box_missing_width_or_height() {
        let input = make_real_png();
        let mut req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.png",
            "png",
            80,
            false,
        );
        req.options = json!({ "quality": 80, "overwrite": false, "resize_mode": "fit_box", "resize_width": 100 });
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("resize_height is required"));

        req.options = json!({ "quality": 80, "overwrite": false, "resize_mode": "fit_box", "resize_height": 100 });
        let result2 = validate_image_conversion(&req, &input_format);
        assert!(result2.is_err());
        assert!(result2
            .unwrap_err()
            .to_string()
            .contains("resize_width is required"));
    }

    #[test]
    fn validate_rejects_zero_dimension() {
        let input = make_real_png();
        let mut req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.png",
            "png",
            80,
            false,
        );
        req.options = json!({ "quality": 80, "overwrite": false, "resize_mode": "max_width", "resize_width": 0 });
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must be greater than 0"));
    }

    #[test]
    fn validate_rejects_dimension_above_16384() {
        let input = make_real_png();
        let mut req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.png",
            "png",
            80,
            false,
        );
        req.options = json!({ "quality": 80, "overwrite": false, "resize_mode": "max_width", "resize_width": 16385 });
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must not exceed 16384"));
    }

    #[test]
    fn validate_rejects_malformed_resize_mode() {
        let input = make_real_png();
        let mut req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.png",
            "png",
            80,
            false,
        );
        req.options = json!({ "quality": 80, "overwrite": false, "resize_mode": "fitbox" }); // malformed
        let input_format = InputFormat("png".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid options"));
    }

    // ═══════════════════════════════════════════
    // GIF Validation Tests
    // ═══════════════════════════════════════════

    fn make_static_gif() -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new().suffix(".gif").tempfile().unwrap();
        // 1x1 static GIF
        let data = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\xff\xff\x00\x00\x00!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";
        use std::io::Write;
        file.write_all(data).unwrap();
        file
    }

    fn make_animated_gif() -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new().suffix(".gif").tempfile().unwrap();
        // Mock animated gif (two frames)
        let data = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\xff\xff\x00\x00\x00!\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00!\xf9\x04\x09\x0a\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02L\x01\x00!\xf9\x04\x09\x0a\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02L\x01\x00;";
        use std::io::Write;
        file.write_all(data).unwrap();
        file
    }

    fn make_malformed_gif() -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new().suffix(".gif").tempfile().unwrap();
        // Corrupt GIF
        let data = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        use std::io::Write;
        file.write_all(data).unwrap();
        file
    }

    #[test]
    fn validate_accepts_static_gif() {
        let input = make_static_gif();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.png",
            "png",
            80,
            false,
        );
        let input_format = InputFormat("gif".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_rejects_animated_gif() {
        let input = make_animated_gif();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.png",
            "png",
            80,
            false,
        );
        let input_format = InputFormat("gif".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Animated GIF conversion is not supported yet"));
    }

    #[test]
    fn gif_canvas_past_the_budget_is_refused_before_decoding() {
        // 12000 x 9000 canvas (108 MP) holding one 1x1 frame: small file,
        // huge decode. Must be refused from the header alone.
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&12_000u16.to_le_bytes());
        bytes.extend_from_slice(&9_000u16.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0]);
        bytes.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0x80]);
        bytes.extend_from_slice(&[0, 0, 0, 255, 255, 255]);
        bytes.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00, 0x3B]);
        let mut f = tempfile::Builder::new().suffix(".gif").tempfile().unwrap();
        std::io::Write::write_all(&mut f, &bytes).unwrap();

        let err = validate_static_gif(f.path()).unwrap_err().to_string();
        assert!(err.contains("exceed safe limit"), "{err}");
    }

    #[test]
    fn validate_rejects_malformed_gif() {
        let input = make_malformed_gif();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.png",
            "png",
            80,
            false,
        );
        let input_format = InputFormat("gif".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Malformed GIF"));
    }

    #[test]
    fn validate_rejects_gif_output() {
        let input = make_static_gif();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.gif",
            "gif",
            80,
            false,
        );
        let input_format = InputFormat("png".to_string()); // Does not matter, output will fail
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unsupported output format"));
    }

    #[test]
    fn validate_gif_to_ico_enforces_size() {
        let input = make_static_gif();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.ico",
            "ico",
            80,
            false,
        );
        let input_format = InputFormat("gif".to_string());
        // Since my make_static_gif is 1x1, this should be allowed
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_animated_gif_to_ico_rejects_early() {
        let input = make_animated_gif();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.ico",
            "ico",
            80,
            false,
        );
        let input_format = InputFormat("gif".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        // Must reject as animated, NOT as ICO validation or dimension check
        assert!(err_msg.contains("Animated GIF conversion is not supported yet"));
    }

    // ═══════════════════════════════════════════
    // PDF Output Validation Tests
    // ═══════════════════════════════════════════

    #[test]
    fn validate_accepts_png_to_pdf() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.pdf",
            "pdf",
            80,
            false,
        );
        let input_format = InputFormat("png".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_accepts_jpg_to_pdf() {
        let input = make_real_png(); // dummy file for exists check
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.pdf",
            "pdf",
            80,
            false,
        );
        let input_format = InputFormat("jpg".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_accepts_jpeg_alias_to_pdf() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.pdf",
            "pdf",
            80,
            false,
        );
        let input_format = InputFormat("jpeg".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_accepts_webp_to_pdf() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.pdf",
            "pdf",
            80,
            false,
        );
        let input_format = InputFormat("webp".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_rejects_svg_to_pdf() {
        // Must use a real SVG file so SVG-specific validation passes before the PDF gate
        let svg_content = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>"#;
        let mut input = tempfile::Builder::new().suffix(".svg").tempfile().unwrap();
        std::io::Write::write_all(&mut input, svg_content.as_bytes()).unwrap();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.pdf",
            "pdf",
            80,
            false,
        );
        let input_format = InputFormat("svg".to_string());
        let result = validate_image_conversion(&req, &input_format);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("to PDF conversion is not supported"),
            "Expected PDF rejection for SVG, got: {}",
            err
        );
    }

    #[test]
    fn validate_accepts_gif_to_pdf() {
        let input = make_static_gif();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.pdf",
            "pdf",
            80,
            false,
        );
        let input_format = InputFormat("gif".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_accepts_ico_to_pdf() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.pdf",
            "pdf",
            80,
            false,
        );
        let input_format = InputFormat("ico".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_accepts_bmp_to_pdf() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.pdf",
            "pdf",
            80,
            false,
        );
        let input_format = InputFormat("bmp".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }

    #[test]
    fn validate_accepts_tiff_to_pdf() {
        let input = make_real_png();
        let req = make_request(
            input.path().to_str().unwrap(),
            "/tmp/output.pdf",
            "pdf",
            80,
            false,
        );
        let input_format = InputFormat("tiff".to_string());
        assert!(validate_image_conversion(&req, &input_format).is_ok());
    }
}
