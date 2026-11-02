import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";

export type ImageFormat = "png" | "jpeg" | "webp";
export type VideoCodec = "h264" | "hevc" | "av1";
export type RecordingFormat = "mp4" | "gif";
export type AfterCapture = "editor" | "preview" | "nothing";
export type Folder = "screenshots" | "recordings" | "logs";

/** Mirrors `kadra_core::Config`. */
export interface Config {
  hotkeys: { capture: string; record: string };
  capture: {
    output_dir: string;
    format: ImageFormat;
    copy_to_clipboard: boolean;
    save_to_disk: boolean;
    after_capture: AfterCapture;
    delay_secs: number;
  };
  recording: {
    output_dir: string;
    fps: number;
    codec: VideoCodec;
    quality: number;
    capture_cursor: boolean;
    system_audio: boolean;
    microphone: boolean;
    countdown_secs: number;
    format: RecordingFormat;
  };
  gif: {
    fps: number;
    max_width: number;
    quality: number;
    dither: boolean;
    repeat: boolean;
  };
}

export interface AppInfo {
  version: string;
  configFile: string;
  logDir: string;
}

export const FPS_MIN = 1;
export const FPS_MAX = 240;
export const FPS_PRESETS = [15, 24, 30, 60, 120, 144, 240] as const;
export const GIF_FPS_MAX = 50;
export const GIF_FPS_PRESETS = [10, 15, 20, 25, 30] as const;
/** Common GIF widths; 0 keeps the recorded size. */
export const GIF_WIDTHS = [0, 1280, 960, 800, 640, 480] as const;

export const getConfig = () => invoke<Config>("get_config");
export const saveConfig = (config: Config) => invoke<void>("save_config", { config });
export const appInfo = () => invoke<AppInfo>("app_info");
export const openFolder = (folder: Folder) => invoke<void>("open_folder", { folder });

/** Configuration changed outside the settings window (e.g. the overlay). */
export const onConfigChanged = (handler: (config: Config) => void) =>
  listen<Config>("config-changed", (event) => handler(event.payload));

/** Progress of a video job (conversion to GIF, export from the editor). */
export interface JobProgress {
  job: number;
  input: string;
  done: number;
  output: string | null;
  bytes: number | null;
  error: string | null;
}

export const VIDEO_EXTENSIONS = ["mp4", "mov", "m4v", "wmv", "avi", "mkv"];
export const isVideo = (path: string) =>
  VIDEO_EXTENSIONS.includes(path.split(".").pop()?.toLowerCase() ?? "");

/** Starts converting a video to GIF; resolves to the job id. */
export const convertToGif = (path: string) => invoke<number>("convert_to_gif", { path });

export const onJobProgress = (handler: (progress: JobProgress) => void) =>
  listen<JobProgress>("video-job", (event) => handler(event.payload));

/** Files dropped on the window; `over` tells whether a drag is in progress. */
export const onFileDrop = (handler: (paths: string[] | null, over: boolean) => void) =>
  getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload;
    if (p.type === "enter" || p.type === "over") handler(null, true);
    else if (p.type === "drop") handler(p.paths, false);
    else handler(null, false);
  });

/** Selects a file created by Kadra in Explorer. */
export const showInFolder = (path: string) => invoke<void>("show_in_folder", { path });

// ── Editor ──────────────────────────────────────────────────────────────

/** Reads a file opened in the editor. */
export const editorRead = (path: string) => invoke<ArrayBuffer>("editor_read", { path });

/** Saves an edited image (PNG bytes) over the original file. */
export const editorSave = (path: string, png: Uint8Array) =>
  invoke<void>("editor_save", png, { headers: { "x-path": encodeURIComponent(path) } });

/** Copies an edited image (PNG bytes) to the clipboard. */
export const editorCopy = (png: Uint8Array) => invoke<void>("editor_copy", png);

export type ExportTask =
  | { kind: "mp4"; start: number; end: number | null; audio: boolean }
  | { kind: "gif"; gif: Config["gif"]; range: [number, number] | null };

/** Exports part of a video; resolves to the job id. */
export const editorExport = (path: string, task: ExportTask) =>
  invoke<number>("editor_export", { path, task });
