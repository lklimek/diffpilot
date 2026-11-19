// The Files view's view model (brief §4.5): hunks, who explains their lines (R12 owners), the tree, permalinks.
import type {
  FileHunk,
  Flow,
  HunkPart,
  LineRange,
  LineSide,
  NotAnalysedCategory,
  PrFile,
  Step,
  StepId,
  Tour,
} from "../../api/generated/types";
import { rangeText } from "../section/format";

/** Lines shown around every change; the rest of the file is folded. */
export const CONTEXT_LINES = 3;

const count = (r: LineRange) => r.end_line - r.start_line + 1;
const overlap = (r: LineRange, from: number, to: number) => r.start_line <= to && r.end_line >= from;

/** The side the file is read on: the base commit for a deleted file, else the head. */
export const displaySide = (file: PrFile): LineSide => (file.status === "deleted" ? "base" : "head");

/** The file's path on `side` (the old path of a renamed file at the base commit). */
export const pathOn = (file: PrFile, side: LineSide) =>
  side === "base" ? (file.old_path ?? file.path) : file.path;

/** "lines 96–109 (+ 2 removed)", "line 35", "removed lines 52–62". */
export function hunkLabel(hunk: FileHunk): string {
  const lines = (r: LineRange) => `${count(r) === 1 ? "line" : "lines"} ${rangeText(r)}`;
  if (!hunk.head) return hunk.base ? `removed ${lines(hunk.base)}` : "no lines";
  return hunk.base ? `${lines(hunk.head)} (+ ${count(hunk.base)} removed)` : lines(hunk.head);
}

/** The lines a hunk's permalink names: its head lines, else (a pure deletion) its base lines. */
export function hunkLink(hunk: FileHunk): { side: LineSide } & LineRange {
  return hunk.head ? { side: "head", ...hunk.head } : { side: "base", ...hunk.base! };
}

/** The hunk holding `line` on `side`. */
export const hunkAt = (file: PrFile, side: LineSide, line: number) =>
  file.hunks.find((h) => {
    const r = side === "head" ? h.head : h.base;
    return r !== null && r.start_line <= line && line <= r.end_line;
  });

/** Hunks with a changed line in `from..=to` on `side`, in file order. */
export const hunksIn = (file: PrFile, side: LineSide, from: number, to: number) =>
  file.hunks.filter((h) => {
    const r = side === "head" ? h.head : h.base;
    return r !== null && overlap(r, from, to);
  });

/** The hunk parts in `from..=to` on `side`, clipped to it. */
export const partsIn = (file: PrFile, side: LineSide, from: number, to: number): HunkPart[] =>
  file.hunks
    .flatMap((h) => h.parts)
    .filter((p) => p.side === side && overlap(p.range, from, to))
    .map((p) => ({
      ...p,
      range: { start_line: Math.max(from, p.range.start_line), end_line: Math.min(to, p.range.end_line) },
    }));

/** Parts in no flow (Unexplained, Not analysed, a line rule, an identical move, whitespace only). */
export const noFlowParts = (parts: readonly HunkPart[]) =>
  parts.filter((p) => p.owner.kind !== "step" && p.owner.kind !== "flow");

/** The step owning every changed line of `from..=to` on `side`; null with none or with several owners (A4). */
export function selectionStep(file: PrFile, side: LineSide, from: number, to: number): StepId | null {
  const owners = new Set(
    partsIn(file, side, from, to).map((p) => (p.owner.kind === "step" ? p.owner.step_id : "")),
  );
  const [only] = owners;
  return owners.size === 1 && only ? only : null;
}

/** A step of the tour with its flow. */
export interface StepPlace {
  flow: Flow;
  step: Step;
}

/** Where a step sits in a set of lines: it explains them, or shows lines another part explains. */
export interface StepAtLines extends StepPlace {
  role: "explained" | "also";
  /** The head lines of this set it explains or shows. */
  lines: LineRange[];
  /** The removed (base) lines of this set it explains or shows. */
  removed: LineRange[];
}

/** A flow at some lines: its steps there, or (`planned`) lines its plan holds before it was analysed. */
export interface FlowAtLines {
  flow: Flow;
  planned: boolean;
  steps: StepAtLines[];
}

/** The tour's step `id` with its flow. */
export function stepPlace(tour: Tour, id: StepId): StepPlace | undefined {
  for (const flow of tour.flows) {
    const step = flow.steps.find((s) => s.id === id);
    if (step) return { flow, step };
  }
  return undefined;
}

