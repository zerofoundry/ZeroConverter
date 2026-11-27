// QOI (Quite OK Image) — reference implementation of an image format module.
// Copy the shape of this file when adding a format.
use super::ImageFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::FormatDescriptor;
use crate::handlers::image::options::ImageOptions;
use image::{DynamicImage, ImageFormat};
use std::io::{BufReader, Cursor, Read};
use std::path::Path;

pub static MODULE: ImageFormatModule = ImageFormatModule {
    descriptor: FormatDescriptor {
        id: "qoi",
        canonical_extension: "qoi",
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
    header.starts_with(b"qoif")
}

/// Header: "qoif", width u32 BE, height u32 BE, channels u8, colorspace u8.
fn dimensions(path: &Path) -> Result<(u32, u32), AppError> {
    let mut header = [0u8; 12];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .map_err(|e| AppError::Probe(format!("Failed to read QOI header: {}", e)))?;
    if !sniff(&header) {
        return Err(AppError::Probe("Not a QOI file".to_string()));
    }
    let width = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
    let height = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
    Ok((width, height))
}

fn decode(path: &Path) -> Result<DynamicImage, AppError> {
    let file = std::fs::File::open(path)
        .map_err(|e| AppError::Execute(format!("Failed to open QOI: {}", e)))?;
    image::load(BufReader::new(file), ImageFormat::Qoi)
        .map_err(|e| AppError::Execute(format!("Failed to decode QOI: {}", e)))
}

/// QOI stores 8-bit RGB or RGBA only; everything else is converted first.
fn encode(image: &DynamicImage, _options: &ImageOptions) -> Result<Vec<u8>, AppError> {
    let mut out = Cursor::new(Vec::new());
    let result = match image {
        DynamicImage::ImageRgb8(_) | DynamicImage::ImageRgba8(_) => {
            image.write_to(&mut out, ImageFormat::Qoi)
        }
        other if other.color().has_alpha() => {
            DynamicImage::ImageRgba8(other.to_rgba8()).write_to(&mut out, ImageFormat::Qoi)
        }
        other => DynamicImage::ImageRgb8(other.to_rgb8()).write_to(&mut out, ImageFormat::Qoi),
    };
    result.map_err(|e| AppError::Execute(format!("Failed to encode QOI: {}", e)))?;
    Ok(out.into_inner())
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
            input_path: input.to_string_lossy().to_string(),
            output_path: output.to_string_lossy().to_string(),
            output_root: output.parent().unwrap().to_string_lossy().to_string(),
            output_format: OutputFormat(format.to_string()),
            options: serde_json::to_value(ImageOptions::default()).unwrap(),
        }
    }

    /// End to end through the public handler: probe, validate, execute, both directions.
    #[test]
    fn png_to_qoi_to_png_round_trips_pixels() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("in.png");
        let qoi = dir.path().join("mid.qoi");
        let back = dir.path().join("out.png");
        let source = image::RgbaImage::from_fn(7, 5, |x, y| {
            image::Rgba([(x * 30) as u8, (y * 40) as u8, 200, (x * y * 7) as u8])
        });
        source.save(&png).unwrap();

        let handler = ImageConverterHandler;
        let to_qoi = request(&png, &qoi, "qoi");
        handler.validate(&to_qoi).unwrap();
        assert!(handler.execute(&to_qoi).unwrap().success);
        assert_eq!(handler.probe(&qoi).unwrap().0, "qoi");
        assert_eq!(dimensions(&qoi).unwrap(), (7, 5));

        let to_png = request(&qoi, &back, "png");
        handler.validate(&to_png).unwrap();
        assert!(handler.execute(&to_png).unwrap().success);
        assert_eq!(image::open(&back).unwrap().to_rgba8(), source);
    }

    #[test]
    fn qoi_is_advertised_as_input_and_output() {
        let caps = ImageConverterHandler.capabilities();
        assert!(caps.iter().any(|c| c.input.0 == "qoi"));
        let png = caps.iter().find(|c| c.input.0 == "png").unwrap();
        assert!(png.outputs.iter().any(|o| o.0 == "qoi"));
    }

    /// A file cut inside its 14-byte header fails validation and writes nothing.
    #[test]
    fn truncated_qoi_header_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut full = b"qoif".to_vec();
        full.extend_from_slice(&8u32.to_be_bytes());
        full.extend_from_slice(&8u32.to_be_bytes());
        full.extend_from_slice(&[4, 0]);
        for len in [4, 8, 11] {
            let path = dir.path().join(format!("cut{len}.qoi"));
            std::fs::write(&path, &full[..len]).unwrap();
            let out = dir.path().join(format!("cut{len}.png"));
            assert!(
                ImageConverterHandler
                    .validate(&request(&path, &out, "png"))
                    .is_err(),
                "{len} bytes"
            );
            assert!(!out.exists());
        }
    }

    #[test]
    fn oversized_qoi_header_is_rejected_before_decode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.qoi");
        let mut bytes = b"qoif".to_vec();
        bytes.extend_from_slice(&60_000u32.to_be_bytes());
        bytes.extend_from_slice(&60_000u32.to_be_bytes());
        bytes.extend_from_slice(&[4, 0]);
        std::fs::write(&path, bytes).unwrap();
        let out = dir.path().join("out.png");
        let err = ImageConverterHandler
            .validate(&request(&path, &out, "png"))
            .unwrap_err();
        assert!(err.to_string().contains("exceed safe limit"), "{}", err);
    }
}
