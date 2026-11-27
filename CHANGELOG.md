# Changelog

## Unreleased

- M4A and AAC output for audio files (#9): rename `song.mp3` to `song.m4a`. Encoded with FFmpeg's AAC encoder at 192 kbps, or stream-copied when the source is already AAC; needs FFmpeg like video does.
- Frontend dependencies updated past all reported npm advisories (#8): Vite 7.3.8, PostCSS 8.5.33 and their build tooling; i18next 27 with react-i18next 18. `npm audit` reports none.
- The options panel shows what each output format can actually be set to, as the engine declares it: video quality for MP4, MKV, MOV and WebM; bitrate, sample rate and mono when a video's audio is saved as MP3, Opus, FLAC or AIFF; and per-family sections in a mixed batch. The `+` window and the details pane use the same list, and only the options a format honours are sent with a conversion.
- WebP can be written lossy: Compression › Lossy with a quality slider (lossless stays the default). Encoded with libwebp.
- M4A and AAC take a bitrate, sample rate and mono, in the app, the `+` window and the command line; with none set, AAC audio is still copied as it is.
- Video: Resolution scales a taller video down to 2160p, 1440p, 1080p, 720p or 480p (MP4, MKV, MOV, WebM); a smaller video is never scaled up. Audio saved from a video as WAV takes a sample rate and mono.
- The same defaults everywhere: image quality starts at 90 in the app as it does for rename and the command line (it was 80 in the app). Settings from 0.1.x carry over.
- Opus no longer offers a sample rate it would not use (Opus always runs at 48 kHz), and its bitrate defaults to "Automatic" (64 kbps mono, 128 kbps stereo), which is what it did.
- A DDS compression the engine does not know is refused before converting, not during.
- `zeroconverter capabilities`: each format lists its `options`; family summaries add `min`/`max` and name the `formats` a key applies to.

## 0.1.1

- Settings › Convert on rename › Keep the original file (#7): option to keep originals on disk without typing `&` (`&` then sends that one original to the Recycle Bin instead).
- Explorer refreshes the file list after `@N`, `&`, and `+` renames instead of keeping the typed name on screen (#4), and shows the restored original when keeping originals.
- Waits for Explorer to finish a file rename before swapping or restoring the file, preventing transient "file not found" errors.
- Works when Explorer hides file extensions, and extracts audio when renaming `.mp4` / `.m4v` to `.m4a`.
- Explains when a renamed file can't be read or isn't the format its name says, instead of doing nothing.

## 0.1.0

First public release.

- Interface in 12 languages (English, Türkçe, Deutsch, Español, Français, Português, 日本語, 한국어, 简体中文, Русский, हिन्दी, العربية), chosen from Windows and changeable in Settings.
- Converts 26 image, texture, HDR and audio formats offline, plus video through FFmpeg (downloaded on first use).
- Batch queue with per-file progress and cancel.
- Rename to convert: change a file's extension in Explorer and it is converted. `&` keeps the original, `@N` resizes, `+` opens options; renaming a folder converts its files.
- Right-click **ZeroConverter** menu on supported files.
- Video container changes copy streams instead of re-encoding.
- Tells you when a new version is on GitHub Releases and installs it when you say so (signed; the check can be turned off).
- Files over 100 MB convert only after you confirm; a folder convert skips them.
- `zeroconverter` command line: `version`, `capabilities`, `validate`, `run` with a JSON job file, JSON-lines output, stable exit and error codes (interface v0, experimental).
