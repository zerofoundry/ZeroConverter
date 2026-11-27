# Architecture

A short map of the source tree as it is now. If this file and the code disagree, the code wins; please fix this file.

## Layers

```
src/components/        React UI. No Tauri imports (checked by npm run check:frontend-boundaries).
src/application/       Stores and use cases. Talks to the backend only through src/platform/tauri/.
src/platform/tauri/    The one place that calls invoke() and Tauri plugins.
───────────────────── IPC ─────────────────────
src-tauri/src/api/     Tauri commands and DTOs. Thin: parse, call core, map the result.
src-tauri/crates/zeroconverter-core/  The engine: conversion rules, jobs, probing. Its own crate, no Tauri
                       (checked by architecture_tests.rs). The app and the CLI both use it.
src-tauri/crates/zeroconverter-cli/   The `zeroconverter` command line (docs/CLI.md).
src-tauri/crates/zero_fs/  How a finished output reaches the disk.
src-tauri/src/rename_convert/, context_menu.rs, tray.rs, settings.rs, updater.rs   Desktop integration.
```

The backend is authoritative. The UI asks `get_app_capabilities` what can be read, written and converted, and shows only that. It never decides a route, an output path or whether an option is valid.

## What other programs can rely on

In v0.1 the supported way for another program to use ZeroConverter is the command line and its JSON contract ([docs/CLI.md](CLI.md), schemas in `docs/schema/`). The Rust crates (`zeroconverter-core`, `zero_fs`) are internal to this workspace and not published: you may reuse their source under the MIT license, but their `pub` items carry no compatibility promise yet.

## One conversion, one lifecycle

Every conversion, whether it comes from the queue, the command line, a rename in Explorer or the right-click menu, ends in `core::conversion::pipeline::convert`:

1. The caller checks the request first: `ConverterRegistry::validate` asks the family's handler (image, audio, video) whether the route and the options are valid. The queue does this in `src-tauri/src/api/bridge.rs` and the job pipeline, the command line in `zeroconverter-cli/src/main.rs`, rename-to-convert in `rename_convert/apply.rs`. `probe` is separate: it reads an input's metadata for the UI and the command line (`core/probe/`) and is not a step of every conversion.
2. `convert` gives the handler a `zero_tmp_*` scratch folder next to the destination (`zero_fs::TempWorkspace`, same volume) and the handler's `execute` writes there. Only when it finished and the job was not cancelled is the result moved to its final name with `zero_fs::commit`. A cancel or a failure leaves the destination as it was; nothing is ever deleted by name afterwards. If a file took the destination name meanwhile, it is kept and the job says so.
3. `AppError::Cancelled` is the one way to say "cancelled"; the frontend checks for the word `CANCELLED`.

The job manager (`core/jobs/`) owns queueing, a concurrency ceiling and progress events. From the moment a job is created until it ends, it holds its input with `zero_fs::InputGuard` when the file can be opened that way. It is best effort: if the file is already open for writing elsewhere, the job runs without it. On Windows others may still read a guarded file, but nothing can delete, rename or replace it; on other systems the guard is a plain open file and blocks nothing. Output names are reserved per job, and another job's input is never given out as an output name (the job takes a free name or is refused), so one job never writes over a file another job is still reading. Events go out through listeners registered by the API layer, so core does not know about Tauri channels.

## Writing outputs

Handlers never write to the destination directly. They call `zero_fs::write_output(path, bytes, clobber)`, or encode to a temp file and call `zero_fs::commit(temp, dest, clobber)`:

- `Clobber::Never` moves the finished file with `zero_fs::rename_no_replace`, which succeeds only if the name is free at that moment: `MoveFileExW` without the replace flag on Windows; elsewhere a hard link and then removing the temporary name. Where hard links are not possible it fails rather than check first and rename after, which could replace a file that appeared in between.
- `Clobber::Replace` replaces atomically.

`ConversionRequest::clobber()` is the only place that reads the user's overwrite flag. A crash or a cancel never leaves a half-written file at the destination.

## Formats

Each format is one module, registered with one line in a table:

