//! Uncompressed AIFF (big-endian signed PCM, 16 or 24 bit) streaming output.
//! Header sizes are patched only after all samples have been written.
use super::AudioFormatModule;
use crate::app_error::AppError;
use crate::conversion::types::{ConversionResult, FormatDescriptor};
use crate::handlers::audio::shape::{self, AudioOptions};
use crate::handlers::audio::{execute, pcm};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

pub const MODULE: AudioFormatModule = AudioFormatModule {
    descriptor: FormatDescriptor {
        id: "aiff",
        canonical_extension: "aiff",
        aliases: &["aif"],
        can_read: true,
        can_write: true,
    },
    encoder: Some(encode),
    options: &[&shape::SAMPLE_RATE, &shape::CHANNELS],
};

struct AiffWriter<W: Write + Seek> {
    sink: W,
    channels: usize,
    bits: u32,
    sample_bytes: u64,
    frames: u64,
}

fn extended_sample_rate(rate: u32) -> [u8; 10] {
    // IEEE 754 extended (80-bit): sign/exponent followed by explicit-integer
    // 64-bit significand. Integer Hz values are exactly representable.
    let top_bit = 31 - rate.leading_zeros();
    let exponent = (16_383 + top_bit) as u16;
    let mantissa = (rate as u64) << (63 - top_bit);
    let mut out = [0u8; 10];
    out[..2].copy_from_slice(&exponent.to_be_bytes());
    out[2..].copy_from_slice(&mantissa.to_be_bytes());
    out
}

impl<W: Write + Seek> AiffWriter<W> {
    #[cfg(test)]
    fn new(sink: W, spec: pcm::AudioSpec) -> Result<Self, AppError> {
        Self::with_bits(sink, spec, 16)
    }

