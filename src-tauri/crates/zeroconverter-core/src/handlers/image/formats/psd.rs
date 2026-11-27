// PSD flattened composite reader (Photoshop format version 1).
//
// Only 8-bit grayscale (1/2 channels) and RGB (3/4 channels) with
// uncompressed or PackBits/RLE composite pixels are supported. Layers are
// never reconstructed. Unsupported color spaces, bit depths and PSB files
// fail explicitly rather than producing inaccurate colors.
use super::ImageFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::FormatDescriptor;
use image::{DynamicImage, RgbaImage};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub static MODULE: ImageFormatModule = ImageFormatModule {
    descriptor: FormatDescriptor {
        id: "psd",
        canonical_extension: "psd",
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

const PSD_MAX_DIMENSION: u32 = 30_000;

fn sniff(header: &[u8]) -> bool {
    header.len() >= 6
        && header.starts_with(b"8BPS")
        && (header[4..6] == [0, 1] || header[4..6] == [0, 2])
}

struct Header {
    width: u32,
    height: u32,
    channels: usize,
    mode: u16,
}

fn read_header<R: Read>(reader: &mut R) -> Result<Header, AppError> {
    let mut bytes = [0u8; 26];
    reader
        .read_exact(&mut bytes)
        .map_err(|e| AppError::Probe(format!("Truncated PSD header: {}", e)))?;
    if &bytes[..4] != b"8BPS" {
        return Err(AppError::Probe("Invalid PSD signature".to_string()));
    }
    if bytes[4..6] != [0, 1] {
        return Err(AppError::Probe(
            "Unsupported Photoshop file version (PSB is not supported)".to_string(),
        ));
    }
    if bytes[6..12] != [0; 6] {
        return Err(AppError::Probe(
            "Invalid PSD reserved header bytes".to_string(),
        ));
    }
    let channels = u16::from_be_bytes([bytes[12], bytes[13]]) as usize;
    let height = u32::from_be_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
    let width = u32::from_be_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
    let depth = u16::from_be_bytes([bytes[22], bytes[23]]);
    let mode = u16::from_be_bytes([bytes[24], bytes[25]]);
    if width == 0 || height == 0 || width > PSD_MAX_DIMENSION || height > PSD_MAX_DIMENSION {
        return Err(AppError::Probe(
            "Invalid or unsupported PSD dimensions".to_string(),
        ));
    }
    if depth != 8 {
        return Err(AppError::Probe(format!(
            "Unsupported PSD bit depth {}; only 8-bit is supported",
            depth
        )));
    }
    match (mode, channels) {
        (1, 1 | 2) | (3, 3 | 4) => {}
        _ => {
            return Err(AppError::Probe(format!(
                "Unsupported PSD color mode {} with {} channels; only grayscale/RGB composites are supported",
                mode, channels
            )));
        }
    }
    Ok(Header {
        width,
        height,
        channels,
        mode,
    })
}

fn dimensions(path: &Path) -> Result<(u32, u32), AppError> {
    let file = std::fs::File::open(path)
        .map_err(|e| AppError::Probe(format!("Failed to open PSD: {}", e)))?;
    let header = read_header(&mut BufReader::new(file))?;
    Ok((header.width, header.height))
}

fn read_u32<R: Read>(reader: &mut R) -> Result<u32, AppError> {
    let mut bytes = [0u8; 4];
    reader
        .read_exact(&mut bytes)
        .map_err(|e| AppError::Execute(format!("Truncated PSD section length: {}", e)))?;
    Ok(u32::from_be_bytes(bytes))
}

/// Three length-prefixed PSD sections: color-mode data, image resources,
/// and layer/mask information. Seek past them without allocating their contents.
fn skip_section<R: Read + Seek>(reader: &mut R, file_len: u64) -> Result<(), AppError> {
    let length = u64::from(read_u32(reader)?);
    let start = reader
        .stream_position()
        .map_err(|e| AppError::Execute(format!("PSD seek failed: {}", e)))?;
    let end = start
        .checked_add(length)
        .filter(|end| *end <= file_len)
        .ok_or_else(|| AppError::Execute("Truncated PSD metadata section".to_string()))?;
    reader
        .seek(SeekFrom::Start(end))
        .map_err(|e| AppError::Execute(format!("PSD seek failed: {}", e)))?;
    Ok(())
}

fn unpack_packbits(encoded: &[u8], output: &mut [u8]) -> Result<(), AppError> {
    let (mut src, mut dst) = (0usize, 0usize);
    while src < encoded.len() {
        let control = encoded[src] as i8;
        src += 1;
        if control >= 0 {
            let count = control as usize + 1;
            let end_src = src
                .checked_add(count)
                .ok_or_else(|| AppError::Execute("PSD RLE literal length overflow".to_string()))?;
            let end_dst = dst
                .checked_add(count)
                .ok_or_else(|| AppError::Execute("PSD RLE row length overflow".to_string()))?;
            if end_src > encoded.len() || end_dst > output.len() {
                return Err(AppError::Execute("Invalid PSD RLE literal".to_string()));
            }
            output[dst..end_dst].copy_from_slice(&encoded[src..end_src]);
            src = end_src;
            dst = end_dst;
        } else if control != -128 {
            let count = (1i16 - i16::from(control)) as usize;
            let end_dst = dst
                .checked_add(count)
                .ok_or_else(|| AppError::Execute("PSD RLE row length overflow".to_string()))?;
            if src >= encoded.len() || end_dst > output.len() {
                return Err(AppError::Execute("Invalid PSD RLE repeat".to_string()));
            }
            output[dst..end_dst].fill(encoded[src]);
            src += 1;
            dst = end_dst;
        }
        // -128 is a PackBits no-op.
    }
    if dst != output.len() {
        return Err(AppError::Execute(
            "PSD RLE scanline does not match declared width".to_string(),
        ));
    }
    Ok(())
}

fn write_plane_row(
    pixels: &mut [u8],
    width: usize,
    y: usize,
    channel: usize,
    mode: u16,
    row: &[u8],
) {
    for (x, value) in row.iter().copied().enumerate() {
        let pos = (y * width + x) * 4;
        let rgba = &mut pixels[pos..pos + 4];
        if mode == 1 && channel == 0 {
            rgba[0] = value;
            rgba[1] = value;
            rgba[2] = value;
        } else if mode == 1 {
            rgba[3] = value;
        } else {
            rgba[channel] = value;
        }
    }
}

fn decode(path: &Path) -> Result<DynamicImage, AppError> {
    let file = std::fs::File::open(path)
        .map_err(|e| AppError::Execute(format!("Failed to open PSD: {}", e)))?;
    let file_len = file
        .metadata()
        .map_err(|e| AppError::Execute(format!("Failed to stat PSD: {}", e)))?
        .len();
    let mut reader = BufReader::new(file);
    let header = read_header(&mut reader)?;
    crate::image_limits::check_pixel_budget(&crate::image_limits::dimensions_from_pair(
        header.width,
        header.height,
    ))?;
    for _ in 0..3 {
        skip_section(&mut reader, file_len)?;
    }
    let mut compression_bytes = [0u8; 2];
    reader
        .read_exact(&mut compression_bytes)
        .map_err(|e| AppError::Execute(format!("Missing PSD composite: {}", e)))?;
    let compression = u16::from_be_bytes(compression_bytes);
    if compression > 1 {
        return Err(AppError::Execute(format!(
            "Unsupported PSD compression {}; only RAW and RLE are supported",
            compression
        )));
    }

    let w = header.width as usize;
    let h = header.height as usize;
    let plane_size = w
        .checked_mul(h)
        .ok_or_else(|| AppError::Execute("PSD plane dimensions overflow".to_string()))?;
    let pixel_bytes = plane_size
        .checked_mul(4)
        .ok_or_else(|| AppError::Execute("PSD RGBA dimensions overflow".to_string()))?;
    let mut pixels = vec![255u8; pixel_bytes];
    let mut row = vec![0u8; w];

    let mut rle_lengths = Vec::new();
    if compression == 1 {
        let scanlines = h
            .checked_mul(header.channels)
            .ok_or_else(|| AppError::Execute("PSD scanline count overflow".to_string()))?;
        let count_bytes = scanlines
            .checked_mul(2)
            .ok_or_else(|| AppError::Execute("PSD RLE table length overflow".to_string()))?;
        let pos = reader
            .stream_position()
            .map_err(|e| AppError::Execute(format!("PSD seek failed: {}", e)))?;
        if pos
            .checked_add(count_bytes as u64)
            .is_none_or(|end| end > file_len)
        {
            return Err(AppError::Execute("Truncated PSD RLE row table".to_string()));
        }
        rle_lengths.reserve(scanlines);
        for _ in 0..scanlines {
            let mut bytes = [0u8; 2];
            reader
                .read_exact(&mut bytes)
                .map_err(|e| AppError::Execute(format!("Truncated PSD RLE table: {}", e)))?;
            rle_lengths.push(u16::from_be_bytes(bytes));
        }
    }
    let remaining = file_len.saturating_sub(
        reader
            .stream_position()
            .map_err(|e| AppError::Execute(format!("PSD seek failed: {}", e)))?,
    );
    let required = if compression == 0 {
        (plane_size as u64)
            .checked_mul(header.channels as u64)
            .ok_or_else(|| AppError::Execute("PSD raw byte count overflow".to_string()))?
    } else {
        rle_lengths.iter().map(|&n| u64::from(n)).sum()
    };
    if required > remaining {
        return Err(AppError::Execute(
            "Truncated PSD composite pixels".to_string(),
        ));
    }

    let mut packed = Vec::new();
    for channel in 0..header.channels {
        for y in 0..h {
            if compression == 0 {
                reader
                    .read_exact(&mut row)
                    .map_err(|e| AppError::Execute(format!("Truncated PSD raw row: {}", e)))?;
            } else {
                let len = usize::from(rle_lengths[channel * h + y]);
                packed.resize(len, 0);
                reader
                    .read_exact(&mut packed)
                    .map_err(|e| AppError::Execute(format!("Truncated PSD RLE row: {}", e)))?;
                unpack_packbits(&packed, &mut row)?;
            }
            write_plane_row(&mut pixels, w, y, channel, header.mode, &row);
        }
    }
    let rgba = RgbaImage::from_raw(header.width, header.height, pixels)
        .ok_or_else(|| AppError::Execute("Invalid PSD RGBA buffer".to_string()))?;
    Ok(DynamicImage::ImageRgba8(rgba))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::traits::ConverterHandler;
    use crate::conversion::types::{AssetFamily, ConversionRequest, OutputFormat};
    use crate::handlers::image::ImageConverterHandler;

    fn fixture(width: u32, height: u32, mode: u16, planes: &[&[u8]], rle: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"8BPS");
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&[0u8; 6]);
        bytes.extend_from_slice(&(planes.len() as u16).to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&8u16.to_be_bytes());
        bytes.extend_from_slice(&mode.to_be_bytes());
        for _ in 0..3 {
            bytes.extend_from_slice(&0u32.to_be_bytes());
        }
        bytes.extend_from_slice(&(if rle { 1u16 } else { 0u16 }).to_be_bytes());
        if rle {
            let mut scanlines = Vec::new();
            for plane in planes {
                for row in plane.chunks(width as usize) {
                    let mut data = Vec::new();
                    for literal in row.chunks(128) {
                        data.push((literal.len() - 1) as u8);
                        data.extend_from_slice(literal);
                    }
                    scanlines.push(data);
                }
            }
            for data in &scanlines {
                bytes.extend_from_slice(&(data.len() as u16).to_be_bytes());
            }
            for data in scanlines {
                bytes.extend_from_slice(&data);
            }
        } else {
            for plane in planes {
                bytes.extend_from_slice(plane);
            }
        }
        bytes
    }

    fn request(input: &Path, output: &Path, format: &str) -> ConversionRequest {
        ConversionRequest {
            family: AssetFamily::Image,
            input_path: input.to_string_lossy().to_string(),
            output_path: output.to_string_lossy().to_string(),
            output_root: output.parent().unwrap().to_string_lossy().to_string(),
            output_format: OutputFormat(format.to_string()),
            options: serde_json::to_value(crate::handlers::image::options::ImageOptions::default())
                .unwrap(),
        }
    }

    #[test]
    fn raw_and_rle_composites_round_trip_to_png() {
        for rle in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let psd = dir.path().join("source.psd");
            let png = dir.path().join("out.png");
            std::fs::write(
                &psd,
                fixture(2, 1, 3, &[&[10, 20], &[30, 40], &[50, 60], &[70, 80]], rle),
            )
            .unwrap();
            // Four-channel RGB: R, G, B, alpha.
            let handler = ImageConverterHandler;
            assert_eq!(handler.probe(&psd).unwrap().0, "psd");
            assert_eq!(dimensions(&psd).unwrap(), (2, 1));
            handler.validate(&request(&psd, &png, "png")).unwrap();
            handler.execute(&request(&psd, &png, "png")).unwrap();
            let result = image::open(&png).unwrap().to_rgba8();
            assert_eq!(result.get_pixel(0, 0).0, [10, 30, 50, 70]);
            assert_eq!(result.get_pixel(1, 0).0, [20, 40, 60, 80]);
        }
    }

    #[test]
    fn grayscale_composite_and_pdf_route() {
        let dir = tempfile::tempdir().unwrap();
        let psd = dir.path().join("gray.psd");
        let png = dir.path().join("gray.png");
        let pdf = dir.path().join("gray.pdf");
        std::fs::write(&psd, fixture(2, 1, 1, &[&[20, 220]], false)).unwrap();
        let handler = ImageConverterHandler;
        assert!(handler
            .capabilities()
            .iter()
            .any(|cap| { cap.input.0 == "psd" && cap.outputs.iter().any(|out| out.0 == "pdf") }));
        handler.validate(&request(&psd, &png, "png")).unwrap();
        handler.execute(&request(&psd, &png, "png")).unwrap();
        assert_eq!(
            image::open(&png).unwrap().to_rgba8().get_pixel(0, 0).0,
            [20, 20, 20, 255]
        );
        handler.validate(&request(&psd, &pdf, "pdf")).unwrap();
        handler.execute(&request(&psd, &pdf, "pdf")).unwrap();
        let pdf_bytes = std::fs::read(&pdf).unwrap();
        assert!(String::from_utf8_lossy(&pdf_bytes).contains("/MediaBox [0 0 2 1]"));
    }

    #[test]
    fn rejects_oversized_header_before_decoding() {
        let dir = tempfile::tempdir().unwrap();
        let psd = dir.path().join("huge.psd");
        let mut bytes = fixture(1, 1, 3, &[&[0], &[0], &[0]], false);
        bytes[14..18].copy_from_slice(&30_000u32.to_be_bytes());
        bytes[18..22].copy_from_slice(&30_000u32.to_be_bytes());
        std::fs::write(&psd, bytes).unwrap();
        let png = dir.path().join("out.png");
        let err = ImageConverterHandler
            .validate(&request(&psd, &png, "png"))
            .unwrap_err();
        assert!(err.to_string().contains("exceed safe limit"), "{}", err);
    }

    #[test]
    fn rejects_psb_and_non_rgb_depth_or_color() {
        let dir = tempfile::tempdir().unwrap();
        let psd = dir.path().join("unsupported.psd");
        let original = fixture(1, 1, 3, &[&[0], &[0], &[0]], false);
        for (offset, val) in [(4usize, 2u16), (22, 16), (24, 4)] {
            let mut bytes = original.clone();
            bytes[offset..offset + 2].copy_from_slice(&val.to_be_bytes());
            std::fs::write(&psd, bytes).unwrap();
            assert!(dimensions(&psd).is_err());
        }
    }

    #[test]
    fn truncation_and_invalid_rle_fail_without_output() {
        let dir = tempfile::tempdir().unwrap();
        let psd = dir.path().join("bad.psd");
        let png = dir.path().join("out.png");
        let mut truncated = fixture(
            3,
            1,
            3,
            &[&[10, 20, 30], &[40, 50, 60], &[70, 80, 90]],
            false,
        );
        truncated.pop();
        std::fs::write(&psd, truncated).unwrap();
        assert!(ImageConverterHandler
            .execute(&request(&psd, &png, "png"))
            .is_err());
        assert!(!png.exists());

        // RLE row with a 128-pixel run but a 3-pixel destination.
        let mut corrupt = fixture(
            3,
            1,
            3,
            &[&[10, 20, 30], &[40, 50, 60], &[70, 80, 90]],
            true,
        );
        let first_row_data_offset = 26 + 12 + 2 + 3 * 2;
        corrupt[first_row_data_offset] = 129u8; // -127 => 128 repeated pixels
        std::fs::write(&psd, corrupt).unwrap();
        assert!(ImageConverterHandler
            .execute(&request(&psd, &png, "png"))
            .is_err());
        assert!(!png.exists());
    }
}