- **Images:** `core/handlers/image/formats/`. An `ImageFormatModule` has a descriptor, a strict `sniff` on the header bytes, a header-only `dimensions` (checked against the pixel budget before decoding), and optional `decode` / `encode`. `raster.rs` holds the everyday formats (PNG, JPG, WebP, BMP, TIFF, GIF, ICO); the others have a file each. Signatures are tried in `MODULES` order, so the weakest (ICO) is last. SVG and PDF are not modules but routes (`SPECIAL_FORMATS` in `image/mod.rs`): SVG is rendered at a target size, PDF is written from the input file.
- **Audio:** `core/handlers/audio/formats/`. Decoding goes through Symphonia into PCM (`pcm.rs`), and each writable format has an `encode`.
- **Video:** `core/handlers/video/`. FFmpeg, run as a separate program: an FFmpeg already installed (PATH, winget, scoop, chocolatey) is used when it runs and has every encoder and muxer the routes need (`ffmpeg.rs`); otherwise a pinned LGPL build is downloaded on first use (`install.rs`, the only module allowed to use the network). `plan.rs` decides between stream copy and re-encode.

A test in `registry.rs` checks that every readable extension belongs to exactly one family.

## Options

The engine declares what can be set for each target format, and every caller shows that list:

- **Declared:** an `OptionSpec` (`core/conversion/options.rs`) names the request key, the control (slider, number with presets, choice, toggle), its bounds and the engine's own default, whether a chosen value is shared by the family (resize, sample rate) or kept per format (quality, bitrate), whether the `+` window offers it, and when it is in play (`show_when`: resize width only for "max width" and "fit inside box"). A format module lists the specs its encoder honours (`ImageFormatModule::options`, `AudioFormatModule::options`; video in `VideoHandler::output_options`), and `ConverterHandler::output_options(target)` returns them in display order. A key that is not listed is ignored for that target.
- **Shown:** `get_app_capabilities` carries the specs on each writable descriptor (`options`), the `+` window gets them in `QuickJobInfo`, and `zeroconverter capabilities` reports them per format and summarised per family. In the app, `src/application/outputOptions.mjs` keeps the user's values (saved per scope in the settings) and builds each request from the specs; `components/options/OptionFields.tsx` draws any spec, labelled from `option.<key>` in the catalogs. The panel, the `+` window and the inspector have no list of formats or options of their own.
- **Checked:** options are not typed out in the API layer. `ConversionSubmitDto` flattens every field it does not know into one JSON map, and the request carries it as-is. Each family reads and checks its own options (`ImageOptions`, `AudioOptions::parse`, video quality), and a bad value is a `Validate` error. A test next to each family (`specs_agree_with_validation`) keeps the two in step: every default, preset and choice a spec offers validates, and every value past its bounds is refused.

Adding an option: read and check it in the family, add an `OptionSpec` to the formats that honour it, and give it a label (`option.<key>.label`) in every catalog. The panel, the `+` window, the inspector and the command line pick it up from there.

## Rename-to-convert

`src-tauri/src/rename_convert/` watches the fixed drives. When a file's extension is renamed to another format the app can write (including `<stem>.<target>.<ext>` when Explorer hides file extensions), the file is converted and the original name comes back.

- **Who renamed it:** outside the folders the user trusts, a rename only counts when a file manager is in the foreground, so a program renaming its own files is not mistaken for the user.
- **Marks in the new name:** `&` reverses whether the original stays (keeps it by default, or sends it to the Recycle Bin when **Keep the original file** is on in Settings), `@N` limits the longer side to N pixels (images), `+` opens the options window. They can come in any order.
- **Folders:** renaming a folder to `Name.webp` converts the files inside it, up to 2,000. The folder gets its name back first, but only after Explorer has announced its own rename to the shell (or 1.5 s without a notice), and the shell is told about the rename back (`shell_notice.rs`). A folder renamed again sooner makes Explorer show "The file or folder does not exist" or keep the old label.
- **Right-click menu:** `context_menu.rs` writes a per-user "ZeroConverter" submenu (HKCU, no admin) that runs `--convert <format> "<file>"`. A running instance takes the request over. It converts a copy next to the file and works whether rename-to-convert is on or off. The uninstaller removes the menu.

How a renamed file is swapped (`rename_convert/apply.rs`), so that the user's file is never lost:

