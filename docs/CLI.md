# Command line

`zeroconverter` runs the same engine as the app, driven by a JSON job file. It is meant for scripts, build pipelines and other tools.

> **Interface v0, experimental.** The shape below can still change before v1. Within a major version, fields and error codes are only ever added, never renamed or removed. `zeroconverter version --json` tells you which version you are talking to.

The installer puts it in the `cli` folder next to the app; each release also has it as a separate download.

## Commands

```
zeroconverter version [--json]
zeroconverter capabilities [--json]
zeroconverter validate --job <job.json>
zeroconverter run --job <job.json>
```

stdout carries JSON only, one object per line. Messages for people go to stderr. `--json` is accepted for clarity; the output is always JSON.

## First conversion

In PowerShell, in an empty folder, with `zeroconverter.exe` on `PATH` (or use its full path). This makes a 2×2 PNG, checks the job and runs it:

```powershell
[IO.File]::WriteAllBytes("$PWD\red.png", [Convert]::FromBase64String('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR42mO4pKsLRAwQCgAjcgSxctmVjAAAAABJRU5ErkJggg=='))
Set-Content job.json '{"schema":"zeroconverter.job/v0","inputs":["red.png"],"format":"webp"}'
zeroconverter validate --job job.json
zeroconverter run --job job.json
```

`validate` prints one `result` line with `"status":"valid"` and writes nothing. `run` prints `job_started`, `item_started`, `item_finished` and finally a `result` line with `"summary":{"cancelled":0,"failed":0,"ok":1}`; `red.webp` is next to `red.png` and the exit code is 0. From a source checkout, `cargo run -p zeroconverter-cli -- run --job job.json` in `src-tauri` does the same.

## Job file

```json
{
  "schema": "zeroconverter.job/v0",
  "inputs": ["textures/rock.png", "textures/sand.png"],
  "format": "webp",
  "output_dir": "out",
  "options": { "quality": 90 },
  "overwrite": false,
  "confirm_large": false
}
```

| Field | | |
|---|---|---|
| `schema` | required | `zeroconverter.job/v0` |
| `inputs` | required | Files to convert. Relative paths are relative to the current directory. |
| `format` | required | Output format id or extension, as listed by `capabilities`. |
| `output_dir` | optional | Must exist. Default: next to each input. |
| `options` | optional | Format options, the same ones the app uses: `quality` (1–100), `resize_mode` (`none`, `max_width`, `max_height`, `fit_box`), `resize_width`, `resize_height`, `bitrate_kbps`, `sample_rate`, `channels` (only 1, for mono), `dds_format`, `dds_mipmaps`, `webp_mode` (`lossless`, the default, or `lossy` at `quality`), `max_height` (video: scale a taller video down, 144–4320). Each format family checks its own and rejects bad values. Unset: `quality` 90, `resize_mode` `none`. `capabilities` lists the keys per family and which formats use them, and each format's own `options` with its bounds and default. |
| `overwrite` | optional | `true` replaces an existing output. Default `false`: a clash gets `name (1).ext`. The input itself is never overwritten. |
| `confirm_large` | optional | Inputs over 100 MB convert only with `true`. |

Unknown top-level fields are an error, so a typo there does not silently do nothing. A JSON Schema for the job file is in [schema/zeroconverter.job.v0.json](schema/zeroconverter.job.v0.json). Inside `options`, each format family reads the keys it knows and ignores the rest.

## Output

