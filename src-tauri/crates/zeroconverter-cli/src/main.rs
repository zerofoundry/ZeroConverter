//! `zeroconverter`: the same engine as the app, driven by a JSON job file.
//!
//!   zeroconverter version [--json]
//!   zeroconverter capabilities [--json]
//!   zeroconverter validate --job <job.json>
//!   zeroconverter run --job <job.json>
//!
//! stdout carries JSON only (one object per line); messages for people go to
//! stderr. The interface is described in docs/CLI.md (v0, experimental).
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use zeroconverter_core::app_error::AppError;
use zeroconverter_core::conversion::options::{Control, OptionSpec, OptionValue};
use zeroconverter_core::conversion::output_naming::generate_output_path;
use zeroconverter_core::conversion::pipeline::convert;
use zeroconverter_core::conversion::registry::ConverterRegistry;
use zeroconverter_core::conversion::traits::{ConverterHandler, ExecContext};
use zeroconverter_core::conversion::types::{AssetFamily, ConversionRequest, OutputFormat};
use zeroconverter_core::jobs::types::OutputReservationAttempt;

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The machine interface's version. Within one major version, fields and
/// codes are only added.
const INTERFACE: &str = "0";
const JOB_SCHEMA: &str = "zeroconverter.job/v0";
const RESULT_SCHEMA: &str = "zeroconverter.result/v0";
const EVENT_SCHEMA: &str = "zeroconverter.event/v0";
/// The shared contract this tool follows (version, capabilities, job,
/// JSON-lines events, exit codes).
const STANDARD: &str = "zero-app/v0";
/// Git short hash of the source, or "unknown" (set by build.rs).
const BUILD: &str = env!("ZC_BUILD");
/// Free-text messages are cut to this many bytes.
const MAX_MESSAGE_BYTES: usize = 4096;

// Exit codes (docs/CLI.md).
const EXIT_OK: u8 = 0;
const EXIT_SOME_FAILED: u8 = 1;
const EXIT_INVALID_JOB: u8 = 2;
const EXIT_CANCELLED: u8 = 3;
const EXIT_INTERNAL: u8 = 4;

const USAGE: &str = "usage:
  zeroconverter version [--json]
  zeroconverter capabilities [--json]
  zeroconverter validate --job <job.json>
  zeroconverter run --job <job.json>";

/// A job file. Paths are relative to the current directory.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    schema: String,
    inputs: Vec<PathBuf>,
    /// Output format id or extension, e.g. "webp".
    format: String,
    /// Where outputs go. Default: next to each input.
    #[serde(default)]
    output_dir: Option<PathBuf>,
    /// Format options (quality, resize_mode, bitrate_kbps, ...).
    #[serde(default)]
    options: Map<String, Value>,
    /// Replace an existing output instead of picking `name (1).ext`.
    #[serde(default)]
    overwrite: bool,
    /// Consent to convert inputs over 100 MB.
    #[serde(default)]
    confirm_large: bool,
}

/// What the command line asked for. Anything else is a usage error (exit 2).
enum Command {
    Version,
    Capabilities,
    Validate(PathBuf),
    Run(PathBuf),
    Help,
}

fn parse_args(args: &[String]) -> Option<Command> {
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    match (args.first().map(String::as_str)?, rest.as_slice()) {
        ("version", [] | ["--json"]) => Some(Command::Version),
        ("capabilities", [] | ["--json"]) => Some(Command::Capabilities),
        ("validate", ["--job", path]) => Some(Command::Validate(PathBuf::from(path))),
        ("run", ["--job", path]) => Some(Command::Run(PathBuf::from(path))),
        ("help" | "--help" | "-h", []) => Some(Command::Help),
        _ => None,
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = parse_args(&args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(EXIT_INVALID_JOB);
    };

    if let Some(dir) = app_data_dir() {
        zeroconverter_core::handlers::video::ffmpeg::set_data_dir(dir);
    }
    let registry = ConverterRegistry::standard();

    let code = match command {
        Command::Version => {
            emit(&json!({
                "schema": "zeroconverter.version/v0",
                "tool": "zeroconverter",
                "version": VERSION,
                "build": BUILD,
                "standard": STANDARD,
                "interface": INTERFACE,
                "job_schema": JOB_SCHEMA,
                "result_schema": RESULT_SCHEMA,
            }));
            EXIT_OK
        }
        Command::Capabilities => {
            emit(&capabilities(&registry));
            EXIT_OK
        }
        Command::Validate(path) => match read_job(&path) {
            Ok(job) => validate(&registry, &job),
            Err(e) => invalid_job(&e),
        },
        Command::Run(path) => match read_job(&path) {
            Ok(job) => run(&registry, &job),
            Err(e) => invalid_job(&e),
        },
        Command::Help => {
            eprintln!("{USAGE}");
            EXIT_OK
        }
    };
    // A reader that could not get the output must not see a success code.
    if STDOUT_FAILED.load(Ordering::Relaxed) {
        eprintln!("could not write to stdout");
        return ExitCode::from(EXIT_INTERNAL);
    }
    ExitCode::from(code)
}

/// The app's data folder, so the CLI finds the FFmpeg the app downloaded.
/// The CLI itself never downloads anything. Without APPDATA there is none;
/// a usable system FFmpeg (PATH, winget, scoop, chocolatey) still works.
fn app_data_dir() -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var_os("APPDATA")?);
    base.is_absolute()
        .then(|| base.join("com.zerofoundry.zeroconverter"))
}

