import type { AppCapabilitiesDto, OptionSpecDto, OptionValue } from "./converterTypes";

/** Chosen values per scope ("image", "audio:mp3"), then per option key. */
export type OptionValues = Record<string, Record<string, OptionValue>>;

export function optionSpecsFor(
  capabilities: AppCapabilitiesDto | null,
  family: string,
  target: string
): OptionSpecDto[];

export function scopeKey(spec: OptionSpecDto, family: string, target: string): string;
export function isAllowed(spec: OptionSpecDto, value: unknown): boolean;
export function valueOf(spec: OptionSpecDto, values: OptionValues, family: string, target: string): OptionValue;
export function withValue(
  values: OptionValues,
  spec: OptionSpecDto,
  family: string,
  target: string,
  value: OptionValue
): OptionValues;
export function isVisible(
  spec: OptionSpecDto,
  specs: OptionSpecDto[],
  values: OptionValues,
  family: string,
  target: string
): boolean;
export function requestOptions(
  specs: OptionSpecDto[],
  values: OptionValues,
  family: string,
  target: string
): Record<string, OptionValue>;
export function migrateLegacySettings(saved: unknown): OptionValues;