/** Every flow and step at `parts`, in tour order; a step that explains some lines is never listed as also shown. */
export function flowsAt(tour: Tour, parts: readonly HunkPart[]): FlowAtLines[] {
  const steps = new Map<StepId, StepAtLines>();
  const planned = new Set<string>();
  const add = (id: StepId, role: StepAtLines["role"], part: HunkPart) => {
    const place = stepPlace(tour, id);
    if (!place) return;
    const at = steps.get(id) ?? { ...place, role, lines: [], removed: [] };
    if (role === "explained") at.role = "explained";
    (part.side === "base" ? at.removed : at.lines).push(part.range);
    steps.set(id, at);
  };
  for (const part of parts) {
    if (part.owner.kind === "step") add(part.owner.step_id, "explained", part);
    if (part.owner.kind === "flow") planned.add(part.owner.flow_id);
    for (const id of part.also_in) add(id, "also", part);
  }
  return tour.flows.flatMap((flow) => {
    const here = flow.steps.flatMap((s) => {
      const at = steps.get(s.id);
      return at ? [{ ...at, lines: mergeRanges(at.lines), removed: mergeRanges(at.removed) }] : [];
    });
    return here.length > 0 || planned.has(flow.id)
      ? [{ flow, planned: planned.has(flow.id), steps: here }]
      : [];
  });
}

/** All parts of a file. */
export const fileParts = (file: PrFile) => file.hunks.flatMap((h) => h.parts);

/** Flow positions, ticked and all steps with lines in the file (explained or shown there). */
export function fileTicks(tour: Tour, file: PrFile, ticked: readonly StepId[]) {
  const flows = flowsAt(tour, fileParts(file));
  const steps = flows.flatMap((f) => f.steps.map((s) => s.step.id));
  return {
    flows: flows.map((f) => f.flow.position),
    ticked: steps.filter((id) => ticked.includes(id)).length,
    total: steps.length,
  };
}

/** Changed lines of the file in no flow because the analysis did not explain them. */
export const unexplainedLines = (file: PrFile) =>
  fileParts(file)
    .filter((p) => p.owner.kind === "unexplained")
    .reduce((n, p) => n + count(p.range), 0);

/** A group of the file tree: Code, or one Not analysed category. */
export interface TreeGroup {
  key: "code" | NotAnalysedCategory;
  files: PrFile[];
}

const GROUP_ORDER: TreeGroup["key"][] = ["code", "tests", "docs_comments", "generated_mechanical"];
const groupKey = (file: PrFile): TreeGroup["key"] =>
  file.group.kind === "code" ? "code" : file.group.category;
const byPath = (a: PrFile, b: PrFile) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0);

/** Tree groups in their fixed order (Code first), files in path order; empty groups are left out. */
export function treeGroups(files: readonly PrFile[]): TreeGroup[] {
  return GROUP_ORDER.map((key) => ({
    key,
    files: files.filter((f) => groupKey(f) === key).sort(byPath),
  })).filter((g) => g.files.length > 0);
}

/** Files in tree order: `n` / `p`, and the file opened after marking one Viewed (A1). */
export const treeOrder = (files: readonly PrFile[]) => treeGroups(files).flatMap((g) => g.files);

/** The file after (`1`) or before (`-1`) `path` in tree order; none past either end. */
export function nextFile(files: readonly PrFile[], path: string, step: 1 | -1 = 1): PrFile | undefined {
  const order = treeOrder(files);
  const at = order.findIndex((f) => f.path === path);
  return at < 0 ? undefined : order[at + step];
}

/** Files GitHub reports viewed (not those changed since), out of all files. */
export const viewedCount = (files: readonly PrFile[]) => ({
  viewed: files.filter((f) => f.viewed === "viewed").length,
  total: files.length,
});

/** Sorted ranges with overlapping and adjacent ones joined. */
export function mergeRanges(ranges: readonly LineRange[]): LineRange[] {
  const sorted = [...ranges].sort((a, b) => a.start_line - b.start_line);
  const out: LineRange[] = [];
  for (const r of sorted) {
    const last = out.at(-1);
    if (last && r.start_line <= last.end_line + 1) last.end_line = Math.max(last.end_line, r.end_line);
    else out.push({ ...r });
  }
  return out;
}

/**
 * The lines shown on the file's side: every change with `CONTEXT_LINES` around it (a pure deletion around its
 * marker) and the ranges the reader opened. The server clamps the end of a range to the file.
 */
