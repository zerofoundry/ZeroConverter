use super::options::OptionSpec;
use super::types::{
    ConversionCapability, ConversionRequest, ConversionResult, FormatDescriptor, InputFormat,
};
use crate::app_error::AppError;
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub type ProgressFn = dyn Fn(f32, &str) + Send + Sync;

/// Cancellation and progress for handlers that run long enough to need them
/// (external processes such as FFmpeg). Pure in-process handlers ignore it.
#[derive(Clone, Default)]
pub struct ExecContext {
    pub token: Option<CancellationToken>,
    pub progress: Option<Arc<ProgressFn>>,
}

impl ExecContext {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.as_ref().is_some_and(|t| t.is_cancelled())
    }

    /// `percent` is 0..=100.
    pub fn report(&self, percent: f32, message: &str) {
        if let Some(progress) = &self.progress {
            progress(percent, message);
        }
    }
}

pub trait ConverterHandler: Send + Sync {
    /// The authoritative format descriptor table for this handler.
    /// All format metadata (extensions, aliases, read/write ability) derives from this.
    fn format_descriptors(&self) -> &'static [FormatDescriptor];

    /// Which file extensions this handler claims (provisional truth).
    /// Default implementation derives from format_descriptors() — readable formats + aliases.
    fn supported_extensions(&self) -> Vec<&'static str> {
        self.format_descriptors()
            .iter()
            .filter(|d| d.can_read)
            .flat_map(|d| std::iter::once(d.canonical_extension).chain(d.aliases.iter().copied()))
            .collect()
    }

    /// Probe a file to determine its true format (signature-first).
    fn probe(&self, path: &Path) -> Result<InputFormat, AppError>;

    fn capabilities(&self) -> Vec<ConversionCapability>;

    /// The options the encoder for `target` honours, in display order. A key
    /// listed here must change the output when set; one that is not listed is
    /// ignored for this target. See `conversion::options`.
    fn output_options(&self, _target: &str) -> Vec<&'static OptionSpec> {
        Vec::new()
    }

    fn validate(&self, request: &ConversionRequest) -> Result<(), AppError>;
    fn execute(&self, request: &ConversionRequest) -> Result<ConversionResult, AppError>;

    /// Like `execute`, with cancellation and progress. Handlers that cannot
    /// report either keep the default.
    fn execute_with(
        &self,
        request: &ConversionRequest,
        _ctx: &ExecContext,
    ) -> Result<ConversionResult, AppError> {
        self.execute(request)
    }
}
