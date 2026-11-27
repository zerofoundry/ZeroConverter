// Narrow, lossless KTX2 profile: 2D single-mip RGBA8_UNORM, not Basis/UASTC.
// KTX2 has many more capabilities; unsupported texture kinds fail closed.
use super::ImageFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::FormatDescriptor;
use crate::handlers::image::options::ImageOptions;
use image::DynamicImage;
use std::io::Read;
use std::path::Path;

const MAGIC: &[u8; 12] = b"\xABKTX 20\xBB\r\n\x1A\n";
const HEADER_LEN: usize = 80;
const INDEX_LEN: usize = 24;
const DFD_LEN: usize = 92;
const PIXEL_OFFSET: usize = 200;
const MAX_PIXELS: u64 = 16_000_000;
const MAX_INPUT_BYTES: u64 = MAX_PIXELS * 4 + 1024 * 1024;

pub static MODULE: ImageFormatModule = ImageFormatModule {
    descriptor: FormatDescriptor {
        id: "ktx2",
        canonical_extension: "ktx2",
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

fn sniff(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

#[derive(Debug)]
struct Ktx2Profile {
    width: u32,
    height: u32,
    offset: usize,
    len: usize,
}

fn parse_profile(header: &[u8], file_len: u64) -> Result<Ktx2Profile, AppError> {
    let reject =
        || AppError::Probe("KTX2 is malformed or outside the 2D RGBA8 single-level profile".into());
    if header.len() < HEADER_LEN + INDEX_LEN || !sniff(header) {
        return Err(reject());
    }
    // Only Vulkan VK_FORMAT_R8G8B8A8_UNORM (37), with uncompressed level data.
    if read_u32(header, 12) != Some(37)
        || read_u32(header, 16) != Some(1)
        || read_u32(header, 28) != Some(0) // depth
        || read_u32(header, 32) != Some(0) // layers
        || read_u32(header, 36) != Some(1) // faces
        || read_u32(header, 40) != Some(1) // mips
        || read_u32(header, 44) != Some(0)
    // supercompression
    {
        return Err(reject());
    }
    let (width, height) = (
        read_u32(header, 20).ok_or_else(reject)?,
        read_u32(header, 24).ok_or_else(reject)?,
    );
    let pixels = (width as u64)
        .checked_mul(height as u64)
        .ok_or_else(reject)?;
    if width == 0 || height == 0 || pixels > MAX_PIXELS {
        return Err(reject());
    }
    let dfd_start = read_u32(header, 48).ok_or_else(reject)? as u64;
    let dfd_len = read_u32(header, 52).ok_or_else(reject)? as u64;
    let kvd_len = read_u32(header, 60).ok_or_else(reject)?;
    let sgd_len = read_u64(header, 72).ok_or_else(reject)?;
    // Require a complete 4-channel DFD, reject unknown metadata and compression.
    if dfd_start != 104 || dfd_len != DFD_LEN as u64 || kvd_len != 0 || sgd_len != 0 {
        return Err(reject());
    }
    let offset = read_u64(header, 80).ok_or_else(reject)?;
    let len = read_u64(header, 88).ok_or_else(reject)?;
    let unpacked = read_u64(header, 96).ok_or_else(reject)?;
    if offset % 8 != 0
        || offset < dfd_start + dfd_len
        || len != pixels * 4
        || unpacked != len
        || offset
            .checked_add(len)
            .filter(|end| *end <= file_len)
            .is_none()
    {
        return Err(reject());
    }
    let offset = usize::try_from(offset).map_err(|_| reject())?;
    let len = usize::try_from(len).map_err(|_| reject())?;
    Ok(Ktx2Profile {
        width,
        height,
        offset,
        len,
    })
}

fn read_header(path: &Path) -> Result<([u8; HEADER_LEN + INDEX_LEN], u64), AppError> {
    let file_len = std::fs::metadata(path)
        .map_err(|e| AppError::Probe(format!("KTX2 stat failed: {e}")))?
        .len();
    if file_len > MAX_INPUT_BYTES {
        return Err(AppError::Probe("KTX2 exceeds input byte budget".into()));
    }
    let mut header = [0u8; HEADER_LEN + INDEX_LEN];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .map_err(|e| AppError::Probe(format!("KTX2 header read failed: {e}")))?;
    Ok((header, file_len))
}

fn dimensions(path: &Path) -> Result<(u32, u32), AppError> {
    let (header, size) = read_header(path)?;
    let profile = parse_profile(&header, size)?;
    Ok((profile.width, profile.height))
}

// Reject mismatched texel channel description: the raw level bytes are
// interpreted as RGBA8 only after validating the Khronos DFD layout.
fn valid_rgba8_dfd(bytes: &[u8]) -> bool {
    let Some(dfd) = bytes.get(104..104 + DFD_LEN) else {
        return false;
    };
    read_u32(dfd, 0) == Some(DFD_LEN as u32)
        && read_u32(dfd, 4) == Some(0)
        && dfd.get(8..12) == Some(&[2, 0, 88, 0][..])
        && dfd.get(12..16) == Some(&[1, 1, 1, 0][..])
        && dfd.get(16..20) == Some(&[0, 0, 0, 0][..])
        && dfd.get(20..28) == Some(&[4, 0, 0, 0, 0, 0, 0, 0][..])
        && [(0u16, 0u8), (8, 1), (16, 2), (24, 15)]
            .iter()
            .enumerate()
            .all(|(i, (bit, channel))| {
                let at = 28 + i * 16;
                dfd.get(at..at + 2) == Some(&bit.to_le_bytes()[..])
                    && dfd.get(at + 2..at + 8) == Some(&[7, *channel, 0, 0, 0, 0][..])
                    && read_u32(dfd, at + 8) == Some(0)
                    && read_u32(dfd, at + 12) == Some(255)
            })
}

fn decode(path: &Path) -> Result<DynamicImage, AppError> {
    let (header, file_len) = read_header(path)?;
    let profile = parse_profile(&header, file_len)?;
    // One RGBA8 level, already inside the pixel budget, plus room for the
    // header and metadata. Anything bigger is not a file we can read.
    let cap = (profile.offset as u64)
        .saturating_add(profile.len as u64)
        .saturating_add(1024 * 1024);
    if file_len > cap {
        return Err(AppError::Execute(
            "KTX2 file is larger than its one level".into(),
        ));
    }
    let mut bytes = Vec::new();
    {
        use std::io::Read;
        std::fs::File::open(path)
            .and_then(|f| f.take(cap).read_to_end(&mut bytes))
            .map_err(|e| AppError::Execute(format!("KTX2 read failed: {e}")))?;
    }
    if !valid_rgba8_dfd(&bytes) {
        return Err(AppError::Execute("KTX2 DFD is not linear RGBA8".into()));
    }
    // The reference parser validates the container before pixels are accessed.
    let parsed = ktx2::Reader::new(bytes.as_slice())
        .map_err(|e| AppError::Execute(format!("KTX2 container invalid: {e:?}")))?;
    let first_level = parsed
        .levels()
        .next()
        .ok_or_else(|| AppError::Execute("KTX2 missing level zero".into()))?;
    let pixel_end = profile
        .offset
        .checked_add(profile.len)
        .ok_or_else(|| AppError::Execute("KTX2 level offset overflow".into()))?;
    if pixel_end > bytes.len() || first_level.data != &bytes[profile.offset..pixel_end] {
        return Err(AppError::Execute("KTX2 level index mismatch".into()));
    }
    let pixels = bytes[profile.offset..pixel_end].to_vec();
    let img = image::RgbaImage::from_raw(profile.width, profile.height, pixels)
        .ok_or_else(|| AppError::Execute("KTX2 RGBA dimensions mismatch".into()))?;
    Ok(DynamicImage::ImageRgba8(img))
}

fn write_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn write_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn write_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn encode(img: &DynamicImage, _options: &ImageOptions) -> Result<Vec<u8>, AppError> {
    let (w, h) = (img.width(), img.height());
    let pixels = (w as u64)
        .checked_mul(h as u64)
        .ok_or_else(|| AppError::Execute("KTX2 dimension overflow".into()))?;
    if pixels == 0 || pixels > MAX_PIXELS {
        return Err(AppError::Execute("KTX2 pixel budget exceeded".into()));
    }
    let rgba = img.to_rgba8();
    let raw = rgba.as_raw();
    let mut out = Vec::with_capacity(PIXEL_OFFSET.saturating_add(raw.len()));
    out.extend_from_slice(MAGIC);
    for n in [37, 1, w, h, 0, 0, 1, 1, 0, 104, DFD_LEN as u32, 0, 0] {
        write_u32(&mut out, n);
    }
    write_u64(&mut out, 0); // supercompression global data offset
    write_u64(&mut out, 0); // supercompression global data length
                            // One mip level index.
    write_u64(&mut out, PIXEL_OFFSET as u64);
    write_u64(&mut out, raw.len() as u64);
    write_u64(&mut out, raw.len() as u64);

    // Khronos basic DFD: 4-byte total length plus 88-byte block, linear BT.709
    // RGBA8 4 samples, 8 bits each. No additional/unknown metadata.
    write_u32(&mut out, DFD_LEN as u32);
    write_u32(&mut out, 0); // vendor 0 / descriptor type 0
    write_u16(&mut out, 2); // KHR_DF_VERSIONNUMBER_LATEST
    write_u16(&mut out, (DFD_LEN - 4) as u16);
    out.extend_from_slice(&[1, 1, 1, 0]); // RGBSDA, BT709, linear, flags
    out.extend_from_slice(&[0, 0, 0, 0]); // 1x1x1x1 texel block
    out.extend_from_slice(&[4, 0, 0, 0, 0, 0, 0, 0]); // 4 bytes per texel
    for (bit, channel) in [(0u16, 0u8), (8, 1), (16, 2), (24, 15)] {
        write_u16(&mut out, bit);
        out.push(7); // 8 bits minus one
        out.push(channel);
        out.extend_from_slice(&[0, 0, 0, 0]); // sample positions
        write_u32(&mut out, 0); // lower
        write_u32(&mut out, 255); // upper
    }
    if out.len() != HEADER_LEN + INDEX_LEN + DFD_LEN {
        return Err(AppError::Execute("Unexpected KTX2 DFD size".into()));
    }
    out.resize(PIXEL_OFFSET, 0); // 8-byte alignment
    out.extend_from_slice(raw);
    // Verify that a different parser accepts our output container.
    ktx2::Reader::new(out.as_slice())
        .map_err(|e| AppError::Execute(format!("Generated KTX2 header invalid: {e:?}")))?;
    Ok(out)
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
    fn png_ktx2_png_roundtrip_exact() {
        let d = tempfile::tempdir().expect("directory");
        let input = d.path().join("in.png");
        let mid = d.path().join("mid.ktx2");
        let back = d.path().join("back.png");
        let source = image::RgbaImage::from_fn(7, 5, |x, y| {
            image::Rgba([(x * 30) as u8, (y * 40) as u8, 120, (x * y * 7) as u8])
        });
        source.save(&input).expect("save");
        let h = ImageConverterHandler;
        h.validate(&req(&input, &mid, "ktx2")).expect("validate");
        h.execute(&req(&input, &mid, "ktx2")).expect("encode");
        assert_eq!(h.probe(&mid).expect("probe").0, "ktx2");
        assert_eq!(dimensions(&mid).expect("dims"), (7, 5));
        h.validate(&req(&mid, &back, "png")).expect("validate");
        h.execute(&req(&mid, &back, "png")).expect("decode");
        assert_eq!(image::open(&back).expect("png").to_rgba8(), source);
    }

    #[test]
    fn reject_truncated_header_huge_dimensions_and_multilevel() {
        let d = tempfile::tempdir().expect("directory");
        let file = d.path().join("bad.ktx2");
        std::fs::write(&file, MAGIC).expect("write");
        assert!(dimensions(&file).is_err());
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([1, 2, 3, 4]),
        ));
        let mut encoded = encode(&img, &ImageOptions::default()).expect("encode");
        encoded[20..24].copy_from_slice(&60_000u32.to_le_bytes());
        encoded[24..28].copy_from_slice(&60_000u32.to_le_bytes());
        std::fs::write(&file, &encoded).expect("write");
        assert!(dimensions(&file).is_err());
        encoded[20..24].copy_from_slice(&1u32.to_le_bytes());
        encoded[24..28].copy_from_slice(&1u32.to_le_bytes());
        encoded[40..44].copy_from_slice(&2u32.to_le_bytes());
        std::fs::write(&file, &encoded).expect("write");
        assert!(dimensions(&file).is_err());
        encoded[40..44].copy_from_slice(&1u32.to_le_bytes());
        encoded[104 + 12] = 255; // Unknown color model must be rejected.
        std::fs::write(&file, &encoded).expect("write");
        assert!(decode(&file).is_err());
        encoded[104 + 12] = 1;
        encoded.pop(); // Valid header but truncated level data.
        std::fs::write(&file, encoded).expect("write");
        assert!(decode(&file).is_err());
    }
}
