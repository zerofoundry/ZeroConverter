//! Output shaping shared by every audio encoder: bitrate choice, mono
//! downmix and sample-rate conversion. Streams packet by packet like `pcm`.
use super::pcm::{self, AudioSpec};
use crate::app_error::AppError;
use crate::conversion::options::{Control, OptionSpec, OptionValue, Scope};
use rubato::{FftFixedIn, Resampler};
use serde::Deserialize;
use std::path::Path;

/// Audio settings from the conversion request. Missing fields keep the source.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AudioOptions {
    /// Lossy encoders only (MP3, Opus).
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    /// `1` folds every channel into mono.
    pub channels: Option<usize>,
    /// "Overwrite existing files".
    pub overwrite: bool,
}

impl AudioOptions {
    /// For encoders: the request was validated with [`AudioOptions::parse`].
    pub fn from_json(value: &serde_json::Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }

    /// Reads and checks the options; a malformed or out-of-range value is a
    /// validation error, the same policy as images.
    pub fn parse(value: &serde_json::Value) -> Result<Self, AppError> {
        let options: Self = serde_json::from_value(value.clone())
            .map_err(|e| AppError::Validate(format!("Invalid audio options: {e}")))?;
        if let Some(rate) = options.sample_rate {
            if !(8_000..=192_000).contains(&rate) {
                return Err(AppError::Validate(format!(
                    "Sample rate {rate} Hz is out of range"
                )));
            }
        }
        // Mono is the one channel change on offer; unset keeps the source's.
        if let Some(channels) = options.channels {
            if channels != 1 {
                return Err(AppError::Validate(format!(
                    "Channels can only be 1 (mono); leave it unset to keep the source's channels, got {channels}"
                )));
            }
        }
        if let Some(kbps) = options.bitrate_kbps {
            if !(32..=512).contains(&kbps) {
                return Err(AppError::Validate(format!(
                    "Bitrate {kbps} kbps is out of range"
                )));
            }
        }
        Ok(options)
    }
}

// ── What the panel offers ───────────────────────────────────────────────────
// Attached to the formats whose encoder honours them (`formats/*.rs`, see
// `conversion::options`). The bounds are the ones `AudioOptions::parse` checks.

pub static SAMPLE_RATE: OptionSpec = OptionSpec {
    key: "sample_rate",
    control: Control::Number {
        min: 8_000,
        max: 192_000,
        presets: &[44_100, 48_000],
    },
    default: OptionValue::Unset,
    unit: Some("hz"),
    scope: Scope::Family,
    quick: false,
    show_when: None,
    note: Some("unset keeps the source's sample rate"),
};

pub static CHANNELS: OptionSpec = OptionSpec {
    key: "channels",
    control: Control::Choice {
        values: &[OptionValue::Unset, OptionValue::Int(1)],
    },
    default: OptionValue::Unset,
    unit: None,
    scope: Scope::Family,
    quick: false,
    show_when: None,
    note: Some("1 makes the output mono; otherwise the source's channels are kept"),
};

/// The bitrates the panel lists; `parse` accepts any value in range.
const fn bitrate(presets: &'static [i64], default: OptionValue) -> OptionSpec {
    OptionSpec {
        key: "bitrate_kbps",
        control: Control::Number {
            min: 32,
            max: 512,
            presets,
        },
        default,
        unit: Some("kbps"),
        scope: Scope::Format,
        quick: true,
        show_when: None,
        note: None,
    }
}

pub static MP3_BITRATE: OptionSpec = bitrate(&[128, 192, 256, 320], OptionValue::Int(320));
/// Unset: 64 kbps for mono, 128 for stereo.
pub static OPUS_BITRATE: OptionSpec = bitrate(&[64, 96, 128, 192], OptionValue::Unset);
/// M4A and AAC, written by FFmpeg. Unset: AAC audio is copied as it is,
/// anything else is encoded at 192 kbps (`video::plan::plan_audio_direct`).
pub static AAC_BITRATE: OptionSpec = bitrate(&[128, 192, 256, 320], OptionValue::Unset);

/// The spec the encoder will receive for this source.
pub fn output_spec(source: AudioSpec, options: &AudioOptions) -> AudioSpec {
    AudioSpec {
        sample_rate: options.sample_rate.unwrap_or(source.sample_rate),
        channels: match options.channels {
            Some(1) => 1,
            _ => source.channels,
        },
    }
}

