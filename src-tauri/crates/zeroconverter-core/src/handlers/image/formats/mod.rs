// ==============================================================================
// OWNERSHIP:
//   The extension seam for image formats. One format = one file in this folder
//   plus ONE line in `MODULES` below. Nothing else in the codebase needs to
//   change for a new format to be probed, validated, converted and advertised
//   to the frontend.
//
// DO NOT ADD:
//   - Format-specific logic in this file (it belongs in the format's own file)
//   - Output path, naming or job concerns
//
// NOTES:
//   SVG and PDF are the only image formats outside this table
//   (`super::SPECIAL_FORMATS`): they are routes, not codecs.
//   Functions are plain `fn` pointers: no allocation, and one indirect call per
//   file. Every one of them runs on many worker threads at once, so a module
//   must not keep shared mutable state.
// ==============================================================================
use super::options::ImageOptions;
use crate::app_error::AppError;
use crate::conversion::options::OptionSpec;
use crate::conversion::types::FormatDescriptor;
use image::DynamicImage;
use std::path::Path;
use std::sync::OnceLock;

pub mod avif;
pub mod dds;
pub mod exr;
pub mod hdr;
pub mod jxl;
pub mod ktx2;
pub mod psd;
pub mod qoi;
pub mod raster;
pub mod tga;

/// Number of leading bytes handed to `sniff`.
pub const SNIFF_LEN: usize = 64;

pub type DecodeFn = fn(path: &Path) -> Result<DynamicImage, AppError>;
pub type EncodeFn = fn(image: &DynamicImage, options: &ImageOptions) -> Result<Vec<u8>, AppError>;

pub struct ImageFormatModule {
    pub descriptor: FormatDescriptor,
    /// Signature check on the first `SNIFF_LEN` bytes (fewer for tiny files).
    /// Must be strict: a false positive steals files from other formats.
    pub sniff: fn(header: &[u8]) -> bool,
    /// Width/height from the header only. Called before decode to enforce the
    /// pixel budget, so it must not decode pixel data.
    pub dimensions: fn(path: &Path) -> Result<(u32, u32), AppError>,
    /// `Some` iff `descriptor.can_read`.
    pub decode: Option<DecodeFn>,
    /// `Some` iff `descriptor.can_write`. Returns the complete output file bytes;
    /// the caller writes them atomically.
    pub encode: Option<EncodeFn>,
    /// Options this format's encoder honours beyond resize, which every
    /// writable image takes (`super::options`). Empty when there are none.
    pub options: &'static [&'static OptionSpec],
}

/// Registration table, one module per line. Signatures are tried in this
/// order: the everyday formats first (strong magic numbers), then the rest
/// alphabetically, and ICO last because its four-byte header is the weakest.
#[rustfmt::skip]
pub static MODULES: &[&ImageFormatModule] = &[
    &raster::PNG,
    &raster::JPG,
    &raster::WEBP,
    &raster::BMP,
    &raster::TIFF,
    &raster::GIF,
    &avif::MODULE,
    &dds::MODULE,
    &exr::MODULE,
    &hdr::MODULE,
    &jxl::MODULE,
    &ktx2::MODULE,
    &psd::MODULE,
    &qoi::MODULE,
    &tga::MODULE,
    &raster::ICO,
];

/// Module descriptors followed by the two special routes. Built once.
pub fn all_descriptors() -> &'static [FormatDescriptor] {
    static ALL: OnceLock<Vec<FormatDescriptor>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut all: Vec<_> = MODULES.iter().map(|m| m.descriptor).collect();
        all.extend_from_slice(super::SPECIAL_FORMATS);
        all
    })
}

/// Module lookup by format id, canonical extension or alias.
pub fn find(format: &str) -> Option<&'static ImageFormatModule> {
    MODULES
        .iter()
        .copied()
        .find(|m| m.descriptor.matches_format(format))
}

/// Module lookup by file signature.
pub fn sniff(header: &[u8]) -> Option<&'static ImageFormatModule> {
    MODULES.iter().copied().find(|m| (m.sniff)(header))
}

/// Reads the file header and returns the module that claims it, if any.
pub fn sniff_file(path: &Path) -> Option<&'static ImageFormatModule> {
    use std::io::Read;
    let mut header = [0u8; SNIFF_LEN];
    let mut file = std::fs::File::open(path).ok()?;
    let n = file.read(&mut header).ok()?;
    sniff(&header[..n])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_are_consistent_and_do_not_shadow_special_routes() {
        for m in MODULES {
            let d = &m.descriptor;
            assert_eq!(d.can_read, m.decode.is_some(), "{}: can_read", d.id);
            assert_eq!(d.can_write, m.encode.is_some(), "{}: can_write", d.id);
            let names = std::iter::once(d.id)
                .chain(std::iter::once(d.canonical_extension))
                .chain(d.aliases.iter().copied());
            for name in names {
                assert!(
                    !super::super::SPECIAL_FORMATS
                        .iter()
                        .any(|b| b.matches_format(name)),
                    "module format {} collides with SVG or PDF",
                    name
                );
                let claims = MODULES
                    .iter()
                    .filter(|o| o.descriptor.matches_format(name))
                    .count();
                assert_eq!(claims, 1, "{} is claimed by more than one module", name);
            }
        }
    }
}