/// Set when a stdout write fails; the process then exits 4.
static STDOUT_FAILED: AtomicBool = AtomicBool::new(false);

fn emit(value: &Value) {
    let mut out = std::io::stdout().lock();
    let written = serde_json::to_writer(&mut out, value)
        .map_err(std::io::Error::from)
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush());
    if written.is_err() {
        STDOUT_FAILED.store(true, Ordering::Relaxed);
    }
}

fn invalid_job(message: &str) -> u8 {
    eprintln!("invalid job: {message}");
    emit(&json!({
        "event": "error",
        "error": { "code": "invalid_job", "message": cap(message) },
    }));
    EXIT_INVALID_JOB
}

/// Cuts free text to `MAX_MESSAGE_BYTES` on a character boundary.
fn cap(message: &str) -> String {
    if message.len() <= MAX_MESSAGE_BYTES {
        return message.to_string();
    }
    let mut end = MAX_MESSAGE_BYTES - '…'.len_utf8();
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &message[..end])
}

/// Whether running the same item again could succeed. `"unknown"` when it
/// depends on the file or the machine.
fn retryable(code: &str) -> Value {
    match code {
        "cancelled" => true.into(),
        "consent_required" | "invalid_request" | "unsupported_input" | "no_route" => false.into(),
        _ => "unknown".into(),
    }
}

/// `path` made absolute against the current directory, without touching
/// the disk. Relative paths in a job are relative to where the CLI runs.
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Where an input's output goes: `output_dir`, else the input's folder.
fn output_root(job: &Job, input: &Path) -> PathBuf {
    match &job.output_dir {
        Some(dir) => absolute(dir),
        None => absolute(input)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    }
}

/// The output's path and its path relative to the output folder.
fn output_paths(job: &Job, input: &Path, path: &str) -> (String, String) {
    let root = std::fs::canonicalize(output_root(job, input)).ok();
    let canon = std::fs::canonicalize(path).ok();
    let rel = match (root, canon) {
        (Some(root), Some(canon)) => canon.strip_prefix(root).ok().map(Path::to_path_buf),
        _ => None,
    }
    .or_else(|| Path::new(path).file_name().map(PathBuf::from))
    .map(|p| p.to_string_lossy().replace('\\', "/"))
    .unwrap_or_default();
    (path.to_string(), rel)
}

/// A job file is a short list of paths; anything bigger is a mistake.
const MAX_JOB_BYTES: u64 = 16 * 1024 * 1024;

fn read_job(path: &Path) -> Result<Job, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut text = String::new();
    file.take(MAX_JOB_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if text.len() as u64 > MAX_JOB_BYTES {
        return Err(format!("{} is larger than 16 MB", path.display()));
    }
    let job: Job = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if job.schema != JOB_SCHEMA {
        return Err(format!(
            "schema must be \"{JOB_SCHEMA}\", got \"{}\"",
            job.schema
        ));
    }
    if job.inputs.is_empty() {
        return Err("inputs is empty".into());
    }
    // The schema asks for non-empty strings: a bad job file, not a bad item.
    if job.inputs.iter().any(|p| p.as_os_str().is_empty()) {
        return Err("inputs has an empty path".into());
    }
    if job.format.trim().is_empty() {
        return Err("format is empty".into());
    }
    if let Some(dir) = &job.output_dir {
        if !dir.is_dir() {
            return Err(format!("output_dir {} is not a folder", dir.display()));
        }
    }
    Ok(job)
}

