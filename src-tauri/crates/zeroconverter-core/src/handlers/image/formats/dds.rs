// DDS 2D texture module. Reject arrays/cubemaps/mipchains rather than
// silently exporting only their first surface.
use super::ImageFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::FormatDescriptor;
use crate::handlers::image::options;
use crate::handlers::image::options::ImageOptions;
use dds::{
    header::Header, ColorFormat, CompressionQuality, Decoder, Encoder, Format, ImageView,
    ImageViewMut, Size,
};
use image::DynamicImage;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

const MAX_DDS_PIXELS: u64 = 32_000_000;

pub static MODULE: ImageFormatModule = ImageFormatModule {
    descriptor: FormatDescriptor {
        id: "dds",
        canonical_extension: "dds",
        aliases: &[],
        can_read: true,
        can_write: true,
    },
    sniff,
    dimensions,
    decode: Some(decode),
    encode: Some(encode),
    options: &[&options::DDS_FORMAT, &options::DDS_MIPMAPS],
};

fn sniff(h: &[u8]) -> bool {
    h.starts_with(b"DDS ")
}

fn open_decoder(path: &Path) -> Result<Decoder<BufReader<File>>, AppError> {
    let mut magic = [0u8; 4];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map_err(|e| AppError::Probe(format!("DDS header read failed: {e}")))?;
    if !sniff(&magic) {
        return Err(AppError::Probe("Not a DDS file".into()));
    }
    let file = File::open(path).map_err(|e| AppError::Probe(format!("DDS open failed: {e}")))?;
    let decoder = Decoder::new(BufReader::new(file))
        .map_err(|e| AppError::Probe(format!("DDS header invalid: {e}")))?;
    let layout = decoder.layout();
    layout.texture().ok_or_else(|| {
        AppError::Probe(
            "Only a single 2D DDS texture is supported (not cubemaps/arrays/volumes)".into(),
        )
    })?;
    // A mip chain is fine: converting reads the full-size level 0.
    let size = decoder.main_size();
    if size.width == 0 || size.height == 0 || size.pixels() > MAX_DDS_PIXELS {
        return Err(AppError::Probe("DDS pixel budget exceeded".into()));
    }
    Ok(decoder)
}

fn dimensions(path: &Path) -> Result<(u32, u32), AppError> {
    let decoder = open_decoder(path)?;
    let size = decoder.main_size();
    Ok((size.width, size.height))
}

fn decode(path: &Path) -> Result<DynamicImage, AppError> {
    let mut decoder = open_decoder(path)?;
    let size = decoder.main_size();
    let len = usize::try_from(
        size.pixels()
            .checked_mul(4)
            .ok_or_else(|| AppError::Execute("DDS allocation size overflow".into()))?,
    )
    .map_err(|_| AppError::Execute("DDS allocation exceeds platform size".into()))?;
    let mut pixels = vec![0u8; len];
    let view = ImageViewMut::new(&mut pixels, size, ColorFormat::RGBA_U8)
        .ok_or_else(|| AppError::Execute("Invalid DDS pixel view".into()))?;
    decoder
        .read_surface(view)
        .map_err(|e| AppError::Execute(format!("DDS decode failed: {e}")))?;
    let image = image::RgbaImage::from_raw(size.width, size.height, pixels)
        .ok_or_else(|| AppError::Execute("Invalid DDS output dimensions".into()))?;
    Ok(DynamicImage::ImageRgba8(image))
}

fn encode(img: &DynamicImage, options: &ImageOptions) -> Result<Vec<u8>, AppError> {
    let fmt = match options
        .dds_format
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("bc7") => Format::BC7_UNORM,
        Some("bc1") => Format::BC1_UNORM,
        Some("bc2") => Format::BC2_UNORM,
        Some("bc3") => Format::BC3_UNORM,
        Some("bc4") => Format::BC4_UNORM,
        Some("bc5") => Format::BC5_UNORM,
        Some(other) => {
            return Err(AppError::Validate(format!(
                "Unknown DDS compression: {other}"
            )))
        }
    };
    encode_with(img, fmt, options.dds_mipmaps)
}

#[cfg(test)]
fn encode_format(img: &DynamicImage, fmt: Format) -> Result<Vec<u8>, AppError> {
    encode_with(img, fmt, false)
}

