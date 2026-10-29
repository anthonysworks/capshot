import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type ImageFormat = "png" | "jpeg" | "webp";
export type VideoCodec = "h264" | "hevc" | "av1";
export type Folder = "screenshots" | "recordings" | "logs";

/** Mirrors `kadra_core::Config`. */
export interface Config {
  hotkeys: { capture: string; record: string };
  capture: {
    output_dir: string;
    format: ImageFormat;
    copy_to_clipboard: boolean;
    save_to_disk: boolean;
    show_preview: boolean;
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

export const getConfig = () => invoke<Config>("get_config");
export const saveConfig = (config: Config) => invoke<void>("save_config", { config });
export const appInfo = () => invoke<AppInfo>("app_info");
export const openFolder = (folder: Folder) => invoke<void>("open_folder", { folder });

/** Configuration changed outside the settings window (e.g. the overlay). */
export const onConfigChanged = (handler: (config: Config) => void) =>
  listen<Config>("config-changed", (event) => handler(event.payload));