fn capabilities(registry: &ConverterRegistry) -> Value {
    let families: Vec<Value> = [
        ("image", AssetFamily::Image),
        ("audio", AssetFamily::Audio),
        ("video", AssetFamily::Video),
    ]
    .into_iter()
    .filter_map(|(name, family)| {
        let handler = registry.get_handler(&family)?;
        let formats: Vec<Value> = handler
            .format_descriptors()
            .iter()
            .map(|d| {
                json!({
                    "id": d.id,
                    "extension": d.canonical_extension,
                    "aliases": d.aliases,
                    "read": d.can_read,
                    "write": d.can_write,
                    "options": handler.output_options(d.id),
                })
            })
            .collect();
        let routes: Vec<Value> = handler
            .capabilities()
            .into_iter()
            .map(|c| json!({ "input": c.input.0, "outputs": c.outputs.into_iter().map(|o| o.0).collect::<Vec<_>>() }))
            .collect();
        Some(json!({ "family": name, "formats": formats, "routes": routes, "options": family_options(handler.as_ref()) }))
    })
    .collect();
    json!({
        "schema": "zeroconverter.capabilities/v0",
        "version": VERSION,
        "job_schemas": [JOB_SCHEMA],
        "families": families,
        "job_options": [
            { "key": "overwrite", "type": "boolean", "default": false },
            { "key": "confirm_large", "type": "boolean", "default": false },
        ],
        "effects": {
            "network": "none",
            "writes": {
                "outputs": "output_dir, or the input's folder when output_dir is not set",
                "temp_files": ["<output name>.tmp.<unique> next to the output", "zero_tmp_<unique>/ folders in the output folder"],
                "temp_cleanup": "removed when the item ends; a finished output is renamed into place",
            },
            "child_processes": [
                { "program": "ffmpeg", "families": ["video"] },
                { "program": "ffprobe", "families": ["video"] },
            ],
            "reads_env": ["APPDATA", "PATH", "ZC_FFMPEG_DIR", "LOCALAPPDATA", "SCOOP", "USERPROFILE", "ChocolateyInstall", "ProgramData"],
        },
    })
}

/// Keys accepted in `options`, per family, summarised from what each
/// writable format declares (`zeroconverter_core::conversion::options`).
/// `formats` limits a key to those output formats; elsewhere it is ignored.
/// `default`, `values`, `min` and `max` appear when every listed format
/// agrees; each format's own entry in `formats[].options` has the full spec.
fn family_options(handler: &dyn ConverterHandler) -> Value {
    let targets: Vec<&str> = handler
        .format_descriptors()
        .iter()
        .filter(|d| d.can_write)
        .map(|d| d.id)
        .collect();
    let mut keys: Vec<&str> = Vec::new();
    let mut by_key: Vec<Vec<&'static OptionSpec>> = Vec::new();
    for target in &targets {
        for spec in handler.output_options(target) {
            match keys.iter().position(|k| *k == spec.key) {
                Some(i) => by_key[i].push(spec),
                None => {
                    keys.push(spec.key);
                    by_key.push(vec![spec]);
                }
            }
        }
    }
    let mut out = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let specs = &by_key[i];
        let formats: Vec<&str> = targets
            .iter()
            .copied()
            .filter(|t| handler.output_options(t).iter().any(|s| s.key == *key))
            .collect();
        let first = specs[0];
        let bounds = |c: Control| match c {
            Control::Slider { min, max } | Control::Number { min, max, .. } => Some((min, max)),
            _ => None,
        };
        let mut entry = Map::new();
        entry.insert("key".into(), json!(key));
        entry.insert("type".into(), json!(first.json_type()));
        if first.default != OptionValue::Unset && specs.iter().all(|s| s.default == first.default) {
            entry.insert("default".into(), first.default.to_json());
        }
        if let Control::Choice { values } = first.control {
            if specs.iter().all(|s| s.control == first.control) {
                let listed: Vec<Value> = values
                    .iter()
                    .filter(|v| **v != OptionValue::Unset)
                    .map(|v| v.to_json())
                    .collect();
                entry.insert("values".into(), json!(listed));
            }
        }
        if let Some((min, max)) = bounds(first.control) {
            if specs.iter().all(|s| bounds(s.control) == Some((min, max))) {
                entry.insert("min".into(), json!(min));
                entry.insert("max".into(), json!(max));
            }
        }
        if formats.len() < targets.len() {
            entry.insert("formats".into(), json!(formats));
        }
        if let Some(note) = first.note {
            entry.insert("note".into(), json!(note));
        }
        out.push(Value::Object(entry));
    }
    Value::Array(out)
}