pub fn inspect_with(input: &Path, options: &AudioOptions) -> Result<AudioSpec, AppError> {
    Ok(output_spec(pcm::inspect(input)?, options))
}

/// Like `pcm::stream_pcm`, but hands the encoder audio already shaped to
/// `output_spec`. Without changes it is a plain pass-through.
pub fn stream_with<F>(input: &Path, options: &AudioOptions, mut consume: F) -> Result<(), AppError>
where
    F: FnMut(&[f32], AudioSpec) -> Result<(), AppError>,
{
    let source = pcm::inspect(input)?;
    let target = output_spec(source, options);
    if target == source {
        return pcm::stream_pcm(input, consume);
    }
    let mut shaper = Shaper::new(source, target)?;
    pcm::stream_pcm(input, |samples, found| {
        if found != source {
            return Err(AppError::Execute(
                "Source audio specification changed".into(),
            ));
        }
        let shaped = shaper.push(samples)?;
        if shaped.is_empty() {
            Ok(())
        } else {
            consume(&shaped, target)
        }
    })?;
    let tail = shaper.finish()?;
    if tail.is_empty() {
        Ok(())
    } else {
        consume(&tail, target)
    }
}

const CHUNK_FRAMES: usize = 1024;

struct Shaper {
    source: AudioSpec,
    target: AudioSpec,
    resampler: Option<FftFixedIn<f32>>,
    /// Planar input waiting for a full resampler chunk.
    pending: Vec<Vec<f32>>,
    frames_in: u64,
    frames_out: u64,
    /// Leading output frames that are resampler latency, not audio.
    skip: usize,
}

fn resample_error(e: impl std::fmt::Display) -> AppError {
    AppError::Execute(format!("Resampling failed: {e}"))
}

impl Shaper {
    fn new(source: AudioSpec, target: AudioSpec) -> Result<Self, AppError> {
        if !(8_000..=192_000).contains(&target.sample_rate) {
            return Err(AppError::Validate(format!(
                "Unsupported output sample rate: {} Hz",
                target.sample_rate
            )));
        }
        let resampler = if source.sample_rate == target.sample_rate {
            None
        } else {
            Some(
                FftFixedIn::<f32>::new(
                    source.sample_rate as usize,
                    target.sample_rate as usize,
                    CHUNK_FRAMES,
                    2,
                    target.channels,
                )
                .map_err(resample_error)?,
            )
        };
        let skip = resampler.as_ref().map_or(0, |r| r.output_delay());
        Ok(Self {
            source,
            target,
            resampler,
            pending: vec![Vec::with_capacity(CHUNK_FRAMES * 2); target.channels],
            frames_in: 0,
            frames_out: 0,
            skip,
        })
    }

    /// Interleaved source samples in, interleaved shaped samples out.
    fn push(&mut self, samples: &[f32]) -> Result<Vec<f32>, AppError> {
        let channels = self.source.channels;
        let frames = samples.len() / channels;
        self.frames_in += frames as u64;
        let mono = self.target.channels == 1 && channels > 1;
        if self.resampler.is_none() {
            return Ok(if mono {
                samples
                    .chunks_exact(channels)
                    .map(|f| f.iter().sum::<f32>() / channels as f32)
                    .collect()
            } else {
                samples.to_vec()
            });
        }
        for frame in samples.chunks_exact(channels) {
            if mono {
                self.pending[0].push(frame.iter().sum::<f32>() / channels as f32);
            } else {
                for (lane, &s) in self.pending.iter_mut().zip(frame) {
                    lane.push(s);
                }
            }
        }
        let mut out = Vec::new();
        loop {
            let resampler = self.resampler.as_mut().expect("checked above");
            let need = resampler.input_frames_next();
            if self.pending[0].len() < need {
                break;
            }
            let chunk: Vec<Vec<f32>> = self
                .pending
                .iter_mut()
                .map(|lane| lane.drain(..need).collect())
                .collect();
            let produced = resampler.process(&chunk, None).map_err(resample_error)?;
            self.emit(&produced, &mut out);
        }
        Ok(out)
    }

