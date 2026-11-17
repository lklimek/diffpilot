// @vitest-environment node
import { readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

// A natively disabled control cannot take focus, so its tooltip is unreachable by keyboard. Controls with a tip
// use `aria-disabled` instead: they stay focusable, ignore activation, and the tip says why (WAI-ARIA APG,
// "Focusability of disabled controls").
const root = new URL(".", import.meta.url).pathname;

function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sources(path);
    return entry.name.endsWith(".tsx") && !entry.name.includes(".test.") ? [path] : [];
  });
}

/** The `<Tooltip …>…</Tooltip>` elements of a source file (not nested in practice). */
const tooltips = (code: string) => [...code.matchAll(/<Tooltip\b[\s\S]*?<\/Tooltip>/g)].map((m) => m[0]);

describe("controls with a tooltip", () => {
  it("are never natively disabled (aria-disabled keeps them focusable)", () => {
    const offenders = [join(root, "components"), join(root, "screens")].flatMap(sources).flatMap((file) =>
      tooltips(readFileSync(file, "utf8"))
        .filter((block) => /\sdisabled=\{/.test(block))
        .map(() => relative(root, file)),
    );
    expect(offenders).toEqual([]);
  });

  it("the check finds a disabled control inside a tooltip", () => {
    expect(tooltips('<Tooltip tip="x"><button disabled={busy}>Go</button></Tooltip>')[0]).toMatch(
      /\sdisabled=\{/,
    );
  });
});
