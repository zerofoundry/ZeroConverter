import React from "react";
import { OptionSpecDto, OptionValue, PreflightResultDto } from "../../application/converterTypes";
import { ArrowRight, ChevronLeft } from "lucide-react";
import { useTranslation } from "react-i18next";
import { FormatPicker } from "../common/FormatPicker";
import { OptionFields } from "../options/OptionFields";
import type { OptionValues } from "../../application/outputOptions.mjs";

export interface OptionSection {
  family: string;
  target: string;
  specs: OptionSpecDto[];
}

interface LeftRailProps {
  onAddFiles: () => void;
  selectedCount: number;
  format: string;
  setFormat: (f: string) => void;
  availableFormats: string[];
  isProcessing: boolean;
  isMixedBatch: boolean;
  detectedFamilies: Array<'image' | 'audio' | 'video'>;
  familyFormats: Record<string, string>;
  setFamilyFormat: (family: string, fmt: string) => void;
  familyFormatOptions: Record<string, string[]>;
  /** Per family being converted: its target and the options the engine declares. */
  optionSections: OptionSection[];
  optionValues: OptionValues;
  setOptionValue: (family: string, target: string, spec: OptionSpecDto, value: OptionValue) => void;
  applySuggestion: (action: string, family: string, target: string) => void;
  overwrite: boolean;
  setOverwrite: (val: boolean) => void;
  onSubmit: () => void;
  canSubmit: boolean;
  submitHint?: string | null;
  version: string;
  preflightResult: PreflightResultDto | null;
  outputDirectory: string | null;
  collapsed: boolean;
  onToggleCollapsed: () => void;
}

export const LeftRail: React.FC<LeftRailProps> = ({
  onAddFiles: _onAddFiles,
  selectedCount,
  format,
  setFormat,
  availableFormats,
  isProcessing,
  isMixedBatch,
  detectedFamilies,
  familyFormats,
  setFamilyFormat,
  familyFormatOptions,
  optionSections,
  optionValues,
  setOptionValue,
  applySuggestion,
  overwrite,
  setOverwrite,
  onSubmit,
  canSubmit,
  submitHint,
  version: _version,
  preflightResult,
  outputDirectory: _outputDirectory,
  collapsed,
  onToggleCollapsed,
}) => {
  const { t } = useTranslation();
  const isIcoTarget = isMixedBatch
    ? Object.entries(familyFormats).some(([fam, fmt]) => fam === "image" && fmt === "ico")
    : format === "ico";
  const imageSection = optionSections.find((section) => section.family === "image");

  const convertLabel = isProcessing
    ? t("rail.converting")
    : selectedCount > 0
      ? t("rail.convertCount", { count: selectedCount })
      : t("rail.convert");
  // Rail header: the whole row folds and unfolds the panel.
  const railHeader = (
    <button
      type="button"
      onClick={onToggleCollapsed}
      className="zc-rail-logo"
      data-press="wide"
      aria-label={collapsed ? t("rail.show") : t("rail.hide")}
      aria-expanded={!collapsed}
      title={collapsed ? t("rail.showTitle") : undefined}
    >
      {collapsed ? (
        <img src="/zeroconverter-mark.svg" alt="" width={20} height={20} className="h-5 w-5 shrink-0" />
      ) : (
        <>
          <span className="zc-rail-brand">ZeroConverter</span>
          <ChevronLeft size={14} aria-hidden="true" className="zc-rail-fold rtl:-scale-x-100" />
        </>
      )}
    </button>
  );

  if (collapsed) {
    return (
      <aside className="shell-left overflow-hidden flex flex-col items-center select-none">
        {railHeader}
        
        <div className="flex-1" />
        <button
          type="button"
          onClick={onSubmit}
          disabled={isProcessing || !canSubmit}
          title={submitHint ?? convertLabel}
          aria-label={convertLabel}
          className="zc-btn zc-btn-primary mb-3 !h-9 !w-9 !p-0"
        >
          <ArrowRight size={17} strokeWidth={2} aria-hidden="true" />
        </button>
      </aside>
    );
  }

  return (
    <aside className="shell-left overflow-hidden flex flex-col select-none">
      {railHeader}

      <div className="flex h-8 shrink-0 items-end px-4 pb-1.5">
        <h2 className="zc-label">{t("rail.convertTo")}</h2>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto px-4 pb-4 space-y-5">
        <section className="space-y-4">
          {isMixedBatch ? (
            detectedFamilies.map((family) => (
              <div key={family} className="space-y-2">
                <h3 className="zc-label">{t(`common.family.${family}`)}</h3>
                <FormatPicker
                  label={t("rail.familyOutputFormat", { family: t(`common.family.${family}`) })}
                  options={familyFormatOptions[family] ?? []}
                  value={familyFormats[family] ?? ""}
                  onChange={(selected) => setFamilyFormat(family, selected)}
                  disabled={isProcessing}
                />
              </div>
            ))
          ) : (
            <FormatPicker
              label={t("rail.outputFormat")}
              options={availableFormats}
              value={format}
              onChange={setFormat}
              disabled={isProcessing}
            />
          )}
          {isIcoTarget && (
            <p className="type-helper text-text-tertiary">{t("rail.icoLimit")}</p>
          )}
        </section>

        <section className="space-y-3">
          <h3 className="zc-label">{t("rail.options")}</h3>

          {optionSections.map((section) => (
            <div key={`${section.family}:${section.target}`} className="space-y-3">
              {isMixedBatch && section.specs.length > 0 && (
                <h4 className="zc-label">
                  {t(`common.family.${section.family}`)} → {section.target.toUpperCase()}
                </h4>
              )}
              <OptionFields
                idPrefix={`opt-${section.family}`}
                specs={section.specs}
                values={optionValues}
                family={section.family}
                target={section.target}
                onChange={(spec, value) => setOptionValue(section.family, section.target, spec, value)}
                disabled={isProcessing}
              />
            </div>
          ))}

          <label
            className="flex cursor-pointer items-center gap-2.5 py-1"
            title={overwrite ? t("rail.overwriteOn") : t("rail.overwriteOff")}
          >
            <input
              type="checkbox"
              checked={overwrite}
              disabled={isProcessing}
              onChange={(e) => setOverwrite(e.target.checked)}
              className="zc-check"
            />
            <span className="type-control-label text-text-secondary">{t("rail.overwrite")}</span>
          </label>
        </section>

        {preflightResult && preflightResult.is_valid === false && preflightResult.severity === "error" && (
          <div className="rounded-card border border-warning-border bg-warning-soft p-3" role="alert">
            <p className="text-[12px] leading-snug text-warning">{preflightResult.message}</p>
            {preflightResult.suggested_action === "FIT_BOX_256" && imageSection && (
              <button
                type="button"
                onClick={() => applySuggestion("FIT_BOX_256", imageSection.family, imageSection.target)}
                className="zc-btn zc-btn-sm mt-2 border-warning-border text-warning hover:bg-warning-border/40"
              >
                {t("rail.fitTo256")}
              </button>
            )}
          </div>
        )}
      </div>

      <div className="shrink-0 space-y-2 p-4">
        {submitHint && !isProcessing && (
          <p className={`text-[12px] leading-relaxed ${canSubmit ? "text-text-tertiary" : "text-warning"}`} role="status">
            {submitHint}
          </p>
        )}
        <button
          type="button"
          onClick={onSubmit}
          disabled={isProcessing || !canSubmit}
          className="zc-btn zc-btn-lg zc-btn-primary w-full"
          data-press="wide"
        >
          {convertLabel}
        </button>
      </div>
    </aside>
  );
};
