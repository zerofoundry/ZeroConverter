import { useCallback, useEffect, useState, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useSettingsRequests } from "./application/videoEngine";
import { subscribeWindowDragDrop } from "./platform/tauri/windowGateway";
import { useConverterStatus, useBatchConverter, PendingFile } from "./application/converterStore";
import { useAppCapabilities } from "./application/capabilitiesStore";
import { useBatchCompletionNotice } from "./application/notifications";
import { useRailCollapsed } from "./application/useRailCollapsed";
import { revealCompletedOutput, openCompletedOutput } from "./platform/tauri/osGateway";
import { getThumbnailDebugStats, resetThumbnailDebugStats } from "./application/thumbnailService";

declare global {
  interface Window {
    __zeroLoadMockFiles?: (count?: number, mode?: "unique-fake-images" | "same-fake-image" | "mixed-non-image") => void;
    __zeroClearMockFiles?: () => void;
    __zeroThumbnailStats?: () => void;
    __zeroResetThumbnailStats?: () => void;
  }
}



// Layout Components
import { TopBar } from "./components/layout/TopBar";
import { LeftRail } from "./components/layout/LeftRail";

// Workspace Components
import { QueueWorkspace } from "./components/queue/QueueWorkspace";
import { InspectorPanel } from "./components/inspector/InspectorPanel";

// State Components
import { GlobalDropOverlay } from "./components/overlay/GlobalDropOverlay";
import { StartupScreen } from "./components/states/StartupScreen";
import { SettingsDialog } from "./components/settings/SettingsDialog";

