use super::AudioFormatModule;
use crate::conversion::types::FormatDescriptor;

// ADTS AAC-LC. Read with Symphonia's opt-in "aac" feature; written by FFmpeg
// (see `AudioFormatModule::via_ffmpeg`).
pub const MODULE: AudioFormatModule = AudioFormatModule {
    descriptor: FormatDescriptor {
        id: "aac",
        canonical_extension: "aac",
        aliases: &[],
        can_read: true,
        can_write: true,
    },
    encoder: None,
};
