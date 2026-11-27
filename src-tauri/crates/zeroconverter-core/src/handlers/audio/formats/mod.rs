//! One audio format per module; MODULES is the single route/encoder registry.
//! FormatDescriptor entries in the public handler table are projected from these constants.
pub mod aac;
pub mod aiff;
pub mod flac;
pub mod m4a;
pub mod mp3;
pub mod ogg;
pub mod opus;
pub mod wav;

use crate::app_error::AppError;
use crate::conversion::options::OptionSpec;
use crate::conversion::types::{ConversionResult, FormatDescriptor};
use std::path::Path;

pub type AudioEncoder =
    fn(&Path, &Path, &super::shape::AudioOptions) -> Result<ConversionResult, AppError>;

#[derive(Clone, Copy)]
pub struct AudioFormatModule {
    pub descriptor: FormatDescriptor,
    pub encoder: Option<AudioEncoder>,
    /// Options this format's encoder honours (`super::shape`), in panel order.
    pub options: &'static [&'static OptionSpec],
}

impl AudioFormatModule {
    /// Writable with no pure-Rust encoder: FFmpeg writes it (M4A and AAC).
    pub fn via_ffmpeg(&self) -> bool {
        self.descriptor.can_write && self.encoder.is_none()
    }
}

pub static MODULES: &[AudioFormatModule] = &[
    wav::MODULE,
    mp3::MODULE,
    flac::MODULE,
    ogg::MODULE,
    aiff::MODULE,
    opus::MODULE,
    aac::MODULE,
    m4a::MODULE,
];

pub fn find(format: &str) -> Option<&'static AudioFormatModule> {
    MODULES
        .iter()
        .find(|module| module.descriptor.matches_format(format))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_have_unique_ids_and_consistent_encoders() {
        for (index, module) in MODULES.iter().enumerate() {
            if module.encoder.is_some() {
                assert!(module.descriptor.can_write);
            }
            // Only targets the FFmpeg audio path knows may go without an encoder.
            if module.via_ffmpeg() {
                assert!(matches!(
                    crate::handlers::video::formats::target_of(module.descriptor.id),
                    Some(crate::handlers::video::formats::Target::Audio(_))
                ));
            }
            for other in &MODULES[index + 1..] {
                assert_ne!(module.descriptor.id, other.descriptor.id);
            }
        }
        // No two formats claim the same name, in any case.
        let mut names = std::collections::HashSet::new();
        for module in MODULES {
            let d = &module.descriptor;
            let mut own = vec![
                d.id.to_ascii_lowercase(),
                d.canonical_extension.to_ascii_lowercase(),
            ];
            own.extend(d.aliases.iter().map(|a| a.to_ascii_lowercase()));
            own.sort();
            own.dedup();
            for name in own {
                assert!(names.insert(name.clone()), "{name} is claimed twice");
            }
        }
        assert!(find("AIFF").is_some());
        assert!(find("M4A").is_some());
        assert!(find("opus").and_then(|m| m.encoder).is_some());
        assert!(find("ogg").and_then(|m| m.encoder).is_none());
        assert!(!find("ogg").unwrap().via_ffmpeg());
        assert!(find("m4a").unwrap().via_ffmpeg());
        assert!(find("aac").unwrap().via_ffmpeg());
    }
}