export function fileRegions(file: PrFile, opened: readonly LineRange[]): LineRange[] {
  const side = displaySide(file);
  const around = (first: number, last: number): LineRange => ({
    start_line: Math.max(1, first - CONTEXT_LINES),
    end_line: last + CONTEXT_LINES,
  });
  const changes = file.hunks.flatMap((h) => {
    const r = side === "head" ? h.head : h.base;
    if (r) return [around(r.start_line, r.end_line)];
    // Removed lines sit between head lines `after_line` and `after_line + 1`.
    if (side === "head" && h.after_line !== null) return [around(h.after_line + 1, h.after_line)];
    return [];
  });
  return mergeRanges([...changes, ...opened]);
}

/** A line permalink of the Files view (`#lines?file&side&line&end`, the Flows view's line format). */
export interface FilesLink {
  file: string;
  side: LineSide;
  line?: number;
  end?: number;
  /** A step to narrow the hunk to (links from the Flows view). */
  step?: StepId;
}

const positive = (value: string | null) => {
  const n = Number(value);
  return value !== null && Number.isSafeInteger(n) && n > 0 ? n : undefined;
};

/** The link in a location hash; none for any other hash. */
export function parseFilesHash(hash: string): FilesLink | undefined {
  if (!hash.startsWith("#lines?")) return undefined;
  const query = new URLSearchParams(hash.slice("#lines?".length));
  const file = query.get("file");
  if (!file) return undefined;
  const side: LineSide = query.get("side") === "base" ? "base" : "head";
  const line = positive(query.get("line"));
  const end = line === undefined ? undefined : positive(query.get("end"));
  const step = line === undefined ? undefined : (query.get("step") ?? undefined);
  return {
    file,
    side,
    ...(line !== undefined ? { line } : {}),
    ...(end !== undefined ? { end } : {}),
    ...(step ? { step } : {}),
  };
}

/** The location hash of a link. */
export function filesHash(link: FilesLink): string {
  const query = new URLSearchParams({ file: link.file, side: link.side });
  if (link.line !== undefined) {
    query.set("line", String(link.line));
    if (link.end !== undefined && link.end !== link.line) query.set("end", String(link.end));
    if (link.step) query.set("step", link.step);
  }
  return `#lines?${query}`;
}

/** What the Inspector describes: the file, a hunk (optionally narrowed to one step) or the reader's lines. */
export type FilesSubject =
  | { kind: "file" }
  | { kind: "hunk"; id: string; step?: StepId }
  | { kind: "lines"; side: LineSide; from: number; to: number };

/** The subject a link opens: a range equal to a hunk's lines is that hunk (A3); a step narrows its hunk. */
export function subjectOf(file: PrFile, link: FilesLink): FilesSubject {
  if (link.line === undefined) return { kind: "file" };
  const end = link.end ?? link.line;
  const [from, to] = [Math.min(link.line, end), Math.max(link.line, end)];
  // A step narrows the hunk at the line only when it explains or shows lines of that hunk.
  const at = link.step ? hunkAt(file, link.side, from) : undefined;
  const step = link.step;
  if (
    at &&
    step &&
    at.parts.some((p) => (p.owner.kind === "step" && p.owner.step_id === step) || p.also_in.includes(step))
  )
    return { kind: "hunk", id: at.id, step };
  const exact = file.hunks.find((h) => {
    const l = hunkLink(h);
    return l.side === link.side && l.start_line === from && l.end_line === to;
  });
  return exact ? { kind: "hunk", id: exact.id } : { kind: "lines", side: link.side, from, to };
}

/** The link of a subject of `file`. */
export function subjectLink(file: PrFile, subject: FilesSubject): FilesLink {
  const side = displaySide(file);
  if (subject.kind === "file") return { file: file.path, side };
  if (subject.kind === "lines")
    return { file: file.path, side: subject.side, line: subject.from, end: subject.to };
  const hunk = file.hunks.find((h) => h.id === subject.id);
  if (!hunk) return { file: file.path, side };
  const l = hunkLink(hunk);
  return {
    file: file.path,
    side: l.side,
    line: l.start_line,
    end: l.end_line,
    ...(subject.step ? { step: subject.step } : {}),
  };
}

/** The Files view's link of a step: its file at its first changed line (or its first line), narrowed to it. */
export function stepFilesLink(step: Step): FilesLink {
  const first = step.facts.changed_lines[0]?.start_line ?? step.facts.range.start_line;
  return { file: step.facts.file, side: step.facts.side, line: first, step: step.id };
}
