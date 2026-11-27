// OpenEXR — single image through image-rs. Multi-part/deep images are not promised.
use super::ImageFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::FormatDescriptor;
use crate::handlers::image::options::ImageOptions;
use image::{DynamicImage, ImageFormat, ImageReader};
use std::io::{BufReader, Cursor, Read};
use std::path::Path;

// A float RGBA image already uses 16 bytes/pixel; avoid 100MP multi-GiB spikes.
const MAX_FLOAT_PIXELS: u64 = 32_000_000;

pub static MODULE: ImageFormatModule = ImageFormatModule {
    descriptor: FormatDescriptor {
        id: "exr",
        canonical_extension: "exr",
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

fn sniff(header: &[u8]) -> bool {
    header.starts_with(&[0x76, 0x2f, 0x31, 0x01])
}

fn verify_file(path: &Path) -> Result<(), AppError> {
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map_err(|e| AppError::Probe(format!("OpenEXR header read failed: {e}")))?;
    if !sniff(&magic) {
        return Err(AppError::Probe("Not an OpenEXR file".into()));
    }
    Ok(())
}

fn dimensions(path: &Path) -> Result<(u32, u32), AppError> {
    verify_file(path)?;
    let f = std::fs::File::open(path)
        .map_err(|e| AppError::Probe(format!("OpenEXR open failed: {e}")))?;
    let (w, h) = ImageReader::with_format(BufReader::new(f), ImageFormat::OpenExr)
        .into_dimensions()
        .map_err(|e| AppError::Probe(format!("OpenEXR dimensions failed: {e}")))?;
    if w == 0 || h == 0 || (w as u64) * (h as u64) > MAX_FLOAT_PIXELS {
        return Err(AppError::Probe(
            "OpenEXR exceeds bounded float image profile".into(),
        ));
    }
    Ok((w, h))
}

fn decode(path: &Path) -> Result<DynamicImage, AppError> {
    dimensions(path)?;
    let file = std::fs::File::open(path)
        .map_err(|e| AppError::Execute(format!("OpenEXR open failed: {e}")))?;
    image::load(BufReader::new(file), ImageFormat::OpenExr)
        .map_err(|e| AppError::Execute(format!("OpenEXR decode failed: {e}")))
}

fn encode(img: &DynamicImage, _options: &ImageOptions) -> Result<Vec<u8>, AppError> {
    let pixels = (img.width() as u64) * (img.height() as u64);
    if pixels == 0 || pixels > MAX_FLOAT_PIXELS {
        return Err(AppError::Execute(
            "OpenEXR float pixel budget exceeded".into(),
        ));
    }
    let mut out = Cursor::new(Vec::new());
    // Preserve existing floating point pixels when possible; 8-bit inputs are
    // promoted to [0,1] floats, not tone-mapped. EXR encoder controls precision.
    DynamicImage::ImageRgba32F(img.to_rgba32f())
        .write_to(&mut out, ImageFormat::OpenExr)
        .map_err(|e| AppError::Execute(format!("OpenEXR encode failed: {e}")))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::traits::ConverterHandler;
    use crate::conversion::types::{AssetFamily, ConversionRequest, OutputFormat};
    use crate::handlers::image::ImageConverterHandler;

    fn req(i: &Path, o: &Path, f: &str) -> ConversionRequest {
        ConversionRequest {
            family: AssetFamily::Image,
            input_path: i.to_string_lossy().into_owned(),
            output_path: o.to_string_lossy().into_owned(),
            output_root: o
                .parent()
                .expect("directory")
                .to_string_lossy()
                .into_owned(),
            output_format: OutputFormat(f.into()),
            options: serde_json::to_value(ImageOptions::default()).expect("options"),
        }
    }

    #[test]
    fn png_exr_png_roundtrip_with_quantization_tolerance() {
        let d = tempfile::tempdir().expect("dir");
        let input = d.path().join("in.png");
        let mid = d.path().join("out.exr");
        let back = d.path().join("back.png");
        let src = image::RgbaImage::from_fn(6, 4, |x, y| {
            image::Rgba([(x * 40) as u8, (y * 60) as u8, 140, 255])
        });
        src.save(&input).expect("png");
        let h = ImageConverterHandler;
        h.validate(&req(&input, &mid, "exr")).expect("validate");
        h.execute(&req(&input, &mid, "exr")).expect("encode");
        assert_eq!(h.probe(&mid).expect("probe").0, "exr");
        assert_eq!(dimensions(&mid).expect("dims"), (6, 4));
        h.validate(&req(&mid, &back, "png")).expect("validate");
        h.execute(&req(&mid, &back, "png")).expect("decode");
        let dst = image::open(&back).expect("png").to_rgba8();
        for (original, result) in src.pixels().zip(dst.pixels()) {
            for (a, b) in original.0.iter().zip(result.0.iter()) {
                assert!((*a as i16 - *b as i16).abs() <= 2);
            }
        }
    }

    #[test]
    fn truncated_and_extreme_headers_rejected() {
        let d = tempfile::tempdir().expect("dir");
        let file = d.path().join("bad.exr");
        std::fs::write(&file, [0x76, 0x2f, 0x31, 0x01]).expect("write");
        assert!(dimensions(&file).is_err());
        assert!(crate::image_limits::check_pixel_budget(
            &crate::image_limits::dimensions_from_pair(60_000, 60_000)
        )
        .is_err());
    }
}
