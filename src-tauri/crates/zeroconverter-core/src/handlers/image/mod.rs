use crate::app_error::AppError;
use crate::conversion::options::OptionSpec;
use crate::conversion::traits::ConverterHandler;
use crate::conversion::types::{
    ConversionCapability, ConversionRequest, ConversionResult, FormatDescriptor, InputFormat,
    OutputFormat,
};
use std::path::Path;

pub mod execute;
pub mod formats;
pub mod options;
pub mod probe;
pub mod svg;
pub mod validate;

/// The two image formats that are not format modules, because they are
/// routes rather than codecs: SVG is rendered at a target size, and PDF is
/// written straight from the input file. Every other format is one file in
/// `formats/` (see `formats/mod.rs`).
pub static SPECIAL_FORMATS: &[FormatDescriptor] = &[
    FormatDescriptor {
        id: "svg",
        canonical_extension: "svg",
        aliases: &[],
        can_read: true,
        can_write: false,
    },
    FormatDescriptor {
        id: "pdf",
        canonical_extension: "pdf",
        aliases: &[],
        can_read: false,
        can_write: true,
    },
];

pub struct ImageConverterHandler;

impl ConverterHandler for ImageConverterHandler {
    fn format_descriptors(&self) -> &'static [FormatDescriptor] {
        formats::all_descriptors()
    }

    // supported_extensions() uses the default trait implementation derived from format_descriptors().

    fn probe(&self, path: &Path) -> Result<InputFormat, AppError> {
        probe::probe_image(path)
    }

    fn capabilities(&self) -> Vec<ConversionCapability> {
        let readable: Vec<_> = self
            .format_descriptors()
            .iter()
            .filter(|d| d.can_read)
            .collect();
        let writable: Vec<_> = self
            .format_descriptors()
            .iter()
            .filter(|d| d.can_write)
            .collect();

        // All readable raster formats (built-ins and registered modules) can
        // produce PDF pages; SVG remains deliberately out of scope.
        readable
            .iter()
            .map(|input| {
                let supports_pdf = input.id != "svg";
                ConversionCapability {
                    input: InputFormat(input.id.to_string()),
                    outputs: writable
                        .iter()
                        .filter(|o| o.id != "pdf" || supports_pdf)
                        .map(|o| OutputFormat(o.id.to_string()))
                        .collect(),
                }
            })
            .collect()
    }

    /// The format's own options (quality, DDS) first, then resize, which
    /// every writable image takes, PDF included.
    fn output_options(&self, target: &str) -> Vec<&'static OptionSpec> {
        let own: &[&OptionSpec] = match formats::find(target) {
            Some(module) if module.descriptor.can_write => module.options,
            Some(_) => return Vec::new(),
            None if target.eq_ignore_ascii_case("pdf") => &[],
            None => return Vec::new(),
        };
        own.iter().chain(options::RESIZE.iter()).copied().collect()
    }

    fn validate(&self, request: &ConversionRequest) -> Result<(), AppError> {
        let path = std::path::Path::new(&request.input_path);
        let input_format = probe::probe_image(path)?;
        validate::validate_image_conversion(request, &input_format)
    }

    fn execute(&self, request: &ConversionRequest) -> Result<ConversionResult, AppError> {
        execute::execute_image_conversion(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion::types::find_descriptor;

    #[test]
    fn every_writable_image_declares_options_that_validation_agrees_with() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
            .save(&input)
            .unwrap();
        let handler = ImageConverterHandler;
        for target in handler.format_descriptors().iter().filter(|d| d.can_write) {
            let specs = handler.output_options(target.id);
            assert!(
                specs.iter().any(|s| s.key == "resize_mode"),
                "{} takes resize",
                target.id
            );
            crate::conversion::options::spec_tests::specs_agree_with_validation(
                target.id,
                &specs,
                &|options| {
                    handler.validate(&crate::test_utils::make_request(
                        crate::conversion::types::AssetFamily::Image,
                        &input,
                        &dir.path().join("out.bin"),
                        target.id,
                        Some(options),
                    ))
                },
            );
        }
        let keys =
            |t: &str| -> Vec<&str> { handler.output_options(t).iter().map(|s| s.key).collect() };
        assert_eq!(keys("jpg")[0], "quality");
        // WebP: lossless unless asked, quality only when lossy.
        assert_eq!(keys("webp")[..2], ["webp_mode", "quality"]);
        assert_eq!(keys("dds")[..2], ["dds_format", "dds_mipmaps"]);
        assert!(
            !keys("png").contains(&"quality"),
            "PNG has no quality to set"
        );
        assert!(keys("svg").is_empty() && keys("nope").is_empty());
    }

    #[test]
    fn dds_choices_are_the_ones_validation_accepts() {
        use crate::conversion::options::{Control, OptionValue};
        let Control::Choice { values } = options::DDS_FORMAT.control else {
            panic!("DDS compression is a choice")
        };
        let listed: Vec<_> = values
            .iter()
            .map(|v| match v {
                OptionValue::Text(t) => *t,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(listed, options::DDS_FORMATS);
    }

    #[test]
    fn ico_descriptor_supports_input_and_output() {
        let ico_desc =
            find_descriptor(formats::all_descriptors(), "ico").expect("Missing ICO descriptor");
        assert!(ico_desc.can_read, "ICO must be readable (input)");
        assert!(
            ico_desc.can_write,
            "ICO must be writable (basic single-frame output)"
        );
        assert_eq!(ico_desc.canonical_extension, "ico");
    }

    #[test]
    fn pdf_descriptor_is_write_only() {
        let pdf_desc = find_descriptor(SPECIAL_FORMATS, "pdf").expect("Missing PDF descriptor");
        assert!(
            !pdf_desc.can_read,
            "PDF must NOT be readable (no PDF input)"
        );
        assert!(pdf_desc.can_write, "PDF must be writable (output only)");
        assert_eq!(pdf_desc.canonical_extension, "pdf");
        assert!(pdf_desc.aliases.is_empty(), "PDF should have no aliases");
    }

    #[test]
    fn capabilities_expose_pdf_for_all_raster_inputs() {
        use crate::conversion::traits::ConverterHandler;
        let handler = ImageConverterHandler;
        let caps = handler.capabilities();

        // PDF output is available for all readable raster formats, including PSD and QOI.
        for input_id in &[
            "png", "jpg", "webp", "bmp", "tiff", "ico", "gif", "psd", "qoi",
        ] {
            let cap = caps
                .iter()
                .find(|c| c.input.0 == *input_id)
                .unwrap_or_else(|| panic!("Missing capability for {}", input_id));
            assert!(
                cap.outputs.iter().any(|o| o.0 == "pdf"),
                "{} route must include pdf output",
                input_id
            );
        }

        // PDF remains disabled for vector input until its rendering contract is specified.
        {
            let input_id = &"svg";
            let cap = caps
                .iter()
                .find(|c| c.input.0 == *input_id)
                .unwrap_or_else(|| panic!("Missing capability for {}", input_id));
            assert!(
                !cap.outputs.iter().any(|o| o.0 == "pdf"),
                "{} route must NOT include pdf output",
                input_id
            );
        }
    }

    #[test]
    fn pdf_does_not_appear_as_input_route() {
        use crate::conversion::traits::ConverterHandler;
        let handler = ImageConverterHandler;
        let caps = handler.capabilities();
        assert!(
            !caps.iter().any(|c| c.input.0 == "pdf"),
            "PDF must never appear as an input route (can_read: false)"
        );
    }
}
