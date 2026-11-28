# Video conversion

Engineering notes for the FFmpeg-backed video family (`src-tauri/crates/zeroconverter-core/src/handlers/video/`).
Not legal advice; see the license section before shipping.

## How a conversion is decided

`plan.rs` looks at every stream (via `ffprobe`) and the target container:

* **Stream copy** (`-c copy`) for each stream the target container can hold. Nothing is decoded.
* **Re-encode only the streams that do not fit**: video to H.264 (MP4/MOV/MKV) or VP9 (WebM); audio to AAC (MP4/MOV/MKV) or Opus (WebM).
* Subtitles: text subtitles are converted to what the container supports (`mov_text` for MP4/MOV, `webvtt` for WebM, `srt` for MKV); bitmap subtitles, attachments and data streams that cannot be carried over are dropped and the job's result message says so.
* Rotation metadata (phone clips) survives a copy; any re-encode applies it (FFmpeg autorotate) so the output is upright.
* HEVC copied into MP4/MOV is tagged `hvc1` so Apple players accept it.
* Output is written into a `zero_tmp_*` workspace next to the final file and renamed into place only when FFmpeg exits 0; cancelling kills the child process and removes the workspace.
* Progress comes from `-progress pipe:1` (`out_time_us` / duration) and is forwarded as the job's `Executing` percentage.

## Route table

Every source can go to every writable target except itself (`avi` is read-only). What actually happens:

| Source → target | MP4 | MKV | MOV | WebM | GIF | audio (WAV, MP3, FLAC, AIFF, Opus, M4A, AAC) |
|---|---|---|---|---|---|---|
| **H.264/HEVC + AAC** (MP4, MKV, MOV) | n/a or copy | **copy** | **copy** | re-encode: VP9 + Opus | 2-pass palette | MP4/MOV with decodable AAC/MP3: pure-Rust decode (no FFmpeg); otherwise FFmpeg decodes to WAV then the existing encoders run; M4A/AAC copy an AAC track |
| **VP9/AV1 + Opus/Vorbis** (WebM) | **copy** | **copy** | video → H.264, audio → AAC | n/a | 2-pass palette | as above |
| **MPEG-4 + MP3** (AVI) | **copy** | **copy** | **copy** | re-encode | 2-pass palette | as above |

Codecs each container takes as a copy (anything else is re-encoded, per stream, not the whole file): MP4 video H.264, HEVC, AV1, MPEG-4, MPEG-2, VP9, audio AAC, MP3, AC-3, E-AC-3, ALAC, Opus. MOV video H.264, HEVC, MPEG-4, MPEG-2, ProRes, MJPEG, DNxHD, audio AAC, MP3, AC-3, E-AC-3, ALAC, PCM. MKV takes everything. WebM video VP8, VP9, AV1, audio Opus, Vorbis.

### Measured on the dev machine

`cargo run --example video_e2e -- <empty dir> 20` generates 20 s, 1080p clips (H.264+AAC in MP4/MKV/MOV, a portrait clip with a 90° display matrix, a clip without audio, VP9+Opus WebM, MPEG-4+MP3 AVI, HEVC+AAC) and runs every route through `VideoHandler`, checking each output with ffprobe (duration within 5%, displayed size, track count). Last run, FFmpeg 8.1.2 from PATH, RTX 4060 Ti: **89 of 89 pass** (including the refusal of audio extraction from the silent clip) and a cancelled VP9 encode stops in 1.55 s leaving nothing behind.

| Route (20 s, 1080p) | Time |
|---|---|
| any container change by stream copy | 0.1 to 0.5 s (one run showed 1.8 s while the machine was busy) |
| WebM → MOV, video re-encoded with `h264_nvenc` | 2 to 3 s |
| H.264 → WebM, VP9 `-deadline good -cpu-used 4` | 16 to 44 s (about 1x to 2x real time, and the dominant cost: a copy-only WebM exists only when the source is already VP8/VP9/AV1) |
| MP4/MKV/WebM/AVI → GIF | 2.5 to 16 s |
| audio extraction | 0.1 to 1.5 s |

