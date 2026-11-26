pub mod execute;
pub mod formats;
pub mod pcm;
pub mod probe;
pub mod shape;
pub mod validate;

use crate::app_error::AppError;
use crate::conversion::traits::{ConverterHandler, ExecContext};
use crate::conversion::types::{
    ConversionCapability, ConversionRequest, ConversionResult, FormatDescriptor, InputFormat,
    OutputFormat,
};
use std::path::Path;
use std::sync::OnceLock;

/// All supported audio descriptors, built once from the authoritative registry.
pub fn all_descriptors() -> &'static [FormatDescriptor] {
    static ALL: OnceLock<Vec<FormatDescriptor>> = OnceLock::new();
    ALL.get_or_init(|| formats::MODULES.iter().map(|m| m.descriptor).collect())
}

pub struct AudioHandler;

impl ConverterHandler for AudioHandler {
    fn format_descriptors(&self) -> &'static [FormatDescriptor] {
        all_descriptors()
    }

    fn probe(&self, path: &Path) -> Result<InputFormat, AppError> {
        probe::probe_audio(path)?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let format = formats::find(&ext)
            .ok_or_else(|| AppError::Probe(format!("Unsupported audio extension: {ext}")))?;
        if !format.descriptor.can_read {
            return Err(AppError::Probe(format!(
                "Audio format is not readable: {ext}"
            )));
        }
        Ok(InputFormat(format.descriptor.id.to_owned()))
    }

    fn capabilities(&self) -> Vec<ConversionCapability> {
        formats::MODULES
            .iter()
            .filter(|module| module.descriptor.can_read)
            .map(|source| ConversionCapability {
                input: InputFormat(source.descriptor.id.to_owned()),
                outputs: formats::MODULES
                    .iter()
                    .filter(|dest| {
                        dest.descriptor.can_write && source.descriptor.id != dest.descriptor.id
                    })
                    .map(|dest| OutputFormat(dest.descriptor.id.to_owned()))
                    .collect(),
            })
            .collect()
    }

    fn validate(&self, request: &ConversionRequest) -> Result<(), AppError> {
        validate::validate_audio(request)
    }

    fn execute(&self, request: &ConversionRequest) -> Result<ConversionResult, AppError> {
        execute::execute_audio_with(request, &ExecContext::none())
    }

    fn execute_with(
        &self,
        request: &ConversionRequest,
        ctx: &ExecContext,
    ) -> Result<ConversionResult, AppError> {
        execute::execute_audio_with(request, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_capabilities_match_encoder_registry() {
        let handler = AudioHandler;
        let caps = handler.capabilities();
        assert_eq!(handler.format_descriptors().len(), formats::MODULES.len());
        for module in formats::MODULES {
            let cap = caps
                .iter()
                .find(|cap| cap.input.0 == module.descriptor.id)
                .expect("every readable audio module is a capability input");
            for output in &cap.outputs {
                let dest = formats::find(&output.0).expect("output module exists");
                assert!(dest.descriptor.can_write);
                assert_ne!(cap.input.0, output.0);
            }
        }
    }

    #[test]
    fn every_other_audio_format_can_become_m4a_and_aac() {
        for cap in AudioHandler.capabilities() {
            let outs: Vec<&str> = cap.outputs.iter().map(|o| o.0.as_str()).collect();
            for target in ["m4a", "aac"] {
                assert_eq!(
                    outs.contains(&target),
                    cap.input.0 != target,
                    "{} -> {target}",
                    cap.input.0
                );
            }
        }
    }
}
