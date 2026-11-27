# Contributing to ZeroConverter

> **License.** ZeroConverter is released under the [MIT License](LICENSE). By submitting a contribution you agree that it is released under the same license.

## Workflow

1. Open a focused issue describing the affected format, intended input/output route, expected and actual outcomes, and whether the file is confidential.
2. Keep changes small and respect `src-tauri/crates/zeroconverter-core/` as the source of conversion rules. The frontend collects intent; it does not guess conversion eligibility, output paths or validation.
3. Every new format needs round-trip, malformed-input and over-budget tests (see [Adding a format](#adding-a-format)). Avoid process-per-conversion, unnecessary intermediate copies and unbounded allocations.
4. Work offline by default. The FFmpeg download and the update check are the only network use; do not add more, or telemetry, external fonts/CDNs or checked-in third-party binaries.
5. Record every new dependency's pinned version, SPDX license, native build requirements and redistribution effect. GPL/AGPL dependencies are out of scope; LGPL requires explicit legal review.
6. Run the Windows-first [local verification script](docs/scripts/verify-local.ps1) before requesting review, or state exactly what could not run and why.

## Development

The one command that runs every check, the same ones CI runs on each pull request (`.github/workflows/ci.yml`):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\docs\scripts\verify-local.ps1
```

The same checks one by one:

```sh
npm ci
npm run build
npm run check:frontend-boundaries
npm run test:ui
node scripts/licenses.mjs --check
cd src-tauri
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --no-fail-fast --locked
cargo check -p zeroconverter-core -p zeroconverter-cli --no-default-features --locked
```

The Rust/Tauri build expects the frontend `dist/` directory: run `npm run build` from the repository root first. `verify-local.ps1` also runs a secret scan with Gitleaks; without Gitleaks installed, add `-SkipSecretScan` and say so in the pull request.

[AGENTS.md](AGENTS.md) lists where code goes and the rules every change keeps. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) explains how the pieces fit together.

## Using AI tools

You are welcome to use AI assistants. Most of them read [AGENTS.md](AGENTS.md) on their own; point yours at it if it does not. The same bar applies as to any change:

- **You are the author.** Read and understand every line before you open a pull request, and be ready to explain it in review.
- **Run the checks yourself** and paste the real summary. "Tests pass" written by a tool that did not run them is not enough.
- **Keep it focused.** One purpose per pull request. Large generated rewrites, mass reformatting and drive-by renames will be closed.
- **Check what it claims.** Functions, file paths, format details and license statements in code, comments and the pull request description must be real.
- **Never paste** other people's files, private paths or keys into a prompt or a pull request.

## Translating

Every interface text is in `src/i18n/locales/<code>.json`. To fix or improve a translation, edit that file and run `npm run test:i18n`. To add a language, add its code and native name to `src/i18n/locales.ts`, import its catalog in `src/i18n/index.ts`, add the code to `CODES` in `scripts/i18n/validate-catalogs.mjs`, and copy `en.json` as a start. Keep it short and natural, not word for word. Never translate format names or file extensions.

## Adding a format

**An image format** is one file and one line. `qoi.rs` is the example to follow:

1. Copy `src-tauri/crates/zeroconverter-core/src/handlers/image/formats/qoi.rs` to `yourformat.rs` and fill in:
   - the **descriptor**: id, canonical extension, aliases, whether it can be read and written;
   - **`sniff`**: a strict check on the first 64 bytes. A false positive steals other formats' files;
   - **`dimensions`**: width and height from the header only. It runs before decoding, so the pixel budget (`image_limits.rs`) can refuse a huge file without allocating it;
   - **`decode`** and/or **`encode`**: `encode` returns the whole file as bytes; the caller writes them safely.
2. Add `pub mod yourformat;` and one line in `MODULES` in `formats/mod.rs`. Order matters only for weak signatures: put yours before ICO.
3. Add tests next to the module, as `qoi.rs` does: a round trip, a file cut inside its header, and a header that claims more pixels than the budget allows.
4. Run the checks (see [Development](#development)). These tests fail if something is off:
   - `formats/mod.rs` and `registry.rs`: the extensions collide with another format;
   - `sniff.rs` (`every_writable_image_module_is_recognised`): your own output is not recognised;
   - `zeroconverter-cli/tests/cli.rs`: the command line can see QOI the same way it will see yours.
5. Try it: `cargo run -p zeroconverter-cli -- capabilities` lists the format, and a job with `"format": "yourformat"` converts a PNG.

Probing, validation, the queue, the command line, rename-to-convert, the right-click menu and the capabilities the UI shows all pick it up from the table.

**An audio format** lives in `zeroconverter-core/src/handlers/audio/formats/`: a module with a descriptor and one line in `MODULES`.

- **Read only:** reading goes through Symphonia (`pcm.rs`), so the format can be read only if Symphonia's features in `zeroconverter-core/Cargo.toml` include its container and codec. The module then needs just the descriptor with `can_write: false` and `encoder: None`.
- **Writable:** add an `encoder` that takes the decoded PCM and the checked `AudioOptions` and writes the file. Options are read in `AudioOptions::parse`; a key that only some formats use is listed with those formats in the command line's capabilities.
- **Written by FFmpeg:** `can_write: true` with `encoder: None` (M4A, AAC). `AudioFormatModule::via_ffmpeg` sends these to the video family's audio path, so the target must also be an `AudioTarget` in `handlers/video/formats.rs`; a test checks that.
- **Recognising the file:** audio signatures are in `zeroconverter-core/src/sniff.rs`, in the hand-written part. Add yours with a test next to the others, or rename-to-convert and the menu will not offer the format.
- **Tests:** a round trip through `AudioHandler` (see the existing format modules), a damaged file, and the options it accepts and refuses. The test in `formats/mod.rs` fails if a name is claimed twice.

**An option** (a quality setting, a bitrate) is read and checked where it is used: `ImageOptions`, `AudioOptions::parse` or the video handler. Then declare it: an `OptionSpec` (`conversion/options.rs`) in the format modules whose encoder honours it, with the same bounds and default as the check, and a label under `option.<key>` in every catalog in `src/i18n/locales/`. The app's panel, the `+` window and `zeroconverter capabilities` show it from there; the UI and the API layer never list options themselves. The family's `specs_agree_with_validation` test fails if the spec offers a value the check refuses. See [Options](docs/ARCHITECTURE.md#options).

## Trying the desktop app

Automated tests do not cover Explorer. Before a pull request that touches rename-to-convert, the right-click menu or the tray, run through [docs/SMOKE_TEST.md](docs/SMOKE_TEST.md) on Windows and say which steps you ran.

## Pull requests

Provide a concise rationale, changed-file list, the command/output summary (not just “tests pass”), new dependency licenses, and any remaining known limitations.

## Sensitive assets and security issues

Never attach proprietary customer files, personal paths, API keys or large binary test fixtures to public issues. Use a reproducible minimal synthetic input. For a suspected vulnerability, see [SECURITY.md](SECURITY.md); do not publicly disclose exploit details.
