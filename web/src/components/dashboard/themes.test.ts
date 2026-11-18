// @vitest-environment node
import { readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

// Light and dark themes (plan §F.3, §F.4): dashboard and shell components colour only through theme tokens that
// index.css defines for both themes, so the dark theme follows the system without per-component code.
const src = new URL("../../", import.meta.url).pathname;
const css = readFileSync(join(src, "index.css"), "utf8");
const light = css.slice(css.indexOf("@theme"), css.indexOf("}", css.indexOf("@theme")));
const darkStart = css.indexOf("@media (prefers-color-scheme: dark)");
const dark = css.slice(darkStart, css.indexOf("}\n}", darkStart));
const tokens = (block: string) => new Set([...block.matchAll(/--color-([a-z0-9-]+):/g)].map((m) => m[1]));
const LIGHT = tokens(light);
const DARK = tokens(dark);
// Colours that read the same in both themes.
const NEUTRAL = new Set(["transparent", "current", "inherit", "black", "white"]);

function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sources(path);
    return /\.tsx?$/.test(entry.name) && !entry.name.includes(".test.") ? [path] : [];
  });
}

const files = [join(src, "components/dashboard"), join(src, "components/app-shell")].flatMap(sources);
const COLOUR_CLASS =
  /(?<![\w-])(?:[a-z-]+:)*!?(?:bg|text|border(?:-[lrtbxy])?|fill|stroke|outline|ring|decoration|shadow)-([a-z][a-z0-9-]*?)(?:\/\d+)?(?=[\s"'`\]})]|$)/g;
// Widths, sizes and styles that share the prefixes (`border-l-4`, `outline-offset-4`, `text-xs`).
const NOT_COLOUR =
  /^(?:(?:[lrtbxy]|offset)-)?\d|^(?:xs|sm|base|lg|xl|\dxl|left|right|center|none|solid|dashed|dotted|double|collapse|separate|tip|[lrtbxy])$/;

describe("dashboard themes", () => {
  it("defines the chart tokens in both themes", () => {
    for (const token of ["c-pos", "c-neg"]) {
      expect(LIGHT.has(token), `light --color-${token}`).toBe(true);
      expect(DARK.has(token), `dark --color-${token}`).toBe(true);
    }
  });

  it("uses no literal colours", () => {
    const literal = files.filter((file) =>
      /#[0-9a-f]{3,8}\b|\brgba?\(|\bhsla?\(/i.test(readFileSync(file, "utf8")),
    );
    expect(literal.map((file) => relative(src, file))).toEqual([]);
  });

  it("colours only with tokens defined for light and dark", () => {
    expect(files.length).toBeGreaterThan(20);
    const missing = files.flatMap((file) =>
      [...readFileSync(file, "utf8").matchAll(COLOUR_CLASS)]
        .map((m) => m[1]!)
        .filter((name) => !NOT_COLOUR.test(name) && !NEUTRAL.has(name))
        .filter((name) => !(LIGHT.has(name) && DARK.has(name)))
        .map((name) => `${relative(src, file)}: ${name}`),
    );
    expect([...new Set(missing)]).toEqual([]);
  });
});