1. When the file can be read, its identity is taken: volume and file index, size, last write, and a fingerprint of its first and last 4 KB (on FAT drives the index can repeat and the time is coarse). Every later step checks it. A file that was replaced or written to meanwhile is left alone, and so is a file whose identity cannot be read.
2. A hidden `.zc-*` folder next to the file holds a hard link to it (a copy where the volume has no hard links), named with the source extension, and the converted output.
3. The swap is two moves that never replace a name: the renamed file to a free name (its original one when possible), then the output to the final name. If the second move fails, the first is undone. Before moving a file from an Explorer rename event away (for the swap or to restore its old name on failure), `apply.rs` waits in parallel with conversion for Explorer's shell rename notice (`SHCNE_RENAMEITEM` via `shell_notice.rs`, bounded by the same 1.5 s / 0.3 s window as folders) so Explorer does not report a missing file.
4. Unless the job keeps the original (from the **Keep the original file** setting XOR `&`), the original goes to the Recycle Bin only if it is still the file that was converted.
5. The hard link in the scratch folder is a second name for the original. It is removed with the folder only when the original is confirmed in place or in the bin. If it has become the last name, it is moved back into the folder under a visible free name. If even that fails, the folder is kept.
6. Single renames and menu copies convert at most as many at a time as the machine has cores; the rest wait under their new name. A folder conversion has its own pool of the same size.
7. Turning rename-to-convert off, quitting, or an update stops every job that has not swapped yet (menu copies stop only on quitting); the file gets its old name back. Quitting waits up to five seconds for a swap under way. On the Desktop the new file is put back where the old icon was (`desktop.rs`, best effort).
8. When a finished rename conversion lands at a different name from the one typed (`@N`, `+`, `&`), restores a kept original next to a plain rename, or reverts a file back to its old name, `shell_notice.rs` notifies the shell (`SHCNE_RENAMEITEM`, `SHCNE_CREATE` for newly created or restored files, and `SHCNE_UPDATEDIR`) right away and once more after 1.5 s off the rename thread so Explorer updates immediately and drops any stale typed label.

## Updates

`updater.rs` uses the Tauri updater. Release builds read `latest.json` from the newest GitHub Release once a day (setting on by default) and say when a newer version exists. The installer is downloaded and run only when the user chooses Download and restart. The installer must carry a signature from the project's private key; the public half is in `tauri.conf.json`. `scripts/release.mjs` builds, signs and writes `latest.json`.

## Languages

Text lives in `src/i18n/locales/<code>.json`, one file per language (English is the reference). Components call `t("section.key")` from `react-i18next`; code outside components uses `i18n.t`. The language is chosen once at start: the saved choice (`localStorage`, key `zeroconverter-language`, shared by the main, `+` and notification windows), else the Windows display language, else English. A key a language lacks falls back to English. `src/i18n/locales.ts` lists the languages; Arabic sets `dir="rtl"` on `<html>`.

- Format names and extensions (PNG, DDS, `.webp`…) are never in a catalog; they are passed in as `{{format}}` values.
- Plurals use i18next suffixes (`_one`, `_few`, `_many`, `_other`, and `_zero`, `_two` for Arabic); each language needs the forms its CLDR rules use.
- `npm run test:i18n` (also part of `npm run test:ui`) checks that every catalog has the same keys, the same `{{placeholders}}`, the right plural forms and the product names that must stay, and that every `t("…")` in `src/` exists.
- Under right to left the shell regions keep their places (window buttons stay top right) and the text inside them flows right to left (`index.css`, "Languages").
- The Rust side (tray menu, Explorer right-click entry, Windows notices, notification cards) reads the same catalogs: the `native` section, embedded with `include_str!` in `src-tauri/src/i18n.rs`. The window reports its language with `set_ui_language`; a change rebuilds the tray and re-writes the right-click entries. Open windows follow a change through the `storage` event. The installer offers 11 languages (NSIS has no Hindi).
- Not translated: the text the engine returns in errors and progress (`AppError` messages are English; the window shows them as they come).

## Tests

- `cargo test --workspace` runs the Rust tests, including the format round trips, the architecture guards and hostile-input cases.
- `npm run test:ui` covers the frontend format catalog; `npm run check:frontend-boundaries` covers the layering rules above.
- `docs/scripts/verify-local.ps1` runs everything. CI (`.github/workflows/ci.yml`) runs the same checks on Windows.

## Known debt

- File identity is still decided partly by extension (`core/sniff.rs`) instead of each family's own `identify`.
- `converterStore.ts` is large and could be split.
- `ConversionResult.success` is redundant with `Result`.
- `api/bridge.rs`, `rename_convert/mod.rs` and `rename_convert/apply.rs` are large and could be split, each as a move behind the existing tests.
- A result does not say whether its output name was deduplicated (`name (1).png`) or replaced an existing file; the inspector cannot show it yet.
