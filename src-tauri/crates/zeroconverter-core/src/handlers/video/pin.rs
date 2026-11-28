//! The FFmpeg build the app downloads on first video use: one exact file,
//! checked against its SHA-256. No network code here (see `install.rs`).
//!
//! The builds come from <https://github.com/FFmpeg-Builds/FFmpeg-Builds> (the
//! project formerly at BtbN/FFmpeg-Builds). Refreshing the pin: pick an
//! `autobuild-*` release, take the `win64-lgpl` zip (NOT `-gpl` or `-shared`),
//! `sha256sum` it and update the constants below. Old autobuilds are pruned
//! upstream, so the pin is refreshed for every ZeroConverter release; an
//! install whose pin has gone away falls back to the rolling `latest` build
//! (see `install.rs`).

pub struct Pin {
    /// Folder name under `<app data>/ffmpeg/`.
    pub id: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size_bytes: u64,
    pub label: &'static str,
    pub license: &'static str,
}

pub const PIN: Pin = Pin {
    id: "n8.1.3-35-gf1acb0a266-lgpl",
    url: "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-12-15-40/ffmpeg-n8.1.3-35-gf1acb0a266-win64-lgpl-8.1.zip",
    sha256: "f8cd2e3a9570cc0101876569f21f130f7741cc86a97476f98132cba9f610d906",
    size_bytes: 167_647_897,
    label: "FFmpeg 8.1.3 (FFmpeg-Builds LGPL build, Windows x64)",
    license: "LGPL-2.1-or-later",
};
