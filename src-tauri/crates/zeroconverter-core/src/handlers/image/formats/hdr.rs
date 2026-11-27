// Radiance RGBE HDR. Headers are bounded before the decoder sees image data.
use super::ImageFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::FormatDescriptor;
use crate::handlers::image::options::ImageOptions;
use image::{DynamicImage, ImageFormat};
use std::io::{BufReader, Cursor, Read};
use std::path::Path;

const MAX_HEADER_BYTES: usize = 8192;
const MAX_FLOAT_PIXELS: u64 = 32_000_000;

pub static MODULE: ImageFormatModule = ImageFormatModule {
    descriptor: FormatDescriptor {
        id: "hdr",
        canonical_extension: "hdr",
        aliases: &["rgbe"],
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
    header.starts_with(b"#?RADIANCE\n")
        || header.starts_with(b"#?RGBE\n")
        || header.starts_with(b"#?RADIANCE\r\n")
        || header.starts_with(b"#?RGBE\r\n")
}

// Resolution is *after* the blank header separator: "-Y height +X width".
fn parse_dimensions(bytes: &[u8]) -> Result<(u32, u32), AppError> {
    if !sniff(bytes) {
        return Err(AppError::Probe("Not a Radiance HDR header".into()));
    }
    let text = String::from_utf8_lossy(bytes);
    let mut separated = false;
    for line in text.lines() {
        let line = line.trim_end_matches('\r').trim();
        if !separated {
            if line.is_empty() {
                separated = true;
            }
            continue;
        }
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        if fields.len() != 4
            || !matches!(fields[0], "+Y" | "-Y")
            || !matches!(fields[2], "+X" | "-X")
        {
            return Err(AppError::Probe("Invalid Radiance HDR resolution".into()));
        }
        let height: u32 = fields[1]
            .parse()
            .map_err(|_| AppError::Probe("Invalid HDR height".into()))?;
        let width: u32 = fields[3]
            .parse()
            .map_err(|_| AppError::Probe("Invalid HDR width".into()))?;
        if width == 0 || height == 0 {
            return Err(AppError::Probe("Zero-sized HDR image".into()));
        }
        return Ok((width, height));
    }
    Err(AppError::Probe("Missing Radiance HDR resolution".into()))
}

fn dimensions(path: &Path) -> Result<(u32, u32), AppError> {
    let f =
        std::fs::File::open(path).map_err(|e| AppError::Probe(format!("HDR open failed: {e}")))?;
    let mut header = Vec::with_capacity(MAX_HEADER_BYTES);
    f.take(MAX_HEADER_BYTES as u64)
        .read_to_end(&mut header)
        .map_err(|e| AppError::Probe(format!("HDR header read failed: {e}")))?;
    let (w, h) = parse_dimensions(&header)?;
    if (w as u64) * (h as u64) > MAX_FLOAT_PIXELS {
        return Err(AppError::Probe("HDR float pixel budget exceeded".into()));
    }
    Ok((w, h))
}

fn decode(path: &Path) -> Result<DynamicImage, AppError> {
    dimensions(path)?;
    let file = std::fs::File::open(path)
        .map_err(|e| AppError::Execute(format!("HDR open failed: {e}")))?;
    image::load(BufReader::new(file), ImageFormat::Hdr)
        .map_err(|e| AppError::Execute(format!("HDR decode failed: {e}")))
}

fn encode(img: &DynamicImage, _options: &ImageOptions) -> Result<Vec<u8>, AppError> {
    let pixels = (img.width() as u64) * (img.height() as u64);
    if pixels == 0 || pixels > MAX_FLOAT_PIXELS {
        return Err(AppError::Execute("HDR float pixel budget exceeded".into()));
    }
    let mut out = Cursor::new(Vec::new());
    // RGBE cannot preserve alpha; float RGB is preserved up to RGBE quantization.
    DynamicImage::ImageRgb32F(img.to_rgb32f())
        .write_to(&mut out, ImageFormat::Hdr)
        .map_err(|e| AppError::Execute(format!("HDR encode failed: {e}")))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::traits::ConverterHandler;
    use crate::conversion::types::{AssetFamily, ConversionRequest, OutputFormat};
    use crate::handlers::image::ImageConverterHandler;

    fn req(i: &Path, o: &Path, fmt: &str) -> ConversionRequest {
        ConversionRequest {
            family: AssetFamily::Image,
            input_path: i.to_string_lossy().into_owned(),
            output_path: o.to_string_lossy().into_owned(),
            output_root: o.parent().expect("parent").to_string_lossy().into_owned(),
            output_format: OutputFormat(fmt.into()),
            options: serde_json::to_value(ImageOptions::default()).expect("opts"),
        }
    }

    #[test]
    fn png_hdr_png_roundtrip_rgb_with_quantization_tolerance() {
        let d = tempfile::tempdir().expect("dir");
        let input = d.path().join("in.png");
        let mid = d.path().join("mid.hdr");
        let back = d.path().join("back.png");
        let src = image::RgbImage::from_fn(8, 4, |x, y| {
            image::Rgb([(x * 25) as u8, (y * 45) as u8, 150])
        });
        src.save(&input).expect("png");
        let h = ImageConverterHandler;
        h.validate(&req(&input, &mid, "hdr")).expect("validate");
        h.execute(&req(&input, &mid, "hdr")).expect("encode");
        assert_eq!(h.probe(&mid).expect("probe").0, "hdr");
        assert_eq!(dimensions(&mid).expect("dims"), (8, 4));
        h.validate(&req(&mid, &back, "png")).expect("validate");
        h.execute(&req(&mid, &back, "png")).expect("decode");
        let decoded = image::open(&back).expect("png").to_rgb8();
        for (a, b) in src.pixels().zip(decoded.pixels()) {
            for (x, y) in a.0.iter().zip(b.0.iter()) {
                assert!((*x as i16 - *y as i16).abs() <= 4);
            }
        }
    }

    #[test]
    fn rejects_truncation_and_huge_dimensions() {
        let d = tempfile::tempdir().expect("dir");
        let path = d.path().join("bad.hdr");
        std::fs::write(&path, b"#?RADIANCE\n").expect("write");
        assert!(dimensions(&path).is_err());
        let bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 60000 +X 60000\n";
        std::fs::write(&path, bytes).expect("write");
        assert_eq!(parse_dimensions(bytes).expect("parse"), (60_000, 60_000));
        assert!(ImageConverterHandler
            .validate(&req(&path, &d.path().join("out.png"), "png"))
            .is_err());
    }
}
