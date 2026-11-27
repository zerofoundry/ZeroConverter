// ==============================================================================
// OWNERSHIP:
//   Owns frontend conversion state, queue state, batch configuration, and submission intent.
//
// DO NOT ADD:
//   - Authoritative output path construction
//   - Backend business logic
//   - Direct Tauri invoke calls
//   - Format-specific conversion algorithms
//
// NOTES:
//   The frontend sends user intent; the backend generates final output paths.
// ==============================================================================
import { useState, useCallback, useEffect, useRef, useMemo } from "react";
import { optionSpecsFor, requestOptions, withValue, type OptionValues } from './outputOptions.mjs';
import i18n from "../i18n";
import { confirmAction, pickInputFiles, pickOutputDirectory } from "../platform/tauri/dialogGateway";
import { toAssetUrl } from "../platform/tauri/assetGateway";
import { converterClient, VideoEngineDeclined } from "./converterClient";
import { JobStatusDto, QueueSummaryDto, BatchItemRequest, JobEventDto, OptionSpecDto, OptionValue, PreflightResultDto, VideoEngineState } from "./converterTypes";
import { useAppCapabilities } from "./capabilitiesStore";
import { SettingsManager } from "./settingsStore";
import { availableOutputFormats, findFormatByExtension, pickSupportedFormat } from "./formatCatalog.mjs";

export interface AppStatus {
  engine: string;
  version: string;
}

/** Same limit as the backend's `LARGE_INPUT_BYTES`. */
const LARGE_INPUT_BYTES = 100 * 1024 * 1024;

function formatSize(bytes: number): string {
  const gb = bytes / 1024 ** 3;
  return gb >= 1 ? `${gb.toFixed(1)} GB` : `${Math.round(bytes / 1024 ** 2)} MB`;
}

export interface PendingFile {
  id: string;
  path: string;
  name: string;
  extension: string;
  sizeBytes: number;
  kind: 'image' | 'audio' | 'video' | 'unknown';
  probeStatus: 'Provisional' | 'Recognized' | 'Failed';
  status: 'Idle' | 'Ready' | 'Unsupported' | 'Error';
  thumbnailSrc?: string;
  // Audio Metadata (Optional)
  durationSeconds?: number;
  sampleRate?: number;
  channels?: number;
}

export function useConverterStatus() {
  const [status, setStatus] = useState<AppStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const refreshStatus = useCallback(async () => {
    setLoading(true);
    try {
      const res = await converterClient.getStatus();
      setStatus(res);
      setError(null);
    } catch (e: any) {
      setError(e.toString());
    } finally {
      setLoading(false);
    }
  }, []);

  return { status, error, loading, refreshStatus };
}

