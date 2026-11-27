import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";

export type WindowEventUnsubscribe = () => void;

export interface WindowDragDropHandlers {
  onHover: () => void;
  onDrop: (paths: string[]) => void;
  onLeave: () => void;
}

/**
 * Subscribes to native window drag-and-drop events.
 * Translates raw Tauri payload structures into application callbacks.
 */
export async function subscribeWindowDragDrop(
  handlers: WindowDragDropHandlers
): Promise<WindowEventUnsubscribe> {
  return getCurrentWindow().onDragDropEvent((event) => {
    const payload = event.payload;

    if (payload.type === "over" || payload.type === "enter") {
      handlers.onHover();
      return;
    }

    if (payload.type === "drop") {
      handlers.onDrop(payload.paths ?? []);
      return;
    }

    // "leave" or any other type
    handlers.onLeave();
  });
}

/** Window chrome actions for the custom title bar (the native frame is disabled). */
export const windowChrome = {
  minimize: () => getCurrentWindow().minimize(),
  toggleMaximize: () => getCurrentWindow().toggleMaximize(),
  close: () => getCurrentWindow().close(),
  isMaximized: () => getCurrentWindow().isMaximized(),
  onResized: (handler: () => void) => getCurrentWindow().onResized(handler),
  /** Interface scale, like browser zoom: 1 = 100%. */
  setZoom: (factor: number) => getCurrentWebview().setZoom(factor),
  /** Resize this window's content area (logical pixels). */
  setSize: (width: number, height: number) => getCurrentWindow().setSize(new LogicalSize(width, height)),
  /** The title shown in the taskbar and Alt+Tab. */
  setTitle: (title: string) => getCurrentWindow().setTitle(title),
};