/// One input turned into a checked request with its planned output path.
/// Windows file names ignore case: `photo.webp` and `PHOTO.webp` are one file.
fn name_key(path: &str) -> String {
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.to_string()
    }
}

/// Output names already handed out in this job, and the job's inputs,
/// which no output may replace.
struct Names {
    taken: HashSet<String>,
    inputs: HashSet<PathBuf>,
}

impl Names {
    fn new(job: &Job) -> Self {
        Self {
            taken: HashSet::new(),
            inputs: job
                .inputs
                .iter()
                .filter_map(|i| std::fs::canonicalize(i).ok())
                .collect(),
        }
    }

    fn reserve(&mut self, path: &str) -> OutputReservationAttempt {
        if std::fs::canonicalize(path).is_ok_and(|p| self.inputs.contains(&p)) {
            OutputReservationAttempt::Protected
        } else if self.taken.insert(name_key(path)) {
            OutputReservationAttempt::Reserved
        } else {
            OutputReservationAttempt::AlreadyReserved
        }
    }

    /// A failed item gives its name back.
    fn release(&mut self, path: &str) {
        self.taken.remove(&name_key(path));
    }
}

fn plan(
    registry: &ConverterRegistry,
    job: &Job,
    input: &Path,
    names: &mut Names,
) -> Result<ConversionRequest, AppError> {
    let input = &absolute(input);
    if !input.is_file() {
        return Err(AppError::Io(format!("{} is not a file", input.display())));
    }
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let family = registry.resolve_family(&ext);
    let descriptor = registry.get_format_descriptor(&family, &job.format)?;
    let output_root = output_root(job, input);
    let output = generate_output_path(input, &output_root, &descriptor, job.overwrite, |p| {
        Ok(names.reserve(p))
    })?;
    // Defaults for unset options live in the engine, shared with the app.
    let mut options = job.options.clone();
    options.insert("overwrite".into(), job.overwrite.into());
    options.insert("confirm_large".into(), job.confirm_large.into());
    let request = ConversionRequest {
        family: family.clone(),
        input_path: input.to_string_lossy().into_owned(),
        output_path: output.to_string_lossy().into_owned(),
        output_root: output_root.to_string_lossy().into_owned(),
        output_format: OutputFormat(job.format.to_lowercase()),
        options: Value::Object(options),
    };
    // Consent first: a 4 GB file should be a question before anything else.
    let checked = match zeroconverter_core::conversion::types::large_input(input) {
        Some(bytes) if !request.large_confirmed() => Err(AppError::ConsentRequired { bytes }),
        _ => registry.validate(family, &request),
    };
    if let Err(e) = checked {
        names.release(&request.output_path);
        return Err(e);
    }
    Ok(request)
}

fn cancelled_item(index: usize, input: &Path) -> Value {
    let e = AppError::Cancelled;
    json!({
        "index": index,
        "input": input,
        "status": "cancelled",
        "retryable": retryable(e.code()),
        "error": { "code": e.code(), "message": "Cancelled" },
    })
}

fn error_json(e: &AppError) -> Value {
    json!({ "code": e.code(), "message": cap(&e.to_string()) })
}

/// A failed or invalid item.
fn error_item(index: usize, input: &Path, status: &str, e: &AppError) -> Value {
    json!({
        "index": index,
        "input": input,
        "status": status,
        "retryable": retryable(e.code()),
        "error": error_json(e),
    })
}

fn validate(registry: &ConverterRegistry, job: &Job) -> u8 {
    let mut names = Names::new(job);
    let mut items = Vec::new();
    let mut invalid = 0;
    for (index, input) in job.inputs.iter().enumerate() {
        items.push(match plan(registry, job, input, &mut names) {
            Ok(request) => {
                let (path, rel_path) = output_paths(job, input, &request.output_path);
                json!({
                    "index": index,
                    "input": input,
                    "status": "valid",
                    "output": { "path": path, "rel_path": rel_path },
                })
            }
            Err(e) => {
                invalid += 1;
                error_item(index, input, "invalid", &e)
            }
        });
    }
    emit(&json!({
        "event": "result",
        "schema": RESULT_SCHEMA,
        "tool": { "name": "zeroconverter", "version": VERSION },
        "command": "validate",
        "items": items,
        "summary": { "valid": job.inputs.len() - invalid, "invalid": invalid },
    }));
    if invalid > 0 {
        EXIT_SOME_FAILED
    } else {
        EXIT_OK
    }
}