function App() {
  const { t } = useTranslation();
  // 1. Core State & Store
  const { status, loading: statusLoading, refreshStatus } = useConverterStatus();
  const { capabilities, loading: capsLoading, error: capsError } = useAppCapabilities();
  const converter = useBatchConverter();
  
  // 2. UI State
  const [selectedJobId, _setSelectedJobId] = useState<string | null>(null);
  const [isDragging, setIsDragging] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  // A new object each time, so asking for the same page twice still opens it.
  const [settingsRequest, setSettingsRequest] = useState<{ section: string } | null>(null);
  const openSettingsAt = useCallback((section: string) => {
    setSettingsRequest({ section });
    setSettingsOpen(true);
  }, []);
  useSettingsRequests(openSettingsAt);
  const [railCollapsed, toggleRail] = useRailCollapsed();
  const [query, setQuery] = useState("");

  // 3. Derived State
  const selectedJob = converter.jobs.find(j => j.id === selectedJobId);

  // 4. Global Effects
  useEffect(() => {
    refreshStatus();
  }, [refreshStatus]);

  useBatchCompletionNotice(converter.queueSummary);

  // 5. Native Drag & Drop Orchestration (Tauri v2)
  const addFilesRef = useRef(converter.addFiles);
  useEffect(() => {
    addFilesRef.current = converter.addFiles;
  }, [converter.addFiles]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    subscribeWindowDragDrop({
      onHover: () => setIsDragging(true),
      onDrop: (paths) => {
        setIsDragging(false);
        if (paths.length > 0) {
          addFilesRef.current(paths);
        }
      },
      onLeave: () => setIsDragging(false),
    })
      .then((cleanup) => {
        unlisten = cleanup;
      })
      .catch(console.error);

    return () => {
      unlisten?.();
    };
  }, []);

  // 6. Dev-Only Stress Harness
  useEffect(() => {
    if (import.meta.env.DEV) {
      window.__zeroLoadMockFiles = (count = 10000, mode = "unique-fake-images") => {
        const mockFiles: PendingFile[] = [];

        for (let i = 0; i < count; i++) {
          let kind: 'image' | 'audio' | 'unknown' = 'image';
          let extension = 'jpg';
          let path = `C:\\FakePath\\image_${i}.jpg`;
          let name = `image_${i}.jpg`;

          if (mode === "same-fake-image") {
            path = "C:\\FakePath\\shared_image.jpg";
            name = "shared_image.jpg";
          } else if (mode === "mixed-non-image") {
            const types: ('image' | 'audio' | 'unknown')[] = ['image', 'audio', 'unknown'];
            kind = types[i % 3];
            extension = kind === 'image' ? 'jpg' : kind === 'audio' ? 'mp3' : 'txt';
            path = `C:\\FakePath\\file_${i}.${extension}`;
            name = `file_${i}.${extension}`;
          }

          mockFiles.push({
            id: crypto.randomUUID(),
            path,
            name,
            extension,
            sizeBytes: 1024 * 1024 * (i % 10 + 1),
            kind,
            probeStatus: 'Recognized',
            status: 'Ready'
          });
        }

        converter.devLoadMockPendingFiles(mockFiles);
        console.log(`Loaded ${count} mock files in mode: ${mode}`);
      };

      window.__zeroClearMockFiles = () => {
        converter.devClearMockPendingFiles();
        console.log("Cleared mock files.");
      };

      window.__zeroThumbnailStats = () => {
        console.table(getThumbnailDebugStats());
      };

      window.__zeroResetThumbnailStats = () => {
        resetThumbnailDebugStats();
        console.log("Reset thumbnail stats.");
      };
    }

    return () => {
      if (import.meta.env.DEV) {
        delete window.__zeroLoadMockFiles;
        delete window.__zeroClearMockFiles;
        delete window.__zeroThumbnailStats;
        delete window.__zeroResetThumbnailStats;
      }
    };
  }, [converter.devLoadMockPendingFiles, converter.devClearMockPendingFiles]);


  // 6. Startup: the mark and ring until the core has answered.
  if (statusLoading || (!capsError && (capsLoading || !capabilities))) {
    return <StartupScreen />;
  }

  // Derived: Selected pending file or selected job
  const selectedPendingFile = converter.pendingFiles.find(f => f.id === converter.selectedPendingId);

  // If capabilities fail to load: say what happened and what to do next.
  if (capsError || !capabilities) {
    return (
      <div className="grid h-screen place-items-center bg-rail px-6 text-text-primary">
        <div className="max-w-sm text-center" role="alert">
          <img src="/zeroconverter-mark.svg" alt="" width={40} height={40} className="mx-auto mb-5 opacity-80" />
          <h1 className="text-[15px] font-semibold">{t("startup.failedTitle")}</h1>
          <p className="mt-2 text-[13px] leading-relaxed text-text-secondary">
            {t("startup.failedBody")}
          </p>
          {capsError && <p className="mt-4 font-mono text-[11px] text-text-tertiary">{capsError}</p>}
        </div>
      </div>
    );
  }

  return (
    <div className="app-shell overflow-hidden" data-rail={railCollapsed ? "collapsed" : "open"}>

      {/*
        ======================================================================
        Global Drop Overlay
        ======================================================================
      */}
      <GlobalDropOverlay isDragging={isDragging} />

      {/* 🔴 Layer 2: Shell Infrastructure */}
      <TopBar query={query} onQueryChange={setQuery} onOpenSettings={() => setSettingsOpen(true)} />
      <SettingsDialog
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        version={status?.version || "0.0.0"}
        request={settingsRequest}
      />

      <LeftRail 
        onAddFiles={converter.selectFiles}
        selectedCount={converter.pendingFiles.filter((f) => f.status === "Ready" && f.kind !== "unknown").length}
        format={converter.format}
        setFormat={converter.setFormat}
        availableFormats={converter.availableFormats}
        isProcessing={converter.isProcessing}
        isMixedBatch={converter.isMixedBatch}
        detectedFamilies={converter.detectedFamilies}
        familyFormats={converter.familyFormats}
        setFamilyFormat={converter.setFamilyFormat}
        familyFormatOptions={converter.familyFormatOptions}
        optionSections={converter.optionSections}
        optionValues={converter.optionValues}
        setOptionValue={converter.setOptionValue}
        applySuggestion={converter.applySuggestion}
        overwrite={converter.overwrite}
        setOverwrite={converter.setOverwrite}
        collapsed={railCollapsed}
        onToggleCollapsed={toggleRail}
        onSubmit={converter.submitBatch}
        canSubmit={converter.canSubmit}
        submitHint={converter.submitHint}
        version={status?.version || "0.0.0"}
        preflightResult={converter.preflightResult}
        outputDirectory={converter.outputDirectory}
      />

      {/* 🟢 Layer 4: Primary Content & Layout */}
      <QueueWorkspace 
        jobs={converter.jobs}
        pendingFiles={converter.pendingFiles}
        query={query}
        summary={converter.queueSummary}
        selectedJobId={selectedJobId}
        selectedPendingId={converter.selectedPendingId}
        onSelectJob={_setSelectedJobId}
        onSelectPending={converter.setSelectedPendingId}
        onCancelJob={converter.cancelJob}
        onRemoveFile={converter.removeFile}
        onClearAll={converter.clearAll}
        onResetBatch={converter.resetBatch}
        onAddFiles={converter.selectFiles}
        capabilities={capabilities}
        onReveal={revealCompletedOutput}
        isDragging={isDragging}
        isSubmitting={converter.isSubmitting}
        videoEngine={converter.videoEngine}
        onAnswerVideoEngine={converter.answerVideoEngine}
        onCancelVideoEngine={converter.cancelVideoEngine}
        error={converter.error}
        onDismissError={converter.dismissError}
      />

      <InspectorPanel 
        selectedJob={selectedJob}
        selectedPendingFile={selectedPendingFile}
        isProcessing={converter.isProcessing}
        pendingCount={converter.pendingFiles.length}
        format={converter.format}
        targetSummary={
          converter.isMixedBatch
            ? converter.detectedFamilies
                .map((family) => `${t(`common.family.${family}`)} → ${(converter.familyFormats[family] ?? "").toUpperCase()}`)
                .join(", ")
            : undefined
        }
        outputDirectory={converter.outputDirectory}
        selectOutputDirectory={converter.selectOutputDirectory}
        clearOutputDirectory={converter.clearOutputDirectory}
        capabilities={capabilities}
        onRevealOutput={revealCompletedOutput}
        onOpenFile={openCompletedOutput}
      />
    </div>
  );
}

export default App;
