/** 20 × 20 stroke icons for the editor toolbars (SVG path data). */
export const ICONS = {
  select: "M5 3l10 7-4.5 1.2L13 16l-2 1-2.2-4.6L5 15z",
  arrow: "M4 16L16 4M9 4h7v7",
  rect: "M3.5 5.5h13v9h-13z",
  ellipse: "M10 4.5c4 0 7 2.5 7 5.5s-3 5.5-7 5.5S3 13 3 10s3-5.5 7-5.5z",
  highlight: "M4 16h12M6.5 12.5l6-8 3 2.2-6 8H6.5z",
  text: "M4.5 5V3.5h11V5M10 3.5v13M7.5 16.5h5",
  number: "M10 3a7 7 0 100 14 7 7 0 000-14zM9 7.5l1.5-1v7",
  pixelate: "M3.5 3.5h5v5h-5zM11.5 3.5h5v5h-5zM3.5 11.5h5v5h-5zM11.5 11.5h5v5h-5z",
  crop: "M6 2.5V14h11.5M2.5 6H14v11.5",
  undo: "M7 5L3.5 8.5 7 12M4 8.5h7.5a4.5 4.5 0 010 9H9",
  redo: "M13 5l3.5 3.5L13 12M16 8.5H8.5a4.5 4.5 0 000 9H11",
  copy: "M7 7h9.5v9.5H7zM13 7V3.5H3.5V13H7",
  save: "M4 3.5h9.5l3 3v10h-13zM7 3.5v4h6v-4M6.5 16.5v-5h7v5",
  play: "M6 4l10 6-10 6z",
  pause: "M6 4h3v12H6zM11 4h3v12h-3z",
  folder: "M2.5 5.5h5l1.5 1.5h8.5v9h-15z",
} as const;

export type IconName = keyof typeof ICONS;
