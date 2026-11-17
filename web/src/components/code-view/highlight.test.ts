import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import { linesFor } from "../../mocks/fixtures";
import {
  CODE_BACKGROUNDS,
  LANGUAGES,
  MAX_HIGHLIGHT_CHARS,
  MAX_TOKENS,
  contrastRatio,
  createShikiHighlight,
  languageOf,
  readable,
  type Language,
  type ShikiModules,
  type Token,
} from "./highlight";
import highlightSource from "./highlight.ts?raw";

const langs = {
  rust: () => import("shiki/langs/rust.mjs"),
  typescript: () => import("shiki/langs/typescript.mjs"),
  toml: () => import("shiki/langs/toml.mjs"),
} as Record<Language, ShikiModules["langs"][Language]>;

function modules(overrides: Partial<ShikiModules> = {}): ShikiModules {
  return {
    core: () => import("shiki/core"),
    engine: () => import("shiki/engine/javascript"),
    light: () => import("shiki/themes/github-light-default.mjs"),
    dark: () => import("shiki/themes/github-dark-default.mjs"),
    langs,
    ...overrides,
  };
}

const text = (line: Token[]) => line.map((t) => t.content).join("");

describe("languageOf", () => {
  it("maps file extensions and well-known file names to a supported language", () => {
    expect(languageOf("src/rpc/client.rs")).toBe("rust");
    expect(languageOf("web/src/App.tsx")).toBe("tsx");
    expect(languageOf("Cargo.toml")).toBe("toml");
    expect(languageOf("deploy/server/Dockerfile")).toBe("docker");
    expect(languageOf(".github/workflows/ci.YML")).toBe("yaml");
  });

  it("returns null for unknown files so they stay plain text", () => {
    expect(languageOf("LICENSE")).toBeNull();
    expect(languageOf("data/blob.xyz")).toBeNull();
  });

  it("only names languages that have a loader slot", () => {
    for (const path of ["a.rs", "a.ts", "a.py", "a.go", "a.sh", "a.md", "a.json", "a.proto", "a.cpp"]) {
      expect(LANGUAGES).toContain(languageOf(path));
    }
  });
});

