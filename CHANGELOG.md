# Changelog

## Unreleased

- Frontend dependencies updated past all reported npm advisories (#8): Vite 7.3.8, PostCSS 8.5.33 and their build tooling; i18next 27 with react-i18next 18. `npm audit` reports none.

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