`version` reports the tool version, the git hash it was built from (`build`, or `"unknown"`), the interface version and the schema names. `capabilities` lists, per family, the formats, the routes and the option keys (with `formats` when only some outputs use a key, and `min`/`max`, `values` and `default` when they agree across them; each writable format's `options` has the full list it honours, in the order the app shows them), plus the job schemas the tool accepts and its `effects` (network, files, child processes, environment), the same facts as the sections below.

`run` writes JSON lines. The set of events is fixed for v0:

| Event | When |
|---|---|
| `job_started` | First line: `schema` (`zeroconverter.event/v0`), tool version, number of items. |
| `item_started` | An input begins. |
| `progress` | Long items (video): `percent` and a short `message`. |
| `warning` | Reserved; not emitted yet. |
| `item_finished` | An input is done; carries the same item as the result. |
| `result` | Always the last line. |
| `error` | The job could not start (`invalid_job`): this is the only line, and the exit code is 2. |

For a job that was read, the last line is always `result`, and every input gets one `item_finished` line, including items that were cancelled before they started. New fields can appear in any event, so read the fields you need and ignore the rest. Every free-text `message` is at most about 4 KB. If stdout cannot be written, the exit code is 4.

```json
{"event":"result","schema":"zeroconverter.result/v0","tool":{"name":"zeroconverter","version":"0.1.0"},"command":"run",
 "items":[{"index":0,"input":"textures/rock.png","status":"ok","output":{"path":"out/rock.webp","rel_path":"rock.webp","bytes":48211,"sha256":"…"},"warnings":[]},
          {"index":1,"input":"textures/sand.png","status":"failed","retryable":false,"error":{"code":"unsupported_input","message":"…"}}],
 "summary":{"ok":1,"failed":1,"cancelled":0}}
```

`output.rel_path` is the output relative to `output_dir` (or to the input's folder when there is none), with `/` separators. Items that did not succeed carry `retryable`: `true` when running the same item again can work (`cancelled`), `false` when it cannot without changing the job (`consent_required`, `invalid_request`, `unsupported_input`, `no_route`), and `"unknown"` when it depends on the file or the machine (`io`, `conversion_failed`, `internal`).

For images and audio, the same input, options and tool version give the same output bytes, so the `sha256` can be compared between runs. Video goes through FFmpeg and is not guaranteed to be byte-identical. The hash is of the file ZeroConverter wrote, nothing more. Inputs are not hashed.

## Dry run

`validate` looks at each input's name and size, checks the options and the route, applies the 100 MB consent rule and plans each output path, all without writing anything. It does not decode the files, so a damaged file can be `valid` and still fail in `run`. Its result lists each item as `valid` (with the planned `output.path` and `rel_path`) or `invalid` (with the error and `retryable`). A later `run` can still pick a different name if a file appeared in between.

## Re-runs

Running the same job again does not overwrite earlier outputs unless `overwrite` is `true`: a clash gets `name (1).ext`, `name (2).ext` and so on. To repeat a job in place, set `overwrite`. Two inputs in one job that would land on the same name get distinct names.

## Partial output

An output is written to a temporary file next to it and renamed into place only when it is complete, so a crash, an error or a cancel never leaves a half-written file under the output name. With `overwrite`, the old file is replaced only by a finished new one.

## Temp files

While an item runs, the CLI may create, in the output folder:

- `<output name>.tmp.<unique>`: the output being written.
- `zero_tmp_<unique>/`: a scratch folder for multi-step conversions.

Both are removed when the item ends, whether it succeeded or not. If the process is killed outright (not cancelled), they can be left behind and are safe to delete.

## Cancelling

Ctrl+C or Ctrl+Break in the console (SIGINT, SIGTERM or SIGHUP on other systems) cancels the job. Closing the console window ends the process instead, which can leave temp files behind. The item in progress stops and leaves no new partial file, the rest are reported as `cancelled`, and the exit code is 3. Image and audio encodes finish the file they are on before stopping.

## Environment

| Variable | Use |
|---|---|
| `APPDATA` | Finds the FFmpeg the app downloaded (`%APPDATA%\com.zerofoundry.zeroconverter`). |
| `PATH` | Finds `ffmpeg` and `ffprobe` when the app has not downloaded them. |
| `LOCALAPPDATA`, `SCOOP`, `USERPROFILE`, `ChocolateyInstall`, `ProgramData` | Windows only: also look for a system FFmpeg in the winget links, scoop shims and chocolatey bin folders. |
| `ZC_FFMPEG_DIR` | Development override: a folder holding `ffmpeg` and `ffprobe`, used first. |

Nothing else is read. Video is the only family that starts other programs (`ffmpeg`, `ffprobe`).

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Everything succeeded. |
| 1 | Some items failed (the result still lists every item), including an item that failed with `internal`. |
| 2 | The job file or the command line is invalid (an unknown command, a missing or repeated `--job`, or any extra argument). |
| 3 | Cancelled (see Cancelling). Finished items stay. (With `overwrite`, an output that already replaced the old file is kept and reported.) |
| 4 | The tool itself failed (for example, stdout could not be written). |

## Error codes

| Code | Meaning |
|---|---|
| `invalid_job` | The job file could not be read or does not match the schema. |
| `io` | A file could not be read or written. |
| `unsupported_input` | The input is not a format ZeroConverter reads, or it is damaged. |
| `invalid_request` | The options or the route are not valid for this input. |
| `no_route` | No converter for this input family or format. |
| `consent_required` | The input is over 100 MB and the job does not set `confirm_large`. |
| `conversion_failed` | The converter failed on this file. |
| `cancelled` | Stopped by the user. |
| `internal` | A bug; please report it. |

## What the CLI never does

- It does not touch the network. Video needs FFmpeg, which the app downloads on first use; the CLI uses that copy (or a usable system FFmpeg, see `docs/VIDEO.md`) and fails with an error if there is none.
- It writes nothing but the outputs and the temp files above: no logs, no settings, no side files.
- It does not know about any host application. Anything that wants to drive ZeroConverter (a build script, a pipeline tool, an editor plugin) uses this same interface.
