/** Text roles in the diagram; each has one font (size and weight) shared by measuring and drawing. */
export type TextStyle = "node" | "number" | "edge";

/** CSS font size and weight per role; the drawing uses the matching Tailwind classes. */
export const TEXT_FONT: Record<TextStyle, string> = {
  node: "400 12px",
  number: "700 12px",
  edge: "600 11px",
};
const FONT_PX: Record<TextStyle, number> = { node: 12, number: 12, edge: 11 };

/** Width in px of `text` drawn in `style`. */
export type Measure = (text: string, style: TextStyle) => number;

// Conservative em widths, at least those of DejaVu Sans (the widest common `system-ui` face).
function emWidth(char: string): number {
  const code = char.codePointAt(0) ?? 0;
  if (char === " ") return 0.32;
  if ("iljfrtI.,:;'|!`".includes(char)) return 0.4;
  if ("mwMW@%".includes(char)) return 1.0;
  if (char >= "A" && char <= "Z") return 0.8;
  if (char >= "a" && char <= "z") return 0.62;
  if (char >= "0" && char <= "9") return 0.66;
  if (code >= 0x2e80) return 1.05;
  return 0.8;
}

/** Deterministic width estimate per character (tests, and browsers without OffscreenCanvas). */
export const estimateWidth: Measure = (text, style) => {
  let em = 0;
  for (const char of text) em += emWidth(char);
  return Math.ceil(em * FONT_PX[style] * (style === "node" ? 1 : 1.08));
};

/** Measures with the font actually used (`fontFamily` of the page); null when no canvas is available. */
export function canvasMeasure(fontFamily: string): Measure | null {
  if (typeof OffscreenCanvas === "undefined") return null;
  const context = new OffscreenCanvas(1, 1).getContext("2d");
  if (!context) return null;
  const cache = new Map<string, number>();
  return (text, style) => {
    const key = `${style}\u0000${text}`;
    let width = cache.get(key);
    if (width === undefined) {
      context.font = `${TEXT_FONT[style]} ${fontFamily}`;
      width = Math.ceil(context.measureText(text).width);
      cache.set(key, width);
    }
    return width;
  };
}