These are one-machine numbers, not a benchmark claim. Software `h264_mf` was checked by hand on the installed FFmpeg; OpenH264 and QSV/AMF could not be exercised here.


## Where FFmpeg comes from

| Order | Source | Notes |
|---|---|---|
| 1 | `ZC_FFMPEG_DIR` | developer/test override |
| 2 | `<app data>/ffmpeg/<pin>/` | downloaded by the app, only used if `verified.sha256` equals the pinned hash |
| 3 | system | first `ffmpeg` + `ffprobe` pair on `PATH`, then (Windows) winget links, scoop shims, chocolatey bin, that answers `-version` within 5 s and has the encoders (`aac`, `libopus`, `libvpx-vp9`, `gif`, `pcm_s16le/24le`, one H.264 encoder from the LGPL list) and muxers the routes use; looked up once per process. The user's own build and license; never modified or deleted. Off Windows x64 (no download) a runnable one is taken even if it lacks something. |

If none qualifies, the app offers the download. `ffmpeg_status` reports `source` (`managed`, `system`, `override`) and `path`.

The download (`install.rs`): HTTPS only (Windows TLS stack), streamed to disk while hashing, compared with the pinned SHA-256 **before** anything is unpacked (mismatch deletes the file), then only `ffmpeg.exe`, `ffprobe.exe` and `LICENSE.txt` are extracted by base name into a staging folder that is renamed into place last. It starts only with the user's yes: from **Settings → Video → Download** at any time, or from the card shown before the first video job in the queue (`converterClient.ensureVideoEngine`). Rename-to-convert and the right-click menu cannot ask in place: they leave the file as it was and open the window at Settings → Video.

Pinned build: `PIN` in `pin.rs` (FFmpeg-Builds autobuild, formerly BtbN, `win64-lgpl`, static, FFmpeg 8.1.3). Size 167,647,897 bytes; installed size is about 260 MB (ffmpeg.exe and ffprobe.exe are static builds of ~130 MB each). **BtbN prunes old autobuilds**, so the pinned URL will eventually 404. The installer then falls back to `ffmpeg-n8.1-latest-win64-lgpl-8.1.zip` under the permanent `latest` tag and verifies it against that release's `checksums.sha256`, so a stale pin no longer blocks new installs. Still refresh the pin (URL, SHA-256, size, id) from a current `autobuild-*` release and re-run `cargo run --example video_e2e`.

## Licensing

* ZeroConverter itself is MIT. It does **not** link FFmpeg; it starts `ffmpeg.exe` as a separate process, which keeps the LGPL obligations with the FFmpeg distribution, not with this code.
* FFmpeg is **not bundled** in the installer. The user downloads it at their own request from the FFmpeg-Builds project (formerly BtbN); the build used is the **LGPL** variant (`win64-lgpl`), which excludes every GPL-only library (x264, x265, ...). The downloaded `LICENSE.txt` is stored next to the binaries.
* Code never selects `libx264`/`libx265`. H.264 encoders tried: `h264_nvenc`, `h264_qsv`, `h264_amf`, `h264_mf` (Media Foundation), `libopenh264` (Cisco OpenH264, BSD, binary patent license from Cisco applies to Cisco's own binaries only: the BtbN build compiles it from source). **H.264/AAC/HEVC are patent-encumbered formats**: stream copy involves no encoder, but any re-encode to H.264 or AAC is the user's responsibility in jurisdictions where patent licences are required. Hardware and Media Foundation encoders use the licensed implementations from the OS/GPU vendor.
* A user-supplied `ffmpeg` on PATH may be GPL; we never redistribute it. We never choose its x264 even if present.
* Source for the downloaded build: <https://github.com/FFmpeg-Builds/FFmpeg-Builds> (build scripts) and <https://ffmpeg.org/> (FFmpeg source, LGPL-2.1-or-later for this configuration).
