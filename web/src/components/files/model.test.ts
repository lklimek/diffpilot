import { describe, expect, it } from "vitest";
import type { FilesResponse, PrFile, Tour } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import seed from "../../mocks/files-482.json";
import {
  fileRegions,
  fileTicks,
  filesHash,
  flowsAt,
  hunkLabel,
  hunkLink,
  hunksIn,
  nextFile,
  noFlowParts,
  parseFilesHash,
  partsIn,
  selectionStep,
  subjectOf,
  treeGroups,
  treeOrder,
  unexplainedLines,
  viewedCount,
} from "./model";

const tour = seedFor("default", "tour") as Tour;
const files = (seed as FilesResponse).files;
const file = (path: string) => files.find((f) => f.path === path)!;
const client = file("src/rpc/client.rs");
const hunk = (f: PrFile, id: string) => f.hunks.find((h) => h.id === id)!;

describe("Files view model", () => {
  it("labels hunks by their lines, removed lines counted, a pure deletion by its base lines", () => {
    expect(hunkLabel(hunk(client, "h19"))).toBe("lines 96–109 (+ 2 removed)");
    expect(hunkLabel(hunk(client, "h14"))).toBe("line 35");
    expect(hunkLabel(hunk(client, "h16"))).toBe("removed lines 52–62");
    expect(hunkLabel(hunk(file("src/lib.rs"), "h13"))).toBe("removed line 18");
  });

  it("links a hunk by its head lines, a pure deletion by its base lines", () => {
    expect(hunkLink(hunk(client, "h19"))).toEqual({ side: "head", start_line: 96, end_line: 109 });
    expect(hunkLink(hunk(client, "h16"))).toEqual({ side: "base", start_line: 52, end_line: 62 });
  });

  it("round-trips the line permalink and reads a bare file link", () => {
    const hash = filesHash({ file: "src/rpc/client.rs", side: "head", line: 100, end: 102 });
    expect(hash).toBe("#lines?file=src%2Frpc%2Fclient.rs&side=head&line=100&end=102");
    expect(parseFilesHash(hash)).toEqual({ file: "src/rpc/client.rs", side: "head", line: 100, end: 102 });
    expect(parseFilesHash("#lines?file=README.md")).toEqual({ file: "README.md", side: "head" });
    expect(parseFilesHash("#lines?file=a.rs&line=-1")).toEqual({ file: "a.rs", side: "head" });
    expect(parseFilesHash("#questions")).toBeUndefined();
    expect(parseFilesHash(filesHash({ file: "a.rs", side: "head", line: 4, step: "f1.2" }))).toEqual({
      file: "a.rs",
      side: "head",
      line: 4,
      step: "f1.2",
    });
  });

  it("opens a link equal to a hunk's lines as that hunk, any other range as a selection (A3)", () => {
    expect(subjectOf(client, { file: client.path, side: "head", line: 96, end: 109 })).toEqual({
      kind: "hunk",
      id: "h19",
    });
    expect(subjectOf(client, { file: client.path, side: "head", line: 100, end: 102 })).toEqual({
      kind: "lines",
      side: "head",
      from: 100,
      to: 102,
    });
    expect(subjectOf(client, { file: client.path, side: "head", line: 102, end: 100 })).toEqual({
      kind: "lines",
      side: "head",
      from: 100,
      to: 102,
    });
    expect(subjectOf(client, { file: client.path, side: "head" })).toEqual({ kind: "file" });
    expect(subjectOf(client, { file: client.path, side: "head", line: 99, step: "f1.5" })).toEqual({
      kind: "hunk",
      id: "h19",
      step: "f1.5",
    });
  });

  it("lists every flow and step of a hunk: explained in, also shown in, in tour order", () => {
    const flows = flowsAt(tour, hunk(client, "h19").parts);
    expect(flows.map((f) => f.flow.id)).toEqual(["f1"]);
    expect(flows[0]!.steps.map((s) => [s.step.id, s.role])).toEqual([
      ["f1.2", "explained"],
      ["f1.3", "explained"],
      ["f1.4", "explained"],
      ["f1.5", "explained"],
      ["f1.6", "explained"],
      ["f1.7", "explained"],
      ["f1.8", "explained"],
    ]);
    // A step's head lines and removed (base) lines are kept apart: their numbers are of different commits.
    expect(flows[0]!.steps[0]).toMatchObject({
      lines: [{ start_line: 96, end_line: 96 }],
      removed: [{ start_line: 102, end_line: 103 }],
    });
    const context = flowsAt(tour, hunk(client, "h17").parts);
    expect(context[0]!.steps.map((s) => [s.step.id, s.role])).toEqual([["f1.2", "also"]]);
    expect(noFlowParts(hunk(client, "h17").parts).map((p) => p.owner.kind)).toEqual([
      "context_rule",
      "context_rule",
    ]);
  });

  it("names a flow not analysed yet as planned, with no step", () => {
    const lazy = structuredClone(tour);
    lazy.flows[1]!.steps = [];
    const parts = [
      {
        side: "head" as const,
        range: { start_line: 23, end_line: 23 },
        owner: { kind: "flow" as const, flow_id: "f2" },
        also_in: [],
      },
    ];
    const [flow] = flowsAt(lazy, parts);
    expect(flow).toMatchObject({ planned: true, steps: [] });
    expect(flow!.flow.id).toBe("f2");
  });

  it("clips the parts and finds the hunks a selection touches", () => {
    expect(partsIn(client, "head", 100, 102).map((p) => [p.range, p.owner])).toEqual([
      [
        { start_line: 100, end_line: 102 },
        { kind: "step", flow_id: "f1", step_id: "f1.6" },
      ],
    ]);
    expect(hunksIn(client, "head", 94, 97).map((h) => h.id)).toEqual(["h18", "h19"]);
    expect(hunksIn(client, "head", 1, 5)).toEqual([]);
  });

  it("names a selection's step only when every changed line in it lies in one step (A4)", () => {
    expect(selectionStep(client, "head", 100, 102)).toBe("f1.6");
    expect(selectionStep(client, "head", 99, 101)).toBeNull();
    expect(selectionStep(client, "head", 1, 5)).toBeNull();
    expect(selectionStep(client, "head", 88, 89)).toBeNull();
  });

  it("counts step ticks and Unexplained lines per file", () => {
    expect(fileTicks(tour, client, ["f1.2", "f4.1"])).toEqual({ flows: [1, 4], ticked: 2, total: 9 });
    expect(fileTicks(tour, file("README.md"), [])).toEqual({ flows: [], ticked: 0, total: 0 });
    expect(unexplainedLines(file("src/rpc/metrics.rs"))).toBe(6);
    expect(unexplainedLines(client)).toBe(0);
  });

  it("orders the tree: Code, then each Not analysed group, path order inside", () => {
    expect(treeGroups(files).map((g) => [g.key, g.files.length])).toEqual([
      ["code", 6],
      ["tests", 4],
      ["docs_comments", 2],
      ["generated_mechanical", 1],
    ]);
    const order = treeOrder(files).map((f) => f.path);
    expect(order.slice(0, 3)).toEqual(["src/config.rs", "src/lib.rs", "src/rpc/client.rs"]);
    expect(nextFile(files, "src/config.rs")?.path).toBe("src/lib.rs");
    expect(nextFile(files, order.at(-1)!)).toBeUndefined();
  });

  it("counts only files GitHub reports viewed", () => {
    expect(viewedCount(files)).toEqual({ viewed: 3, total: 13 });
  });

  it("shows changes with three lines of context, merged, plus the ranges the reader opened", () => {
    expect(fileRegions(file("src/lib.rs"), [])).toEqual([{ start_line: 10, end_line: 21 }]);
    expect(fileRegions(client, []).slice(0, 2)).toEqual([
      { start_line: 32, end_line: 45 },
      { start_line: 50, end_line: 55 },
    ]);
    expect(fileRegions(client, [{ start_line: 46, end_line: 49 }]).slice(0, 1)).toEqual([
      { start_line: 32, end_line: 55 },
    ]);
    expect(fileRegions(file("tests/send_once.rs"), [])).toEqual([{ start_line: 1, end_line: 13 }]);
  });
});
