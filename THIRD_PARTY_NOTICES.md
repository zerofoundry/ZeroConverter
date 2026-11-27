# Third-party notices

This file explains the third-party components with terms beyond a plain notice (LAME, FFmpeg, fonts). It is not legal advice. The complete list of every Rust crate and npm package built into the app and the command line, each with its license text, is `licenses/THIRD_PARTY_LICENSES.txt` in the installed app and in the command-line zip. `node scripts/licenses.mjs` makes it from the source tree and refuses GPL and AGPL licenses.

## MP3 encoding: LAME (LGPL), built into the app

ZeroConverter writes MP3 with the **LAME 3.100** encoder library. It is compiled into the application (static linking) through the Rust crates `mp3lame-encoder 0.2.2` and `mp3lame-sys 0.1.11` (both LGPL-3.0; LAME itself is LGPL-2.0-or-later).

How the LGPL terms are met:

- **Optional.** LAME is behind the `mp3` cargo feature (on by default). `--no-default-features` builds the engine and the command line without it; MP3 can then still be read but not written.
- **Notice.** This file, installed with the app, states that LAME is used and covered by the LGPL.
- **License texts.** The installer places [`GPL-3.0`](licenses/GPL-3.0.txt), [`LGPL-3.0`](licenses/LGPL-3.0.txt) and [`LGPL-2.0`](licenses/LGPL-2.0.txt) in the `licenses` folder next to the program.
- **Library source.** The exact LAME source used is the `lame-3.100` tree inside `mp3lame-sys 0.1.11` ([crates.io](https://crates.io/crates/mp3lame-sys/0.1.11)), pinned in `src-tauri/Cargo.lock`. Each release links it in its notes.
- **Relinking.** ZeroConverter's own source is public under MIT, published at the same tag as each installer. Anyone can rebuild the app against a modified LAME by pointing `mp3lame-sys` at their copy (a Cargo `[patch]` entry) and running the normal build.

This is the project's engineering reading of the license, not legal advice. Re-check it if the build switches LAME version, crate, or linking.

## FFmpeg (video): downloaded by the user, not bundled

- Video conversion starts `ffmpeg.exe`/`ffprobe.exe` as separate processes. The installer does **not** contain FFmpeg. On first use the app asks, then downloads one pinned build (`PIN` in `src-tauri/crates/zeroconverter-core/src/handlers/video/pin.rs`) from the [FFmpeg-Builds](https://github.com/FFmpeg-Builds/FFmpeg-Builds) project (formerly BtbN/FFmpeg-Builds), `win64-lgpl` release (static, LGPL-2.1-or-later configuration, no x264/x265), verifies its SHA-256 and stores it, with the build's `LICENSE.txt`, in the app data folder.
- ZeroConverter code never selects `libx264`/`libx265`. H.264 is encoded with NVENC, QSV, AMF, Windows Media Foundation or OpenH264 (BSD), whichever the machine offers; container changes copy streams. A user's own `ffmpeg` on `PATH` is used as-is if no downloaded copy exists and is never redistributed.
- Because FFmpeg is neither linked nor shipped, LGPL obligations for it rest with the party that distributes it (FFmpeg-Builds, downloading on the user's request). Re-check this if the project ever bundles or mirrors the binary. H.264, HEVC and AAC are patent-encumbered formats; see [docs/VIDEO.md](docs/VIDEO.md).
- New Rust crates for this feature: `ureq` (HTTP client, MIT OR Apache-2.0, Windows TLS via the OS) and `zip` (MIT) with its `flate2` backend; both are used only by the one-time FFmpeg download.

## Fonts (SIL Open Font License 1.1)

The interface bundles **Inter** (Copyright 2016 The Inter Project Authors) and **JetBrains Mono** (Copyright 2020 The JetBrains Mono Project Authors), both under the SIL Open Font License 1.1; full texts are installed as `licenses/Inter-OFL.txt` and `licenses/JetBrainsMono-OFL.txt`.

### Space Grotesk

The interface uses **Space Grotesk**, Copyright 2020 The Space Grotesk Project Authors, under the SIL Open Font License 1.1. The full text is installed as `licenses/SpaceGrotesk-OFL.txt` (source: [`src/assets/fonts/SpaceGrotesk-OFL.txt`](src/assets/fonts/SpaceGrotesk-OFL.txt)).

## Notification sounds

The four notification sounds (`src-tauri/assets/notification-*.wav`) were generated in code for this project. They are original to ZeroConverter and covered by its MIT license; no third-party audio is included.

## Selected Rust source dependencies

| Dependency | Purpose | License indicated by upstream/previous audit |
|---|---|---|
| `symphonia` | Audio decoding | MPL-2.0 |
| `resvg`, `usvg` | SVG parsing/rasterization | MPL-2.0 |
| `image`, `pdf-writer`, `flac-codec` | Codec and PDF implementations | MIT OR Apache-2.0 |
| `tauri`, Tauri plugins | Desktop framework | MIT OR Apache-2.0 |
| `mp3lame-encoder`, `mp3lame-sys` | MP3 encoding | LGPL-3.0 |
| `libwebp-sys` (libwebp, built from its bundled source) | Lossy WebP encoding | MIT (crate); libwebp BSD-3-Clause with Google's patent grant |

This table lists the main dependencies, not every transitive one; `cargo license` in `src-tauri/` prints the full list for the locked versions. Preserve MPL notices and required source offers for any covered modified files. The complete [MPL-2.0 license text](licenses/MPL-2.0.txt) is included for reference; verify shipment of every license text applicable to the final bundle.

## npm packages

The committed `package-lock.json` includes permissive packages plus `lightningcss` packages under MPL-2.0 and `caniuse-lite` under CC-BY-4.0. These are **lockfile entries**, not evidence that all corresponding packages ship with the final installer.
