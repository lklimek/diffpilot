import { describe, expect, it } from "vitest";
import type { AskAnchor, AskQuestion } from "./generated/types";
import { withoutRetried } from "./queries";

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
const step = (id: string): AskAnchor => ({ kind: "step", step_id: id, lines: null });

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
