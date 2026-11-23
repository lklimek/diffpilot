// The mocks' server model follows the server's effect function (crates/server/src/review_loop/effects/tests.rs).
import { beforeEach, describe, expect, it } from "vitest";
import type { CommentItem, DecideRequest, DecisionKind } from "../api/generated/types";
import { DISMISS_MESSAGE, decideRest, plan, type PlanScope, type Planned } from "./effects-plan";
import { listItems, resetReviewLoop } from "./review-loop";

const byNo = (no: number) => listItems().find((i) => i.no === no)!;
const request = (kind: DecisionKind, fields: Partial<DecideRequest> = {}): DecideRequest => ({
  kind,
  reason: null,
  target: null,
  override: null,
  resolve_ticked: true,
  reply_text: null,
  comment: null,
  item_revision: 1,
  ...fields,
});
const scope: PlanScope = { number: 482, remaining: [], staleReviews: ["PRR_r5"] };
const planned = (item: CommentItem, req: DecideRequest, at = scope) => {
  const result = plan(item, req, at);
  if (typeof result === "string") throw new Error(result);
  return result;
};
const shape = (p: Planned) => p.effects.map((e) => [e.kind, e.after]);

beforeEach(resetReviewLoop);

describe("mock effect function (effects.rs plan)", () => {
  it("D1 replies only to human authors; D2 fixes, replies after the fix and resolves", () => {
    const bot = byNo(22);
    const human = { ...bot, author_kind: "human" as const };
    expect(shape(planned(bot, request("resolve")))).toEqual([["resolve", null]]);
    const d1 = planned(human, request("resolve"));
    expect(shape(d1)).toEqual([
      ["reply", null],
      ["resolve", 1],
    ]);
    expect(d1.reply_text).toBe("Per maintainer decision, this is already addressed in this pull request.");
    expect(shape(planned(bot, request("fix_here")))).toEqual([
      ["fix", null],
      ["reply", 1],
      ["resolve", 2],
    ]);
  });

  it("Accept despite the gate names the gate, resolves, and needs the reason", () => {
    const gated = byNo(14);
    const accept = request("reject", {
      override: { type: "blocker_gate", gate: "G-FUNDS" },
      resolve_ticked: true,
    });
    expect(plan(gated, accept, scope)).toBe("this decision needs a reason");
    const p = planned(gated, { ...accept, reason: "Tracked in #455." });
    expect(p.reply_text).toBe("Per maintainer decision, accepted despite G-FUNDS: Tracked in #455.");
    expect(plan(gated, { ...accept, reason: "x", resolve_ticked: false }, scope)).toBe(
      "this decision always resolves the thread",
    );
  });

  it("body items get a PR-level comment and nothing to resolve; an edited reply replaces the default", () => {
    const body = byNo(23);
    const p = planned(body, request("reject", { reason: "No.", reply_text: " Edited. " }));
    expect(shape(p)).toEqual([["pr_comment", null]]);
    expect(p.reply_text).toBe("Edited.");
  });

  it("refuses this PR as the other PR and a TODO without title", () => {
    const item = byNo(22);
    expect(
      plan(item, request("handled_in_pr", { target: { type: "existing_pr", number: 482 } }), scope),
    ).toBe("the target does not fit this decision");
    expect(plan(item, request("todo", { target: { type: "todo", title: " " } }), scope)).toBe(
      "this decision needs its target",
    );
  });

  it("P1 plans nothing without remaining items; P3 posts `reason`, else the default message", () => {
    const merge = byNo(24);
    expect(planned(merge, request("merge_with_follow_ups")).effects).toEqual([]);
    const p1 = planned(merge, request("merge_with_follow_ups"), {
      ...scope,
      remaining: [byNo(22), byNo(23)],
    });
    expect(p1.effects[0]!.params.item_nos).toEqual([22, 23]);
    const dismiss = { ...merge, stable_key: "pr:3:dismiss" };
    expect(planned(dismiss, request("dismiss_stale_reviews")).effects[0]!.params.text).toBe(DISMISS_MESSAGE);
    const typed = planned(
      dismiss,
      request("dismiss_stale_reviews", { reason: "Superseded.", reply_text: "x" }),
    );
    expect(typed.reply_text).toBe("Superseded.");
    expect(plan(merge, request("dismiss_stale_reviews"), scope)).toBe(
      "this decision is not offered for this item",
    );
  });

  it("Decide the rest leaves out gate items, repeats asked again and items without an assessment", () => {
    const rest = decideRest(listItems(), () => scope);
    expect(rest.left_out).toEqual(
      expect.arrayContaining([
        { item_no: 14, reason: "gate" },
        { item_no: 15, reason: "gate" },
        { item_no: 21, reason: "asked_again" },
      ]),
    );
    expect(rest.rows.map((r) => r.row.item_no)).not.toContain(21);
    expect(rest.rows.every((r) => r.row.resolve_ticked && r.row.reply_text === null)).toBe(true);
  });
});
