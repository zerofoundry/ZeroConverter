use super::AudioFormatModule;
use crate::conversion::types::FormatDescriptor;

// ISO BMFF / MP4 audio container, not an AAC encoder. `.mp4` belongs to the
// video family; its audio-extraction routes decode MP4 audio with Symphonia
// before falling back to FFmpeg.
pub const MODULE: AudioFormatModule = AudioFormatModule {
    descriptor: FormatDescriptor {
        id: "m4a",
        canonical_extension: "m4a",
        aliases: &["m4b"],
        can_read: true,
        can_write: false,
    },
    encoder: None,
    options: &[],
};
