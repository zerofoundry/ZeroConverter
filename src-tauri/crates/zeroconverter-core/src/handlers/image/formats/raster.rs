// The everyday raster formats the `image` crate reads and writes: PNG, JPG,
// WebP, BMP, TIFF, ICO, and GIF (read only). One small module each, the same
// shape as `qoi.rs`; what differs is the signature and how a float or
// alpha image is brought to something the encoder accepts.
use super::ImageFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::FormatDescriptor;
use crate::handlers::image::options;
use crate::handlers::image::options::ImageOptions;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat};
use std::borrow::Cow;
use std::io::{BufReader, Cursor};
use std::path::Path;

const fn descriptor(
    id: &'static str,
    aliases: &'static [&'static str],
    can_write: bool,
) -> FormatDescriptor {
    FormatDescriptor {
        id,
        canonical_extension: id,
        aliases,
        can_read: true,
        can_write,
    }
}

pub static PNG: ImageFormatModule = ImageFormatModule {
    descriptor: descriptor("png", &[], true),
    sniff: |h| h.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
    dimensions: |p| dimensions(p, ImageFormat::Png),
    decode: Some(|p| decode(p, ImageFormat::Png)),
    encode: Some(|img, _| write(&float_to_int(img, true), ImageFormat::Png)),
    options: &[],
};

pub static JPG: ImageFormatModule = ImageFormatModule {
    descriptor: descriptor("jpg", &["jpeg"], true),
    sniff: |h| h.starts_with(&[0xFF, 0xD8, 0xFF]),
    dimensions: |p| dimensions(p, ImageFormat::Jpeg),
    decode: Some(|p| decode(p, ImageFormat::Jpeg)),
    encode: Some(encode_jpg),
    options: &[&options::QUALITY],
};

pub static WEBP: ImageFormatModule = ImageFormatModule {
    descriptor: descriptor("webp", &[], true),
    sniff: |h| h.len() >= 12 && h.starts_with(b"RIFF") && &h[8..12] == b"WEBP",
    dimensions: |p| dimensions(p, ImageFormat::WebP),
    decode: Some(|p| decode(p, ImageFormat::WebP)),
    encode: Some(|img, _| write(&float_to_int(img, false), ImageFormat::WebP)),
    options: &[],
};

pub static BMP: ImageFormatModule = ImageFormatModule {
    descriptor: descriptor("bmp", &[], true),
    sniff: |h| h.starts_with(b"BM"),
    dimensions: |p| dimensions(p, ImageFormat::Bmp),
    decode: Some(|p| decode(p, ImageFormat::Bmp)),
    encode: Some(|img, _| write(&float_to_int(img, false), ImageFormat::Bmp)),
    options: &[],
};

pub static TIFF: ImageFormatModule = ImageFormatModule {
    descriptor: descriptor("tiff", &["tif"], true),
    sniff: |h| h.starts_with(&[0x49, 0x49, 0x2A, 0x00]) || h.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]),
    dimensions: |p| dimensions(p, ImageFormat::Tiff),
    decode: Some(|p| decode(p, ImageFormat::Tiff)),
    encode: Some(|img, _| write(&float_to_int(img, true), ImageFormat::Tiff)),
    options: &[],
};

/// ICO: `00 00 01 00` and at least one image. Cursor files (`02 00`) and
/// empty icons are refused by the probe with their own message.
pub static ICO: ImageFormatModule = ImageFormatModule {
    descriptor: descriptor("ico", &[], true),
    sniff: |h| {
        h.len() >= 6
            && h.starts_with(&[0x00, 0x00, 0x01, 0x00])
            && u16::from_le_bytes([h[4], h[5]]) > 0
    },
    dimensions: |p| dimensions(p, ImageFormat::Ico),
    decode: Some(|p| decode(p, ImageFormat::Ico)),
    // The encoder needs RGBA8; JPG or BMP sources have no alpha.
    encode: Some(|img, _| {
        let rgba = DynamicImage::ImageRgba8(float_to_int(img, false).to_rgba8());
        write(&rgba, ImageFormat::Ico)
    }),
    options: &[],
};

