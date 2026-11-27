# Working on ZeroConverter

Instructions for anyone changing this code, and for the AI coding assistants many contributors use. Read it before a change; [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) has the details and [CONTRIBUTING.md](CONTRIBUTING.md) the process. If this file and the code disagree, the code wins; fix this file in the same change.

## What the project is

A desktop converter (Tauri 2, Rust, React) for Windows first. Every conversion, from the queue, the command line, a rename in Explorer or the right-click menu, goes through one engine: `src-tauri/crates/zeroconverter-core`. The engine is organised by **family** (image, audio, video), and each family by **format module**. There are no pair converters (`jpg_to_png.rs`): a route exists when the source can be read and the target can be written.

## Where code goes

| Layer | Path | Its job | Never |
|---|---|---|---|
| UI | `src/components/` | Show state, collect intent | Import Tauri, decide routes, build output paths, validate options |
| Application | `src/application/` | Stores and use cases | Import `@tauri-apps/*` directly |
| Gateway | `src/platform/tauri/` | The only place that calls `invoke()` and Tauri plugins | Hold business rules |
| API | `src-tauri/src/api/` | Tauri commands: parse, call core, map to DTOs | Convert, or list per-format options |
| Engine | `src-tauri/crates/zeroconverter-core/` | Detection, validation, conversion, jobs | Depend on Tauri or the UI |
| Disk | `src-tauri/crates/zero_fs/` | How a finished file reaches its name | Be bypassed by a handler |
| Desktop | `src-tauri/src/rename_convert/`, `context_menu.rs`, `tray.rs`, `settings.rs`, `updater.rs` | Explorer, tray and OS integration | Contain conversion logic |
| CLI | `src-tauri/crates/zeroconverter-cli/` | The `zeroconverter` command and its JSON contract | Change the v0 schema in place |

The boundaries are enforced: `npm run check:frontend-boundaries` for the frontend, `architecture_tests.rs` for the engine. Do not weaken a check to make a change pass; move the code instead.

## Rules that are not negotiable

1. **The engine decides.** What can be read, written and converted comes from the descriptors and `get_app_capabilities`. The UI shows that list; it never keeps its own.
2. **Check, then convert.** `ConverterRegistry::validate` runs before `convert`. Validation has no side effects: no files, no temp folders, no state changes.
3. **The user's files are never at risk.**
   - Handlers write into the scratch folder they are given and finish with `zero_fs::write_output` or `zero_fs::commit`. Never write to the destination directly, never delete by name afterwards.
   - Never overwrite unless the request says so (`ConversionRequest::clobber()` is the only place that reads it).
   - A cancel or a failure leaves everything as it was.
   - Rename-to-convert checks the file's identity at every step (`rename_convert/apply.rs`). Keep that: a file that changed, or whose identity cannot be read, is left alone.
4. **Detection is by content.** A format's `sniff` checks the header bytes, strictly. An extension never overrides a header that says otherwise.
5. **Bounded work.** Batches use a fixed number of workers. Nothing spawns a thread or process per file without a limit, and nothing blocks the UI thread.
6. **Errors are typed.** Use `AppError` variants with a stable `code()`; no `unwrap`/`expect` on paths a user's file can reach. Messages say what happened and what to do, in plain words.
7. **Offline by default.** The FFmpeg download (`handlers/video/install.rs`) and the update check are the only network use. No telemetry, no CDNs, no external fonts.
8. **Licenses.** The app is MIT. No GPL or AGPL dependencies; LGPL only after review in an issue. FFmpeg is never linked or bundled: it runs as a separate program. Every new dependency needs a reason, a pinned version and its license in the pull request, and `node scripts/licenses.mjs --check` must pass.
9. **The CLI contract is versioned.** `docs/CLI.md` and `docs/schema/` describe `zeroconverter.job/v0`. Add fields only in a backwards-compatible way; anything else is a new version.

## Common changes

- **A new image or audio format:** one module and one line in `MODULES`. Follow [Adding a format](CONTRIBUTING.md#adding-a-format); `handlers/image/formats/qoi.rs` is the model. Nothing outside the family should need to change.
- **A new option:** read and check it in the family that uses it (`ImageOptions`, `AudioOptions::parse`, the video handler), then declare it with an `OptionSpec` on the formats that honour it and label it (`option.<key>`) in the catalogs. The panel, the `+` window and the CLI show it from the declaration; the UI never keeps its own list. The API passes options through untouched.
- **A new family:** open an issue first. It touches the registry, capabilities and the UI, and the design should be agreed before code.
- **Desktop behaviour (rename, menu, tray):** automated tests cannot reach Explorer. Run the steps in [docs/SMOKE_TEST.md](docs/SMOKE_TEST.md) that your change touches and list them in the pull request.

## How to work

- **Keep a change to one purpose.** A format, a bug fix or a refactor, not all three. Do not reformat or rename code you did not need to touch.
- **Read before you write.** Find the existing helper before adding one; most things (naming, temp folders, limits, error mapping) already exist once.
- **Tests come with the change.** A format needs a round trip, a damaged file and a header over the pixel budget. A bug fix needs a test that failed before it. Happy-path-only tests are not enough.
- **Run the checks** before you say it works:

  ```powershell
  powershell -NoProfile -ExecutionPolicy Bypass -File .\docs\scripts\verify-local.ps1
  ```

  This is what CI runs (add `-SkipSecretScan` if Gitleaks is not installed). Report what you ran and what you could not run.
- **Keep docs true.** If you change how something works, update `docs/ARCHITECTURE.md` (or the relevant doc) in the same pull request.
- **Never commit** secrets, signing keys, personal paths, or users' files. Test inputs are small and synthetic.

## For AI assistants

- Follow this file over general habits. If a request conflicts with a rule above, say so instead of working around it.
- Do not invent APIs, files or formats. Check that a function exists before calling it, and that a claim about the code is true before writing it in a comment or a pull request.
- Do not mark work done until the checks have run and passed; if they could not run, say which and why.
- Prefer a small, complete change over a large partial one. Leave unrelated code alone.
