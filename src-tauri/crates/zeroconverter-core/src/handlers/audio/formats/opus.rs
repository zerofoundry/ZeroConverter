//! Streaming RFC 7845 Ogg/Opus encode + decode, using pure-Rust opus-pure.
//! At most one PCM packet plus a 20 ms pending frame is retained.
use super::AudioFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::{ConversionResult, FormatDescriptor};
use crate::handlers::audio::shape::{self, AudioOptions};
use crate::handlers::audio::{execute, pcm};
use opus_pure::{
    Application, OggOpusReader, OggOpusWriter, OpusEncoder, OpusHead, Trim, MAX_PACKET_BYTES,
    MAX_PACKET_SAMPLES,
};
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

pub const MODULE: AudioFormatModule = AudioFormatModule {
    descriptor: FormatDescriptor {
        id: "opus",
        canonical_extension: "opus",
        aliases: &[],
        can_read: true,
        can_write: true,
    },
    encoder: Some(encode),
    options: &[&shape::OPUS_BITRATE, &shape::CHANNELS],
};

fn codec_error(error: impl std::fmt::Display) -> AppError {
    AppError::Execute(format!("Ogg/Opus error: {error}"))
}

/// Opus decoder output is always 48 kHz, regardless of the original header.
pub fn inspect(input: &Path) -> Result<pcm::AudioSpec, AppError> {
    let file = std::fs::File::open(input).map_err(|e| AppError::Io(e.to_string()))?;
    let reader = OggOpusReader::new(BufReader::new(file)).map_err(codec_error)?;
    let channels = reader.head().channel_count as usize;
    if !(1..=2).contains(&channels) {
        return Err(codec_error("Only mono and stereo Opus are supported"));
    }
    Ok(pcm::AudioSpec {
        sample_rate: 48_000,
        channels,
    })
}

pub fn stream_decode<F>(input: &Path, mut consume: F) -> Result<(), AppError>
where
    F: FnMut(&[f32], pcm::AudioSpec) -> Result<(), AppError>,
{
    let file = std::fs::File::open(input).map_err(|e| AppError::Io(e.to_string()))?;
    let mut reader = OggOpusReader::new(BufReader::new(file)).map_err(codec_error)?;
    let head = reader.head().clone();
    let channels = head.channel_count as usize;
    if !(1..=2).contains(&channels) {
        return Err(codec_error("Only mono and stereo Opus are supported"));
    }
    let spec = pcm::AudioSpec {
        sample_rate: 48_000,
        channels,
    };
    let mut decoder = head.decoder(48_000).map_err(codec_error)?;
    let mut trim = Trim::new(&head, 48_000, channels).map_err(codec_error)?;
    let mut block = vec![0.0f32; MAX_PACKET_SAMPLES * channels];
    let mut saw_audio = false;
    // Container CRC and end-trim are checked by the pure-Rust decoder.
    for packet in reader.packets() {
        let packet = packet.map_err(codec_error)?;
        let decoded = decoder
            .decode(&packet.data, MAX_PACKET_SAMPLES, &mut block)
            .map_err(codec_error)?;
        let kept = trim.keep(&packet, &block[..decoded * channels]);
        if !kept.is_empty() {
            saw_audio = true;
            consume(kept, spec)?;
        }
    }
    if !saw_audio {
        return Err(codec_error("Opus stream contains no decoded audio"));
    }
    Ok(())
}

fn queue_encoded_packet<W: std::io::Write>(
    encoder: &mut OpusEncoder,
    writer: &mut OggOpusWriter<W>,
    block: &[f32],
    frames_per_channel: usize,
    bytes: &mut [u8],
    previous: &mut Vec<u8>,
    has_previous: &mut bool,
) -> Result<(), AppError> {
    let size = encoder
        .encode(block, frames_per_channel, bytes)
        .map_err(codec_error)?;
    // Hold exactly one packet so the last packet can carry the RFC 7845
    // end-trim granule once the total number of source frames is known.
    if *has_previous {
        writer.write_packet(previous).map_err(codec_error)?;
    }
    previous.clear();
    previous.extend_from_slice(&bytes[..size]);
    *has_previous = true;
    Ok(())
}

