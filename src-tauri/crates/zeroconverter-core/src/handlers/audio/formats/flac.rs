//! FLAC output: lossless, 16 or 24 bits like the source.
use super::AudioFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::ConversionResult;
use crate::conversion::types::FormatDescriptor;
use crate::handlers::audio::execute::{success_result, write_atomically};
use crate::handlers::audio::pcm;
use crate::handlers::audio::shape::{self, AudioOptions};
use std::io::BufWriter;
use std::path::Path;

pub const MODULE: AudioFormatModule = AudioFormatModule {
    descriptor: FormatDescriptor {
        id: "flac",
        canonical_extension: "flac",
        aliases: &[],
        can_read: true,
        can_write: true,
    },
    encoder: Some(encode),
    options: &[&shape::SAMPLE_RATE, &shape::CHANNELS],
};

pub fn encode(
    input: &Path,
    output: &Path,
    options: &AudioOptions,
) -> Result<ConversionResult, AppError> {
    let spec = shape::inspect_with(input, options)?;
    let bits = pcm::lossless_bits(input);
    write_atomically(output, options, |temporary| {
        let file = std::fs::File::create(temporary).map_err(|e| AppError::Io(e.to_string()))?;
        let mut writer = flac_codec::encode::FlacSampleWriter::new(
            BufWriter::new(file),
            flac_codec::encode::Options::default(),
            spec.sample_rate,
            bits,
            spec.channels as u8,
            None,
        )
        .map_err(|e| AppError::Execute(format!("FLAC encoder init: {e}")))?;
        shape::stream_with(input, options, |samples, found| {
            if found != spec {
                return Err(AppError::Execute(
                    "Source audio specification changed".into(),
                ));
            }
            let pcm_i32: Vec<i32> = samples.iter().map(|&s| pcm::to_int(s, bits)).collect();
            writer
                .write(&pcm_i32)
                .map_err(|e| AppError::Execute(format!("FLAC encoding: {e}")))
        })?;
        writer
            .finalize()
            .map_err(|e| AppError::Execute(format!("FLAC finalize: {e}")))
    })?;
    Ok(success_result(
        output,
        &format!("Successfully encoded to FLAC ({bits}-bit lossless)"),
    ))
}
