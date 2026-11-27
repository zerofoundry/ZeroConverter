// JPEG XL — pure-Rust, still-image input via jxl-oxide.
// The caller enforces the output path and writes output bytes atomically.
use super::ImageFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::FormatDescriptor;
use image::{DynamicImage, ImageDecoder};
use jxl_oxide::integration::JxlDecoder;
use std::fs::File;
use std::path::Path;

pub static MODULE: ImageFormatModule = ImageFormatModule {
    descriptor: FormatDescriptor {
        id: "jxl",
        canonical_extension: "jxl",
        aliases: &[],
        can_read: true,
        can_write: false,
    },
    sniff,
    dimensions,
    decode: Some(decode),
    encode: None,
    options: &[],
};

// In addition to the 100 MP input policy, bound compressed input and the
// decoder's eventual output. JXL can contain high-bit-depth floating pixels.
const MAX_COMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_DECODED_BYTES: u64 = 512 * 1024 * 1024;
const CONTAINER_MAGIC: &[u8; 12] = b"\x00\x00\x00\x0cJXL \x0d\x0a\x87\x0a";

fn sniff(header: &[u8]) -> bool {
    header.starts_with(&[0xff, 0x0a]) || header.starts_with(CONTAINER_MAGIC)
}

/// Initializes only the JXL header/decoder, not the raster.
fn prepare(path: &Path) -> Result<JxlDecoder<File>, AppError> {
    let file =
        File::open(path).map_err(|e| AppError::Probe(format!("Failed to open JPEG XL: {e}")))?;
    let bytes = file
        .metadata()
        .map_err(|e| AppError::Probe(format!("Failed to inspect JPEG XL: {e}")))?
        .len();
    if bytes > MAX_COMPRESSED_BYTES {
        return Err(AppError::Probe(format!(
            "JPEG XL file is larger than {MAX_COMPRESSED_BYTES} bytes"
        )));
    }

    let mut decoder = JxlDecoder::new(file)
        .map_err(|e| AppError::Probe(format!("Malformed JPEG XL header: {e}")))?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(AppError::Probe(
            "JPEG XL has an invalid zero dimension".to_string(),
        ));
    }
    crate::image_limits::check_pixel_budget(&crate::image_limits::dimensions_from_pair(
        width, height,
    ))?;
    let output_bytes = decoder.total_bytes();
    if output_bytes > MAX_DECODED_BYTES {
        return Err(AppError::Validate(format!(
            "JPEG XL decoded image requires {output_bytes} bytes, above {MAX_DECODED_BYTES} limit"
        )));
    }
    // image::Limits is non_exhaustive: construct then assign fields.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    decoder
        .set_limits(limits)
        .map_err(|e| AppError::Probe(format!("JPEG XL limits rejected: {e}")))?;
    Ok(decoder)
}

fn dimensions(path: &Path) -> Result<(u32, u32), AppError> {
    Ok(prepare(path)?.dimensions())
}

fn decode(path: &Path) -> Result<DynamicImage, AppError> {
    let decoder = prepare(path)?;
    DynamicImage::from_decoder(decoder)
        .map_err(|e| AppError::Execute(format!("Failed to decode JPEG XL: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::traits::ConverterHandler;
    use crate::conversion::types::{AssetFamily, ConversionRequest, OutputFormat};
    use crate::handlers::image::{options::ImageOptions, ImageConverterHandler};

    // 4x4 self-generated RGB gradient, converted from PPM to JXL using
    // FFmpeg/libjxl (ffmpeg -i input.ppm -frames:v 1 -c:v libjxl -effort 1 input.jxl).
    // No third-party sample or redistributed copyrighted test asset.
    const JXL_FIXTURE: &[u8] = &[
        0xff, 0x0a, 0x18, 0x90, 0x01, 0x00, 0x13, 0x08, 0x01, 0x00, 0x88, 0x01, 0x67, 0x13, 0x28,
        0x01, 0x00, 0x50, 0xa1, 0x32, 0xca, 0xb8, 0xc1, 0xcb, 0xb9, 0x9e, 0x2f, 0x3f, 0x74, 0x58,
        0x4c, 0xdb, 0xb8, 0xce, 0xd0, 0x56, 0xdb, 0x16, 0x96, 0x24, 0x8a, 0x8c, 0x63, 0x93, 0x74,
        0x02, 0x87, 0x24, 0xb1, 0x0a, 0x6c, 0x5c, 0x80, 0xcc, 0x0c, 0x00, 0x00, 0x33, 0x17, 0x8e,
        0x0e, 0x10, 0x2b, 0x49, 0x24, 0x85, 0x72, 0x63, 0xf7, 0xbf, 0x4d, 0x03, 0xc1, 0xb4, 0x2a,
        0xa0, 0xfd, 0x1f, 0x6f, 0xf7, 0xab, 0xef, 0x63, 0x8c, 0xc8, 0xe8, 0xd5, 0x52, 0x21, 0x0c,
        0x08, 0x24, 0x00, 0xd0, 0x7c, 0x7c, 0xb7, 0xaa, 0x8b, 0xa6, 0x08, 0x4b, 0x00, 0x08, 0x1f,
        0xf3, 0x2c, 0x27, 0xb6, 0x00,
    ];

    #[test]
    fn sniffs_jxl_codestream_and_container() {
        assert!(sniff(JXL_FIXTURE));
        assert!(sniff(CONTAINER_MAGIC));
        assert!(!sniff(b"NOT JXL"));
        assert!(!sniff(&[0xff]));
    }

    #[test]
    fn rejects_corrupt_and_truncated_jxl_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.jxl");
        std::fs::write(&path, [0xff, 0x0a]).unwrap();
        assert!(dimensions(&path).is_err());
        std::fs::write(&path, CONTAINER_MAGIC).unwrap();
        assert!(dimensions(&path).is_err());
    }

    #[test]
    fn jxl_to_png_via_handler_uses_header_budget_and_decodes_pixels() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("gradient.jxl");
        let output = dir.path().join("out.png");
        std::fs::write(&input, JXL_FIXTURE).unwrap();
        assert_eq!(dimensions(&input).unwrap(), (4, 4));

        let request = ConversionRequest {
            family: AssetFamily::Image,
            input_path: input.to_string_lossy().to_string(),
            output_path: output.to_string_lossy().to_string(),
            output_root: dir.path().to_string_lossy().to_string(),
            output_format: OutputFormat("png".to_string()),
            options: serde_json::to_value(ImageOptions::default()).unwrap(),
        };
        let handler = ImageConverterHandler;
        assert_eq!(handler.probe(&input).unwrap().0, "jxl");
        handler.validate(&request).unwrap();
        assert!(handler.execute(&request).unwrap().success);
        let output_image = image::open(&output).unwrap().to_rgb8();
        assert_eq!(output_image.dimensions(), (4, 4));
        let [r, g, b] = output_image.get_pixel(0, 0).0;
        assert!(r <= 32 && g <= 32 && b <= 32);
    }

    #[test]
    fn jxl_is_advertised_only_as_an_input() {
        let capabilities = ImageConverterHandler.capabilities();
        let jxl = capabilities.iter().find(|c| c.input.0 == "jxl").unwrap();
        assert!(jxl.outputs.iter().any(|o| o.0 == "png"));
        assert!(!capabilities
            .iter()
            .any(|c| c.outputs.iter().any(|o| o.0 == "jxl")));
    }
}
