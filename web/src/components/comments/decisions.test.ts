import { describe, expect, it } from "vitest";
import type { CommentItem } from "../../api/generated/types";
import {
  choicesFor,
  decisionLabel,
  missingInput,
  plannedEffects,
  replyText,
  requestOf,
  resolveLocked,
} from "./decisions";

const rec = (kind: "fix_here" | "reject") => ({
  assessment: { recommendation: { kind, reason: "why", reason_class: "c", target: null } },
});
const gate = { gates: [{ gate: "G-FUNDS" as const, source: "assessment" as const }] };
type Item = Pick<CommentItem, "gates" | "assessment" | "revision">;
const item = (fields: Partial<Record<string, unknown>>) =>
  ({ gates: [], revision: 3, ...fields }) as unknown as Item;

describe("decisions (plan §2.3)", () => {
  it("lists the recommended option first (RQ7) and D8 only on gate items, last", () => {
    expect(choicesFor(item(rec("reject")))[0]).toBe("reject");
    expect(choicesFor(item(rec("reject")))).not.toContain("accept");
    const gated = choicesFor(item({ ...rec("fix_here"), ...gate }));
    expect(gated[0]).toBe("fix_here");
    expect(gated.at(-1)).toBe("accept");
  });

  it("without an assessment keeps the base order and recommends nothing", () => {
    expect(choicesFor(item({ assessment: null }))[0]).toBe("fix_here");
  });

  it("D8 is stored as Reject with the blocker-gate override and always resolves", () => {
    const request = requestOf(
      item({ ...rec("fix_here"), ...gate }),
      { choice: "accept", value: " fee display is pre-existing ", resolve: false, comment: "" },
      null,
    );
    expect(request).toMatchObject({
      kind: "reject",
      reason: "fee display is pre-existing",
      override: { type: "blocker_gate", gate: "G-FUNDS" },
      resolve_ticked: true,
      comment: null,
      item_revision: 3,
    });
    expect(resolveLocked("accept")).toBe(true);
    expect(resolveLocked("resolve")).toBe(true);
    expect(resolveLocked("reject")).toBe(false);
  });

  it("the submitted Resolve state is the explicit resolve decision", () => {
    const request = requestOf(
      item(rec("reject")),
      { choice: "reject", value: "no", resolve: false, comment: "c" },
      null,
    );
    expect(request.resolve_ticked).toBe(false);
    expect(request.comment).toBe("c");
  });

  it("asks for a reason (D3, D8), a PR number (D4) and a TODO title; never a link for a follow-up PR", () => {
    expect(missingInput({ choice: "reject", value: " " })).toMatch(/reason is required/);
    expect(missingInput({ choice: "accept", value: "" })).toMatch(/reason is required/);
    expect(missingInput({ choice: "handled_in_pr", value: "abc" })).toMatch(/number/);
    expect(missingInput({ choice: "handled_in_pr", value: "#490" })).toBeNull();
    expect(missingInput({ choice: "todo", value: "" })).toMatch(/TODO title/);
    expect(missingInput({ choice: "follow_up_pr", value: "" })).toBeNull();
    const request = requestOf(
      item({}),
      { choice: "follow_up_pr", value: "", resolve: true, comment: "" },
      null,
    );
    expect(request.target).toEqual({ type: "follow_up_pr", title: null });
  });

  it("effects: threads reply and resolve only when ticked; body items get a PR-level comment", () => {
    expect(plannedEffects("fix_here", true, true).map((e) => e.kind)).toEqual(["fix", "reply", "resolve"]);
    expect(plannedEffects("reject", false, true).map((e) => e.kind)).toEqual(["reply"]);
    expect(plannedEffects("accept", false, true).map((e) => e.kind)).toEqual(["reply", "resolve"]);
    expect(plannedEffects("reject", true, false).map((e) => e.kind)).toEqual(["pr_comment"]);
    expect(plannedEffects("todo", true, true)[0]!.text).toMatch(/never mentioned on GitHub/);
  });

  it("replies say 'per maintainer decision'; the TODO reply never names the TODO", () => {
    expect(replyText("handled_in_pr", "#490")).toBe("Per maintainer decision, this is addressed in #490.");
    expect(replyText("todo", "secret title")).toBe("Per maintainer decision, out of scope for this PR.");
    expect(replyText("resolve", "")).toBeNull();
  });

  it("labels a stored D8 with its gate", () => {
    expect(
      decisionLabel({ kind: "reject", override: { type: "blocker_gate", gate: "G-FUNDS" }, target: null }),
    ).toBe("Accept despite the gate (G-FUNDS)");
    expect(
      decisionLabel({ kind: "handled_in_pr", override: null, target: { type: "existing_pr", number: 490 } }),
    ).toBe("Handled in #490");
  });
});
