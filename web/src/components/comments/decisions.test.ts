import { describe, expect, it } from "vitest";
import type { CommentItem, PlannedEffect } from "../../api/generated/types";
import {
  choicesFor,
  decisionLabel,
  effectLines,
  missingInput,
  plannedReply,
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

  it("words the server's planned effects in order, with the reply text the server would post", () => {
    const effect = (
      seq: number,
      kind: PlannedEffect["kind"],
      fields: Partial<PlannedEffect["params"]> = {},
    ) => ({
      seq,
      after: seq > 1 ? seq - 1 : null,
      kind,
      executor: "agent" as const,
      params: {
        text: null,
        thread_id: null,
        review_id: null,
        title: null,
        url: null,
        item_nos: [7],
        ...fields,
      },
    });
    const planned = [effect(1, "fix"), effect(2, "reply", { text: "Posted text." }), effect(3, "resolve")];
    expect(effectLines(planned)).toEqual([
      { executor: "agent", text: "fixes it and pushes." },
      { executor: "agent", text: "replies with the reply text below." },
      { executor: "agent", text: "resolves the thread." },
    ]);
    expect(plannedReply(planned)).toBe("Posted text.");
    expect(plannedReply([effect(1, "resolve")])).toBeNull();
    const group = [
      effect(1, "reply", { text: "Same.", item_nos: [18] }),
      effect(2, "reply", { text: "Same.", item_nos: [19] }),
    ];
    expect(effectLines(group, true)).toEqual([{ executor: "agent", text: 'replies on C18, C19: "Same."' }]);
  });

  it("never merges separate creations: each follow-up PR, upstream issue and TODO is its own line", () => {
    const effect = (seq: number, kind: PlannedEffect["kind"], fields: Partial<PlannedEffect["params"]>) => ({
      seq,
      after: null,
      kind,
      executor: "agent" as const,
      params: {
        text: null,
        thread_id: null,
        review_id: null,
        title: null,
        url: null,
        item_nos: [],
        ...fields,
      },
    });
    const group = [
      effect(1, "follow_up_pr", { title: "Follow-up: retries", item_nos: [18] }),
      effect(2, "follow_up_pr", { title: "Follow-up: retries", item_nos: [19] }),
      effect(3, "upstream_issue", { item_nos: [18] }),
      effect(4, "upstream_issue", { item_nos: [19] }),
      effect(5, "todo", { title: "Check the cap", item_nos: [18] }),
      effect(6, "todo", { title: "Check the cap", item_nos: [19] }),
    ];
    expect(effectLines(group, true).map((l) => l.text)).toEqual([
      'creates a new draft follow-up PR "Follow-up: retries" for C18 (no link needed; it does not exist yet).',
      'creates a new draft follow-up PR "Follow-up: retries" for C19 (no link needed; it does not exist yet).',
      'creates the upstream issue for C18 and adds a "Known limitation" line.',
      'creates the upstream issue for C19 and adds a "Known limitation" line.',
      'saves a TODO "Check the cap" for C18 (never mentioned on GitHub).',
      'saves a TODO "Check the cap" for C19 (never mentioned on GitHub).',
    ]);
    expect(effectLines([effect(1, "follow_up_pr", { title: "T", item_nos: [7] })])).toEqual([
      {
        executor: "agent",
        text: 'creates a new draft follow-up PR "T" (no link needed; it does not exist yet).',
      },
    ]);
    // "Merge with follow-ups": one PR listing the remaining items.
    expect(effectLines([effect(1, "follow_up_pr", { item_nos: [11, 22] })], true)).toEqual([
      {
        executor: "agent",
        text: "creates one draft follow-up PR listing C11, C22 (no link needed; it does not exist yet).",
      },
    ]);
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
