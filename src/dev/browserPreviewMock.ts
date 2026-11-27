/**
 * Dev-only stand-in for the Rust backend so the UI can be opened in a plain
 * browser (`npm run dev`) for design work and screenshots. Never bundled into
 * the app: main.tsx imports it only when DEV and not running inside Tauri.
 */
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import i18n from "../i18n";

const rw = (id: string, ext = id) => ({ id, canonical_extension: ext, aliases: [], can_read: true, can_write: true });
const ro = (id: string, ext = id) => ({ ...rw(id, ext), can_write: false });
const wo = (id: string, ext = id) => ({ ...rw(id, ext), can_read: false });

const imageDescriptors = [
  rw("png"), rw("jpg"), rw("webp"), rw("bmp"), rw("tiff"), rw("ico"), rw("qoi"), rw("tga"),
  rw("dds"), rw("ktx2"), ro("exr"), ro("hdr"), ro("svg"), ro("gif"), ro("psd"), ro("jxl"),
  wo("pdf"), wo("avif"),
];
const audioDescriptors = [rw("wav"), rw("mp3"), rw("flac"), rw("aiff"), rw("opus"), ro("ogg"), ro("aac"), ro("m4a")];

const videoDescriptors = [
  rw("mp4"), rw("mkv"), rw("mov"), rw("webm"), ro("avi"),
  wo("gif"), wo("wav"), wo("mp3"), wo("flac"), wo("aiff"), wo("opus"), wo("m4a"), wo("aac"),
];

const routes = (descs: { id: string; can_read: boolean; can_write: boolean }[]) => {
  const outs = descs.filter((d) => d.can_write).map((d) => d.id);
  return descs.filter((d) => d.can_read).map((d) => ({ input: d.id, outputs: outs.filter((o) => o !== d.id) }));
};

const capabilities = {
  families: [
    { family: "image", descriptors: imageDescriptors, routes: routes(imageDescriptors) },
    { family: "audio", descriptors: audioDescriptors, routes: routes(audioDescriptors) },
    { family: "video", descriptors: videoDescriptors, routes: routes(videoDescriptors) },
  ],
};

const idleSummary = { status: "Idle", total: 0, queued: 0, running: 0, completed: 0, failed: 0, cancelled: 0 };

// FFmpeg download stand-in: ~8 s of fake progress, stoppable with Cancel.
const FFMPEG_BYTES = 167_647_897;
let ffmpegInstalled = false;
let keepOriginal = false;
let extraFolders: string[] = [];
let ffmpegCancelled = false;

function fakeFfmpegInstall(channel: { id: number }): Promise<unknown> {
  const internals = (window as unknown as { __TAURI_INTERNALS__: { runCallback: (id: number, data: unknown) => void } }).__TAURI_INTERNALS__;
  let index = 0;
  const send = (phase: string, done: number, total: number) =>
    internals.runCallback(channel.id, { index: index++, message: { phase, done, total } });
  ffmpegCancelled = false;
  return new Promise((resolve, reject) => {
    let done = 0;
    const tick = () => {
      if (ffmpegCancelled) return reject("Conversion execution error: CANCELLED");
      done = Math.min(FFMPEG_BYTES, done + FFMPEG_BYTES / 40);
      send("download", done, FFMPEG_BYTES);
      if (done < FFMPEG_BYTES) return void setTimeout(tick, 200);
      send("verify", 1, 1);
      setTimeout(() => {
        send("extract", 3, 3);
        ffmpegInstalled = true;
        resolve(ffmpegStatus());
      }, 700);
    };
    setTimeout(tick, 200);
  });
}

const ffmpegStatus = () => ({
  available: ffmpegInstalled,
  source: ffmpegInstalled ? "managed" : null,
  version: ffmpegInstalled ? "ffmpeg version n8.1.3" : null,
  can_install: true,
  download_bytes: FFMPEG_BYTES,
  download_url: "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/x/ffmpeg-win64-lgpl.zip",
  sha256: "0".repeat(64),
  label: "FFmpeg 8.1.3 (FFmpeg-Builds LGPL build, Windows x64)",
  license: "LGPL-2.1-or-later",
});

export function installBrowserPreviewMock() {
  mockWindows("main");
  mockIPC((cmd, args) => {
    switch (cmd) {
      case "get_app_status":
        return { engine: "zero-core", version: "0.1.1" };
      case "get_app_capabilities":
        return capabilities;
      case "get_queue_status":
        return idleSummary;
      case "list_jobs":
        return [];
      case "preflight_conversion": {
        const token = (args as { request?: { token?: number } })?.request?.token ?? 0;
        return { token, is_valid: true, severity: null, code: null, message: null, current_width: null, current_height: null, max_width: null, max_height: null, suggested_action: null };
      }
      case "notification_prefs":
        return { enabled: true, sound: true, sound_id: "silver", volume: 80 };
      case "rename_convert_status":
        return {
          enabled: true,
          autostart: false,
          context_menu: true,
          keep_original: keepOriginal,
          folders: [
            ...["Desktop", "Downloads", "Documents", "Pictures", "Videos", "Music"].map((f) => ({ path: `C:\\Users\\artist\\${f}`, builtin: true })),
            ...extraFolders.map((path) => ({ path, builtin: false })),
          ],
        };
      case "rename_convert_set_keep_original":
        keepOriginal = (args as { enabled: boolean }).enabled;
        return null;
      case "rename_convert_add_folder": {
        const path = (args as { path: string }).path;
        if (!extraFolders.includes(path)) extraFolders.push(path);
        return null;
      }
      case "rename_convert_remove_folder":
        extraFolders = extraFolders.filter((p) => p !== (args as { path: string }).path);
        return null;
      case "quick_job_info":
        return { name: "holiday-photo.png", source: "png", target: "webp", family: "image" };
      case "plugin:window|is_maximized":
        return false;
      // "Choose files" adds a sample video, which walks through the FFmpeg card.
      case "plugin:dialog|open":
        return (args as { options?: { directory?: boolean } })?.options?.directory ? "D:\\Projects\\Hollow Tide\\Art" : ["C:\\Samples\\trailer.mp4"];
      case "get_files_metadata": {
        const paths = (args as { request: { paths: string[] } }).request.paths;
        return {
          results: paths.map((path) => {
            const name = path.split(/[\\/]/).pop() ?? path;
            return { path, name, extension: name.split(".").pop() ?? "", size_bytes: 48_000_000, kind: "video", probe_status: "ok", duration_seconds: 42 };
          }),
        };
      }
      case "ffmpeg_status":
        return ffmpegStatus();
      case "ffmpeg_install":
        return fakeFfmpegInstall((args as { onEvent: { id: number } }).onEvent);
      case "ffmpeg_install_cancel":
        ffmpegCancelled = true;
        return null;
      case "take_settings_request":
        return null;
      case "submit_conversion_batch":
        return { job_ids: [] };
      case "update_status":
        return { auto_update: true, available: null };
      case "update_check":
        return null;
      case "receipt_current": {
        // What the Rust side sends for a finished rename, worded from the same catalog.
        const nat = (key: string, o: Record<string, string> = {}) => i18n.t(`native.rename.${key}`, o);
        return { tone: "success", title: "photo.png → photo.webp", body: `${nat("converted", { time: "106 ms" })}${i18n.t("native.sentenceJoin")}${nat(keepOriginal ? "keptOriginal" : "inBin")}` };
      }
      default:
        return null;
    }
  });
}
