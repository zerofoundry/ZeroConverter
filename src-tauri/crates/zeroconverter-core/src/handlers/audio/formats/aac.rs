use super::AudioFormatModule;
use crate::conversion::types::FormatDescriptor;

// ADTS AAC-LC, decoded by Symphonia's opt-in "aac" feature.
pub const MODULE: AudioFormatModule = AudioFormatModule {
    descriptor: FormatDescriptor {
        id: "aac",
        canonical_extension: "aac",
        aliases: &[],
        can_read: true,
        can_write: false,
    },
    encoder: None,
    options: &[],
};
