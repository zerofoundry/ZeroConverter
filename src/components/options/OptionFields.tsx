import React from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import type { OptionSpecDto, OptionValue } from "../../application/converterTypes";
import { isVisible, valueOf, type OptionValues } from "../../application/outputOptions.mjs";

/**
 * Draws the options the engine declares for one target format. Which
 * controls appear, their bounds and defaults all come from the specs; this
 * file only decides how each kind of control looks. A new option in the
 * engine shows up here with no change, labelled from `option.<key>` in the
 * catalogs (or its key, until it has a label).
 */

const humanize = (key: string) => key.replace(/_/g, " ").replace(/^\w/, (c) => c.toUpperCase());

export function optionLabel(t: TFunction, spec: OptionSpecDto): string {
  return t(`option.${spec.key}.label`, { defaultValue: humanize(spec.key) });
}

/** How one value reads in a list or a summary: "320 kbps", "48 kHz", "Mono", "Original". */
export function valueLabel(t: TFunction, spec: OptionSpecDto, value: OptionValue): string {
  if (value === null) {
    return t(`option.${spec.key}.unset`, { defaultValue: t("option.unset") });
  }
  if (typeof value === "boolean") return value ? t("common.yes") : t("common.no");
  const missing = "\u0000";
  const named = t(`option.${spec.key}.values.${value}`, { defaultValue: missing, name: String(value).toUpperCase() });
  if (named !== missing) return named;
  if (typeof value === "string") return value.toUpperCase();
  switch (spec.unit) {
    case "p":
      return `${value}p`;
    case "hz":
      return value % 1000 === 0 ? `${value / 1000} kHz` : `${(value / 1000).toFixed(1)} kHz`;
    case undefined:
      return String(value);
    default:
      return `${value} ${spec.unit}`;
  }
}

interface FieldProps {
  spec: OptionSpecDto;
  value: OptionValue;
  onChange: (value: OptionValue) => void;
  disabled: boolean;
  id: string;
  /** Inline next to another field: the label moves into the placeholder. */
  compact?: boolean;
}

const OptionField: React.FC<FieldProps> = ({ spec, value, onChange, disabled, id, compact }) => {
  const { t } = useTranslation();
  const label = optionLabel(t, spec);
  const caption = <label htmlFor={id} className="type-control-label text-text-secondary">{label}</label>;

  // A number without presets is typed in.
  if (spec.control === "number" && !spec.presets?.length) {
    const placeholder = t(`option.${spec.key}.placeholder`, { defaultValue: label });
    return (
      <div className={compact ? "flex-1" : "space-y-1.5"}>
        {!compact && caption}
        <input
          id={id}
          type="number"
          inputMode="numeric"
          autoComplete="off"
          min={spec.min}
          max={spec.max}
          placeholder={placeholder}
          aria-label={compact ? label : undefined}
          disabled={disabled}
          value={typeof value === "number" ? value : ""}
          onChange={(e) => onChange(e.target.value ? Math.floor(Number(e.target.value)) : null)}
          className="zc-field"
        />
      </div>
    );
  }

  switch (spec.control) {
    case "slider": {
      const min = spec.min ?? 0;
      const max = spec.max ?? 100;
      const current = typeof value === "number" ? value : min;
      const fill = max > min ? ((current - min) / (max - min)) * 100 : 100;
      return (
        <div className="space-y-1.5">
          <div className="flex items-center justify-between">
            {caption}
            <span className="text-[12px] tabular-nums text-text-primary">{current}</span>
          </div>
          <input
            id={id}
            type="range"
            min={min}
            max={max}
            value={current}
            disabled={disabled}
            onChange={(e) => onChange(Number(e.target.value))}
            className="zc-range w-full"
            style={{ "--fill": `${fill}%` } as React.CSSProperties}
          />
        </div>
      );
    }
    case "toggle":
      return (
        <label className="flex cursor-pointer items-center gap-2.5 py-1">
          <input
            id={id}
            type="checkbox"
            checked={value === true}
            disabled={disabled}
            onChange={(e) => onChange(e.target.checked)}
            className="zc-check"
          />
          <span className="type-control-label text-text-secondary">{label}</span>
        </label>
      );
    case "number":
    case "choice": {
      const listed: OptionValue[] =
        spec.control === "choice"
          ? (spec.values ?? [])
          : [...(spec.default === null ? [null] : []), ...(spec.presets ?? [])];
      // A value typed elsewhere (the command line, an older version) stays visible.
      const shown = listed.some((v) => v === value) ? listed : [...listed, value];
      const encode = (v: OptionValue) => JSON.stringify(v);
      return (
        <div className="space-y-1.5">
          {caption}
          <select
            id={id}
            disabled={disabled}
            value={encode(value)}
            onChange={(e) => onChange(JSON.parse(e.target.value) as OptionValue)}
            className="zc-field"
          >
            {shown.map((v) => (
              <option key={encode(v)} value={encode(v)}>{valueLabel(t, spec, v)}</option>
            ))}
          </select>
        </div>
      );
    }
    default:
      return null;
  }
};

interface OptionFieldsProps {
  specs: OptionSpecDto[];
  values: OptionValues;
  family: string;
  target: string;
  onChange: (spec: OptionSpecDto, value: OptionValue) => void;
  disabled?: boolean;
  /** Prefix for element ids, unique on the page. */
  idPrefix: string;
}

/** Every option in play for one target, in the engine's order. Number fields
 *  switched on by the same option (resize width and height) share a row. */
export const OptionFields: React.FC<OptionFieldsProps> = ({
  specs, values, family, target, onChange, disabled = false, idPrefix,
}) => {
  const shown = specs.filter((spec) => isVisible(spec, specs, values, family, target));
  const rows: OptionSpecDto[][] = [];
  for (const spec of shown) {
    const last = rows[rows.length - 1];
    const pairs = (a: OptionSpecDto) =>
      a.control === "number" && !a.presets?.length && !!a.show_when && a.show_when.key === spec.show_when?.key;
    if (last && pairs(last[0]) && pairs(spec)) last.push(spec);
    else rows.push([spec]);
  }
  const pairable = (a: OptionSpecDto) => a.control === "number" && !a.presets?.length && !!a.show_when;
  const field = (spec: OptionSpecDto, compact: boolean) => (
    <OptionField
      key={spec.key}
      id={`${idPrefix}-${spec.key}`}
      spec={spec}
      value={valueOf(spec, values, family, target)}
      onChange={(value) => onChange(spec, value)}
      disabled={disabled}
      compact={compact}
    />
  );
  return (
    <>
      {rows.map((row) =>
        !pairable(row[0]) ? (
          field(row[0], false)
        ) : (
          <div key={row.map((s) => s.key).join("+")} className="flex gap-2">
            {row.map((spec) => field(spec, true))}
          </div>
        ),
      )}
    </>
  );
};
