import React, { useEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { quickJob, type QuickJobInfo } from "../../application/renameConvert";
import { requestOptions, withValue, type OptionValues } from "../../application/outputOptions.mjs";
import { OptionFields } from "../options/OptionFields";

interface QuickConvertProps {
  id: string;
}

/**
 * The small window a `+` rename opens (`photo.webp+`): the options the engine
 * marks `quick` for this target, starting from the main window's choices,
 * plus the longest side for formats that can be resized. Enter converts, Esc
 * cancels.
 */
export const QuickConvert: React.FC<QuickConvertProps> = ({ id }) => {
  const { t } = useTranslation();
  const [info, setInfo] = useState<QuickJobInfo | null>(null);
  const [values, setValues] = useState<OptionValues>({});
  const [maxSide, setMaxSide] = useState<string>("");
  const headerRef = useRef<HTMLDivElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const footerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    quickJob.info(id).then(setInfo).catch(() => setInfo(null));
    quickJob.savedValues().then(setValues).catch(() => {});
  }, [id]);

  const specs = (info?.options ?? []).filter((spec) => spec.quick);
  const resizable = (info?.options ?? []).some((spec) => spec.key === "resize_mode");
  const maxSideLimit = info?.options.find((spec) => spec.key === "resize_width")?.max;

  const confirm = () => {
    if (!info) return;
    const side = Number(maxSide);
    const resize = resizable && side > 0;
    quickJob
      .confirm(id, {
        ...requestOptions(specs, values, info.family, info.target),
        ...(resizable
          ? { resize_mode: resize ? "fit_box" : "none", resize_width: resize ? side : null, resize_height: resize ? side : null }
          : {}),
      })
      .catch(() => {});
  };
  const cancel = () => {
    quickJob.cancel(id).catch(() => {});
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") cancel();
      // A focused button or menu answers Enter itself (Cancel must stay Cancel).
      const t = e.target as HTMLElement | null;
      const ownsEnter = !!t?.closest("button, select, a, textarea, [role='button']");
      if (e.key === "Enter" && !ownsEnter) confirm();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const stem = info?.name.replace(/\.[^.]+$/, "") ?? "";

  // The window takes the height of what it shows: more options (WebP lossy
  // adds a quality slider) must never push Convert out of view.
  useEffect(() => {
    const body = bodyRef.current;
    if (!body) return;
    const fit = () =>
      quickJob.fitWindow(
        WINDOW_WIDTH,
        (headerRef.current?.offsetHeight ?? 0) + body.offsetHeight + (footerRef.current?.offsetHeight ?? 0),
      );
    const observer = new ResizeObserver(fit);
    observer.observe(body);
    fit();
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (info) quickJob.titleWindow(`${stem}.${info.target} · ZeroConverter`);
  }, [info, stem]);

  return (
    <div className="flex h-screen flex-col bg-titlebar text-text-primary select-none">
      <div ref={headerRef} data-tauri-drag-region className="flex h-11 shrink-0 items-center gap-2 ps-4 pe-1.5">
        <img src="/zeroconverter-mark.svg" alt="" width={16} height={16} className="pointer-events-none h-4 w-4" />
        <span className="pointer-events-none flex-1 font-brand text-[13px] font-semibold">ZeroConverter</span>
        <button
          type="button"
          onClick={cancel}
          aria-label={t("quick.cancel")}
          title={t("quick.cancel")}
          className="grid h-8 w-8 place-items-center rounded-control text-text-secondary transition-colors duration-150 hover:bg-surface-2 hover:text-text-primary"
        >
          <X size={15} aria-hidden="true" />
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
      <div ref={bodyRef} className="space-y-5 px-4 pb-4">
        {info ? (
          <p className="truncate text-[15px] font-medium" title={info.name}>
            <span className="text-text-secondary">{stem}.</span>
            <span className="font-mono text-text-tertiary">{info.source}</span>
            <span className="px-2 text-text-tertiary">→</span>
            <span className="font-mono text-accent-strong">{info.target}</span>
          </p>
        ) : (
          <p className="text-[13px] text-text-tertiary">{t("quick.gone")}</p>
        )}

        {info?.large_bytes && (
          <p className="text-[12px] leading-snug text-warning" role="note">
            {t("quick.largeFile", { size: formatLargeSize(info.large_bytes) })}
          </p>
        )}

        {info && specs.length > 0 && (
          <OptionFields
            idPrefix="quick"
            specs={specs}
            values={values}
            family={info.family}
            target={info.target}
            onChange={(spec, value) => setValues((prev) => withValue(prev, spec, info.family, info.target, value))}
          />
        )}

        {resizable && (
          <div className="space-y-1.5">
            <label htmlFor="quick-size" className="type-control-label text-text-secondary">{t("quick.longestSide")}</label>
            <input
              id="quick-size"
              type="number"
              inputMode="numeric"
              autoComplete="off"
              min="1"
              max={maxSideLimit}
              placeholder={t("quick.original")}
              value={maxSide}
              onChange={(e) => setMaxSide(e.target.value)}
              className="zc-field font-mono"
              autoFocus={specs.length === 0}
            />
          </div>
        )}
      </div>
      </div>

      <div ref={footerRef} className="flex shrink-0 justify-end gap-2 px-4 pb-4 pt-1">
        <button
          type="button"
          onClick={cancel}
          className="h-8 rounded-control px-3 text-[13px] text-text-secondary transition-colors duration-150 hover:bg-surface-2 hover:text-text-primary"
        >
          {t("quick.cancel")}
        </button>
        <button
          type="button"
          onClick={confirm}
          disabled={!info}
          className="h-8 rounded-control bg-accent px-4 text-[13px] font-medium text-on-accent transition-colors duration-150 hover:bg-accent-hover disabled:opacity-40"
        >
          {t("quick.convert")}
        </button>
      </div>
    </div>
  );
};

/** Matches the width the window opens at (`open_quick_window`). */
const WINDOW_WIDTH = 380;

function formatLargeSize(bytes: number): string {
  const gb = bytes / 1024 ** 3;
  return gb >= 1 ? `${gb.toFixed(1)} GB` : `${Math.round(bytes / 1024 ** 2)} MB`;
}
