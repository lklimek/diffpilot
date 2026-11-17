import type {
  GrammarState,
  HighlighterCore,
  LanguageRegistration,
  ThemeRegistrationRaw,
  createHighlighterCore,
} from "shiki/core";
import type { createJavaScriptRegexEngine } from "shiki/engine/javascript";

/** One syntax token of a line: its text and its colour per theme (undefined = inherited text colour). */
export interface Token {
  content: string;
  light?: string;
  dark?: string;
}

/**
 * Colours the given lines of the file at `path` (brief §7: displayed lines only). Resolves one token list per
 * line, or null (plain text) when the file has no supported language, the range is over budget or `signal`
 * aborted; rejects when the highlighter cannot load.
 */
export type Highlight = (
  path: string,
  lines: readonly string[],
  signal?: AbortSignal,
) => Promise<Token[][] | null>;

/** Languages with a grammar slot; the host supplies one loader per language. */
export const LANGUAGES = [
  "c",
  "cpp",
  "css",
  "docker",
  "go",
  "html",
  "java",
  "javascript",
  "json",
  "jsx",
  "kotlin",
  "markdown",
  "proto",
  "python",
  "rust",
  "shellscript",
  "sql",
  "swift",
  "toml",
  "tsx",
  "typescript",
  "yaml",
] as const;

/** A language with a grammar slot. */
export type Language = (typeof LANGUAGES)[number];

const EXTENSIONS: Record<string, Language> = {
  c: "c",
  h: "c",
  cc: "cpp",
  cpp: "cpp",
  cxx: "cpp",
  hh: "cpp",
  hpp: "cpp",
  css: "css",
  go: "go",
  html: "html",
  htm: "html",
  java: "java",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  json: "json",
  jsx: "jsx",
  kt: "kotlin",
  kts: "kotlin",
  md: "markdown",
  proto: "proto",
  py: "python",
  rs: "rust",
  sh: "shellscript",
  bash: "shellscript",
  sql: "sql",
  swift: "swift",
  toml: "toml",
  tsx: "tsx",
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  yaml: "yaml",
  yml: "yaml",
};

/** The language of a repository path, or null (plain text). */
export function languageOf(path: string): Language | null {
  const name = path.slice(path.lastIndexOf("/") + 1);
  if (name === "Dockerfile" || name.startsWith("Dockerfile.")) return "docker";
  const dot = name.lastIndexOf(".");
  return dot < 0 ? null : (EXTENSIONS[name.slice(dot + 1).toLowerCase()] ?? null);
}

/**
 * Backgrounds code text can sit on, per theme: code, risk bars (high, medium, low, not rated), moved, removed,
 * comment line (values of `index.css`), then a conservative stand-in for the context hatch stripe.
 */
export const CODE_BACKGROUNDS = {
  light: ["#fbfbfc", "#fdeceb", "#fff2d2", "#e5f1f9", "#f0e8fa", "#eef0f3", "#fdecec", "#e4eefb", "#e2e5ea"],
  dark: ["#12161d", "#3a1b19", "#3a2d12", "#112f40", "#2a2039", "#1b2029", "#3a1b19", "#17283f", "#283039"],
} as const;

const MIN_CONTRAST = 4.5;

function channels(hex: string): [number, number, number] {
  const h = hex.slice(1, 7);
  return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)) as [number, number, number];
}

