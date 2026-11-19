import { describe, expect, it } from "vitest";
import type { AskAnchor, AskQuestion, Tour } from "../../../api/generated/types";
import { seedFor } from "../../../mocks/fixtures";
import { questionsAt, subjectOf, threadsOf, unreadOf, withoutRetried } from "./model";

const tour = seedFor("default", "tour") as Tour;

const q = (id: string, minute: number, anchor: AskAnchor, extra: Partial<AskQuestion> = {}): AskQuestion => ({
  id,
  mine: true,
  anchor,
  text: `question ${id}`,
  state: "answered",
  answer: { answer: "a", evidence: [] },
  failure: null,
  created_at: `2026-11-19T10:${String(minute).padStart(2, "0")}:00Z`,
  ...extra,
});
const step = (id: string, lines: { start_line: number; end_line: number } | null = null): AskAnchor => ({
  kind: "step",
  step_id: id,
  lines,
});

describe("threadsOf", () => {
  it("groups follow-ups under their first question, oldest first; a follow-up of a follow-up joins the thread", () => {
    const threads = threadsOf([
      q("c", 3, step("f1.5"), { parent_id: "a" }),
      q("b", 2, { kind: "pr" }),
      q("a", 1, step("f1.5")),
      q("d", 4, step("f1.5"), { parent_id: "c" }),
    ]);
    expect(threads.map((t) => [t.root.id, t.followUps.map((f) => f.id)])).toEqual([
      ["a", ["c", "d"]],
      ["b", []],
    ]);
  });

  it("shows a follow-up whose first question is unknown as a thread of its own", () => {
    expect(threadsOf([q("x", 1, { kind: "pr" }, { parent_id: "gone" })]).map((t) => t.root.id)).toEqual([
      "x",
    ]);
  });
});

describe("subjectOf", () => {
  it("names a step with its view, lines, flow and step", () => {
    expect(subjectOf(step("f1.5"), tour)).toMatchObject({
      view: "Flows",
      code: "src/rpc/client.rs:99",
      text: "flow 1 · step 5 · Retry allowed?",
      what: "this step",
    });
  });

  it("names lines with the step they lie in, a whole file, a flow and the whole PR", () => {
    expect(subjectOf(step("f1.6", { start_line: 100, end_line: 102 }), tour)).toMatchObject({
      view: "Files",
      code: "src/rpc/client.rs:100–102",
      text: "flow 1 · step 6",
      what: "these lines",
    });
    expect(
      subjectOf(
        { kind: "lines", path: "src/a.rs", side: "base", range: { start_line: 3, end_line: 9 } },
        tour,
      ),
    ).toMatchObject({ view: "Files", code: "src/a.rs:3–9 (old code)", text: "", what: "these lines" });
    expect(subjectOf({ kind: "lines", path: "src/a.rs", side: "head", range: null }, tour)).toMatchObject({
      view: "Files",
      code: "src/a.rs",
      text: "whole file",
      what: "this file",
    });
    expect(subjectOf({ kind: "flow", flow_id: "f2" }, tour)).toMatchObject({
      view: "Flows",
      code: null,
      text: "flow 2 · Default timeout 30 s → 5 s",
      what: "this flow",
    });
    expect(subjectOf({ kind: "pr" }, tour)).toMatchObject({
      view: null,
      text: "Whole PR",
      what: "the whole PR",
    });
  });
});

describe("questionsAt: the Inspector copy of a place", () => {
  const all = [
    q("a", 1, step("f1.5")),
    q("a2", 2, step("f1.5"), { parent_id: "a" }),
    q("b", 3, step("f1.6", { start_line: 100, end_line: 102 })),
    q("c", 4, {
      kind: "lines",
      path: "src/rpc/client.rs",
      side: "head",
      range: { start_line: 101, end_line: 101 },
    }),
    q("d", 5, { kind: "flow", flow_id: "f1" }),
    q("e", 6, { kind: "pr" }),
    q("f", 7, { kind: "lines", path: "src/rpc/client.rs", side: "head", range: null }),
  ];
  const ids = (place: AskAnchor) => questionsAt(all, place, tour).map((t) => t.root.id);

  it("keeps the threads about the place, with their follow-ups", () => {
    expect(ids(step("f1.5"))).toEqual(["a"]);
    expect(questionsAt(all, step("f1.5"), tour)[0]!.followUps.map((f) => f.id)).toEqual(["a2"]);
    expect(ids(step("f1.6"))).toEqual(["b", "c"]);
    expect(ids({ kind: "flow", flow_id: "f1" })).toEqual(["d"]);
    expect(ids({ kind: "pr" })).toEqual(["e"]);
  });

  it("keeps for lines the threads whose lines overlap them, and for a file every thread on it", () => {
    expect(
      ids({
        kind: "lines",
        path: "src/rpc/client.rs",
        side: "head",
        range: { start_line: 99, end_line: 100 },
      }),
    ).toEqual(["a", "b", "f"]);
    expect(ids({ kind: "lines", path: "src/rpc/client.rs", side: "head", range: null })).toEqual([
      "a",
      "b",
      "c",
      "f",
    ]);
  });
});

describe("unreadOf", () => {
  it("counts outcomes not seen yet, failed answers included; pending and answering ones are not outcomes", () => {
    const all = [
      q("a", 1, { kind: "pr" }),
      q("b", 2, { kind: "pr" }, { state: "failed", answer: null, failure: "x" }),
      q("c", 3, { kind: "pr" }, { state: "answering", answer: null }),
    ];
    expect(unreadOf(all, new Set(["a"]))).toEqual(["b"]);
  });
});

describe("withoutRetried: a failed question asked again is replaced by its retry", () => {
  it("drops a failed question once the same question was asked again in the same place, later", () => {
    const failed = q("a", 1, step("f1.5"), { state: "failed", answer: null, failure: "x" });
    const retry = q("b", 2, step("f1.5"), { text: failed.text });
    const failedFollowUp = q("c", 3, step("f1.5"), {
      parent_id: "b",
      state: "failed",
      answer: null,
      failure: "x",
    });
    const retriedFollowUp = q("d", 4, step("f1.5"), { parent_id: "b", text: failedFollowUp.text });
    expect(withoutRetried([failed, retry, failedFollowUp, retriedFollowUp]).map((x) => x.id)).toEqual([
      "b",
      "d",
    ]);
  });

  it("keeps a failed question without a later retry, one with follow-ups, and a same text elsewhere", () => {
    const lone = q("a", 1, step("f1.5"), { state: "failed", answer: null, failure: "x" });
    const elsewhere = q("b", 2, step("f1.6"), { text: lone.text });
    const withFollowUp = q("c", 3, { kind: "pr" }, { state: "failed", answer: null, failure: "x" });
    const its = q("d", 4, { kind: "pr" }, { parent_id: "c" });
    const again = q("e", 5, { kind: "pr" }, { text: withFollowUp.text });
    expect(withoutRetried([lone, elsewhere, withFollowUp, its, again]).map((x) => x.id)).toEqual([
      "a",
      "b",
      "c",
      "d",
      "e",
    ]);
  });
});
