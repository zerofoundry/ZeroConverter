use super::AudioFormatModule;
use crate::conversion::types::FormatDescriptor;
use crate::handlers::audio::shape;

// ISO BMFF / MP4 audio container. Read with Symphonia; written as AAC by
// FFmpeg (no pure-Rust AAC encoder), so it has no `encoder` here: see
// `AudioFormatModule::via_ffmpeg`. `.mp4` belongs to the video family.
pub const MODULE: AudioFormatModule = AudioFormatModule {
    descriptor: FormatDescriptor {
        id: "m4a",
        canonical_extension: "m4a",
        aliases: &["m4b"],
        can_read: true,
        can_write: true,
    },
    encoder: None,
    options: &[&shape::AAC_BITRATE, &shape::SAMPLE_RATE, &shape::CHANNELS],
};