fn run(registry: &ConverterRegistry, job: &Job) -> u8 {
    let token = CancellationToken::new();
    {
        let token = token.clone();
        if ctrlc::set_handler(move || token.cancel()).is_err() {
            eprintln!("could not install the Ctrl+C handler");
            return EXIT_INTERNAL;
        }
    }
    let mut names = Names::new(job);
    // Held for the whole run: no other process can replace an input that is
    // still waiting its turn.
    let _guards: Vec<_> = job
        .inputs
        .iter()
        .filter_map(|i| zero_fs::InputGuard::hold(i))
        .collect();
    let mut items = Vec::new();
    let (mut ok, mut failed, mut cancelled) = (0usize, 0usize, 0usize);
    emit(&json!({
        "event": "job_started",
        "schema": EVENT_SCHEMA,
        "tool": { "name": "zeroconverter", "version": VERSION },
        "items": job.inputs.len(),
    }));

    for (index, input) in job.inputs.iter().enumerate() {
        if token.is_cancelled() {
            cancelled += 1;
            let item = cancelled_item(index, input);
            emit(&json!({ "event": "item_finished", "item": item }));
            items.push(item);
            continue;
        }
        emit(&json!({ "event": "item_started", "index": index, "input": input }));
        // A decoder bug on one file must not take the rest of the job down.
        let mut planned: Option<String> = None;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            plan(registry, job, input, &mut names).and_then(|request| {
                planned = Some(request.output_path.clone());
                let ctx = ExecContext {
                    token: Some(token.clone()),
                    progress: Some(Arc::new(move |percent: f32, message: &str| {
                        emit(&json!({
                            "event": "progress",
                            "index": index,
                            "percent": percent,
                            "message": cap(message),
                        }));
                    })),
                };
                convert(registry, &request, &ctx)
            })
        }))
        .unwrap_or_else(|panic| {
            let what = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown".into());
            Err(AppError::Internal(format!(
                "converter crashed on this file: {what}"
            )))
        });
        let item = match outcome {
            Ok(result) => match result.output_path.as_deref().map(fingerprint) {
                Some(Ok((path, bytes, sha256))) => {
                    ok += 1;
                    let (path, rel_path) = output_paths(job, input, &path);
                    json!({
                        "index": index,
                        "input": input,
                        "status": "ok",
                        "output": { "path": path, "rel_path": rel_path, "bytes": bytes, "sha256": sha256 },
                        "message": cap(&result.message),
                        "warnings": [],
                    })
                }
                Some(Err(e)) => {
                    failed += 1;
                    error_item(index, input, "failed", &e)
                }
                None => {
                    failed += 1;
                    error_item(index, input, "failed", &AppError::Execute(result.message))
                }
            },
            Err(AppError::Cancelled) => {
                cancelled += 1;
                cancelled_item(index, input)
            }
            Err(e) => {
                failed += 1;
                error_item(index, input, "failed", &e)
            }
        };
        if item["status"] != "ok" {
            if let Some(path) = &planned {
                names.release(path);
            }
        }
        emit(&json!({ "event": "item_finished", "item": item }));
        items.push(item);
    }

    emit(&json!({
        "event": "result",
        "schema": RESULT_SCHEMA,
        "tool": { "name": "zeroconverter", "version": VERSION },
        "command": "run",
        "items": items,
        "summary": { "ok": ok, "failed": failed, "cancelled": cancelled },
    }));
    if cancelled > 0 {
        EXIT_CANCELLED
    } else if failed > 0 {
        EXIT_SOME_FAILED
    } else {
        EXIT_OK
    }
}

/// Path, size and SHA-256 of a finished output. The app reports its own
/// hash only; it does not claim the file was verified by anyone else.
fn fingerprint(path: &str) -> Result<(String, u64, String), AppError> {
    let mut file = std::fs::File::open(path).map_err(|e| AppError::Io(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut bytes = 0u64;
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| AppError::Io(e.to_string()))?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        hasher.update(&buf[..n]);
    }
    let sha = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    Ok((path.to_string(), bytes, sha))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_messages_are_cut_on_a_character_boundary() {
        assert_eq!(cap("short"), "short");
        let long = "ğ".repeat(MAX_MESSAGE_BYTES);
        let cut = cap(&long);
        assert!(cut.len() <= MAX_MESSAGE_BYTES);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn retryable_follows_the_error_code() {
        assert_eq!(retryable("cancelled"), true);
        assert_eq!(retryable("consent_required"), false);
        assert_eq!(retryable("io"), "unknown");
    }
}