/// GIF is read only, and only a single frame (validation refuses animation).
pub static GIF: ImageFormatModule = ImageFormatModule {
    descriptor: descriptor("gif", &[], false),
    sniff: |h| h.starts_with(b"GIF87a") || h.starts_with(b"GIF89a"),
    dimensions: |p| dimensions(p, ImageFormat::Gif),
    decode: Some(|p| decode(p, ImageFormat::Gif)),
    encode: None,
    options: &[],
};

fn open(path: &Path) -> Result<BufReader<std::fs::File>, AppError> {
    std::fs::File::open(path)
        .map(BufReader::new)
        .map_err(|e| AppError::Io(format!("Failed to open image: {}", e)))
}

/// Header dimensions as a viewer shows the image: an EXIF orientation that
/// turns it a quarter (phone photos) swaps width and height, so the pixel
/// budget, resize targets and ICO limits all see the decoded size.
fn dimensions(path: &Path, format: ImageFormat) -> Result<(u32, u32), AppError> {
    let mut decoder = image::ImageReader::with_format(open(path)?, format)
        .into_decoder()
        .map_err(|e| AppError::Probe(format!("Failed to read image dimensions: {}", e)))?;
    let (w, h) = decoder.dimensions();
    Ok(if swaps_axes(orientation(&mut decoder)) {
        (h, w)
    } else {
        (w, h)
    })
}

/// Decodes and applies the EXIF orientation (JPEG, TIFF, WebP, PNG eXIf),
/// so outputs, which never carry the source EXIF, look as viewers show the
/// original.
fn decode(path: &Path, format: ImageFormat) -> Result<DynamicImage, AppError> {
    let mut decoder = image::ImageReader::with_format(open(path)?, format)
        .into_decoder()
        .map_err(|e| AppError::Execute(format!("Failed to decode image: {}", e)))?;
    let orientation = orientation(&mut decoder);
    let mut img = DynamicImage::from_decoder(decoder)
        .map_err(|e| AppError::Execute(format!("Failed to decode image: {}", e)))?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// A malformed EXIF block is ignored, as viewers do, rather than failing the file.
pub(crate) fn orientation(decoder: &mut impl ImageDecoder) -> Orientation {
    decoder.orientation().unwrap_or(Orientation::NoTransforms)
}

fn swaps_axes(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

/// Float sources (EXR, HDR) cannot be written by these encoders. Values are
/// clamped to 0..1 (no tone mapping); `wide` formats keep 16 bits.
fn float_to_int(img: &DynamicImage, wide: bool) -> Cow<'_, DynamicImage> {
    if !matches!(
        img,
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_)
    ) {
        return Cow::Borrowed(img);
    }
    Cow::Owned(match (img.color().has_alpha(), wide) {
        (true, true) => DynamicImage::ImageRgba16(img.to_rgba16()),
        (false, true) => DynamicImage::ImageRgb16(img.to_rgb16()),
        (true, false) => DynamicImage::ImageRgba8(img.to_rgba8()),
        (false, false) => DynamicImage::ImageRgb8(img.to_rgb8()),
    })
}

fn write(img: &DynamicImage, format: ImageFormat) -> Result<Vec<u8>, AppError> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, format)
        .map_err(|e| AppError::Execute(format!("Failed to encode output image: {}", e)))?;
    Ok(out.into_inner())
}

fn encode_jpg(img: &DynamicImage, options: &ImageOptions) -> Result<Vec<u8>, AppError> {
    let img = float_to_int(img, false);
    let mut out = Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, options.quality);
    img.write_with_encoder(encoder)
        .map_err(|e| AppError::Execute(format!("Failed to encode JPEG output image: {}", e)))?;
    Ok(out.into_inner())
}