    fn with_bits(mut sink: W, spec: pcm::AudioSpec, bits: u32) -> Result<Self, AppError> {
        if spec.sample_rate == 0 || spec.channels == 0 || spec.channels > 8 {
            return Err(AppError::Execute("Unsupported AIFF audio header".into()));
        }
        sink.write_all(b"FORM").map_err(io_error)?;
        sink.write_all(&0u32.to_be_bytes()).map_err(io_error)?;
        sink.write_all(b"AIFFCOMM").map_err(io_error)?;
        sink.write_all(&18u32.to_be_bytes()).map_err(io_error)?;
        sink.write_all(&(spec.channels as u16).to_be_bytes())
            .map_err(io_error)?;
        sink.write_all(&0u32.to_be_bytes()).map_err(io_error)?;
        sink.write_all(&(bits as u16).to_be_bytes())
            .map_err(io_error)?;
        sink.write_all(&extended_sample_rate(spec.sample_rate))
            .map_err(io_error)?;
        sink.write_all(b"SSND").map_err(io_error)?;
        sink.write_all(&8u32.to_be_bytes()).map_err(io_error)?;
        sink.write_all(&0u32.to_be_bytes()).map_err(io_error)?;
        sink.write_all(&0u32.to_be_bytes()).map_err(io_error)?;
        Ok(Self {
            sink,
            channels: spec.channels,
            bits,
            sample_bytes: 0,
            frames: 0,
        })
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<(), AppError> {
        if !samples.len().is_multiple_of(self.channels) {
            return Err(AppError::Execute("Partial AIFF interleaved frame".into()));
        }
        let added_bytes = (samples.len() as u64)
            .checked_mul(u64::from(self.bits / 8))
            .ok_or_else(|| AppError::Execute("AIFF data size overflow".into()))?;
        let data_bytes = self
            .sample_bytes
            .checked_add(added_bytes)
            .ok_or_else(|| AppError::Execute("AIFF data size overflow".into()))?;
        let frames = self
            .frames
            .checked_add((samples.len() / self.channels) as u64)
            .ok_or_else(|| AppError::Execute("AIFF frame count overflow".into()))?;
        if data_bytes > u64::from(u32::MAX) - 46 || frames > u64::from(u32::MAX) {
            return Err(AppError::Execute(
                "AIFF exceeds 32-bit container length".into(),
            ));
        }
        let width = (self.bits / 8) as usize;
        for &sample in samples {
            let value = pcm::to_int(sample, self.bits).to_be_bytes();
            self.sink.write_all(&value[4 - width..]).map_err(io_error)?;
        }
        self.sample_bytes = data_bytes;
        self.frames = frames;
        Ok(())
    }

    fn finalize(mut self) -> Result<(), AppError> {
        self.sink.seek(SeekFrom::Start(4)).map_err(io_error)?;
        self.sink
            .write_all(&(46 + self.sample_bytes as u32).to_be_bytes())
            .map_err(io_error)?;
        self.sink.seek(SeekFrom::Start(22)).map_err(io_error)?;
        self.sink
            .write_all(&(self.frames as u32).to_be_bytes())
            .map_err(io_error)?;
        self.sink.seek(SeekFrom::Start(42)).map_err(io_error)?;
        self.sink
            .write_all(&(8 + self.sample_bytes as u32).to_be_bytes())
            .map_err(io_error)?;
        self.sink.flush().map_err(io_error)
    }
}

fn io_error(error: std::io::Error) -> AppError {
    AppError::Io(error.to_string())
}

pub fn encode(
    input: &Path,
    output: &Path,
    options: &AudioOptions,
) -> Result<ConversionResult, AppError> {
    let spec = shape::inspect_with(input, options)?;
    let bits = pcm::lossless_bits(input);
    execute::write_atomically(output, options, |temporary| {
        let file = std::fs::File::create(temporary).map_err(io_error)?;
        let mut writer = AiffWriter::with_bits(BufWriter::new(file), spec, bits)?;
        shape::stream_with(input, options, |samples, found| {
            if found != spec {
                return Err(AppError::Execute(
                    "Source audio specification changed".into(),
                ));
            }
            writer.write_samples(samples)
        })?;
        writer.finalize()
    })?;
    Ok(execute::success_result(
        output,
        &format!("Successfully converted to AIFF ({bits}-bit)"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::audio::probe::probe_audio;
    use crate::test_utils::create_test_wav;
    use std::io::Cursor;

    #[test]
    fn aiff_header_is_big_endian_and_exactly_sized() {
        let mut out = Cursor::new(Vec::<u8>::new());
        let spec = pcm::AudioSpec {
            sample_rate: 44_100,
            channels: 2,
        };
        let mut writer = AiffWriter::new(&mut out, spec).expect("header");
        writer
            .write_samples(&[0.0, -1.0, 1.0, 0.0])
            .expect("samples");
        writer.finalize().expect("finalize");
        let bytes = out.into_inner();
        assert_eq!(bytes.len(), 62);
        assert_eq!(&bytes[0..4], b"FORM");
        assert_eq!(&bytes[4..8], &54u32.to_be_bytes());
        assert_eq!(&bytes[8..12], b"AIFF");
        assert_eq!(&bytes[22..26], &2u32.to_be_bytes());
        assert_eq!(&bytes[28..38], &[0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&bytes[42..46], &16u32.to_be_bytes());
    }

    #[test]
    fn aiff_stream_roundtrip_and_cleanup() {
        let input = create_test_wav(2, 44_100, 100);
        let dir = tempfile::tempdir().expect("tempdir");
        let output = dir.path().join("out.aiff");
        encode(input.path(), &output, &AudioOptions::default()).expect("WAV to AIFF");
        let meta = probe_audio(&output).expect("Symphonia AIFF probe");
        assert_eq!(meta.channels, 2);
        assert_eq!(meta.sample_rate, 44_100);
        let samples = std::fs::read(&output).expect("AIFF bytes");
        assert!(samples.starts_with(b"FORM"));
        assert!(!std::fs::read_dir(dir.path())
            .expect("dir")
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with("zero_tmp_")));
    }

    #[test]
    fn truncated_aiff_header_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bad = dir.path().join("bad.aiff");
        std::fs::write(&bad, b"FORM\0\0\0\0AIFFCOMM").expect("write malformed");
        assert!(probe_audio(&bad).is_err());
    }

    #[test]
    fn overflowing_aiff_size_is_rejected_without_allocating() {
        let mut writer = AiffWriter::new(
            Cursor::new(Vec::<u8>::new()),
            pcm::AudioSpec {
                sample_rate: 48_000,
                channels: 1,
            },
        )
        .expect("writer");
        writer.sample_bytes = u64::from(u32::MAX) - 46;
        assert!(writer.write_samples(&[0.0]).is_err());
    }
    #[test]
    fn aiff_to_wav_roundtrip_preserves_samples_within_pcm16_step() {
        let input = create_test_wav(1, 48_000, 100);
        let dir = tempfile::tempdir().expect("tempdir");
        let middle = dir.path().join("middle.aif");
        let output = dir.path().join("final.wav");
        encode(input.path(), &middle, &AudioOptions::default()).expect("encode AIFF");
        super::super::wav::encode(&middle, &output, &AudioOptions::default()).expect("decode AIFF");
        let mut original = Vec::<f32>::new();
        let mut converted = Vec::<f32>::new();
        pcm::stream_pcm(input.path(), |chunk, _| {
            original.extend_from_slice(chunk);
            Ok(())
        })
        .expect("original");
        pcm::stream_pcm(&output, |chunk, _| {
            converted.extend_from_slice(chunk);
            Ok(())
        })
        .expect("converted");
        assert_eq!(original.len(), converted.len());
        for (a, b) in original.iter().zip(converted.iter()) {
            assert!((a - b).abs() <= 2.0 / 32_767.0);
        }
    }

    #[test]
    fn truncated_aiff_pcm_is_not_committed() {
        let input = create_test_wav(1, 48_000, 100);
        let dir = tempfile::tempdir().expect("tempdir");
        let good = dir.path().join("good.aiff");
        let bad = dir.path().join("bad.aiff");
        let output = dir.path().join("must_not_exist.wav");
        encode(input.path(), &good, &AudioOptions::default()).expect("create valid AIFF");
        let mut bytes = std::fs::read(&good).expect("read AIFF");
        bytes.truncate(bytes.len() / 2);
        std::fs::write(&bad, bytes).expect("write truncated AIFF");
        assert!(pcm::stream_pcm(&bad, |_, _| Ok(())).is_err());
        assert!(super::super::wav::encode(&bad, &output, &AudioOptions::default()).is_err());
        assert!(!output.exists());
        assert!(!std::fs::read_dir(dir.path())
            .expect("dir")
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with("zero_tmp_")));
    }
}
