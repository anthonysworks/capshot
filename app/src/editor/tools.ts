import type { IconName } from "./icons";

/** The drawing tools both editors offer, with the same icon, name and key. */
const SHARED = {
  text: { icon: "text", label: "Text", key: "T" },
  arrow: { icon: "arrow", label: "Arrow", key: "A" },
  curve: { icon: "curve", label: "Curved arrow (drag its middle to bend it)", key: "Q" },
  line: { icon: "line", label: "Line", key: "L" },
  rect: { icon: "rect", label: "Rectangle", key: "R" },
  ellipse: { icon: "ellipse", label: "Ellipse", key: "E" },
  highlight: { icon: "highlight", label: "Highlighter", key: "H" },
  number: { icon: "number", label: "Numbered step", key: "N" },
} as const satisfies Record<string, { icon: IconName; label: string; key: string }>;

export type SharedTool = keyof typeof SHARED;

/** A toolbar entry for one of the shared tools. */
export const sharedTool = <T extends SharedTool>(tool: T) => ({ tool, ...SHARED[tool] });
