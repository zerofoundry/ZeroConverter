<p align="center"><img src="docs/media/zeroconverter-lockup-dark.png" alt="ZeroConverter by ZeroFoundry" width="560"></p>

<p align="center"><b>Rename a file in Explorer, and it converts.</b><br>
A free, open-source file converter for Windows. Everything runs on your own PC.</p>

<p align="center"><a href="https://github.com/ZeroFoundry/ZeroConverter/releases/latest"><b>Download for Windows</b></a></p>

<p align="center"><img src="docs/media/folder.gif" alt="Renaming a folder to Apollo.jpg converts every picture inside it" width="720"></p>

As a kid I renamed a video to `.mp3` and waited for it to become a song. ZeroConverter is the version where that works: while it runs, change `clip.mp4` to `clip.mp3` in File Explorer and the file is converted for real. The original goes to the Recycle Bin, so nothing is lost by accident.

- **Rename to convert:** `photo.png` → `photo.webp`, `texture.png` → `texture.dds`, `song.wav` → `song.mp3`.
- **Whole folders:** rename a folder to `Album.jpg` and every file inside is converted. The folder keeps its name.
- **Little extras in the name:** `photo.jpg@1080` resizes, `photo.webp&` keeps the original (or sends it to the Recycle Bin when **Keep the original file** is on in Settings), `photo.webp+` asks for quality first.
- **An app window too:** drop in a pile of files, pick a format, watch the queue.
- **Private:** no account, no uploads, no telemetry.

<table><tr>
<td><img src="docs/media/mp3.gif" alt="video.mp4 renamed to video.mp3, then played"></td>
<td><img src="docs/media/dds.gif" alt="A folder of textures renamed to .dds"></td>
</tr></table>

<p align="center"><img src="docs/media/app.jpg" alt="The ZeroConverter app window after a batch" width="720"></p>

## Download

