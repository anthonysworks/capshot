import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";

export type ImageFormat = "png" | "jpeg" | "webp";
export type VideoCodec = "h264" | "hevc" | "av1";
export type RecordingFormat = "mp4" | "gif";
export type AfterCapture = "actions" | "editor" | "preview" | "nothing";
export type Folder = "screenshots" | "recordings" | "logs";

/** Mirrors `capshot_core::Config`. */
export interface Config {
  hotkeys: { capture: string; record: string };
  appearance: { theme: "system" | "dark" | "light" };
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
    editable_cursor: boolean;
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

/** Selects a file created by Capshot in Explorer. */
export const showInFolder = (path: string) => invoke<void>("show_in_folder", { path });

// ── Editor ──────────────────────────────────────────────────────────────

/** Reads a file opened in the editor. */
export const editorRead = (path: string) => invoke<ArrayBuffer>("editor_read", { path });

/** Saves an edited image (PNG bytes) over the original file. */
export const editorSave = (path: string, png: Uint8Array) =>
  invoke<void>("editor_save", png, { headers: { "x-path": encodeURIComponent(path) } });

/** Saves an edited image as a new file next to the original; resolves to its path. */
export const editorSaveCopy = (path: string, png: Uint8Array) =>
  invoke<string>("editor_save_copy", png, { headers: { "x-path": encodeURIComponent(path) } });

/** Copies an edited image (PNG bytes) to the clipboard. */
export const editorCopy = (png: Uint8Array) => invoke<void>("editor_copy", png);

/** Image drawn over the video between two times (source seconds). */
export interface OverlayImage {
  png: string;
  x: number;
  y: number;
  start: number;
  end: number;
}

/** What is drawn on the frames of an edited video. */
export interface Composition {
  overlays: OverlayImage[];
  cursor: { visible: boolean; scale: number; image: { png: string; hotspot: [number, number] } | null; halo: [number, number, number, number] | null } | null;
  clicks: { ring: boolean; color: [number, number, number]; sound: boolean; voice: "soft" | "pop" | "click" } | null;
  canvas: { ratio: [number, number]; colors: [number, number, number][]; padding: number; radius: number; shadow: number } | null;
  /** Areas shown as blocks, in video pixels. */
  pixelate: { x: number; y: number; w: number; h: number; block: number; start: number; end: number }[];
}

/** Edits made in the video editor (times in seconds, crop in video pixels). */
export interface VideoEdit {
  segments: [number, number][];
  /** Multiplies every part's speed. */
  speed: number;
  /** Speed of each part, in the order of `segments`. */
  speeds: number[];
  crop: [number, number, number, number] | null;
  composition: Composition;
}

export type ExportTask =
  | {
      kind: "mp4";
      edit: VideoEdit;
      audio: boolean;
      /** 0 (best) – 51; null: as good as the recording. */
      quality: number | null;
      /** null: the recording's frame rate, size. */
      fps: number | null;
      height: number | null;
      /** 1: as recorded (up to 2); fades in seconds. */
      volume: number;
      fade_in: number;
      fade_out: number;
    }
  | { kind: "gif"; gif: Config["gif"]; edit: VideoEdit | null };

/** Recorded cursor, as saved next to a recording. */
export interface CursorTrack {
  samples: { t: number; x: number; y: number; shape: number; visible: boolean }[];
  clicks: { t: number; x: number; y: number; right: boolean }[];
  shapes: { hotspot: [number, number]; png: string }[];
}

/** The copy without cursor (playable) and the cursor track, when kept. */
export interface VideoSources {
  clean: string | null;
  track: CursorTrack | null;
  /** Folder exports are saved in. */
  folder: string;
  /** A recording not saved yet: only exports will be. */
  draft: boolean;
  /** Size of the video file, in bytes. */
  bytes: number;
}

export const editorVideoSources = (path: string) => invoke<VideoSources>("editor_video_sources", { path });

/** Puts a video file on the clipboard. */
export const editorCopyFile = (path: string) => invoke<void>("editor_copy_file", { path });

/** Exports part of a video, to `output` (from the save dialog) or the
 * export folder; resolves to the job id. */
export const editorExport = (path: string, task: ExportTask, copy = false, output: string | null = null) =>
  invoke<number>("editor_export", { path, task, copy, output });

/** Asks for the folder exports go to; resolves to it, or null. */
export const editorChooseFolder = (path: string) => invoke<string | null>("editor_choose_folder", { path });

/** Asks where to save one export; resolves to the file, or null. */
export const editorSaveAs = (path: string, extension: "mp4" | "gif") =>
  invoke<string | null>("editor_save_as", { path, extension });

/** Deletes a recording that was never saved and closes its editor. */
export const editorDiscard = (path: string) => invoke<void>("editor_discard", { path });

/** Asks where to save an edited image; resolves to the file, or null. */
export const editorImageSaveAs = (path: string) => invoke<string | null>("editor_image_save_as", { path });

/** Writes an image (PNG bytes) to a file picked with `editorImageSaveAs`. */
export const editorSaveTo = (path: string, output: string, png: Uint8Array) =>
  invoke<string>("editor_save_to", png, {
    headers: { "x-path": encodeURIComponent(path), "x-output": encodeURIComponent(output) },
  });

// ── Quick actions after a capture ───────────────────────────────────────

export interface QuickInfo {
  video: boolean;
  /** Already in its destination folder. */
  saved: boolean;
  /** Folder Save puts it in. */
  folder: string;
  bytes: number;
}

export const quickInfo = (path: string) => invoke<QuickInfo>("quick_info", { path });
/** Saves the capture to its folder; resolves to its new path. */
export const quickSave = (path: string) => invoke<string>("quick_save", { path });
/** Saves where the user picks; resolves to the new path, or null. */
export const quickSaveAs = (path: string) => invoke<string | null>("quick_save_as", { path });
export const quickCopy = (path: string) => invoke<void>("quick_copy", { path });
/** Opens the capture in the editor and closes the window. */
export const quickEdit = (path: string) => invoke<void>("quick_edit", { path });
/** Keeps the screenshot on screen in a small window. */
export const quickPin = (path: string) => invoke<void>("quick_pin", { path });
/** Text in the screenshot (Windows OCR). */
export const quickText = (path: string) => invoke<string>("quick_text", { path });
/** Contents of the QR codes in the screenshot. */
export const quickCodes = (path: string) => invoke<string[]>("quick_codes", { path });
/** Deletes an unsaved recording and closes the window. */
export const quickDiscard = (path: string) => invoke<void>("quick_discard", { path });
/** Closes the window after an action; a thumbnail confirms it with `label` (screenshots). */
export const quickDone = (path: string, label: string | null = null) => invoke<void>("quick_done", { path, label });
/** Puts text on the clipboard. */
export const copyText = (text: string) => invoke<void>("copy_text", { text });
