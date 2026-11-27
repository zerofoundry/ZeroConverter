// TGA has no magic number: accept only a narrow, internally coherent raster header.
// Unknown or palette-based variants are deliberately not claimed by this module.
use super::ImageFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::FormatDescriptor;
use crate::handlers::image::options::ImageOptions;
use image::{DynamicImage, ImageFormat};
use std::io::{BufReader, Cursor, Read};
use std::path::Path;

pub static MODULE: ImageFormatModule = ImageFormatModule {
    descriptor: FormatDescriptor {
        id: "tga",
        canonical_extension: "tga",
        aliases: &[],
        can_read: true,
        can_write: true,
    },
    sniff,
    dimensions,
    decode: Some(decode),
    encode: Some(encode),
    options: &[],
};

fn sniff(h: &[u8]) -> bool {
    if h.len() < 18 || h[1] != 0 || h[3..8] != [0; 5] || h[17] & 0xc0 != 0 {
        return false;
    }
    let w = u16::from_le_bytes([h[12], h[13]]);
    let height = u16::from_le_bytes([h[14], h[15]]);
    w > 0 && height > 0 && matches!((h[2], h[16]), (2 | 10, 24 | 32) | (3 | 11, 8))
}

fn dimensions(path: &Path) -> Result<(u32, u32), AppError> {
    let mut h = [0u8; 18];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut h))
        .map_err(|e| AppError::Probe(format!("TGA header read failed: {e}")))?;
    if !sniff(&h) {
        return Err(AppError::Probe("Invalid or unsupported TGA header".into()));
    }
    Ok((
        u16::from_le_bytes([h[12], h[13]]) as u32,
        u16::from_le_bytes([h[14], h[15]]) as u32,
    ))
}

fn decode(path: &Path) -> Result<DynamicImage, AppError> {
    let (w, h) = dimensions(path)?;
    if (w as u64) * (h as u64) > crate::image_limits::MAX_IMAGE_PIXELS {
        return Err(AppError::Execute("TGA pixel budget exceeded".into()));
    }
    let file = std::fs::File::open(path)
        .map_err(|e| AppError::Execute(format!("TGA open failed: {e}")))?;
    image::load(BufReader::new(file), ImageFormat::Tga)
        .map_err(|e| AppError::Execute(format!("TGA decode failed: {e}")))
}

fn encode(img: &DynamicImage, _options: &ImageOptions) -> Result<Vec<u8>, AppError> {
    if img.width() > u16::MAX as u32 || img.height() > u16::MAX as u32 {
        return Err(AppError::Execute(
            "TGA dimensions exceed 16-bit header".into(),
        ));
    }
    let mut data = Cursor::new(Vec::new());
    // TGA is an 8-bit format; float/16-bit inputs are explicitly quantized.
    DynamicImage::ImageRgba8(img.to_rgba8())
        .write_to(&mut data, ImageFormat::Tga)
        .map_err(|e| AppError::Execute(format!("TGA encode failed: {e}")))?;
    Ok(data.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::traits::ConverterHandler;
    use crate::conversion::types::{AssetFamily, ConversionRequest, OutputFormat};
    use crate::handlers::image::ImageConverterHandler;

    fn request(input: &Path, output: &Path, format: &str) -> ConversionRequest {
        ConversionRequest {
            family: AssetFamily::Image,
            input_path: input.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            output_root: output
                .parent()
                .expect("output dir")
                .to_string_lossy()
                .into_owned(),
            output_format: OutputFormat(format.into()),
            options: serde_json::to_value(ImageOptions::default()).expect("options"),
        }
    }

    #[test]
    fn png_tga_png_roundtrip_and_probe() {
        let dir = tempfile::tempdir().expect("dir");
        let png = dir.path().join("in.png");
        let tga = dir.path().join("out.tga");
        let back = dir.path().join("back.png");
        let src = image::RgbaImage::from_fn(7, 5, |x, y| {
            image::Rgba([(x * 20) as u8, (y * 40) as u8, 188, 255])
        });
        src.save(&png).expect("png");
        let handler = ImageConverterHandler;
        let a = request(&png, &tga, "tga");
        handler.validate(&a).expect("validate outbound");
        handler.execute(&a).expect("encode");
        assert_eq!(handler.probe(&tga).expect("probe").0, "tga");
        assert_eq!(dimensions(&tga).expect("dimensions"), (7, 5));
        let b = request(&tga, &back, "png");
        handler.validate(&b).expect("validate inbound");
        handler.execute(&b).expect("decode");
        assert_eq!(image::open(&back).expect("image").to_rgba8(), src);
    }

    #[test]
    fn corrupt_and_extreme_headers_are_rejected() {
        let d = tempfile::tempdir().expect("dir");
        let input = d.path().join("bad.tga");
        std::fs::write(&input, [0u8; 7]).expect("write");
        assert!(dimensions(&input).is_err());
        let mut header = [0u8; 18];
        header[2] = 2;
        header[12..14].copy_from_slice(&60_000u16.to_le_bytes());
        header[14..16].copy_from_slice(&60_000u16.to_le_bytes());
        header[16] = 32;
        std::fs::write(&input, header).expect("write header");
        assert_eq!(dimensions(&input).expect("header dims"), (60_000, 60_000));
        let handler = ImageConverterHandler;
        let error = handler.validate(&request(&input, &d.path().join("out.png"), "png"));
        assert!(error.is_err(), "oversized image should fail");
        header[1] = 255; // An invalid palette flag must not claim a random file.
        assert!(!sniff(&header));
    }
}