pub fn encode(
    input: &Path,
    output: &Path,
    options: &AudioOptions,
) -> Result<ConversionResult, AppError> {
    let mut options = *options;
    // Opus only runs at these rates; anything else (44.1 kHz music) goes to 48 kHz.
    let source = pcm::inspect(input)?;
    let rate = options.sample_rate.unwrap_or(source.sample_rate);
    if !matches!(rate, 8_000 | 12_000 | 16_000 | 24_000 | 48_000) {
        options.sample_rate = Some(48_000);
    }
    let spec = shape::output_spec(source, &options);
    if !matches!(spec.sample_rate, 8_000 | 12_000 | 16_000 | 24_000 | 48_000)
        || !(1..=2).contains(&spec.channels)
    {
        return Err(codec_error(
            "Opus holds mono or stereo only; choose Mono for this file",
        ));
    }
    let rate = spec.sample_rate as usize;
    let channels = spec.channels;
    let frame = rate / 50;
    let per_frame = frame * channels;

    execute::write_atomically(output, &options, |temporary| {
        let mut encoder =
            OpusEncoder::new(rate as i32, channels, Application::Audio).map_err(codec_error)?;
        encoder.bitrate_bps = match options.bitrate_kbps {
            Some(kbps) => (kbps.clamp(16, 256) * 1000) as i32,
            None if channels == 1 => 64_000,
            None => 128_000,
        };
        let head = OpusHead::for_encoder(&encoder, spec.sample_rate);
        let ticks = 48_000 / rate;
        let pre_skip = u64::from(head.pre_skip);
        let file = std::fs::File::create(temporary).map_err(|e| AppError::Io(e.to_string()))?;
        let mut writer = OggOpusWriter::new(BufWriter::new(file), head).map_err(codec_error)?;
        let mut pending = Vec::<f32>::with_capacity(per_frame);
        let mut packet_buffer = vec![0u8; MAX_PACKET_BYTES];
        let mut previous = Vec::with_capacity(MAX_PACKET_BYTES);
        let mut has_previous = false;
        let mut input_frames = 0u64;
        let mut written_packets = 0u64;

        shape::stream_with(input, &options, |samples, found| {
            if found != spec || samples.len() % channels != 0 {
                return Err(codec_error("Source audio specification changed"));
            }
            input_frames = input_frames
                .checked_add((samples.len() / channels) as u64)
                .ok_or_else(|| codec_error("Opus source frame count overflow"))?;
            let mut remaining = samples;
            if !pending.is_empty() {
                let take = (per_frame - pending.len()).min(remaining.len());
                pending.extend_from_slice(&remaining[..take]);
                remaining = &remaining[take..];
                if pending.len() == per_frame {
                    queue_encoded_packet(
                        &mut encoder,
                        &mut writer,
                        &pending,
                        frame,
                        &mut packet_buffer,
                        &mut previous,
                        &mut has_previous,
                    )?;
                    written_packets += 1;
                    pending.clear();
                }
            }
            while remaining.len() >= per_frame {
                queue_encoded_packet(
                    &mut encoder,
                    &mut writer,
                    &remaining[..per_frame],
                    frame,
                    &mut packet_buffer,
                    &mut previous,
                    &mut has_previous,
                )?;
                written_packets += 1;
                remaining = &remaining[per_frame..];
            }
            pending.extend_from_slice(remaining);
            Ok(())
        })?;

        let lookahead = pre_skip.div_ceil(ticks as u64);
        let required_packets = input_frames
            .checked_add(lookahead)
            .ok_or_else(|| codec_error("Opus duration overflow"))?
            .div_ceil(frame as u64);
        while written_packets < required_packets {
            let mut block = vec![0.0f32; per_frame];
            if !pending.is_empty() {
                block[..pending.len()].copy_from_slice(&pending);
                pending.clear();
            }
            queue_encoded_packet(
                &mut encoder,
                &mut writer,
                &block,
                frame,
                &mut packet_buffer,
                &mut previous,
                &mut has_previous,
            )?;
            written_packets += 1;
        }

        let target_granule = pre_skip
            .checked_add(
                input_frames
                    .checked_mul(ticks as u64)
                    .ok_or_else(|| codec_error("Opus granule overflow"))?,
            )
            .ok_or_else(|| codec_error("Opus granule overflow"))?;
        if !has_previous {
            return Err(codec_error("No final Opus packet"));
        }
        let final_duration = target_granule
            .checked_sub(writer.granule() as u64)
            .ok_or_else(|| codec_error("Invalid Opus final granule"))?;
        if final_duration == 0 || final_duration > 960 {
            return Err(codec_error("Invalid Opus end-trim duration"));
        }
        writer
            .write_packet_with_duration(&previous, final_duration as u32)
            .map_err(codec_error)?;
        let mut sink = writer.finish().map_err(codec_error)?;
        sink.flush()
            .map_err(|e| AppError::Io(format!("Opus file flush failed: {e}")))?;
        Ok(())
    })?;
    Ok(execute::success_result(
        output,
        "Successfully converted to Ogg/Opus",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::create_test_wav;

    #[test]
    fn opus_roundtrip_preserves_rate_channels_and_sample_count() {
        let wav = create_test_wav(2, 48_000, 100);
        let directory = tempfile::tempdir().expect("tempdir");
        let opus_file = directory.path().join("output.opus");
        encode(wav.path(), &opus_file, &AudioOptions::default()).expect("encode to Opus");
        assert_eq!(&std::fs::read(&opus_file).expect("bytes")[..4], b"OggS");
        let spec = inspect(&opus_file).expect("inspect Opus");
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.channels, 2);
        let mut source_frames = 0usize;
        pcm::stream_pcm(wav.path(), |samples, info| {
            source_frames += samples.len() / info.channels;
            Ok(())
        })
        .expect("read original");
        let mut decoded_frames = 0usize;
        stream_decode(&opus_file, |samples, info| {
            decoded_frames += samples.len() / info.channels;
            Ok(())
        })
        .expect("decode");
        assert_eq!(source_frames, decoded_frames);
        assert!(!std::fs::read_dir(directory.path())
            .expect("dir")
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with("zero_tmp_")));
    }

    #[test]
    fn truncated_opus_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad.opus");
        std::fs::write(&path, b"OggS\0\x02").expect("create malformed Opus");
        assert!(inspect(&path).is_err());
    }

    #[test]
    fn cd_rate_source_is_resampled_to_48k() {
        let wav = create_test_wav(2, 44_100, 100);
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("music.opus");
        encode(wav.path(), &output, &AudioOptions::default()).expect("44.1 kHz to Opus");
        let spec = pcm::inspect(&output).expect("decode Opus");
        assert_eq!(spec.sample_rate, 48_000);
        let mut frames = 0usize;
        pcm::stream_pcm(&output, |s, info| {
            frames += s.len() / info.channels;
            Ok(())
        })
        .expect("stream Opus");
        // 0.1 s of audio, within one Opus frame of padding.
        assert!((4_800..=4_800 + 960).contains(&frames), "{frames}");
    }

    #[test]
    fn bitrate_option_changes_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wav = dir.path().join("noise.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&wav, spec).expect("wav");
        let mut x = 1u32;
        for _ in 0..96_000 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            writer.write_sample((x >> 16) as i16).expect("sample");
        }
        writer.finalize().expect("finalize");
        let size = |kbps| {
            let out = dir.path().join(format!("b{kbps}.opus"));
            let options = AudioOptions {
                bitrate_kbps: Some(kbps),
                ..Default::default()
            };
            encode(&wav, &out, &options).expect("encode");
            std::fs::metadata(&out).unwrap().len()
        };
        assert!(size(32) < size(192));
    }
}
