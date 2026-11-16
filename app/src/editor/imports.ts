/** What can be brought into the video editor besides the recording. */

/** Music added to the video. Times are in seconds of the result, except
 * `from` (where in the file it is played from). */
export interface Music {
  path: string;
  name: string;
  /** Length of the file. */
  length: number;
  start: number;
  from: number;
  volume: number;
  fadeIn: number;
  fadeOut: number;
}

/** A video added after this one: scaled to fit and written one after the other. */
export interface Clip {
  id: number;
  path: string;
  name: string;
  /** Length in seconds. */
  length: number;
}

export const IMAGE_FILE = /\.(png|jpe?g|webp|bmp|gif)$/i;
export const AUDIO_FILE = /\.(mp3|wav|m4a|aac|flac|ogg|oga|opus|wma)$/i;
export const VIDEO_FILE = /\.(mp4|mov|m4v|mkv|webm|avi|wmv)$/i;

/** What a dropped or picked file is, by its name. */
export function mediaKind(path: string): "image" | "audio" | "video" | null {
  if (IMAGE_FILE.test(path)) return "image";
  if (AUDIO_FILE.test(path)) return "audio";
  if (VIDEO_FILE.test(path)) return "video";
  return null;
}

/** The last part of a path. */
export const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

/** Where a picture or a video sits in the frame. */
export type Corner = "top-left" | "top-right" | "center" | "bottom-left" | "bottom-right";

/** The top-left corner of a `w` × `h` box placed at `corner` of a `frame`, with a margin. */
export function cornerOrigin(
  corner: Corner,
  frame: { w: number; h: number },
  box: { w: number; h: number },
  margin: number,
): { x: number; y: number } {
  const left = margin;
  const top = margin;
  const right = frame.w - box.w - margin;
  const bottom = frame.h - box.h - margin;
  switch (corner) {
    case "top-left":
      return { x: left, y: top };
    case "top-right":
      return { x: right, y: top };
    case "bottom-left":
      return { x: left, y: bottom };
    case "bottom-right":
      return { x: right, y: bottom };
    default:
      return { x: (frame.w - box.w) / 2, y: (frame.h - box.h) / 2 };
  }
}