function luminance(hex: string): number {
  const [r, g, b] = channels(hex).map((c) => {
    const v = c / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast ratio of two `#rrggbb` colours. */
export function contrastRatio(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}

const toHex = (rgb: number[]) => `#${rgb.map((c) => Math.round(c).toString(16).padStart(2, "0")).join("")}`;
const readableCache = new Map<string, string>();

/** `colour` moved towards black (light) or white (dark) until it reaches 4.5:1 on every code background. */
export function readable(colour: string | undefined, scheme: "light" | "dark"): string | undefined {
  if (!colour) return undefined;
  const key = `${scheme}${colour}`;
  const cached = readableCache.get(key);
  if (cached) return cached;
  const target = scheme === "light" ? 0 : 255;
  const base = channels(colour.toLowerCase());
  let result = toHex(base);
  for (let t = 0; t <= 1; t += 0.05) {
    result = toHex(base.map((c) => c + (target - c) * t));
    if (CODE_BACKGROUNDS[scheme].every((bg) => contrastRatio(result, bg) >= MIN_CONTRAST)) break;
  }
  readableCache.set(key, result);
  return result;
}

type Loader<T> = () => Promise<{ default: T }>;

/** Shiki modules, loaded by the host on first use (components do not load modules themselves, rule A7). */
export interface ShikiModules {
  /** `import("shiki/core")`. */
  core: () => Promise<{ createHighlighterCore: typeof createHighlighterCore }>;
  /** `import("shiki/engine/javascript")`. */
  engine: () => Promise<{ createJavaScriptRegexEngine: typeof createJavaScriptRegexEngine }>;
  /** Light theme, e.g. `import("shiki/themes/github-light-default.mjs")`. */
  light: Loader<ThemeRegistrationRaw>;
  /** Dark theme, e.g. `import("shiki/themes/github-dark-default.mjs")`. */
  dark: Loader<ThemeRegistrationRaw>;
  /** Grammar per language, e.g. `rust: () => import("shiki/langs/rust.mjs")`. */
  langs: Record<Language, Loader<LanguageRegistration[]>>;
}

/** Lines longer than this stay one plain token (performance; the text is shown in full). */
export const MAX_TOKENIZED_LINE = 2000;
/** Ranges with more characters stay plain text without being tokenized (CWE-770). */
export const MAX_HIGHLIGHT_CHARS = 200_000;
/** Ranges that produce more tokens stay plain text: bounds tokenizing and the rendered spans (CWE-770). */
export const MAX_TOKENS = 25_000;
/** Characters tokenized per synchronous slice; slices yield to the event loop in between. */
const SLICE_CHARS = 4_000;

const nextTurn = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

interface Runtime {
  highlighter: HighlighterCore;
  themes: { light: string; dark: string };
}

/** A `Highlight` backed by Shiki's fine-grained core and JavaScript regex engine, loading grammars lazily. */
export function createShikiHighlight(modules: ShikiModules): Highlight {
  let runtime: Promise<Runtime> | null = null;
  const grammars = new Map<Language, Promise<void>>();

  const start = async (): Promise<Runtime> => {
    const [core, engine, light, dark] = await Promise.all([
      modules.core(),
      modules.engine(),
      modules.light(),
      modules.dark(),
    ]);
    const highlighter = await core.createHighlighterCore({
      themes: [light.default, dark.default],
      langs: [],
      engine: engine.createJavaScriptRegexEngine({ forgiving: true }),
    });
    return {
      highlighter,
      themes: { light: light.default.name ?? "light", dark: dark.default.name ?? "dark" },
    };
  };

  return async (path, lines, signal) => {
    const language = languageOf(path);
    const loader = language && modules.langs[language];
    const chars = lines.reduce((n, line) => n + line.length + 1, 0);
    if (!language || !loader || lines.length === 0 || chars > MAX_HIGHLIGHT_CHARS) return null;
    runtime ??= start().catch((error: unknown) => {
      runtime = null;
      throw error;
    });
    const { highlighter, themes } = await runtime;
    let grammar = grammars.get(language);
    if (!grammar) {
      grammar = loader().then((module) => highlighter.loadLanguage(...module.default));
      grammars.set(language, grammar);
      grammar.catch(() => grammars.delete(language));
    }
    await grammar;
    const result: Token[][] = [];
    let count = 0;
    let state: GrammarState | undefined;
    for (let start = 0; start < lines.length;) {
      if (signal?.aborted) return null;
      let end = start;
      for (let size = 0; end < lines.length && (end === start || size < SLICE_CHARS); end += 1) {
        size += lines[end]!.length + 1;
      }
      const slice = highlighter.codeToTokensWithThemes(lines.slice(start, end).join("\n"), {
        lang: language,
        themes,
        tokenizeMaxLineLength: MAX_TOKENIZED_LINE,
        grammarState: state,
      });
      state = highlighter.getLastGrammarState(slice);
      for (const line of slice) {
        count += line.length;
        result.push(
          line.map((token) => ({
            content: token.content,
            light: readable(token.variants.light?.color, "light"),
            dark: readable(token.variants.dark?.color, "dark"),
          })),
        );
      }
      if (count > MAX_TOKENS) return null;
      start = end;
      await nextTurn();
    }
    return signal?.aborted ? null : result;
  };
}