Get the Windows installer from [Releases](https://github.com/ZeroFoundry/ZeroConverter/releases). It installs for the current user, no admin rights needed. Windows 10 or 11 (x64) with WebView2, which Windows 11 already has.

Once a day it looks on GitHub for a new version and tells you if there is one. Nothing is downloaded until you choose **Download and restart** in Settings › About, where you can also turn the check off.

The installer is not code-signed yet, so Windows SmartScreen may warn on first run ("More info" → "Run anyway"). Each release lists the installer's SHA-256 so you can check it.

## Languages

The interface comes in 12 languages: English, Türkçe, Deutsch, Español, Français, Português (Brasil), 日本語, 한국어, 简体中文, Русский, हिन्दी and العربية (right to left). It follows the display language of Windows; change it any time in Settings › General. File formats and extensions are never translated. The tray menu, the right-click entry and the desktop notices follow it too; only error details from the engine are still English (see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#languages)).

## In more detail

- **Local processing:** conversion jobs run on the local Rust backend; no asset-upload service is part of the conversion pipeline.
- **Batch jobs:** submit multiple files, monitor per-item progress, and cancel queued/running jobs. Files over 100 MB are converted only after you confirm.
- **Image, audio and video:** raster and texture conversion, constrained SVG rasterization, image resize, audio transcodes, and video container changes without re-encoding.
- **Command line:** `zeroconverter run --job job.json` uses the same engine from scripts and pipelines, with JSON in and JSON lines out ([docs/CLI.md](docs/CLI.md)).
- **Rename to convert:** rename `photo.png` to `photo.webp` in Explorer and it is converted (see [below](#rename-to-convert)). There is also a right-click **ZeroConverter** menu on supported files.

## Formats

26 image, texture, HDR and audio formats, converted locally and offline. Everything is pure Rust except MP3 encoding (LAME, LGPL). Video, and writing M4A or AAC, are the exceptions: they run through FFmpeg: one already on the PC is used, otherwise it is downloaded once from Settings → Video or on first use (see [Video](#video)).

| | Read and write | Read only | Write only |
|---|---|---|---|
| **Images** | PNG, JPG, WebP, BMP, TIFF, ICO, QOI, TGA | SVG, GIF, PSD (composite), JPEG XL | PDF, AVIF |
| **Game textures** | DDS (reads BC1–BC7; writes BC1–BC5, BC7), KTX2 (uncompressed RGBA8) | | |
| **HDR** | OpenEXR, Radiance HDR | | |
| **Audio** | WAV, MP3, FLAC, AIFF, Opus; M4A and AAC (written with FFmpeg) | OGG Vorbis | |
| **Video** (FFmpeg) | MP4, MKV, MOV, WebM | AVI | GIF; audio out as WAV, MP3, FLAC, AIFF, Opus, M4A, AAC |

Known limits: Windows only; animated GIF input is rejected; EXR/HDR written to 8/16-bit formats is clamped to 0–1 (no tone mapping); multi-layer PSD is read as its flattened composite; DDS/KTX2 read a single 2D surface (no mip chains, arrays or cube maps).

### Video

Video is about **not re-encoding**. Changing the container (MP4 ↔ MKV ↔ MOV, and WebM when the streams allow) copies the streams (`-c copy`), so it takes seconds, not minutes, and loses nothing; only streams the target container cannot hold are re-encoded. Re-encodes: MP4/MOV/MKV with H.264 + AAC, WebM with VP9 + Opus, GIF (two-pass palette), and audio extraction. Video makes no speed claim; FFmpeg does the work.

FFmpeg is **not in the installer**. The first time you convert a video, ZeroConverter asks, then downloads one pinned **LGPL** Windows build from GitHub (FFmpeg-Builds, formerly BtbN/FFmpeg-Builds), checks it against a pinned SHA-256 and keeps it in the app data folder. An `ffmpeg` and `ffprobe` already on `PATH` are used instead if no downloaded copy exists. Apart from that and the update check, nothing in the app touches the network. Renaming a video's extension in Explorer converts it too (rename-to-convert); if FFmpeg has not been downloaded yet, the rename is reverted with a notice.

**H.264 without x264.** The build is LGPL, so it has no x264/x265. H.264 is encoded with whichever of NVENC, Intel QSV, AMD AMF, Windows Media Foundation (`h264_mf`) or OpenH264 the machine has, tried in that order. WebM uses libvpx-vp9. HEVC/H.264/AAC copied between containers are never re-encoded at all. Licensing details and the route table: [docs/VIDEO.md](docs/VIDEO.md).

## Rename to convert

Rename a file's extension in Explorer to a format ZeroConverter can write, and it converts the file and puts the original name back. The app has to be running (it can sit in the tray).

- **Anywhere:** all fixed drives are watched. Outside the folders you mark as always-on in Settings, a rename only counts when a file manager is the window in front, so programs renaming their own files are left alone.
- **Marks** you can add to the new name, in any order:
  - `&` keeps the original file, or sends it to the Recycle Bin when **Keep the original file** is on in Settings: `photo.webp&`
  - `@N` limits the longer side to N pixels (images): `photo.webp@512`
  - `+` opens a small window for quality and size first: `photo.webp+`
- **Folders:** rename a folder to `Album.webp` and the files inside it are converted (up to 2,000); the folder keeps its name.
- **Hidden extensions:** works when Explorer hides file extensions.
- **Right-click menu:** a per-user **ZeroConverter** submenu on supported files, no admin rights needed. It can be turned off in Settings and is removed on uninstall.

3D format conversion is out of scope for now. Images over 100 megapixels are not previewed in the queue.

## Build from source

**Prerequisites** (Windows is the primary target):

- Git.
- Node.js 20.19, or 22.12 and later, with npm.
- A stable Rust toolchain (`rustup`, the MSVC target).
- Visual Studio Build Tools with "Desktop development with C++" (the MSVC compiler and a Windows SDK).
- WebView2. Windows 11 has it; on Windows 10 install the Evergreen runtime.
- For the full check script only: [Gitleaks](https://github.com/gitleaks/gitleaks) on `PATH`.

The [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/) page has the details. `git`, `node`, `npm` and `cargo` should all answer `--version` in a new terminal.

```sh
git clone https://github.com/ZeroFoundry/ZeroConverter.git
cd ZeroConverter
npm ci
npm run tauri -- dev
```

`npm run build` builds the frontend only. `npm run tauri -- build` builds the app alone; the installer that ships also carries the command line and the full license list, and is made by `scripts/release.mjs` (see the comment at its top).

The command line runs straight from source, no frontend needed:

```sh
cd src-tauri
cargo run -p zeroconverter-cli -- capabilities --json
```

The installed app puts `zeroconverter.exe` in its `cli` folder (`%LOCALAPPDATA%\ZeroConverter\cli`); add that folder to `PATH` to use it anywhere.

### Tests

On Windows, once dependencies and toolchains are installed:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\docs\scripts\verify-local.ps1
```

This checks the lockfile, the frontend build and layering, Rust formatting, Clippy, the Rust tests and runs a secret scan (Gitleaks).

## Security and privacy

- File content is intended to stay local; no cloud upload or telemetry is added by the conversion backend.
- Untrusted inputs can contain malicious or malformed media data. Do not process untrusted assets with elevated privileges.
- The `open_completed_output` command accepts an existing completed job, canonicalizes its output path, verifies a regular file and checks a format allowlist. It does **not** establish trust in third-party OS file viewers.
- SVG restrictions reject unsupported elements and external-resource features; this is not a general-purpose SVG sanitizer.
- Generated outputs should be reviewed before replacing source assets. Keep backups for valuable originals.

See [Security Policy](SECURITY.md) for reporting guidance.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) and the [architecture notes](docs/ARCHITECTURE.md). File bugs and requests in [GitHub Issues](https://github.com/ZeroFoundry/ZeroConverter/issues), without attaching private files.

## Licensing and dependencies

**License.** ZeroConverter's source code is released under the [MIT License](LICENSE). Source-code licensing does not override third-party obligations.

MP3 output uses the **LAME** encoder, which is LGPL and built into the app. How the LGPL terms are met (notice, library source, relinking) is in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md), which also ships with the installer.