describe("readable", () => {
  it("keeps colours that already reach 4.5:1 on every code background", () => {
    expect(readable("#1a1f27", "light")).toBe("#1a1f27");
    expect(readable("#e7eaf0", "dark")).toBe("#e7eaf0");
  });

  it("darkens (light) or lightens (dark) colours until they reach 4.5:1 on every code background", () => {
    for (const [colour, scheme] of [
      ["#cccccc", "light"],
      ["#6e7781", "light"],
      ["#333333", "dark"],
      ["#6e7681", "dark"],
    ] as const) {
      const fixed = readable(colour, scheme)!;
      for (const bg of CODE_BACKGROUNDS[scheme]) expect(contrastRatio(fixed, bg)).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("leaves a missing colour to the inherited text colour", () => {
    expect(readable(undefined, "light")).toBeUndefined();
  });

  it("uses the backgrounds of the design tokens in index.css", () => {
    // vitest runs in web/; CSS imports are stubbed, so read the file.
    const [light, dark] = readFileSync("src/index.css", "utf8").split("@media (prefers-color-scheme: dark)");
    const value = (block: string, name: string) =>
      new RegExp(`--color-${name}:\\s*(#[0-9a-f]{6})`).exec(block!)?.[1];
    const names = ["code-bg", "high-bg", "med-bg", "low-bg", "llm-bg", "mv-bg", "rm-bg", "accent-bg"];
    expect(names.map((n) => value(light!, n))).toEqual(CODE_BACKGROUNDS.light.slice(0, names.length));
    expect(names.map((n) => value(dark!, n))).toEqual(CODE_BACKGROUNDS.dark.slice(0, names.length));
  });
});

describe("createShikiHighlight", () => {
  it("tokenizes only the given lines and keeps their text", async () => {
    const highlight = createShikiHighlight(modules());
    const lines = ["fn main() {", '    let s = "<img src=x onerror=alert(1)>";', "}"];
    const tokens = (await highlight("src/main.rs", lines))!;
    expect(tokens.map(text)).toEqual(lines);
    const keyword = tokens[0]!.find((t) => t.content === "fn")!;
    expect(keyword.light).toMatch(/^#[0-9a-f]{6}$/);
    expect(keyword.dark).toMatch(/^#[0-9a-f]{6}$/);
    expect(keyword.light).not.toBe(keyword.dark);
  });

  it("resolves null for a file without a supported language or without a loader", async () => {
    const highlight = createShikiHighlight(modules());
    expect(await highlight("LICENSE", ["MIT"])).toBeNull();
    expect(await highlight("main.go", ["package main"])).toBeNull();
  });

  it("loads the runtime once and each language once", async () => {
    const core = vi.fn(() => import("shiki/core"));
    const rust = vi.fn(() => import("shiki/langs/rust.mjs"));
    const highlight = createShikiHighlight(modules({ core, langs: { ...langs, rust } }));
    await Promise.all([highlight("a.rs", ["fn a() {}"]), highlight("b.rs", ["fn b() {}"])]);
    await highlight("c.rs", ["fn c() {}"]);
    expect(core).toHaveBeenCalledTimes(1);
    expect(rust).toHaveBeenCalledTimes(1);
  });

  it("rejects when the runtime fails to load and retries on the next call", async () => {
    let fail = true;
    const core = vi.fn(() => (fail ? Promise.reject(new Error("offline")) : import("shiki/core")));
    const highlight = createShikiHighlight(modules({ core }));
    await expect(highlight("a.rs", ["fn a() {}"])).rejects.toThrow("offline");
    fail = false;
    expect(await highlight("a.rs", ["fn a() {}"])).not.toBeNull();
  });

  it("leaves very long lines untokenized (one plain token)", async () => {
    const highlight = createShikiHighlight(modules());
    const long = `let x = "${"a".repeat(30_000)}";`;
    const tokens = (await highlight("a.rs", ["fn a() {}", long]))!;
    expect(text(tokens[1]!)).toBe(long);
    expect(tokens[1]).toHaveLength(1);
  });

  it("gives every token colour at least 4.5:1 contrast on every code background in both themes", async () => {
    const highlight = createShikiHighlight(modules());
    const corpus: [string, string[]][] = [
      ["src/rpc/client.rs", linesFor("default", "src/rpc/client.rs", "head", 1, 224)!.lines],
      ["src/rpc/retry.rs", linesFor("default", "src/rpc/retry.rs", "head", 1, 95)!.lines],
      ["web/x.ts", highlightSource.split("\n")],
    ];
    let checked = 0;
    for (const [path, lines] of corpus) {
      for (const token of (await highlight(path, lines))!.flat()) {
        for (const scheme of ["light", "dark"] as const) {
          const colour = token[scheme];
          if (!colour) continue;
          for (const bg of CODE_BACKGROUNDS[scheme]) {
            expect(contrastRatio(colour, bg), `${colour} on ${bg}`).toBeGreaterThanOrEqual(4.5);
            checked += 1;
          }
        }
      }
    }
    expect(checked).toBeGreaterThan(1000);
  });
});

describe("createShikiHighlight budgets (CWE-770)", () => {
  // Longest stretch without an event-loop turn while `work` runs: a 0 ms timer that re-arms itself.
  async function longestSlice<T>(work: () => Promise<T>): Promise<[T, number]> {
    let last = performance.now();
    let longest = 0;
    let running = true;
    const tick = () => {
      const now = performance.now();
      longest = Math.max(longest, now - last);
      last = now;
      if (running) setTimeout(tick, 0);
    };
    setTimeout(tick, 0);
    const result = await work();
    running = false;
    tick();
    return [result, longest];
  }

  const warm = async (highlight: ReturnType<typeof createShikiHighlight>) => {
    await highlight("warm.rs", ["fn a() {}"]);
  };
  const pathological = (n: number) => Array.from({ length: n }, () => `${"a+".repeat(999)}a`);

  it("leaves a range over the character budget uncoloured without tokenizing it", async () => {
    const highlight = createShikiHighlight(modules());
    await warm(highlight);
    const lines = pathological(200);
    expect(lines.join("\n").length).toBeGreaterThan(MAX_HIGHLIGHT_CHARS);
    const [tokens, slice] = await longestSlice(() => highlight("a.rs", lines));
    expect(tokens).toBeNull();
    expect(slice).toBeLessThan(50);
  });

  it("stops at the token budget in short slices that yield to the event loop", async () => {
    const highlight = createShikiHighlight(modules());
    await warm(highlight);
    const lines = pathological(100);
    expect(lines.join("\n").length).toBeLessThanOrEqual(MAX_HIGHLIGHT_CHARS);
    const [tokens, slice] = await longestSlice(() => highlight("a.rs", lines));
    expect(tokens).toBeNull();
    expect(slice).toBeLessThan(100);
  });

  it("colours an ordinary 2000-line range within the budgets in short slices", async () => {
    const highlight = createShikiHighlight(modules());
    await warm(highlight);
    const file = linesFor("default", "src/rpc/client.rs", "head", 1, 224)!.lines;
    const lines = Array.from({ length: 2000 }, (_, i) => file[i % file.length]!);
    const [tokens, slice] = await longestSlice(() => highlight("a.rs", lines));
    expect(tokens!.map(text)).toEqual(lines);
    expect(tokens!.flat().length).toBeLessThanOrEqual(MAX_TOKENS);
    expect(slice).toBeLessThan(100);
  });

  it("keeps the grammar state across slices (a block comment spanning a slice boundary)", async () => {
    const highlight = createShikiHighlight(modules());
    const lines = ["/*", ...Array.from({ length: 400 }, () => "x".repeat(60)), "*/", "fn a() {}"];
    const tokens = (await highlight("a.rs", lines))!;
    const inComment = tokens[300]![0]!;
    const fn = tokens.at(-1)!.find((t) => t.content === "fn")!;
    expect(inComment.light).toBe(tokens[0]![0]!.light);
    expect(fn.light).not.toBe(inComment.light);
  });

  it("resolves null when aborted between slices", async () => {
    const highlight = createShikiHighlight(modules());
    await warm(highlight);
    const controller = new AbortController();
    const file = linesFor("default", "src/rpc/client.rs", "head", 1, 224)!.lines;
    const pending = highlight("a.rs", [...file, ...file, ...file], controller.signal);
    controller.abort();
    expect(await pending).toBeNull();
  });
});
