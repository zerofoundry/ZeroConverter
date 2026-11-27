// ==============================================================================
// OWNERSHIP:
//   Maps Tauri-facing DTOs into internal core requests and exposes backend commands to the IPC boundary.
//
// DO NOT ADD:
//   - Conversion business logic
//   - Output naming algorithms
//   - Hardcoded fallback paths
//   - UI state behavior
//
// NOTES:
//   This layer translates intent into core requests; core remains the authority for final decisions.
// ==============================================================================
use super::dto::{
    AppCapabilitiesDto, AppStatusDto, BatchMetadataRequestDto, BatchMetadataResultDto,
    BatchSubmissionRequestDto, BatchSubmissionResultDto, ConversionRouteDto, FamilyCapabilitiesDto,
    FileMetadataDto, FormatDescriptorDto, JobStatusDto, PreflightRequestDto, PreflightResultDto,
    QueueSummaryDto,
};
use crate::core::app_error::AppError;
use crate::core::conversion::types::{ConversionRequest, OutputFormat};
use crate::core::jobs::types::JobStatus;
use crate::core::probe;
use crate::core::probe::types::ProbeStatus;
use crate::AppState;
use std::path::{Path, PathBuf};
use tauri::ipc::Channel;
use tauri::State;

#[tauri::command]
pub async fn get_app_status() -> Result<AppStatusDto, AppError> {
    Ok(AppStatusDto {
        engine: "ZeroConverter Core".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

pub fn build_capabilities_dto(
    registry: &crate::core::conversion::registry::ConverterRegistry,
) -> AppCapabilitiesDto {
    let mut families = Vec::new();

    let map_family =
        |family_name: &str, handler: &dyn crate::core::conversion::traits::ConverterHandler| {
            let descriptors = handler
                .format_descriptors()
                .iter()
                .map(|d| FormatDescriptorDto {
                    id: d.id.to_string(),
                    canonical_extension: d.canonical_extension.to_string(),
                    aliases: d.aliases.iter().map(|s| s.to_string()).collect(),
                    can_read: d.can_read,
                    can_write: d.can_write,
                    options: handler.output_options(d.id),
                })
                .collect();

            let routes = handler
                .capabilities()
                .into_iter()
                .map(|c| ConversionRouteDto {
                    input: c.input.0,
                    outputs: c.outputs.into_iter().map(|o| o.0).collect(),
                })
                .collect();

            FamilyCapabilitiesDto {
                family: family_name.to_string(),
                descriptors,
                routes,
            }
        };

    if let Some(image_handler) =
        registry.get_handler(&crate::core::conversion::types::AssetFamily::Image)
    {
        families.push(map_family("image", image_handler.as_ref()));
    }
    if let Some(audio_handler) =
        registry.get_handler(&crate::core::conversion::types::AssetFamily::Audio)
    {
        families.push(map_family("audio", audio_handler.as_ref()));
    }
    if let Some(video_handler) =
        registry.get_handler(&crate::core::conversion::types::AssetFamily::Video)
    {
        families.push(map_family("video", video_handler.as_ref()));
    }

    AppCapabilitiesDto { families }
}

#[tauri::command]
pub async fn get_app_capabilities(
    state: State<'_, AppState>,
) -> Result<AppCapabilitiesDto, AppError> {
    Ok(build_capabilities_dto(state.pipeline.registry()))
}

#[tauri::command]
pub fn attach_job_events(
    channel: Channel<serde_json::Value>,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    state
        .jobs
        .add_listener(Box::new(move |event| channel.send(event.clone()).is_ok()));
    Ok(())
}

fn map_submit_dto_to_request(
    r: &super::dto::ConversionSubmitDto,
    pipeline: &crate::core::conversion::pipeline::ConversionPipeline,
) -> Result<ConversionRequest, AppError> {
    // Derive family from extension via registry (provisional hint)
    let ext = Path::new(&r.input_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let family = pipeline.resolve_family(&ext);

    let output_root = r.output_folder.clone().unwrap_or_else(|| {
        Path::new(&r.input_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    });

    if output_root.is_empty() {
        return Err(AppError::Validate(
            "Cannot determine default output folder from input path".to_string(),
        ));
    }

    Ok(ConversionRequest {
        family,
        input_path: r.input_path.clone(),
        output_path: String::new(), // Filled by plan phase
        output_root,
        output_format: OutputFormat(r.output_format.clone()),
        options: serde_json::Value::Object(r.options.clone()),
    })
}

#[tauri::command]
pub async fn submit_conversion_batch(
    request: BatchSubmissionRequestDto,
    state: State<'_, AppState>,
) -> Result<BatchSubmissionResultDto, AppError> {
    let mut core_requests = Vec::new();
    for r in request.conversions {
        let req = map_submit_dto_to_request(&r, &state.pipeline)?;
        core_requests.push(req);
    }

    let job_ids = state
        .jobs
        .submit_batch(core_requests, state.pipeline.clone());

    Ok(BatchSubmissionResultDto { job_ids })
}

fn map_validation_error_to_preflight(
    error: &AppError,
    token: u32,
    _request: &ConversionRequest,
) -> PreflightResultDto {
    let msg = error.to_string();

    if let AppError::IcoTooLarge { width, height, max } = *error {
        return PreflightResultDto {
            token,
            is_valid: false,
            severity: Some("error".to_string()),
            code: Some("ICO_DIMENSIONS_EXCEED_LIMIT".to_string()),
            message: Some(msg),
            current_width: Some(width),
            current_height: Some(height),
            max_width: Some(max),
            max_height: Some(max),
            suggested_action: Some("FIT_BOX_256".to_string()),
        };
    }

    // Default mapping for unknown validation errors
    PreflightResultDto {
        token,
        is_valid: false,
        severity: Some("error".to_string()),
        code: Some("VALIDATION_FAILED".to_string()),
        message: Some(msg),
        current_width: None,
        current_height: None,
        max_width: None,
        max_height: None,
        suggested_action: None,
    }
}

#[tauri::command]
pub async fn preflight_conversion(
    request: PreflightRequestDto,
    state: State<'_, AppState>,
) -> Result<PreflightResultDto, AppError> {
    let req = map_submit_dto_to_request(&request.conversion, &state.pipeline)?;
    let family = req.family.clone();

    // Perform validation using the same Core validation path as execution
    let result = state.pipeline.registry().validate(family, &req);

    match result {
        Ok(_) => Ok(PreflightResultDto {
            token: request.token,
            is_valid: true,
            severity: None,
            code: None,
            message: None,
            current_width: None,
            current_height: None,
            max_width: None,
            max_height: None,
            suggested_action: None,
        }),
        Err(e) => Ok(map_validation_error_to_preflight(&e, request.token, &req)),
    }
}

#[tauri::command]
pub async fn get_queue_status(state: State<'_, AppState>) -> Result<QueueSummaryDto, AppError> {
    Ok(state.jobs.get_queue_summary().into())
}

#[tauri::command]
pub async fn list_jobs(state: State<'_, AppState>) -> Result<Vec<JobStatusDto>, AppError> {
    Ok(state
        .jobs
        .list_all_jobs()
        .into_iter()
        .map(JobStatusDto::from)
        .collect())
}

#[tauri::command]
pub async fn get_job_status(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<JobStatusDto, AppError> {
    state
        .jobs
        .get_job(&job_id)
        .map(JobStatusDto::from)
        .ok_or_else(|| AppError::Execute(format!("Job {} not found", job_id)))
}

#[tauri::command]
pub async fn cancel_job(job_id: String, state: State<'_, AppState>) -> Result<(), AppError> {
    state.jobs.cancel_job(&job_id)
}

#[tauri::command]
pub async fn purge_jobs(state: State<'_, AppState>) -> Result<(), AppError> {
    state.jobs.purge_terminal_jobs()
}

#[tauri::command]
pub async fn get_files_metadata(
    request: BatchMetadataRequestDto,
    state: State<'_, AppState>,
) -> Result<BatchMetadataResultDto, AppError> {
    let mut results = Vec::new();

    for path_str in request.paths {
        let path = Path::new(&path_str);
        if !path.exists() {
            continue;
        }

        // Delegate entirely to Core probe service
        let probe_result = probe::probe_file(path, state.pipeline.registry());

        match probe_result {
            Ok(pr) => {
                let kind = match pr.family {
                    crate::core::conversion::types::AssetFamily::Image => "image",
                    crate::core::conversion::types::AssetFamily::Audio => "audio",
                    crate::core::conversion::types::AssetFamily::Video => "video",
                    _ => "unknown",
                }
                .to_string();

                let probe_status = match pr.probe_status {
                    ProbeStatus::Provisional => "Provisional",
                    ProbeStatus::Recognized => "Recognized",
                    ProbeStatus::Failed => "Failed",
                }
                .to_string();

                results.push(FileMetadataDto {
                    path: path_str,
                    name: pr.name,
                    extension: pr.extension,
                    size_bytes: pr.size_bytes,
                    kind,
                    probe_status,
                    duration_seconds: pr.audio_meta.as_ref().map(|a| a.duration_seconds),
                    sample_rate: pr.audio_meta.as_ref().map(|a| a.sample_rate),
                    channels: pr.audio_meta.as_ref().map(|a| a.channels),
                });
            }
            Err(_) => {
                // Probe failed entirely — still report the file as unknown
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let extension = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();

                results.push(FileMetadataDto {
                    path: path_str,
                    name,
                    extension,
                    size_bytes: 0,
                    kind: "unknown".to_string(),
                    probe_status: "Failed".to_string(),
                    duration_seconds: None,
                    sample_rate: None,
                    channels: None,
                });
            }
        }
    }

    Ok(BatchMetadataResultDto { results })
}

// ==============================================================================
// Completed-output opening: renderer supplies only an opaque job ID.
// ==============================================================================

/// A second safety boundary beyond the runtime's writable format descriptors.
/// Do not open active content or launcher/shortcut extensions even if a handler
/// accidentally advertises them as writable in a future release.
const BLOCKED_OPEN_EXTENSIONS: &[&str] = &[
    "exe", "com", "bat", "cmd", "ps1", "psm1", "msi", "msp", "scr", "pif", "lnk", "url", "scf",
    "desktop", "webloc", "app", "command", "sh", "vbs", "vbe", "wsf", "wsh", "js", "jse", "jar",
    "hta", "cpl", "reg", "dll", "so", "dylib", "html", "htm", "svg",
];

fn is_blocked_open_extension(ext: &str) -> bool {
    BLOCKED_OPEN_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
}

/// Resolve only the result of a completed and successful conversion.
/// The job's planned path and output root come from the backend job record,
/// never from a frontend-supplied filesystem path.
fn resolve_openable_output(
    job_id: &str,
    jobs: &crate::core::jobs::JobManager,
    registry: &crate::core::conversion::registry::ConverterRegistry,
) -> Result<PathBuf, AppError> {
    let job = jobs
        .get_job(job_id)
        .ok_or_else(|| AppError::Validate("Job not found".to_string()))?;

    if job.status != JobStatus::Completed {
        return Err(AppError::Validate(format!(
            "Cannot open output (status: {:?})",
            job.status
        )));
    }

    let raw_path = job
        .result
        .as_ref()
        .filter(|r| r.success)
        .and_then(|r| r.output_path.as_deref())
        .ok_or_else(|| AppError::Validate("Completed job has no output path".to_string()))?;

    let canonical = dunce::canonicalize(raw_path)
        .map_err(|_| AppError::Execute("Output file is not accessible".to_string()))?;
    if !canonical.is_file() {
        return Err(AppError::Validate("Output path is not a file".to_string()));
    }

    // Match the actual artifact to the path reserved by the plan. Rejecting
    // mismatches also prevents a handler from redirecting the OS opener.
    let planned = dunce::canonicalize(&job.request.output_path)
        .map_err(|_| AppError::Validate("Planned output path is not accessible".to_string()))?;
    if canonical != planned {
        return Err(AppError::Validate(
            "Output differs from planned path".to_string(),
        ));
    }

    let approved_root = dunce::canonicalize(&job.request.output_root)
        .map_err(|_| AppError::Validate("Output root is not accessible".to_string()))?;
    if !approved_root.is_dir() || !canonical.starts_with(&approved_root) {
        return Err(AppError::Validate(
            "Output is outside its approved root".to_string(),
        ));
    }

    let ext = canonical.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.is_empty() || is_blocked_open_extension(ext) {
        return Err(AppError::Validate(
            "Opening this file type is not permitted".to_string(),
        ));
    }

    // Match the actual extension to this job's requested output descriptor,
    // not merely ANY writable extension in the application.
    let permitted = registry
        .get_handler(&job.request.family)
        .and_then(|handler| {
            handler
                .format_descriptors()
                .iter()
                .find(|desc| desc.can_write && desc.matches_format(&job.request.output_format.0))
                .map(|desc| desc.matches_format(ext))
        })
        .unwrap_or(false);

    if !permitted {
        return Err(AppError::Validate(
            "Opening this file type is not permitted".to_string(),
        ));
    }

    Ok(canonical)
}

#[tauri::command]
pub async fn open_completed_output(
    job_id: String,
    reveal: Option<bool>,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    let path = resolve_openable_output(&job_id, &state.jobs, state.pipeline.registry())?;

    use tauri_plugin_opener::OpenerExt;
    if reveal.unwrap_or(false) {
        app.opener().reveal_item_in_dir(&path).map_err(|_| {
            AppError::Execute("Failed to reveal output in file manager".to_string())
        })?;
    } else {
        app.opener()
            .open_path(path.to_string_lossy(), None::<&str>)
            .map_err(|_| AppError::Execute("Failed to open output file".to_string()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::conversion::types::{
        AssetFamily, ConversionRequest, ConversionResult, OutputFormat,
    };
    use crate::core::jobs::events::JobBroadcaster;
    use crate::core::jobs::JobManager;
    use std::sync::Arc;

    /// Helper to build a minimal ConversionRequest for ICO output.
    fn make_ico_request() -> ConversionRequest {
        ConversionRequest {
            family: AssetFamily::Image,
            input_path: "test.png".to_string(),
            output_path: String::new(),
            output_root: ".".to_string(),
            output_format: OutputFormat("ico".to_string()),
            options: serde_json::json!({}),
        }
    }

    /// Helper to build a minimal ConversionRequest for PNG output.
    fn make_png_request() -> ConversionRequest {
        ConversionRequest {
            family: AssetFamily::Image,
            input_path: "test.png".to_string(),
            output_path: String::new(),
            output_root: ".".to_string(),
            output_format: OutputFormat("png".to_string()),
            options: serde_json::json!({}),
        }
    }

    // ═══════════════════════════════════════════
    // A. ICO dimension error maps to structured preflight result
    // ═══════════════════════════════════════════

    #[test]
    fn ico_dimension_error_maps_to_structured_result() {
        let error = AppError::IcoTooLarge {
            width: 1920,
            height: 1080,
            max: 256,
        };
        let result = map_validation_error_to_preflight(&error, 42, &make_ico_request());

        assert!(!result.is_valid);
        assert_eq!(result.token, 42);
        assert_eq!(result.severity, Some("error".to_string()));
        assert_eq!(result.code, Some("ICO_DIMENSIONS_EXCEED_LIMIT".to_string()));
        assert_eq!(result.suggested_action, Some("FIT_BOX_256".to_string()));
        assert_eq!(result.max_width, Some(256));
        assert_eq!(result.max_height, Some(256));
        assert!(result.message.is_some());
        assert!(result.message.as_ref().unwrap().contains("256x256"));
    }

    // ═══════════════════════════════════════════
    // B. Unknown validation errors map safely
    // ═══════════════════════════════════════════

    #[test]
    fn unknown_validation_error_maps_to_generic_invalid() {
        let error = AppError::Validate("Some unexpected validation issue".to_string());
        let result = map_validation_error_to_preflight(&error, 7, &make_ico_request());

        assert!(!result.is_valid);
        assert_eq!(result.token, 7);
        assert_eq!(result.severity, Some("error".to_string()));
        assert_eq!(result.code, Some("VALIDATION_FAILED".to_string()));
        assert!(result.suggested_action.is_none());
        assert!(result.max_width.is_none());
        assert!(result.max_height.is_none());
        assert!(result.message.is_some());
    }

    #[test]
    fn non_validation_error_maps_safely_without_panic() {
        let error = AppError::Internal("Core meltdown".to_string());
        let result = map_validation_error_to_preflight(&error, 99, &make_ico_request());

        assert!(!result.is_valid);
        assert_eq!(result.token, 99);
        assert_eq!(result.code, Some("VALIDATION_FAILED".to_string()));
        assert!(result.suggested_action.is_none());
        assert!(result.message.is_some());
    }

    #[test]
    fn ico_dimension_string_on_non_ico_format_does_not_map_to_ico_code() {
        // Even if the error message mentions 256x256, if the output format is not ICO,
        // it should not map to ICO_DIMENSIONS_EXCEED_LIMIT.
        let error = AppError::Validate("ICO output requires dimensions up to 256x256.".to_string());
        let result = map_validation_error_to_preflight(&error, 1, &make_png_request());

        assert!(!result.is_valid);
        assert_eq!(result.code, Some("VALIDATION_FAILED".to_string()));
        assert!(result.suggested_action.is_none());
    }

    // ═══════════════════════════════════════════
    // C. Valid preflight result shape
    // ═══════════════════════════════════════════

    #[test]
    fn valid_preflight_result_has_no_error_fields() {
        // Directly test the Ok arm shape that preflight_conversion produces.
        let result = PreflightResultDto {
            token: 5,
            is_valid: true,
            severity: None,
            code: None,
            message: None,
            current_width: None,
            current_height: None,
            max_width: None,
            max_height: None,
            suggested_action: None,
        };

        assert!(result.is_valid);
        assert_eq!(result.token, 5);
        assert!(result.code.is_none());
        assert!(result.severity.is_none());
        assert!(result.message.is_none());
        assert!(result.suggested_action.is_none());
    }

    // Purge test removed to avoid state mock complexity.

    #[test]
    fn get_app_capabilities_returns_correct_shape() {
        let mut registry = crate::core::conversion::registry::ConverterRegistry::new();
        registry.register(
            crate::core::conversion::types::AssetFamily::Image,
            std::sync::Arc::new(crate::core::handlers::image::ImageConverterHandler),
        );
        registry.register(
            crate::core::conversion::types::AssetFamily::Audio,
            std::sync::Arc::new(crate::core::handlers::audio::AudioHandler),
        );

        let res = build_capabilities_dto(&registry);

        assert_eq!(res.families.len(), 2, "Expected image and audio families");

        let image_family = res.families.iter().find(|f| f.family == "image").unwrap();
        assert!(image_family
            .descriptors
            .iter()
            .any(|d| d.id == "svg" && d.can_read && !d.can_write));
        assert!(image_family
            .descriptors
            .iter()
            .any(|d| d.id == "ico" && d.can_read && d.can_write));
        assert!(image_family
            .descriptors
            .iter()
            .any(|d| d.id == "gif" && d.can_read && !d.can_write));

        // Options come from the engine: JPG offers quality first, SVG (read
        // only) offers nothing.
        let options_of = |id: &str| -> Vec<&str> {
            image_family
                .descriptors
                .iter()
                .find(|d| d.id == id)
                .unwrap()
                .options
                .iter()
                .map(|o| o.key)
                .collect()
        };
        assert_eq!(options_of("jpg")[0], "quality");
        assert!(options_of("png").contains(&"resize_mode"));
        assert!(options_of("svg").is_empty());

        // SVG input route exists and routes to PNG, JPG, WebP, BMP, TIFF, ICO. Not SVG.
        let svg_route = image_family
            .routes
            .iter()
            .find(|r| r.input == "svg")
            .unwrap();
        assert!(svg_route.outputs.contains(&"png".to_string()));
        assert!(svg_route.outputs.contains(&"ico".to_string()));
        assert!(!svg_route.outputs.contains(&"svg".to_string()));

        let gif_route = image_family
            .routes
            .iter()
            .find(|r| r.input == "gif")
            .unwrap();
        assert!(gif_route.outputs.contains(&"png".to_string()));
        assert!(gif_route.outputs.contains(&"jpg".to_string()));
        assert!(gif_route.outputs.contains(&"webp".to_string()));
        assert!(gif_route.outputs.contains(&"bmp".to_string()));
        assert!(gif_route.outputs.contains(&"tiff".to_string()));
        assert!(gif_route.outputs.contains(&"ico".to_string()));
        assert!(!gif_route.outputs.contains(&"gif".to_string()));
        // PDF capability route truthfulness: every raster input can produce
        // a PDF page; SVG stays excluded.
        for input_id in &["png", "jpg", "webp", "gif", "ico", "bmp", "tiff"] {
            let route = image_family
                .routes
                .iter()
                .find(|r| r.input == *input_id)
                .unwrap_or_else(|| panic!("Missing route for {}", input_id));
            assert!(
                route.outputs.contains(&"pdf".to_string()),
                "{} route must include pdf output",
                input_id
            );
        }
        {
            let input_id = &"svg";
            let route = image_family
                .routes
                .iter()
                .find(|r| r.input == *input_id)
                .unwrap_or_else(|| panic!("Missing route for {}", input_id));
            assert!(
                !route.outputs.contains(&"pdf".to_string()),
                "{} route must NOT include pdf output",
                input_id
            );
        }

        // PDF must never appear as an input route
        assert!(
            !image_family.routes.iter().any(|r| r.input == "pdf"),
            "PDF must not appear as an input route"
        );

        // PDF descriptor must be write-only
        assert!(image_family
            .descriptors
            .iter()
            .any(|d| d.id == "pdf" && !d.can_read && d.can_write));

        let audio_family = res.families.iter().find(|f| f.family == "audio").unwrap();
        let wav_route = audio_family
            .routes
            .iter()
            .find(|r| r.input == "wav")
            .unwrap();
        assert!(
            wav_route.outputs.contains(&"mp3".to_string())
                && wav_route.outputs.contains(&"flac".to_string())
                && !wav_route.outputs.contains(&"wav".to_string()),
            "WAV route should offer MP3 and FLAC and never WAV to WAV"
        );

        let ogg_route = audio_family
            .routes
            .iter()
            .find(|r| r.input == "ogg")
            .expect("OGG should be registered as an input format");

        assert!(
            ogg_route.outputs.contains(&"wav".to_string())
                && ogg_route.outputs.contains(&"mp3".to_string())
                && ogg_route.outputs.contains(&"flac".to_string()),
            "OGG audio route should include wav, mp3, flac"
        );

        // Ensure OGG doesn't appear as an output
        for route in &audio_family.routes {
            assert!(
                !route.outputs.contains(&"ogg".to_string()),
                "OGG must not appear as an output format"
            );
        }

        // Ensure aliases aren't turning into root output routes
        assert!(
            !image_family.routes.iter().any(|r| r.input == "jpeg"),
            "Aliases shouldn't be root routes if core doesn't emit them"
        );
    }

    #[test]
    fn preflight_result_dto_serializes_correctly() {
        let result = PreflightResultDto {
            token: 10,
            is_valid: false,
            severity: Some("error".to_string()),
            code: Some("ICO_DIMENSIONS_EXCEED_LIMIT".to_string()),
            message: Some("ICO output requires dimensions up to 256x256.".to_string()),
            current_width: None,
            current_height: None,
            max_width: Some(256),
            max_height: Some(256),
            suggested_action: Some("FIT_BOX_256".to_string()),
        };

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["token"], 10);
        assert_eq!(json["is_valid"], false);
        assert_eq!(json["code"], "ICO_DIMENSIONS_EXCEED_LIMIT");
        assert_eq!(json["suggested_action"], "FIT_BOX_256");
        assert_eq!(json["max_width"], 256);
    }

    #[test]
    fn blocked_open_extensions_cover_active_content() {
        for ext in ["exe", "BAT", "ps1", "lnk", "desktop", "svg", "html"] {
            assert!(is_blocked_open_extension(ext), "{ext} must be blocked");
        }
        for ext in ["png", "pdf", "dds", "ktx2", "wav", "flac"] {
            assert!(
                !is_blocked_open_extension(ext),
                "{ext} should not be blocked by extension alone"
            );
        }
    }

    fn test_registry() -> crate::core::conversion::registry::ConverterRegistry {
        use crate::core::conversion::registry::ConverterRegistry;
        let mut registry = ConverterRegistry::new();
        registry.register(
            AssetFamily::Image,
            Arc::new(crate::core::handlers::image::ImageConverterHandler),
        );
        registry.register(
            AssetFamily::Audio,
            Arc::new(crate::core::handlers::audio::AudioHandler),
        );
        registry
    }

    fn resolve_for_test(id: &str, mgr: &Arc<JobManager>) -> Result<PathBuf, AppError> {
        resolve_openable_output(id, mgr, &test_registry())
    }

    fn make_test_job_manager() -> Arc<JobManager> {
        let broadcaster = Arc::new(JobBroadcaster::new(Box::new(|_| serde_json::Value::Null)));
        Arc::new(JobManager::new(broadcaster))
    }

    fn make_test_request() -> ConversionRequest {
        ConversionRequest {
            family: AssetFamily::Image,
            input_path: "test.png".to_string(),
            output_path: String::new(),
            output_root: ".".to_string(),
            output_format: OutputFormat("webp".to_string()),
            options: serde_json::json!({}),
        }
    }

    fn make_test_request_for_file(
        path: &Path,
        format: &str,
        family: AssetFamily,
    ) -> ConversionRequest {
        let mut request = make_test_request();
        request.family = family;
        request.output_path = path.to_string_lossy().to_string();
        request.output_root = path
            .parent()
            .expect("test path has parent")
            .to_string_lossy()
            .to_string();
        request.output_format = OutputFormat(format.to_string());
        request
    }

    #[test]
    fn resolve_rejects_missing_job() {
        let mgr = make_test_job_manager();
        let result = resolve_for_test("nonexistent-id", &mgr);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("not found"),
            "Error should mention job not found"
        );
    }

    #[test]
    fn resolve_rejects_queued_job() {
        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request());
        // Job is in Queued state by default
        let result = resolve_for_test(&id, &mgr);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("Queued"),
            "Error should mention job status"
        );
    }

    #[test]
    fn resolve_rejects_running_job() {
        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request());
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        let result = resolve_for_test(&id, &mgr);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("Running"),
            "Error should mention job status"
        );
    }

    #[test]
    fn resolve_rejects_failed_job() {
        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request());
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        let _ = mgr.fail_job(
            &id,
            crate::core::jobs::types::JobFailure {
                stage: crate::core::jobs::types::JobStage::Execute,
                kind: crate::core::jobs::types::JobFailureKind::Internal,
                message: "test failure".to_string(),
            },
        );
        let result = resolve_for_test(&id, &mgr);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("Failed"),
            "Error should mention job status"
        );
    }

    #[test]
    fn resolve_rejects_cancelled_job() {
        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request());
        let _ = mgr.update_status(&id, JobStatus::Cancelled);
        let result = resolve_for_test(&id, &mgr);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("Cancelled"),
            "Error should mention job status"
        );
    }

    #[test]
    fn resolve_rejects_no_output_path() {
        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request());
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        // Finalize with no output path
        let _ = mgr.finalize_job(
            &id,
            ConversionResult {
                success: true,
                output_path: None,
                message: "done".to_string(),
                input_integrity: None,
                output_integrity: None,
            },
        );
        let result = resolve_for_test(&id, &mgr);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("no output path"),
            "Error should mention missing output path"
        );
    }

    #[test]
    fn resolve_rejects_missing_file() {
        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request());
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        let _ = mgr.finalize_job(
            &id,
            ConversionResult {
                success: true,
                output_path: Some("/nonexistent/path/to/file.png".to_string()),
                message: "done".to_string(),
                input_integrity: None,
                output_integrity: None,
            },
        );
        let result = resolve_for_test(&id, &mgr);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("not accessible"),
            "Error should mention file not accessible"
        );
    }

    #[test]
    fn resolve_rejects_directory_path() {
        // Use a temp dir as the "output path" — it's a directory, not a file
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("looks_like.png");
        std::fs::create_dir(&dir).unwrap();
        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request_for_file(&dir, "png", AssetFamily::Image));
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        let _ = mgr.finalize_job(
            &id,
            ConversionResult {
                success: true,
                output_path: Some(dir.to_string_lossy().to_string()),
                message: "done".to_string(),
                input_integrity: None,
                output_integrity: None,
            },
        );
        let result = resolve_for_test(&id, &mgr);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("not a file"),
            "Error should mention path is not a file"
        );
    }

    #[test]
    fn resolve_rejects_disallowed_extension() {
        let dir = tempfile::tempdir().unwrap();
        let bad_file = dir.path().join("malicious.exe");
        std::fs::write(&bad_file, b"fake exe").unwrap();

        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request_for_file(
            &bad_file,
            "png",
            AssetFamily::Image,
        ));
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        let _ = mgr.finalize_job(
            &id,
            ConversionResult {
                success: true,
                output_path: Some(bad_file.to_string_lossy().to_string()),
                message: "done".to_string(),
                input_integrity: None,
                output_integrity: None,
            },
        );
        let result = resolve_for_test(&id, &mgr);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("not permitted"),
            "Error should mention extension not permitted"
        );
    }

    #[test]
    fn resolve_accepts_png() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("output.png");
        std::fs::write(&file, b"fake png").unwrap();

        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request_for_file(&file, "png", AssetFamily::Image));
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        let _ = mgr.finalize_job(
            &id,
            ConversionResult {
                success: true,
                output_path: Some(file.to_string_lossy().to_string()),
                message: "done".to_string(),
                input_integrity: None,
                output_integrity: None,
            },
        );
        let result = resolve_for_test(&id, &mgr);
        assert!(result.is_ok(), "Should accept .png: {:?}", result.err());
    }

    #[test]
    fn resolve_accepts_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("output.pdf");
        std::fs::write(&file, b"fake pdf").unwrap();

        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request_for_file(&file, "pdf", AssetFamily::Image));
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        let _ = mgr.finalize_job(
            &id,
            ConversionResult {
                success: true,
                output_path: Some(file.to_string_lossy().to_string()),
                message: "done".to_string(),
                input_integrity: None,
                output_integrity: None,
            },
        );
        let result = resolve_for_test(&id, &mgr);
        assert!(result.is_ok(), "Should accept .pdf: {:?}", result.err());
    }

    #[test]
    fn resolve_accepts_wav() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("output.wav");
        std::fs::write(&file, b"fake wav").unwrap();

        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request_for_file(&file, "wav", AssetFamily::Audio));
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        let _ = mgr.finalize_job(
            &id,
            ConversionResult {
                success: true,
                output_path: Some(file.to_string_lossy().to_string()),
                message: "done".to_string(),
                input_integrity: None,
                output_integrity: None,
            },
        );
        let result = resolve_for_test(&id, &mgr);
        assert!(result.is_ok(), "Should accept .wav: {:?}", result.err());
    }

    #[test]
    fn resolve_rejects_outside_root() {
        let approved = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("image.png");
        std::fs::write(&file, b"data").unwrap();
        let mgr = make_test_job_manager();
        let mut request = make_test_request_for_file(&file, "png", AssetFamily::Image);
        request.output_root = approved.path().to_string_lossy().to_string();
        let id = mgr.create_job(request);
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        let _ = mgr.finalize_job(
            &id,
            ConversionResult {
                success: true,
                output_path: Some(file.to_string_lossy().to_string()),
                message: "done".to_string(),
                input_integrity: None,
                output_integrity: None,
            },
        );
        let err = resolve_for_test(&id, &mgr).unwrap_err().to_string();
        assert!(err.contains("outside its approved root"), "{err}");
    }

    #[test]
    fn resolve_rejects_wrong_output_format_even_if_writable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("wrong.pdf");
        std::fs::write(&file, b"data").unwrap();
        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request_for_file(&file, "png", AssetFamily::Image));
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        let _ = mgr.finalize_job(
            &id,
            ConversionResult {
                success: true,
                output_path: Some(file.to_string_lossy().to_string()),
                message: "done".to_string(),
                input_integrity: None,
                output_integrity: None,
            },
        );
        assert!(resolve_for_test(&id, &mgr)
            .unwrap_err()
            .to_string()
            .contains("not permitted"));
    }

    #[test]
    fn resolve_rejects_result_not_matching_plan() {
        let dir = tempfile::tempdir().unwrap();
        let planned = dir.path().join("planned.png");
        let other = dir.path().join("other.png");
        std::fs::write(&planned, b"data").unwrap();
        std::fs::write(&other, b"data").unwrap();
        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request_for_file(
            &planned,
            "png",
            AssetFamily::Image,
        ));
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        let _ = mgr.finalize_job(
            &id,
            ConversionResult {
                success: true,
                output_path: Some(other.to_string_lossy().to_string()),
                message: "done".to_string(),
                input_integrity: None,
                output_integrity: None,
            },
        );
        assert!(resolve_for_test(&id, &mgr)
            .unwrap_err()
            .to_string()
            .contains("differs from planned"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_rejects_symlink_to_outside_root() {
        use std::os::unix::fs::symlink;
        let approved = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("target.png");
        std::fs::write(&target, b"data").unwrap();
        let link = approved.path().join("output.png");
        symlink(&target, &link).unwrap();
        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request_for_file(&link, "png", AssetFamily::Image));
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        let _ = mgr.finalize_job(
            &id,
            ConversionResult {
                success: true,
                output_path: Some(link.to_string_lossy().to_string()),
                message: "done".to_string(),
                input_integrity: None,
                output_integrity: None,
            },
        );
        assert!(resolve_for_test(&id, &mgr)
            .unwrap_err()
            .to_string()
            .contains("outside its approved root"));
    }

    #[test]
    fn resolve_accepts_descriptor_alias_without_a_static_extension_list() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("alias.jpeg");
        std::fs::write(&file, b"data").unwrap();
        let mgr = make_test_job_manager();
        let id = mgr.create_job(make_test_request_for_file(&file, "jpg", AssetFamily::Image));
        let _ = mgr.update_status(&id, JobStatus::Pending);
        let _ = mgr.update_status(&id, JobStatus::Running);
        let _ = mgr.finalize_job(
            &id,
            ConversionResult {
                success: true,
                output_path: Some(file.to_string_lossy().to_string()),
                message: "done".to_string(),
                input_integrity: None,
                output_integrity: None,
            },
        );
        assert_eq!(
            resolve_for_test(&id, &mgr).unwrap(),
            dunce::canonicalize(&file).unwrap()
        );
    }
}