fn encode_with(img: &DynamicImage, fmt: Format, mipmaps: bool) -> Result<Vec<u8>, AppError> {
    let size = Size::new(img.width(), img.height());
    if size.is_empty() || size.pixels() > MAX_DDS_PIXELS {
        return Err(AppError::Execute("DDS pixel budget exceeded".into()));
    }
    let rgba = img.to_rgba8();
    let view = ImageView::new(rgba.as_raw(), size, ColorFormat::RGBA_U8)
        .ok_or_else(|| AppError::Execute("Invalid DDS source pixel view".into()))?;
    let mut header = Header::new_image(size.width, size.height, fmt);
    if mipmaps {
        header = header.with_mipmaps();
    }
    let mut output = Vec::new();
    {
        let mut encoder = Encoder::new(&mut output, fmt, &header)
            .map_err(|e| AppError::Execute(format!("DDS encoder initialization failed: {e}")))?;
        encoder.encoding.quality = CompressionQuality::Fast;
        encoder.mipmaps.generate = mipmaps;
        encoder
            .write_surface(view)
            .map_err(|e| AppError::Execute(format!("DDS BCn encoding failed: {e}")))?;
        encoder
            .finish()
            .map_err(|e| AppError::Execute(format!("DDS finish failed: {e}")))?;
    }
    Ok(output)
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
    fn bc1_with_mipmaps_writes_full_chain_and_reads_back() {
        let d = tempfile::tempdir().expect("directory");
        let path = d.path().join("mips.dds");
        let source = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            64,
            32,
            image::Rgba([200, 40, 40, 255]),
        ));
        let options = ImageOptions {
            dds_format: Some("BC1".into()),
            dds_mipmaps: true,
            ..ImageOptions::default()
        };
        std::fs::write(&path, encode(&source, &options).expect("encode")).expect("write");
        let decoder = Decoder::new(BufReader::new(File::open(&path).unwrap())).unwrap();
        assert_eq!(decoder.format(), Format::BC1_UNORM);
        assert_eq!(decoder.layout().texture().unwrap().mipmaps(), 7);
        let top = decode(&path).expect("level 0");
        assert_eq!((top.width(), top.height()), (64, 32));
        let bad = ImageOptions {
            dds_format: Some("bc9".into()),
            ..ImageOptions::default()
        };
        assert!(encode(&source, &bad).is_err());
    }

    #[test]
    fn png_bc7_dds_png_roundtrip_with_lossy_tolerance() {
        let d = tempfile::tempdir().expect("directory");
        let png = d.path().join("in.png");
        let dds = d.path().join("mid.dds");
        let back = d.path().join("back.png");
        let source = image::RgbaImage::from_fn(8, 8, |x, y| {
            image::Rgba([
                if x < 4 { 40 } else { 210 },
                if y < 4 { 30 } else { 200 },
                120,
                255,
            ])
        });
        source.save(&png).expect("save png");
        let handler = ImageConverterHandler;
        handler
            .validate(&req(&png, &dds, "dds"))
            .expect("valid outbound");
        handler
            .execute(&req(&png, &dds, "dds"))
            .expect("encode BC7");
        assert_eq!(handler.probe(&dds).expect("probe").0, "dds");
        assert_eq!(dimensions(&dds).expect("dims"), (8, 8));
        handler
            .validate(&req(&dds, &back, "png"))
            .expect("valid inbound");
        handler
            .execute(&req(&dds, &back, "png"))
            .expect("decode BC7");
        let decoded = image::open(&back).expect("png").to_rgba8();
        for (a, b) in source.pixels().zip(decoded.pixels()) {
            for (x, y) in a.0.iter().zip(b.0.iter()) {
                assert!((*x as i16 - *y as i16).abs() <= 40, "BC7 delta {x} {y}");
            }
        }
    }

    #[test]
    fn bc1_through_bc5_and_bc7_encoded_surfaces_can_decode() {
        let source = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([80, 160, 220, 255]),
        ));
        let d = tempfile::tempdir().expect("directory");
        for fmt in [
            Format::BC1_UNORM,
            Format::BC2_UNORM,
            Format::BC3_UNORM,
            Format::BC4_UNORM,
            Format::BC5_UNORM,
            Format::BC7_UNORM,
        ] {
            let bytes = encode_format(&source, fmt).expect("BCn encode");
            let path = d.path().join("surface.dds");
            std::fs::write(&path, bytes).expect("write");
            assert_eq!(dimensions(&path).expect("dims"), (4, 4));
            let decoded = decode(&path).expect("BCn decode");
            assert_eq!((decoded.width(), decoded.height()), (4, 4));
        }
    }

    #[test]
    fn invalid_truncated_and_oversized_inputs_rejected() {
        let d = tempfile::tempdir().expect("directory");
        let file = d.path().join("bad.dds");
        std::fs::write(&file, b"DDS ").expect("write");
        assert!(dimensions(&file).is_err());
        let huge = image::RgbaImage::new(1, 1);
        let mut encoded =
            encode(&DynamicImage::ImageRgba8(huge), &ImageOptions::default()).expect("encode");
        // DDS width at offset 16 and height at offset 12.
        encoded[12..16].copy_from_slice(&60_000u32.to_le_bytes());
        encoded[16..20].copy_from_slice(&60_000u32.to_le_bytes());
        std::fs::write(&file, encoded).expect("write");
        assert!(dimensions(&file).is_err());
    }
}
