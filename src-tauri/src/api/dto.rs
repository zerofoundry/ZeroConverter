use crate::core::conversion::types::ConversionResult;
use crate::core::jobs::{ConversionJob, JobProgress, QueueSummary};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct AppStatusDto {
    pub engine: String,
    pub version: String,
}

#[derive(Serialize)]
pub struct AppCapabilitiesDto {
    pub families: Vec<FamilyCapabilitiesDto>,
}

#[derive(Serialize)]
pub struct FamilyCapabilitiesDto {
    pub family: String,
    pub descriptors: Vec<FormatDescriptorDto>,
    pub routes: Vec<ConversionRouteDto>,
}

#[derive(Serialize)]
pub struct FormatDescriptorDto {
    pub id: String,
    pub canonical_extension: String,
    pub aliases: Vec<String>,
    pub can_read: bool,
    pub can_write: bool,
    /// What can be set when this format is the target, as the engine
    /// declares it (`core::conversion::options`). Empty for read-only formats.
    pub options: Vec<&'static crate::core::conversion::options::OptionSpec>,
}

#[derive(Serialize)]
pub struct ConversionRouteDto {
    pub input: String,
    pub outputs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct JobFailureDto {
    pub stage: String,
    pub kind: String,
    pub message: String,
}

#[derive(Serialize, Deserialize)]
pub struct JobStatusDto {
    pub id: String,
    pub status: String,
    pub request: JobRequestSnapshotDto,
    pub progress: JobProgressDto,
    pub failure: Option<JobFailureDto>,
    pub result: Option<ConversionResult>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct JobProgressDto {
    pub stage: String,
    pub message: String,
    pub percent: Option<f32>,
}

#[derive(Serialize)]
pub struct StartJobResultDto {
    pub job_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ConversionSubmitDto {
    pub input_path: String,
    pub output_folder: Option<String>,
    pub output_format: String,
    /// Every other field (quality, overwrite, resize, bitrate, DDS, ...)
    /// becomes the request's options unchanged. Each format family reads and
    /// checks its own, so a new option is not typed out again here.
    #[serde(flatten)]
    pub options: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JobRequestSnapshotDto {
    pub input_path: String,
    pub output_path: String,
    pub output_format: String,
    /// The options the job ran with, as they were submitted.
    #[serde(flatten)]
    pub options: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize)]
pub struct BatchSubmissionRequestDto {
    pub conversions: Vec<ConversionSubmitDto>,
}

#[derive(Serialize)]
pub struct BatchSubmissionResultDto {
    pub job_ids: Vec<String>,
}

#[derive(Deserialize)]
pub struct PreflightRequestDto {
    pub token: u32,
    pub conversion: ConversionSubmitDto,
}

#[derive(Serialize)]
pub struct PreflightResultDto {
    pub token: u32,
    pub is_valid: bool,
    pub severity: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
    pub current_width: Option<u32>,
    pub current_height: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub suggested_action: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct QueueSummaryDto {
    pub status: String,
    pub total: usize,
    pub queued: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
}

#[derive(Debug, Serialize, Clone)]
pub struct ConvertImageResultDto {
    pub success: bool,
    pub output_path: Option<String>,
    pub message: String,
}

impl From<ConversionResult> for ConvertImageResultDto {
    fn from(res: ConversionResult) -> Self {
        Self {
            success: res.success,
            output_path: res.output_path,
            message: res.message,
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", content = "payload")]
pub enum JobEventDto {
    JobStarted {
        id: String,
    },
    JobStatusChanged {
        id: String,
        status: String,
    },
    JobProgressUpdated {
        id: String,
        progress: JobProgressDto,
    },
    JobCompleted {
        id: String,
        result: ConvertImageResultDto,
    },
    JobFailed {
        id: String,
        failure: JobFailureDto,
    },
    QueueSummaryChanged {
        summary: QueueSummaryDto,
    },
}

impl From<ConversionJob> for JobStatusDto {
    fn from(job: ConversionJob) -> Self {
        let req = job.request;
        Self {
            id: job.id,
            status: format!("{:?}", job.status),
            request: JobRequestSnapshotDto {
                input_path: req.input_path,
                output_path: req.output_path,
                output_format: req.output_format.0,
                options: req.options.as_object().cloned().unwrap_or_default(),
            },
            progress: job.progress.into(),
            failure: job.failure.map(|f| JobFailureDto {
                stage: format!("{:?}", f.stage),
                kind: format!("{:?}", f.kind),
                message: f.message,
            }),
            result: job.result,
        }
    }
}

impl From<JobProgress> for JobProgressDto {
    fn from(p: JobProgress) -> Self {
        Self {
            stage: p.stage,
            message: p.message,
            percent: p.percent,
        }
    }
}

impl From<QueueSummary> for QueueSummaryDto {
    fn from(s: QueueSummary) -> Self {
        Self {
            status: format!("{:?}", s.status),
            total: s.total,
            queued: s.queued,
            running: s.running,
            completed: s.completed,
            failed: s.failed,
            cancelled: s.cancelled,
        }
    }
}

#[derive(Deserialize)]
pub struct BatchMetadataRequestDto {
    pub paths: Vec<String>,
}

#[derive(Serialize)]
pub struct FileMetadataDto {
    pub path: String,
    pub name: String,
    pub extension: String,
    pub size_bytes: u64,
    pub kind: String,         // image, audio, etc.
    pub probe_status: String, // Provisional, Recognized, Failed
    pub duration_seconds: Option<f64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
}

#[derive(Serialize)]
pub struct BatchMetadataResultDto {
    pub results: Vec<FileMetadataDto>,
}

#[derive(serde::Deserialize)]
pub struct ThumbnailRequestDto {
    pub path: String,
    pub size: u32,
    pub cache_key_hint: Option<String>,
}

#[derive(serde::Serialize)]
pub struct ThumbnailResultDto {
    pub cached_path: String,
    pub width: u32,
    pub height: u32,
    pub cache_hit: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submitted_options_pass_through_without_being_listed() {
        let dto: ConversionSubmitDto = serde_json::from_value(serde_json::json!({
            "input_path": "C:/a/photo.png",
            "output_folder": null,
            "output_format": "webp",
            "quality": 70,
            "overwrite": false,
            "some_new_option": "kept",
        }))
        .unwrap();
        assert_eq!(dto.options["quality"], 70);
        assert_eq!(dto.options["some_new_option"], "kept");
        assert!(!dto.options.contains_key("input_path"));
    }
}
