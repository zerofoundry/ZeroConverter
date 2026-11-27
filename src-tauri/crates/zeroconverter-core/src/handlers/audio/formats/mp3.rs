//! MP3 output through LAME (LGPL, built in; see THIRD_PARTY_NOTICES.md).
//! Built without the `mp3` feature, MP3 stays readable but not writable.
#![cfg_attr(not(feature = "mp3"), allow(unused_imports))]
use super::AudioFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::ConversionResult;
use crate::conversion::types::FormatDescriptor;
use crate::handlers::audio::execute::{success_result, write_atomically};
use crate::handlers::audio::shape::{self, AudioOptions};
#[cfg(feature = "mp3")]
use mp3lame_encoder::{Builder, FlushNoGap, InterleavedPcm, MonoPcm};
use std::io::{BufWriter, Write};
use std::path::Path;

pub const MODULE: AudioFormatModule = AudioFormatModule {
    descriptor: FormatDescriptor {
        id: "mp3",
        canonical_extension: "mp3",
        aliases: &[],
        can_read: true,
        can_write: cfg!(feature = "mp3"),
    },
    encoder: ENCODER,
    options: &[&shape::MP3_BITRATE, &shape::SAMPLE_RATE, &shape::CHANNELS],
};

#[cfg(feature = "mp3")]
const ENCODER: Option<super::AudioEncoder> = Some(encode);
#[cfg(not(feature = "mp3"))]
const ENCODER: Option<super::AudioEncoder> = None;

#[cfg(feature = "mp3")]
pub fn encode(
    input: &Path,
    output: &Path,
    options: &AudioOptions,
) -> Result<ConversionResult, AppError> {
    let spec = shape::inspect_with(input, options)?;
    let (bitrate, kbps) = mp3_bitrate(options.bitrate_kbps.unwrap_or(320));
    if spec.channels > 2 {
        return Err(AppError::Execute(
            "MP3 encoder accepts at most two channels".into(),
        ));
    }
    write_atomically(output, options, |temporary| {
        let mut builder =
            Builder::new().ok_or_else(|| AppError::Execute("LAME Builder init failed".into()))?;
        builder
            .set_num_channels(spec.channels as u8)
            .map_err(|e| AppError::Execute(format!("LAME channels: {e:?}")))?;
        builder
            .set_sample_rate(spec.sample_rate)
            .map_err(|e| AppError::Execute(format!("LAME sample rate: {e:?}")))?;
        builder
            .set_brate(bitrate)
            .map_err(|e| AppError::Execute(format!("LAME bitrate: {e:?}")))?;
        builder
            .set_quality(mp3lame_encoder::Quality::Best)
            .map_err(|e| AppError::Execute(format!("LAME quality: {e:?}")))?;
        let mut encoder = builder
            .build()
            .map_err(|e| AppError::Execute(format!("LAME build: {e:?}")))?;
        let file = std::fs::File::create(temporary).map_err(|e| AppError::Io(e.to_string()))?;
        let mut output_file = BufWriter::new(file);
        shape::stream_with(input, options, |samples, found| {
            if found != spec {
                return Err(AppError::Execute(
                    "Source audio specification changed".into(),
                ));
            }
            let pcm_i16: Vec<i16> = samples.iter().map(|&s| (s * 32767.0) as i16).collect();
            // LAME's documented bound: 1.25 * input frames + 7200 bytes.
            // Input uses interleaved samples, making this conservative.
            let capacity = pcm_i16
                .len()
                .saturating_mul(5)
                .saturating_div(4)
                .saturating_add(7200);
            let mut encoded = Vec::<u8>::with_capacity(capacity);
            // Mono must go in as mono: read as interleaved stereo it plays at
            // double speed and trips an assertion on odd sample counts.
            let encoded_len = if spec.channels == 1 {
                encoder.encode(MonoPcm(&pcm_i16), encoded.spare_capacity_mut())
            } else {
                encoder.encode(InterleavedPcm(&pcm_i16), encoded.spare_capacity_mut())
            };
            let len = encoded_len.map_err(|e| AppError::Execute(format!("MP3 encoding: {e:?}")))?;
            // The encoder writes exactly len initialized bytes into spare capacity.
            unsafe { encoded.set_len(len) };
            output_file
                .write_all(&encoded)
                .map_err(|e| AppError::Io(e.to_string()))
        })?;
        let mut final_buffer = Vec::<u8>::with_capacity(7200);
        let len = encoder
            .flush::<FlushNoGap>(final_buffer.spare_capacity_mut())
            .map_err(|e| AppError::Execute(format!("MP3 flush: {e:?}")))?;
        // Safe per mp3lame_encoder's result contract, as in the previous path.
        unsafe { final_buffer.set_len(len) };
        output_file
            .write_all(&final_buffer)
            .map_err(|e| AppError::Io(e.to_string()))?;
        output_file.flush().map_err(|e| AppError::Io(e.to_string()))
    })?;
    Ok(success_result(
        output,
        &format!("Successfully encoded to MP3 ({kbps}kbps)"),
    ))
}

/// Nearest LAME constant bitrate at or below the request (96 at least),
/// and that rate in kbps for the message.
#[cfg(feature = "mp3")]
fn mp3_bitrate(kbps: u32) -> (mp3lame_encoder::Bitrate, u32) {
    use mp3lame_encoder::Bitrate::*;
    match kbps {
        0..=111 => (Kbps96, 96),
        112..=127 => (Kbps112, 112),
        128..=159 => (Kbps128, 128),
        160..=191 => (Kbps160, 160),
        192..=223 => (Kbps192, 192),
        224..=255 => (Kbps224, 224),
        256..=319 => (Kbps256, 256),
        _ => (Kbps320, 320),
    }
}
