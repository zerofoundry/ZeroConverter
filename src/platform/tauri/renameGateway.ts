import { transportInvoke } from "./ipc";
import type { OptionSpecDto, OptionValue } from "../../application/converterTypes";

export interface WatchedFolder {
  path: string;
  /** One of the user's own folders: always watched, cannot be removed. */
  builtin: boolean;
}

export interface RenameConvertStatus {
  enabled: boolean;
  autostart: boolean;
  folders: WatchedFolder[];
  /** "Convert with ZeroConverter" in Explorer's right-click menu. */
  context_menu: boolean;
  /** Keep the original file by default on rename-to-convert. */
  keep_original: boolean;
}

export interface QuickJobInfo {
  name: string;
  source: string;
  target: string;
  family: string;
  /** Over 100 MB: converting waits for the user's go-ahead. */
  large_bytes: number | null;
  /** What the engine lets the user set for this target. */
  options: OptionSpecDto[];
}

/** Format options keyed as the engine declares them; each family checks its own. */
export type QuickOptions = Record<string, OptionValue>;

export const renameGateway = {
  status: () => transportInvoke<RenameConvertStatus>("rename_convert_status"),
  setEnabled: (enabled: boolean) => transportInvoke<void>("rename_convert_set_enabled", { enabled }),
  setKeepOriginal: (enabled: boolean) => transportInvoke<void>("rename_convert_set_keep_original", { enabled }),
  setAutostart: (enabled: boolean) => transportInvoke<void>("rename_convert_set_autostart", { enabled }),
  addFolder: (path: string) => transportInvoke<void>("rename_convert_add_folder", { path }),
  removeFolder: (path: string) => transportInvoke<void>("rename_convert_remove_folder", { path }),
  setContextMenu: (enabled: boolean) => transportInvoke<void>("context_menu_set", { enabled }),
  quickInfo: (id: string) => transportInvoke<QuickJobInfo | null>("quick_job_info", { id }),
  quickConfirm: (id: string, options: QuickOptions) => transportInvoke<void>("quick_job_confirm", { id, options }),
  quickCancel: (id: string) => transportInvoke<void>("quick_job_cancel", { id }),
};
