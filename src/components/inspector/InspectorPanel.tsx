import React from "react";
import { JobStatusDto, AppCapabilitiesDto } from "../../application/converterTypes";
import { PendingFile } from "../../application/converterStore";
import { ExternalLink, FolderOpen } from "lucide-react";
import { useTranslation } from "react-i18next";
import { FileTypeIcon } from "../common/FileTypeIcon";
import { useThumbnail } from "../../application/thumbnailService";
import { findFormatByExtension } from "../../application/formatCatalog.mjs";
import { optionSpecsFor } from "../../application/outputOptions.mjs";
import { optionLabel, valueLabel } from "../options/OptionFields";

interface InspectorPanelProps {
  selectedJob?: JobStatusDto;
  selectedPendingFile?: PendingFile;
  isProcessing: boolean;
  pendingCount: number;
  format: string;
  /** "jpg → PNG, mp3 → WAV" style summary of where the waiting files go. */
  targetSummary?: string;
  outputDirectory: string | null;
  selectOutputDirectory: () => void;
  clearOutputDirectory: () => void;
  onRevealOutput?: (jobId: string) => void | Promise<void>;
  capabilities: AppCapabilitiesDto;
  onOpenFile?: (jobId: string) => void | Promise<void>;
}

/** Shared size formatter */
const formatBytes = (bytes: number): string => {
  if (bytes >= 1048576) return `${(bytes / 1048576).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
};

/** A titled group of facts. Groups are separated by a hairline, not boxed. */
const Section: React.FC<{ label: string; tone?: 'default' | 'danger'; children: React.ReactNode }> = ({ label, tone = 'default', children }) => (
  <section className="space-y-1.5 border-t border-separator px-4 py-3.5">
    <h3 className={`zc-label mb-2 ${tone === 'danger' ? '!text-danger' : ''}`}>{label}</h3>
    {children}
  </section>
);

const MetaRow: React.FC<{ label: string; value: React.ReactNode; tone?: string }> = ({ label, value, tone }) => (
  <div className="zc-meta">
    <span>{label}</span>
    <span className={tone}>{value}</span>
  </div>
);

export const InspectorPanel: React.FC<InspectorPanelProps> = ({
  selectedJob,
  selectedPendingFile,
  isProcessing,
  pendingCount,
  format,
  targetSummary,
  outputDirectory,
  selectOutputDirectory,
  clearOutputDirectory,
  onRevealOutput,
  capabilities,
  onOpenFile,
}) => {
  const { t } = useTranslation();
  const isSelected = !!selectedJob || !!selectedPendingFile;

  // Local error state for Open File failures
  const [openFileError, setOpenFileError] = React.useState<string | null>(null);

  // Clear error when selected job changes
  React.useEffect(() => {
    setOpenFileError(null);
  }, [selectedJob?.id]);

  // Preview is attempted only for readable image descriptors returned by Core.
  const previewPath = React.useMemo(() => {
    const imageFamily = capabilities.families.find(f => f.family === "image");
    const readableOutput = imageFamily?.descriptors.some(d =>
      d.id === selectedJob?.request.output_format && d.can_read && d.can_write
    );
    if (selectedJob?.status === "Completed" && selectedJob.result?.output_path && readableOutput) {
      return selectedJob.result.output_path;
    }
    if (selectedPendingFile?.kind === "image") return selectedPendingFile.path;
    return undefined;
  }, [selectedJob, selectedPendingFile, capabilities]);

  const thumbnail = useThumbnail(previewPath);

  // Derive input extension from job request path
  const jobInputExtension = React.useMemo(() => {
    if (!selectedJob) return null;
    const name = selectedJob.request.input_path.split(/[\\/]/).pop() || '';
    const dotIdx = name.lastIndexOf('.');
    return dotIdx >= 0 ? name.substring(dotIdx + 1).toUpperCase() : null;
  }, [selectedJob]);

  // The options the job ran with, labelled the way the panel labels them.
  // Only options the engine declares for that target, and only those in play
  // (no width for "Original size").
  const usedOptions = React.useMemo(() => {
    if (!selectedJob) return [];
    const request = selectedJob.request;
    const family = jobInputExtension
      ? findFormatByExtension(capabilities, jobInputExtension)?.family
      : undefined;
    if (!family) return [];
    const specs = optionSpecsFor(capabilities, family, request.output_format);
    return specs
      .filter((spec) => request[spec.key] !== undefined)
      .filter((spec) => !spec.show_when || spec.show_when.values.includes(String(request[spec.show_when.key])))
      .map((spec) => ({
        key: spec.key,
        label: optionLabel(t, spec),
        value: valueLabel(t, spec, request[spec.key] ?? null),
      }));
  }, [selectedJob, jobInputExtension, capabilities, t]);

  const fileName = selectedJob?.request.input_path.split(/[\\/]/).pop() || selectedPendingFile?.name;
  const inputSize = selectedPendingFile?.sizeBytes || selectedJob?.result?.input_integrity?.byte_size || 0;
  const outputSize = selectedJob?.result?.output_integrity?.byte_size ?? 0;
  const outputPath = selectedJob?.status === 'Completed' ? selectedJob.result?.output_path : null;
  const jobTone =
    selectedJob?.status === 'Completed' ? '!text-success'
    : selectedJob?.status === 'Failed' ? '!text-danger'
    : selectedJob?.status === 'Cancelled' ? '!text-text-tertiary'
    : undefined;
  const probeTone =
    selectedPendingFile?.probeStatus === 'Recognized' ? '!text-success'
    : selectedPendingFile?.probeStatus === 'Failed' ? '!text-danger'
    : '!text-text-tertiary';

  const openFile = () => {
    if (!selectedJob || !onOpenFile) return;
    setOpenFileError(null);
    try {
      const result = onOpenFile(selectedJob.id);
      if (result instanceof Promise) {
        result.catch((err) => {
          console.error('Open file failed:', err);
          setOpenFileError(t("error.openFailed"));
        });
      }
    } catch (err) {
      console.error('Open file failed:', err);
      setOpenFileError(t("error.openFailed"));
    }
  };

  const reveal = () => {
    if (!selectedJob || !onRevealOutput) return;
    const result = onRevealOutput(selectedJob.id);
    if (result instanceof Promise) result.catch(console.error);
  };

  return (
    <aside className="shell-inspector flex h-full select-none flex-col overflow-hidden">
      <div className="flex h-11 shrink-0 items-center px-4">
        <h2 className="type-panel-header text-text-primary">{t("inspector.title")}</h2>
      </div>

      <div className="flex-1 overflow-y-auto">
        {isSelected ? (
          <div className="flex flex-col">
            <div className="px-4 pb-4">
              <div className="relative grid aspect-[4/3] w-full place-items-center overflow-hidden rounded-card bg-subtle">
                {previewPath && !thumbnail ? (
                  <div className="h-full w-full animate-pulse bg-surface-2" />
                ) : thumbnail ? (
                  <img src={thumbnail} className="h-full w-full object-contain" alt={t("inspector.preview")} />
                ) : (
                  <FileTypeIcon
                    kind={(selectedJob?.status === 'Completed'
                      ? capabilities.families.find(f => f.descriptors.some(d => d.id === selectedJob.request.output_format))?.family ?? 'unknown'
                      : selectedPendingFile?.kind || 'unknown') as any}
                    className="h-10 w-10 opacity-30"
                  />
                )}
              </div>
              <p className="mt-3 break-all text-[13px] font-medium leading-snug text-text-primary">{fileName}</p>
            </div>

            <Section label={t("inspector.source")}>
              <MetaRow label={t("inspector.format")} value={(jobInputExtension || selectedPendingFile?.extension || '—').toUpperCase()} />
              {inputSize > 0 && <MetaRow label={t("inspector.size")} value={formatBytes(inputSize)} />}
              {selectedPendingFile && (
                <MetaRow
                  label={t("inspector.status")}
                  tone={probeTone}
                  value={
                    selectedPendingFile.probeStatus === "Recognized" ? t("status.Recognized")
                    : selectedPendingFile.probeStatus === "Provisional" ? t("status.Provisional")
                    : t("status.cantBeRead")
                  }
                />
              )}
              {selectedPendingFile?.kind === 'audio' && (
                <>
                  <MetaRow
                    label={t("inspector.duration")}
                    value={
                      selectedPendingFile.durationSeconds
                        ? `${Math.floor(selectedPendingFile.durationSeconds / 60)}:${Math.floor(selectedPendingFile.durationSeconds % 60).toString().padStart(2, '0')}`
                        : t("common.unknown")
                    }
                  />
                  <MetaRow label={t("inspector.sampleRate")} value={selectedPendingFile.sampleRate ? `${selectedPendingFile.sampleRate / 1000} kHz` : t("common.unknown")} />
                  <MetaRow
                    label={t("inspector.channels")}
                    value={
                      selectedPendingFile.channels === 1 ? t("inspector.mono")
                      : selectedPendingFile.channels === 2 ? t("inspector.stereo")
                      : selectedPendingFile.channels ? t("inspector.channelsCount", { count: selectedPendingFile.channels }) : t("common.unknown")
                    }
                  />
                </>
              )}
            </Section>

            {selectedJob && (
              <Section label={t("inspector.settingsUsed")}>
                <MetaRow label={t("inspector.format")} value={selectedJob.request.output_format.toUpperCase()} />
                {usedOptions.map((option) => (
                  <MetaRow key={option.key} label={option.label} value={option.value} />
                ))}
                <MetaRow label={t("inspector.overwrite")} value={selectedJob.request.overwrite ? t("common.yes") : t("common.no")} />
              </Section>
            )}

            {selectedJob && (
              <Section label={t("inspector.result")}>
                <MetaRow label={t("inspector.status")} tone={jobTone} value={t(`status.${selectedJob.status}`, { defaultValue: selectedJob.status })} />
                {outputSize > 0 && <MetaRow label={t("inspector.size")} value={formatBytes(outputSize)} />}

                {outputPath && (
                  <div className="space-y-2 pt-1.5">
                    <p className="break-all font-mono text-[11px] leading-relaxed text-text-tertiary">{outputPath}</p>
                    <div className="flex gap-2">
                      {onOpenFile && (
                        <button type="button" onClick={openFile} className="zc-btn zc-btn-secondary flex-1" aria-label={t("inspector.openLabel")}>
                          <ExternalLink size={13} strokeWidth={1.75} aria-hidden="true" />
                          {t("inspector.open")}
                        </button>
                      )}
                      <button type="button" onClick={reveal} className="zc-btn zc-btn-secondary flex-1" title={t("inspector.showInExplorer")}>
                        <FolderOpen size={13} strokeWidth={1.75} aria-hidden="true" />
                        {t("inspector.showInFolder")}
                      </button>
                    </div>
                    {openFileError && <p className="text-[12px] leading-relaxed text-danger">{openFileError}</p>}
                  </div>
                )}

                {selectedJob.status === 'Failed' && !selectedJob.result?.output_path && (
                  <p className="pt-1 text-[12px] leading-relaxed text-text-tertiary">
                    {t("inspector.noFileWritten")}
                  </p>
                )}
              </Section>
            )}

            {selectedJob?.failure && (
              <Section label={t("inspector.wentWrong")} tone="danger">
                <MetaRow label={t("inspector.step")} value={selectedJob.failure.stage} />
                <MetaRow label={t("inspector.kind")} value={selectedJob.failure.kind} />
                <p className="mt-2 whitespace-pre-wrap break-words rounded-control bg-subtle p-2.5 font-mono text-[11px] leading-relaxed text-text-secondary select-text">
                  {selectedJob.failure.message}
                </p>
              </Section>
            )}
          </div>
        ) : (
          <div className="flex h-full min-h-[240px] flex-col items-center justify-center gap-1 p-8 text-center">
            <p className="text-[13px] text-text-secondary">
              {pendingCount > 0 ? t("inspector.selectToSee") : t("inspector.none")}
            </p>
            {pendingCount > 0 && (
              <p className="text-[12px] tabular-nums text-text-tertiary">
                {t("inspector.summary", { files: t("common.fileCount", { count: pendingCount }), target: targetSummary || format.toUpperCase() })}
              </p>
            )}
          </div>
        )}
      </div>

      {/* Output folder */}
      <div className="shrink-0 space-y-2 border-t border-separator p-4">
        <h3 className="zc-label">{t("inspector.saveTo")}</h3>
        <div className="flex items-center gap-2">
          <span
            className="min-w-0 flex-1 truncate text-[12px] text-text-secondary"
            title={outputDirectory ?? t("inspector.nextToSourceTitle")}
          >
            {outputDirectory ? outputDirectory.split(/[\\/]/).slice(-2).join("/") : t("inspector.nextToSource")}
          </span>
          {outputDirectory && (
            <button
              type="button"
              onClick={clearOutputDirectory}
              disabled={isProcessing}
              className="zc-btn zc-btn-sm zc-btn-ghost"
              title={t("inspector.resetTitle")}
            >
              {t("inspector.reset")}
            </button>
          )}
          <button type="button" onClick={selectOutputDirectory} disabled={isProcessing} className="zc-btn zc-btn-sm zc-btn-secondary">
            {t("inspector.change")}
          </button>
        </div>
      </div>
    </aside>
  );
};