    /// Drains the resampler so the output is exactly as long as the input.
    fn finish(&mut self) -> Result<Vec<f32>, AppError> {
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(Vec::new());
        };
        let expected = (self.frames_in as u128 * self.target.sample_rate as u128
            / self.source.sample_rate as u128) as u64;
        let mut out = Vec::new();
        let first = std::mem::take(&mut self.pending);
        let produced = resampler
            .process_partial(Some(&first), None)
            .map_err(resample_error)?;
        self.emit(&produced, &mut out);
        // Each empty call pushes one more chunk of latency out.
        for _ in 0..8 {
            if self.frames_out >= expected {
                break;
            }
            let resampler = self.resampler.as_mut().expect("checked above");
            let produced = resampler
                .process_partial::<Vec<f32>>(None, None)
                .map_err(resample_error)?;
            self.emit(&produced, &mut out);
        }
        let surplus = self.frames_out.saturating_sub(expected) as usize * self.target.channels;
        out.truncate(out.len().saturating_sub(surplus));
        Ok(out)
    }

    /// Appends planar output as interleaved samples, dropping leading latency.
    fn emit(&mut self, planar: &[Vec<f32>], out: &mut Vec<f32>) {
        let frames = planar.first().map_or(0, Vec::len);
        let start = self.skip.min(frames);
        self.skip -= start;
        for i in start..frames {
            for lane in planar {
                out.push(lane[i]);
            }
        }
        self.frames_out += (frames - start) as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, channels: usize, seconds: f32) -> Vec<f32> {
        let frames = (rate as f32 * seconds) as usize;
        (0..frames)
            .flat_map(|i| {
                let v = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5;
                std::iter::repeat_n(v, channels)
            })
            .collect()
    }

    fn run(source: AudioSpec, target: AudioSpec, input: &[f32]) -> Vec<f32> {
        let mut shaper = Shaper::new(source, target).unwrap();
        let mut out = Vec::new();
        // Uneven packets, like a real decoder.
        for packet in input.chunks(source.channels * 777) {
            out.extend(shaper.push(packet).unwrap());
        }
        out.extend(shaper.finish().unwrap());
        out
    }

    #[test]
    fn resampling_keeps_duration_and_level() {
        let source = AudioSpec {
            sample_rate: 44_100,
            channels: 2,
        };
        let target = AudioSpec {
            sample_rate: 48_000,
            channels: 2,
        };
        let out = run(source, target, &sine(44_100, 2, 1.0));
        assert_eq!(out.len() / 2, 48_000);
        let peak = out[2000..90_000].iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.02, "peak {peak}");
    }

    #[test]
    fn mono_downmix_averages_channels() {
        let source = AudioSpec {
            sample_rate: 48_000,
            channels: 2,
        };
        let target = AudioSpec {
            sample_rate: 48_000,
            channels: 1,
        };
        let out = run(source, target, &[0.2, 0.4, -1.0, 1.0]);
        assert_eq!(out.len(), 2);
        assert!((out[0] - 0.3).abs() < 1e-6 && out[1].abs() < 1e-6);
    }

    #[test]
    fn mono_and_resample_together() {
        let source = AudioSpec {
            sample_rate: 48_000,
            channels: 2,
        };
        let target = AudioSpec {
            sample_rate: 22_050,
            channels: 1,
        };
        let out = run(source, target, &sine(48_000, 2, 0.5));
        assert_eq!(out.len(), 11_025);
    }

    #[test]
    fn bad_options_are_a_validation_error_not_a_silent_default() {
        let bad = [
            serde_json::json!({ "sample_rate": 1_000_000 }),
            serde_json::json!({ "channels": 0 }),
            serde_json::json!({ "bitrate_kbps": 5 }),
            serde_json::json!({ "channels": "two" }),
        ];
        for value in bad {
            assert!(
                matches!(AudioOptions::parse(&value), Err(AppError::Validate(_))),
                "{value}"
            );
        }
        let ok = serde_json::json!({ "sample_rate": 48_000, "channels": 1, "quality": 80 });
        assert_eq!(AudioOptions::parse(&ok).unwrap().sample_rate, Some(48_000));
    }

    #[test]
    fn options_parse_from_request_json() {
        let o = AudioOptions::from_json(&serde_json::json!({
            "quality": 80, "bitrate_kbps": 192, "channels": 1
        }));
        assert_eq!(o.bitrate_kbps, Some(192));
        assert_eq!(o.channels, Some(1));
        assert_eq!(o.sample_rate, None);
        assert_eq!(
            AudioOptions::from_json(&serde_json::json!("x")),
            AudioOptions::default()
        );
    }
}