export function useBatchConverter() {
  // 1. Batch / Pending State
  const [pendingFiles, setPendingFiles] = useState<PendingFile[]>([]);
  const [selectedPendingId, setSelectedPendingId] = useState<string | null>(null);

  // What the engine can read, write and set (formats, routes, options).
  const { capabilities, getReadableExtensions } = useAppCapabilities();

  // 2. Global Batch Config
  const [format, setFormat] = useState("");
  const [overwrite, setOverwrite] = useState(false);
  const [outputDirectory, setOutputDirectory] = useState<string | null>(null);
  // Format options (quality, resize, bitrate, ...): whatever the engine
  // declares for the chosen target, remembered per scope.
  const [optionValues, setOptionValues] = useState<OptionValues>({});

  // Per-family targets (for mixed batches)
  const [familyFormats, setFamilyFormats] = useState<Record<string, string>>({});
  const inFlightProbes = useRef<Set<string>>(new Set());

  // 3. Queue & Jobs State (Sync from Backend)
  const [jobs, setJobs] = useState<JobStatusDto[]>([]);
  const [queueSummary, setQueueSummary] = useState<QueueSummaryDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);
  // First video job: the FFmpeg consent card, then its download progress.
  const [videoEngine, setVideoEngine] = useState<VideoEngineState | null>(null);
  const engineAnswerRef = useRef<((agreed: boolean) => void) | null>(null);
  const answerVideoEngine = useCallback((agreed: boolean) => {
    engineAnswerRef.current?.(agreed);
    engineAnswerRef.current = null;
  }, []);
  const cancelVideoEngine = useCallback(() => {
    converterClient.cancelVideoEngine().catch(() => {});
  }, []);
  const [isSettingsLoaded, setIsSettingsLoaded] = useState(false);

  // 🔴 Preflight State
  const [preflightResult, setPreflightResult] = useState<PreflightResultDto | null>(null);
  const preflightTokenRef = useRef(0);

  const streamCleanupRef = useRef<(() => void) | null>(null);
  const reconcileIntervalRef = useRef<number | null>(null);

  const syncQueue = useCallback(async () => {
    try {
      const [jobsList, summary] = await Promise.all([
        converterClient.listJobs(),
        converterClient.getQueueStatus()
      ]);
      setJobs(jobsList);
      setQueueSummary(summary);
    } catch (e: any) {
      setError(i18n.t("error.sync", { detail: String(e) }));
    }
  }, []);

  // 🔴 Settings Lifecycle: Load on Mount
  useEffect(() => {
    SettingsManager.load().then(settings => {
      setFormat(settings.format);
      setOverwrite(settings.overwrite);
      setOutputDirectory(settings.outputDirectory);
      setFamilyFormats(settings.familyFormats);
      setOptionValues(settings.optionValues);
      setIsSettingsLoaded(true);
    });
  }, []);

  // 🔴 Settings Lifecycle: Save on Change
  useEffect(() => {
    if (!isSettingsLoaded) return;
    SettingsManager.save({
      format,
      overwrite,
      outputDirectory,
      familyFormats,
      optionValues,
    }).catch((e) => setError(i18n.t("error.settingsSave", { detail: String(e) })));
  }, [format, overwrite, outputDirectory, familyFormats, optionValues, isSettingsLoaded]);

  // Detected families in current batch (excluding unknown)
  const detectedFamilies = useMemo(() => {
    const kinds = new Set(pendingFiles.filter(f => f.kind !== 'unknown').map(f => f.kind));
    return Array.from(kinds) as Array<'image' | 'audio' | 'video'>;
  }, [pendingFiles]);

  const isMixedBatch = useMemo(() => {
    return detectedFamilies.length > 1;
  }, [detectedFamilies]);

  // 🔴 Preflight Effect
  useEffect(() => {
    // Use explicitly selected file, or fall back to the first Ready image file.
    // This ensures the warning appears even when no file is clicked in the queue,
    // which is the common flow: drop files → pick format → convert.
    const activeFile = selectedPendingId
      ? pendingFiles.find(f => f.id === selectedPendingId)
      : pendingFiles.find(f => f.status === 'Ready' && f.kind === 'image');
    if (!activeFile || activeFile.status !== 'Ready' || activeFile.kind !== 'image') {
      setPreflightResult(null);
      return;
    }

    const fileFormat = isMixedBatch ? (familyFormats[activeFile.kind] ?? format) : format;
    if (fileFormat !== 'ico') {
      setPreflightResult(null);
      return;
    }

    const specs = optionSpecsFor(capabilities, activeFile.kind, fileFormat);
    const req: BatchItemRequest = {
      ...requestOptions(specs, optionValues, activeFile.kind, fileFormat),
      input_path: activeFile.path,
      output_folder: outputDirectory || null,
      output_format: fileFormat,
      overwrite,
    };

    const token = ++preflightTokenRef.current;

    const timeout = setTimeout(() => {
      converterClient.preflightConversion({ token, conversion: req })
        .then(res => {
          if (preflightTokenRef.current === res.token) {
            setPreflightResult(res);
          }
        })
        .catch(err => {
          console.error("Preflight failed", err);
          // Do not spam UI with generic errors during live editing
        });
    }, 300);

    return () => clearTimeout(timeout);
  }, [pendingFiles, selectedPendingId, format, familyFormats, isMixedBatch, overwrite, outputDirectory, optionValues, capabilities]);

  const handleJobEvent = useCallback((event: JobEventDto) => {
    switch (event.type) {
      case "JobStatusChanged":
        setJobs(prev => prev.map(j =>
          j.id === event.payload.id ? { ...j, status: event.payload.status } : j
        ));
        break;
      case "JobProgressUpdated":
        setJobs(prev => prev.map(j =>
          j.id === event.payload.id ? { ...j, progress: event.payload.progress } : j
        ));
        break;
      case "JobCompleted":
        setJobs(prev => prev.map(j =>
          j.id === event.payload.id ? { ...j, status: "Completed", result: event.payload.result } : j
        ));
        break;
      case "JobFailed":
        setJobs(prev => prev.map(j =>
          j.id === event.payload.id ? { ...j, status: "Failed", failure: event.payload.failure, result: { success: false, output_path: null, message: event.payload.failure.message } } : j
        ));
        break;
      case "QueueSummaryChanged":
        setQueueSummary(event.payload.summary);
        break;
      default:
        break;
    }
  }, []);

  // Sync Logic
  useEffect(() => {
    let mounted = true;
    const initialize = async () => {
      await syncQueue();
      if (!mounted) return;
      try {
        const resetStream = await converterClient.attachJobEvents(handleJobEvent);
        streamCleanupRef.current = resetStream;
      } catch (e) {
        console.error("Failed to attach job events:", e);
      }
    };
    initialize();
    reconcileIntervalRef.current = window.setInterval(syncQueue, 15000);
    return () => {
      mounted = false;
      if (streamCleanupRef.current) streamCleanupRef.current();
      if (reconcileIntervalRef.current) window.clearInterval(reconcileIntervalRef.current);
    };
  }, [syncQueue, handleJobEvent]);

  // Stable trigger: only re-run enrichment when the set of idle paths changes,
  // not on every pendingFiles reference change (which setPendingFiles causes).
  const idlePathsKey = useMemo(() => {
    return pendingFiles
      .filter(f => f.status === 'Idle' && !inFlightProbes.current.has(f.path))
      .map(f => f.path)
      .sort()
      .join('|');
  }, [pendingFiles]);

  // 🔴 Reactive Enrichment Loop (Workstation Grade)
  useEffect(() => {
    if (!idlePathsKey) return;

    const pathsToProbe = idlePathsKey.split('|');
    pathsToProbe.forEach(p => inFlightProbes.current.add(p));

    (async () => {
      try {
        const metadataResults = await converterClient.getFilesMetadata(pathsToProbe);

        setPendingFiles(prev => prev.map(file => {
          const normalize = (p: string) => p.replace(/\\/g, '/');
          const meta = metadataResults.find(m => normalize(m.path) === normalize(file.path));

          if (meta) {
            return {
              ...file,
              sizeBytes: meta.size_bytes,
              kind: meta.kind as any,
              probeStatus: meta.probe_status as any,
              durationSeconds: meta.duration_seconds,
              status: meta.kind === 'unknown' ? 'Unsupported' : 'Ready',
              thumbnailSrc: meta.kind === 'image' ? toAssetUrl(file.path) : undefined,
              sampleRate: meta.sample_rate,
              channels: meta.channels,
            };
          }

          if (pathsToProbe.includes(file.path)) {
            return { ...file, status: 'Error' };
          }

          return file;
        }));
      } catch (e) {
        console.error("Reactive enrichment failed:", e);
        setPendingFiles(prev => prev.map(file => {
          const normalize = (p: string) => p.replace(/\\/g, '/');
          const isTarget = pathsToProbe.some(p => normalize(p) === normalize(file.path));
          return isTarget && file.status === 'Idle' ? { ...file, status: 'Error' } : file;
        }));
      } finally {
        pathsToProbe.forEach(p => inFlightProbes.current.delete(p));
      }
    })();
  }, [idlePathsKey]);


  // Intake Actions
  const addFiles = useCallback(async (paths: string[]) => {
    setPendingFiles(prev => {
      const existing = new Set(prev.map(f => f.path));
      const filtered = paths.filter(p => !existing.has(p));

      if (filtered.length === 0) return prev;

      const skeletons: PendingFile[] = filtered.map(path => {
        const name = path.split(/[\\/]/).pop() || "unknown";
        const extension = name.split('.').pop() || "";

        const family = findFormatByExtension(capabilities, extension)?.family ?? null;

        return {
          id: crypto.randomUUID(),
          path,
          name,
          extension,
          sizeBytes: 0,
          kind: (family as 'image' | 'audio' | 'video') || 'unknown',
          probeStatus: 'Provisional',
          status: 'Idle'
        };
      });

      return [...prev, ...skeletons];
    });
  }, [capabilities]);

  const selectFiles = useCallback(async () => {
    if (!capabilities) return;

    // Group extensions by family, and one for all
    const allExts = getReadableExtensions();
    const filters = [{ name: i18n.t("dialog.allSupported"), extensions: allExts }];

    capabilities.families.forEach(f => {
      const exts = new Set<string>();
      f.descriptors.forEach(d => {
        if (d.can_read) {
          exts.add(d.canonical_extension.toLowerCase());
          d.aliases.forEach(a => exts.add(a.toLowerCase()));
        }
      });
      filters.push({
        name: i18n.t(`dialog.familyFiles_${f.family}`, { defaultValue: f.family }),
        extensions: Array.from(exts)
      });
    });

    const selected = await pickInputFiles(filters);
    if (selected.length > 0) {
      addFiles(selected);
    }
  }, [addFiles, capabilities, getReadableExtensions]);

  const removeFile = useCallback((id: string) => {
    setPendingFiles(prev => prev.filter(f => f.id !== id));
    if (selectedPendingId === id) setSelectedPendingId(null);
  }, [selectedPendingId]);

  const clearAll = useCallback(() => {
    setPendingFiles([]);
    setSelectedPendingId(null);
  }, []);

  // 🔴 Dev-only mock loader
  const devLoadMockPendingFiles = useCallback((mockFiles: PendingFile[]) => {
    if (!import.meta.env.DEV) {
      console.warn("devLoadMockPendingFiles is a development-only action.");
      return;
    }
    setPendingFiles(mockFiles);
  }, []);

  const devClearMockPendingFiles = useCallback(() => {
    if (!import.meta.env.DEV) {
      return;
    }
    clearAll();
  }, [clearAll]);

  const selectOutputDirectory = useCallback(async () => {
    const selected = await pickOutputDirectory(i18n.t("dialog.selectOutput"));
    if (selected) {
      setOutputDirectory(selected);
    }
  }, []);

  const clearOutputDirectory = useCallback(() => {
    setOutputDirectory(null);
  }, []);

  // Per-file route intersection is crucial when several input formats share one batch.
  const availableFormats = useMemo(() => {
    if (!capabilities) return [];
    const family = detectedFamilies[0] ?? capabilities.families[0]?.family;
    if (!family) return [];
    const inputs = pendingFiles
      .filter(f => f.kind === family && f.status !== "Unsupported" && f.status !== "Error")
      .map(f => f.extension);
    return availableOutputFormats(capabilities, family, inputs);
  }, [capabilities, detectedFamilies, pendingFiles]);

  const familyFormatOptions = useMemo(() => {
    const options: Record<string, string[]> = {};
    for (const family of capabilities?.families ?? []) {
      const inputs = pendingFiles
        .filter(f => f.kind === family.family && f.status !== "Unsupported" && f.status !== "Error")
        .map(f => f.extension);
      options[family.family] = availableOutputFormats(capabilities, family.family, inputs);
    }
    return options;
  }, [capabilities, pendingFiles]);

  /** What the options panel shows: per family in the batch, its target and
   *  the options the engine declares for it. With no files yet, the first
   *  family and the chosen format, so the panel is not empty. */
  const optionSections = useMemo(() => {
    if (!capabilities) return [];
    const targets = isMixedBatch
      ? detectedFamilies.map((family) => ({ family, target: familyFormats[family] ?? "" }))
      : [{ family: detectedFamilies[0] ?? capabilities.families[0]?.family ?? "", target: format }];
    return targets
      .filter(({ family, target }) => family && target)
      .map(({ family, target }) => ({ family, target, specs: optionSpecsFor(capabilities, family, target) }));
  }, [capabilities, isMixedBatch, detectedFamilies, familyFormats, format]);

  // Persisted settings may name formats that no longer exist or have no route.
  // Reconcile after BOTH the backend capability load and persisted setting load.
  useEffect(() => {
    if (!capabilities || !isSettingsLoaded) return;
    const safeFormat = pickSupportedFormat(format, availableFormats);
    if (safeFormat !== format) setFormat(safeFormat);
    setFamilyFormats(previous => {
      const next = { ...previous };
      let changed = false;
      for (const [family, options] of Object.entries(familyFormatOptions)) {
        const safe = pickSupportedFormat(previous[family] ?? "", options);
        if (previous[family] !== safe) {
          next[family] = safe;
          changed = true;
        }
      }
      return changed ? next : previous;
    });
  }, [capabilities, isSettingsLoaded, availableFormats, familyFormatOptions, format]);

  const canSubmit = useMemo(() => {
    const ready = pendingFiles.filter(f => f.status === "Ready" && f.kind !== "unknown");
    return ready.length > 0 && ready.every(file => {
      const selected = isMixedBatch ? familyFormats[file.kind] : format;
      return availableOutputFormats(capabilities, file.kind, [file.extension]).includes(selected);
    });
  }, [pendingFiles, isMixedBatch, familyFormats, format, capabilities]);

  /** Why Convert is unavailable, or what will be skipped, in plain words. */
  const submitHint = useMemo(() => {
    if (pendingFiles.length === 0) return null;
    const ready = pendingFiles.filter(f => f.status === "Ready" && f.kind !== "unknown");
    const skipped = pendingFiles.length - ready.length;
    const blocked = ready.find(file => {
      const selected = isMixedBatch ? familyFormats[file.kind] : format;
      return !selected || !availableOutputFormats(capabilities, file.kind, [file.extension]).includes(selected);
    });
    if (blocked) {
      const selected = isMixedBatch ? familyFormats[blocked.kind] : format;
      return selected
        ? i18n.t("hint.cannotConvert", { name: blocked.name, format: selected.toUpperCase() })
        : i18n.t("hint.pickFormat");
    }
    if (ready.length === 0) return i18n.t("hint.nothingReadable");
    if (skipped > 0) return i18n.t("hint.skipped", { count: skipped });
    return null;
  }, [pendingFiles, isMixedBatch, familyFormats, format, capabilities]);

  const submitBatch = useCallback(async () => {
    const validFiles = pendingFiles.filter(f => f.status === 'Ready' && f.kind !== 'unknown');
    if (validFiles.length === 0 || !canSubmit) return;

    // Files over 100 MB convert only after a yes (the backend checks too).
    const large = validFiles.filter(f => f.sizeBytes > LARGE_INPUT_BYTES);
    if (large.length > 0) {
      const what = large.length === 1
        ? i18n.t("dialog.largeOne", { name: large[0].name, size: formatSize(large[0].sizeBytes) })
        : i18n.t("dialog.largeMany", { count: large.length, size: formatSize(large.reduce((n, f) => n + f.sizeBytes, 0)) });
      const ok = await confirmAction(
        i18n.t("dialog.largeWarning", { what }),
        i18n.t("dialog.largeTitle"),
        i18n.t("rail.convert"),
        i18n.t("common.cancel"),
      );
      if (!ok) return;
    }

    setIsSubmitting(true);
    setError(null);

    try {
      const conversions: BatchItemRequest[] = validFiles.map(file => {
        // Per-family format resolution: in mixed batches, each family uses its own target
        const fileFormat = isMixedBatch
          ? (familyFormats[file.kind] || format)
          : format;

        // Check this exact route even if the UI changed during a pending async update.
        if (!availableOutputFormats(capabilities, file.kind, [file.extension]).includes(fileFormat)) {
          throw new Error(i18n.t("error.noRoute", { from: file.extension, to: fileFormat }));
        }

        // Only the options this target honours, as the engine declares them.
        const specs = optionSpecsFor(capabilities, file.kind, fileFormat);
        return {
          ...requestOptions(specs, optionValues, file.kind, fileFormat),
          input_path: file.path,
          output_folder: outputDirectory || null,
          output_format: fileFormat,
          overwrite,
          confirm_large: file.sizeBytes > LARGE_INPUT_BYTES,
        };
      });

      if (validFiles.some(f => f.kind === 'video')) {
        // First video job: ask once, download FFmpeg, then carry on.
        await converterClient.ensureVideoEngine((state) => {
          setVideoEngine(state);
          if (state.stage === "ask") {
            return new Promise<boolean>((resolve) => {
              engineAnswerRef.current = resolve;
            });
          }
        });
        setVideoEngine(null);
      }

      await converterClient.submitBatch(conversions);
      clearAll();
      await syncQueue();
    } catch (e: any) {
      // "Not now" or Cancel keeps the files queued; nothing to report.
      if (!(e instanceof VideoEngineDeclined)) setError(e.toString());
    } finally {
      setVideoEngine(null);
      engineAnswerRef.current = null;
      setIsSubmitting(false);
    }
  }, [pendingFiles, format, familyFormats, isMixedBatch, overwrite, outputDirectory, optionValues, syncQueue, clearAll, canSubmit, capabilities]);

  const cancelJob = useCallback(async (jobId: string) => {
    try {
      await converterClient.cancelJob(jobId);
      await syncQueue();
    } catch (e: any) {
      setError(i18n.t("error.cancel", { detail: String(e) }));
    }
  }, [syncQueue]);

  const resetBatch = useCallback(async () => {
    try {
      // 1. Purge terminal jobs on backend (Source of Truth)
      await converterClient.purgeJobs();

      // 2. Sync with backend truth
      await syncQueue();

      // 3. Clear local UI state
      clearAll();
      setError(null);
    } catch (e: any) {
      setError(i18n.t("error.reset", { detail: String(e) }));
    }
  }, [syncQueue, clearAll]);

  const setFamilyFormat = useCallback((family: string, fmt: string) => {
    setFamilyFormats(prev => ({ ...prev, [family]: fmt }));
  }, []);

  /** Sets one option for converting `family` files to `target`. */
  const setOptionValue = useCallback((family: string, target: string, spec: OptionSpecDto, value: OptionValue) => {
    setOptionValues(prev => withValue(prev, spec, family, target, value));
  }, []);

  /** The engine's own fix for a preflight warning, applied to the options. */
  const applySuggestion = useCallback((action: string, family: string, target: string) => {
    if (action !== "FIT_BOX_256") return;
    const specs = optionSpecsFor(capabilities, family, target);
    const set: Record<string, OptionValue> = { resize_mode: "fit_box", resize_width: 256, resize_height: 256 };
    setOptionValues(prev => specs
      .filter(spec => spec.key in set)
      .reduce((acc, spec) => withValue(acc, spec, family, target, set[spec.key]), prev));
  }, [capabilities]);

  return {
    pendingFiles,
    selectedPendingId,
    availableFormats,
    canSubmit,
    submitHint,
    isMixedBatch,
    detectedFamilies,
    familyFormats,
    familyFormatOptions,
    format,
    overwrite,
    outputDirectory,
    optionValues,
    optionSections,
    setOptionValue,
    applySuggestion,
    jobs,
    queueSummary,
    error,
    dismissError: () => setError(null),
    isSubmitting,
    videoEngine,
    answerVideoEngine,
    cancelVideoEngine,
    preflightResult,
    setSelectedPendingId,
    setFormat,
    setFamilyFormat,
    setOverwrite,
    selectOutputDirectory,
    clearOutputDirectory,
    addFiles,
    selectFiles,
    removeFile,
    clearAll,
    submitBatch,
    cancelJob,
    resetBatch,
    refreshQueue: syncQueue,
    isProcessing: queueSummary?.status === "Active" || queueSummary?.status === "Draining",
    devLoadMockPendingFiles,
    devClearMockPendingFiles,
  };
}
