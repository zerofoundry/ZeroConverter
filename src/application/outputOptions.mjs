/**
 * Output options as the engine declares them (`options` on each writable
 * format in get_app_capabilities). Nothing here knows a format or an option
 * by name: which controls exist, their bounds and defaults all come from the
 * engine. Runtime-independent and tested with node:test.
 *
 * Values the user picked are kept per scope: an option with scope "family"
 * (resize, sample rate) is shared by every target of that family, one with
 * scope "format" (quality, bitrate) is remembered for each target.
 */

/** The specs for converting a `family` file to `target`, in display order. */
export function optionSpecsFor(capabilities, family, target) {
  const fam = capabilities?.families.find((f) => f.family === family);
  const descriptor = fam?.descriptors.find((d) => d.id === target && d.can_write);
  return descriptor?.options ?? [];
}

export function scopeKey(spec, family, target) {
  return spec.scope === "family" ? family : `${family}:${target}`;
}

/** True when `value` is one the spec allows (null means "leave it to the engine"). */
export function isAllowed(spec, value) {
  if (value === undefined) return false;
  switch (spec.control) {
    case "slider":
    case "number":
      return value === null
        ? spec.default === null
        : Number.isInteger(value) && value >= spec.min && value <= spec.max;
    case "choice":
      return spec.values.some((v) => v === value);
    case "toggle":
      return typeof value === "boolean";
    default:
      return false;
  }
}

/** The value in effect: the user's, when still allowed, else the engine's default. */
export function valueOf(spec, values, family, target) {
  const stored = values?.[scopeKey(spec, family, target)]?.[spec.key];
  return isAllowed(spec, stored) ? stored : spec.default;
}

/** A new values object with `spec` set to `value`; the input is not changed. */
export function withValue(values, spec, family, target, value) {
  const scope = scopeKey(spec, family, target);
  return { ...values, [scope]: { ...(values?.[scope] ?? {}), [spec.key]: value } };
}

/** Whether a spec is in play given the others (resize width only for "max width"). */
export function isVisible(spec, specs, values, family, target) {
  if (!spec.show_when) return true;
  const switchSpec = specs.find((s) => s.key === spec.show_when.key);
  if (!switchSpec) return false;
  return spec.show_when.values.includes(valueOf(switchSpec, values, family, target));
}

/** Request fields for one file: every option in play, with its value. */
export function requestOptions(specs, values, family, target) {
  const out = {};
  for (const spec of specs) {
    if (isVisible(spec, specs, values, family, target)) {
      out[spec.key] = valueOf(spec, values, family, target);
    }
  }
  return out;
}

/** Values from settings saved before the engine declared its options (0.1.x). */
export function migrateLegacySettings(saved) {
  if (!saved || saved.optionValues) return saved?.optionValues ?? {};
  const values = {};
  const put = (scope, key, value) => {
    if (value === undefined) return;
    values[scope] = { ...(values[scope] ?? {}), [key]: value };
  };
  // 80 was the app's own default, never a choice; the engine's is 90.
  const quality = saved.familyQualities?.image ?? saved.quality;
  if (typeof quality === "number" && quality !== 80) {
    for (const target of ["jpg", "avif"]) put(`image:${target}`, "quality", quality);
  }
  if (saved.resizeMode && saved.resizeMode !== "none") {
    put("image", "resize_mode", saved.resizeMode);
    put("image", "resize_width", saved.resizeWidth ?? null);
    put("image", "resize_height", saved.resizeHeight ?? null);
  }
  const fo = saved.formatOptions ?? {};
  for (const [target, kbps] of Object.entries(fo.bitrates ?? {})) {
    put(`audio:${target}`, "bitrate_kbps", kbps);
    put(`video:${target}`, "bitrate_kbps", kbps);
  }
  if (fo.sampleRate) for (const fam of ["audio", "video"]) put(fam, "sample_rate", fo.sampleRate);
  if (fo.mono) for (const fam of ["audio", "video"]) put(fam, "channels", 1);
  if (fo.ddsFormat && fo.ddsFormat !== "bc7") put("image:dds", "dds_format", fo.ddsFormat);
  if (fo.ddsMipmaps) put("image:dds", "dds_mipmaps", true);
  return values;
}
