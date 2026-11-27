//! WAV output: 16-bit PCM, or 24-bit when the source has more than 16 bits.
use super::AudioFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::ConversionResult;
use crate::conversion::types::FormatDescriptor;
use crate::handlers::audio::execute::{success_result, write_atomically};
use crate::handlers::audio::pcm;
use crate::handlers::audio::shape::{self, AudioOptions};
use hound::{WavSpec, WavWriter};
use std::path::Path;

pub const MODULE: AudioFormatModule = AudioFormatModule {
    descriptor: FormatDescriptor {
        id: "wav",
        canonical_extension: "wav",
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
    let wav_spec = WavSpec {
        channels: spec.channels as u16,
        sample_rate: spec.sample_rate,
        bits_per_sample: bits as u16,
        sample_format: hound::SampleFormat::Int,
    };
    write_atomically(output, options, |temporary| {
        let mut writer =
            WavWriter::create(temporary, wav_spec).map_err(|e| AppError::Io(e.to_string()))?;
        shape::stream_with(input, options, |samples, found| {
            if found != spec {
                return Err(AppError::Execute(
                    "Source audio specification changed".into(),
                ));
            }
            for &sample in samples {
                writer
                    .write_sample(pcm::to_int(sample, bits))
                    .map_err(|e| AppError::Io(e.to_string()))?;
            }
            Ok(())
        })?;
        writer.finalize().map_err(|e| AppError::Io(e.to_string()))
    })?;
    Ok(success_result(
        output,
        &format!("Successfully converted to WAV ({bits}-bit)"),
    ))
}
