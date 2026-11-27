// AVIF output (ravif/rav1e via the already-included image crate).
// AVIF input remains disabled: pure-Rust decoders currently have unverified
// Windows/NASM and per-job threading costs. Do not falsely advertise it.
use super::ImageFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::FormatDescriptor;
use crate::handlers::image::options;
use crate::handlers::image::options::ImageOptions;
use image::codecs::avif::AvifEncoder;
use image::{DynamicImage, ExtendedColorType, ImageEncoder};
use std::path::Path;

pub static MODULE: ImageFormatModule = ImageFormatModule {
    descriptor: FormatDescriptor {
        id: "avif",
        canonical_extension: "avif",
        aliases: &[],
        can_read: false,
        can_write: true,
    },
    sniff,
    dimensions,
    decode: None,
    encode: Some(encode),
    options: &[&options::QUALITY],
};

// A write-only module must NOT claim input signatures. The current
// validate_image_conversion() does not reject all unreadable inputs.
fn sniff(_header: &[u8]) -> bool {
    false
}

fn dimensions(_path: &Path) -> Result<(u32, u32), AppError> {
    Err(AppError::Probe(
        "AVIF input is not supported by this write-only codec".to_string(),
    ))
}

fn encode(image: &DynamicImage, options: &ImageOptions) -> Result<Vec<u8>, AppError> {
    if image.width() == 0 || image.height() == 0 {
        return Err(AppError::Execute(
            "AVIF encoder requires positive dimensions".to_string(),
        ));
    }
    // The image crate's encoder accepts RGB8/RGBA8. Preserve alpha by always
    // converting to RGBA8 once; let ravif choose chroma/alpha coding.
    let rgba = image.to_rgba8();
    let mut output = Vec::new();
    // The host already schedules one conversion job per logical processor.
    // Limit the codec to one thread to avoid nesting N jobs x N workers.
    let encoder = AvifEncoder::new_with_speed_quality(&mut output, 8, options.quality)
        .with_num_threads(Some(1));
    encoder
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|e| AppError::Execute(format!("Failed to encode AVIF: {e}")))?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::traits::ConverterHandler;
    use crate::conversion::types::{AssetFamily, ConversionRequest, OutputFormat};
    use crate::handlers::image::ImageConverterHandler;

    fn pixels() -> DynamicImage {
        let rgb = image::RgbImage::from_fn(128, 96, |x, y| {
            // Higher-frequency detail provides a stable lossy-quality signal.
            let a = ((x * 17 + y * 43 + (x ^ y) * 13) % 256) as u8;
            let b = ((x * 31 + y * 7 + (x * y) % 251) % 256) as u8;
            image::Rgb([a, b, a.wrapping_add(b)])
        });
        DynamicImage::ImageRgb8(rgb)
    }

    fn is_avif_header(bytes: &[u8]) -> bool {
        bytes.len() >= 16
            && &bytes[4..8] == b"ftyp"
            && bytes[8..].chunks(4).any(|chunk| chunk == b"avif")
    }

    #[test]
    fn writer_produces_avif_signature() {
        let bytes = encode(&pixels(), &ImageOptions::default()).unwrap();
        assert!(is_avif_header(&bytes));
    }

    #[test]
    fn quality_changes_encoded_size_and_bytes() {
        let img = pixels();
        let low = encode(
            &img,
            &ImageOptions {
                quality: 15,
                ..ImageOptions::default()
            },
        )
        .unwrap();
        let high = encode(
            &img,
            &ImageOptions {
                quality: 95,
                ..ImageOptions::default()
            },
        )
        .unwrap();
        assert!(is_avif_header(&low) && is_avif_header(&high));
        assert_ne!(low, high);
        assert!(
            high.len() > low.len(),
            "high={} low={}",
            high.len(),
            low.len()
        );
    }

    #[test]
    fn png_to_avif_uses_public_handler_and_advertises_only_output() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("src.png");
        let output = dir.path().join("out.avif");
        pixels().save(&input).unwrap();
        let req = ConversionRequest {
            family: AssetFamily::Image,
            input_path: input.to_string_lossy().to_string(),
            output_path: output.to_string_lossy().to_string(),
            output_root: dir.path().to_string_lossy().to_string(),
            output_format: OutputFormat("avif".to_string()),
            options: serde_json::to_value(ImageOptions::default()).unwrap(),
        };
        let handler = ImageConverterHandler;
        handler.validate(&req).unwrap();
        assert!(handler.execute(&req).unwrap().success);
        assert!(is_avif_header(&std::fs::read(output).unwrap()));
        let caps = handler.capabilities();
        assert!(!caps.iter().any(|c| c.input.0 == "avif"));
        assert!(caps
            .iter()
            .any(|c| c.input.0 == "png" && c.outputs.iter().any(|o| o.0 == "avif")));
        assert!(!sniff(b"\x00\x00\x00\x1cftypavif"));
    }

    #[test]
    fn rejects_zero_sized_image_without_panicking() {
        let zero = DynamicImage::ImageRgb8(image::RgbImage::new(0, 0));
        assert!(encode(&zero, &ImageOptions::default()).is_err());
    }

    // Required performance probe, intentionally opt-in because it is expensive.
    // Run with: cargo test avif_4k_encoding_time -- --ignored --nocapture
    // Record a measured value in 04_VALIDATION.md; source code is not a measurement.
    #[test]
    #[ignore = "4K CPU benchmark; run on target hardware"]
    fn avif_4k_encoding_time() {
        let source = image::RgbImage::from_fn(3840, 2160, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x ^ y) % 256) as u8])
        });
        let start = std::time::Instant::now();
        let bytes = encode(&DynamicImage::ImageRgb8(source), &ImageOptions::default()).unwrap();
        eprintln!(
            "AVIF 3840x2160 / quality 80 / speed 8 / 1 codec thread: {:?}; {} bytes",
            start.elapsed(),
            bytes.len()
        );
        assert!(is_avif_header(&bytes));
    }
}
