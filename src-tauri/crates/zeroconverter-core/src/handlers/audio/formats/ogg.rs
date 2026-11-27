use super::AudioFormatModule;
use crate::conversion::types::FormatDescriptor;

// Existing Ogg/Vorbis decoding remains supported through Symphonia.
// No Vorbis encoder is advertised until bounded-memory Windows evidence exists.
pub const MODULE: AudioFormatModule = AudioFormatModule {
    descriptor: FormatDescriptor {
        id: "ogg",
        canonical_extension: "ogg",
        aliases: &[],
        can_read: true,
        can_write: false,
    },
    encoder: None,
    options: &[],
};
