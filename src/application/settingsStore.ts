import { getSetting, setSetting } from '../platform/tauri/settingsGateway';
import { migrateLegacySettings, type OptionValues } from './outputOptions.mjs';

export interface AppSettings {
  format: string;
  overwrite: boolean;
  outputDirectory: string | null;
  familyFormats: Record<string, string>;
  /** Format options the user chose, per scope (see outputOptions.mjs). */
  optionValues: OptionValues;
}

const DEFAULT_SETTINGS: AppSettings = {
  format: "",
  overwrite: false,
  outputDirectory: null,
  // Format preferences are populated only after loading core capabilities.
  familyFormats: {},
  optionValues: {},
};

/** Fields of settings saved by 0.1.x that `optionValues` replaced. */
const LEGACY_KEYS = ["quality", "familyQualities", "resizeMode", "resizeWidth", "resizeHeight", "formatOptions"];

function withoutLegacy(saved: Record<string, unknown>): Partial<AppSettings> {
  const rest = { ...saved };
  for (const key of LEGACY_KEYS) delete rest[key];
  return rest as Partial<AppSettings>;
}

export class SettingsManager {
  static async load(): Promise<AppSettings> {
    try {
      const saved = await getSetting<Record<string, unknown>>('app_settings');
      if (saved) {
        return {
          ...DEFAULT_SETTINGS,
          ...withoutLegacy(saved),
          optionValues: migrateLegacySettings(saved),
        };
      }
    } catch (e) {
      console.warn("Failed to load settings:", e);
    }
    return DEFAULT_SETTINGS;
  }

  /** Saves run one after another, so a later save never reads, merges and
   *  writes over an earlier one still in flight. */
  private static queue: Promise<void> = Promise.resolve();

  /** Rejects when the settings could not be written; the caller tells the user. */
  static save(settings: Partial<AppSettings>): Promise<void> {
    const run = this.queue.then(async () => {
      const current = await this.load();
      await setSetting('app_settings', { ...current, ...settings });
    });
    // The next save waits for this one, failed or not.
    this.queue = run.catch(() => undefined);
    return run;
  }
}
