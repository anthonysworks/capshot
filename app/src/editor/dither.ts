/** Pixel "dither" bands: small squares on a grid, scattered and faint,
 * denser towards one edge. Drawn once into a tile that repeats across. */

/** A small deterministic random generator, so the pattern never changes. */
function random(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** A tile `width` × `height` CSS pixels whose squares get denser towards
 * the top (`dense: "top"`) or the bottom. */
export function ditherBand(dense: "top" | "bottom", width = 240, height = 64, cell = 4, seed = 11): string {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) return "none";
  const next = random(seed);
  const rows = height / cell;
  for (let row = 0; row < rows; row++) {
    // 1 at the dense edge, 0 at the other.
    const edge = 1 - row / (rows - 1);
    const density = (dense === "top" ? edge : 1 - edge) ** 1.8;
    for (let col = 0; col < width / cell; col++) {
      if (next() > density * 0.6) continue;
      // Mostly faint grey, now and then a brighter one.
      const bright = next() < 0.06;
      const alpha = bright ? 0.35 + next() * 0.3 : 0.05 + next() * 0.18 * (0.4 + density);
      ctx.fillStyle = `rgba(200, 204, 214, ${alpha.toFixed(3)})`;
      ctx.fillRect(col * cell, row * cell, cell - 1, cell - 1);
    }
  }
  return `url(${canvas.toDataURL("image/png")})`;
}
