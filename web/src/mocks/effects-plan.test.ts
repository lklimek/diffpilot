// The mocks' server model follows the server's effect function (crates/server/src/review_loop/effects/tests.rs).
import { beforeEach, describe, expect, it } from "vitest";
import type { CommentItem, DecideRequest, Decision, DecisionKind } from "../api/generated/types";
import {
  DISMISS_MESSAGE,
  decideRest,
  dismissal,
  dismissBody,
  plan,
  staleReviews,
  type PlanScope,
  type Planned,
} from "./effects-plan";
import {
  BOT_CHANGE_REQUESTS,
  decide,
  decideBatch,
  decideRestList,
  isRefused,
  listItems,
  overview,
  previewEffects,
  reopen,
  resetReviewLoop,
  undo,
} from "./review-loop";

import { setScenario } from "./scenarios";

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
/** The effects RC16 plans for one row of the mock store; a refusal fails the test. */
function previewOf(no: number, req: DecideRequest) {
  const result = previewEffects([{ item_no: no, ...req }]);
  if (isRefused(result)) throw new Error(result[2]);
  return result[0]!.effects;
}
/** C23's review and the line naming it (effects.rs `EffectItem::on_review`). */
const C23_URL = "https://github.com/acme/node-client/pull/482#pullrequestreview-77";
const C23_LINE = `@octo-reviewer, re [your review](${C23_URL}):`;
const REVIEWS_CHANGED =
  "the reviews this question lists changed since it was asked; keep the reviews, the next round asks again";

/** Every item thepastaclaw raised gets a standing decision, so its change request is stale. */
function decideAllOfThepastaclaw() {
  const standing = listItems().flatMap((i) => i.decisions)[0]!;
  for (const i of listItems().filter((i) => i.author === "thepastaclaw" && i.state !== "outside"))
    Object.assign(i, { state: "decided", decisions: [standing] });
}

/** The "Dismiss stale bot reviews?" question of round 3 listing `BOT_CHANGE_REQUESTS`, added to the store. */
function askDismissal(): CommentItem {
  const question: CommentItem = {
    ...byNo(24),
    id: "00000000-0000-4000-8000-000000000030",
    no: 30,
    stable_key: "pr:3:dismiss",
    title: "Dismiss stale bot reviews?",
    body: dismissBody(BOT_CHANGE_REQUESTS),
  };
  listItems().push(question);
  return question;
}

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
    expect(p.reply_text).toBe(`${C23_LINE}\n> Doc comment names the old default\nEdited.`);
  });

  it("a PR-level comment answering a review names it: a person by an @-mention, a bot without one", () => {
    const fixed = "Per maintainer decision, this is fixed in this pull request.";
    expect(planned(byNo(23), request("fix_here")).reply_text).toBe(
      `${C23_LINE}\n> Doc comment names the old default\n${fixed}`,
    );
    const bot = { ...byNo(23), author: "coderabbitai", author_kind: "bot" as const };
    expect(planned(bot, request("fix_here")).effects[1]!.params.text).toBe(
      `Re [coderabbitai's review](${C23_URL}):\n> Doc comment names the old default\n${fixed}`,
    );
    expect(planned(byNo(22), request("fix_here")).reply_text).toBe(fixed);
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
    expect(p1.effects.filter((e) => e.params.text !== null).map((e) => [e.kind, e.params.text])).toEqual([
      ["reply", "Per maintainer decision, this moves to a follow-up pull request."],
      [
        "pr_comment",
        `${C23_LINE}\n> Doc comment names the old default\nPer maintainer decision, this moves to a follow-up pull request.`,
      ],
    ]);
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

  it("P1 (mod.rs `Loaded::scope`) leaves the waiting gate items C14 and C15 in the queue", () => {
    const p1 = previewOf(24, request("merge_with_follow_ups"));
    const touched = p1.flatMap((e) => e.params.item_nos);
    expect(p1[0]!.kind).toBe("follow_up_pr");
    expect(p1[0]!.params.item_nos).toContain(22);
    expect(touched).not.toContain(14);
    expect(touched).not.toContain(15);
  });

  it("P3 dismisses the reviews its question lists while they are still stale (mod.rs `dismissal`)", () => {
    expect(staleReviews(BOT_CHANGE_REQUESTS, listItems())).toEqual([]);
    decideAllOfThepastaclaw();
    expect(staleReviews(BOT_CHANGE_REQUESTS, listItems())).toEqual(BOT_CHANGE_REQUESTS);
    const question = askDismissal();
    expect(question.body).toBe(
      "Every item these reviews raised has a decision:\n- thepastaclaw: https://github.com/acme/node-client/pull/482#pullrequestreview-3",
    );
    const p3 = previewOf(30, request("dismiss_stale_reviews"));
    expect(p3.map((e) => [e.kind, e.params.review_id, e.params.text])).toEqual([
      ["dismiss_review", "PRR_round3", DISMISS_MESSAGE],
    ]);
    // A listed bot's item asked again: its review is no longer stale, so deciding P3 is refused.
    Object.assign(byNo(22), { state: "reopened" });
    expect(dismissal(question, staleReviews(BOT_CHANGE_REQUESTS, listItems()))).toBe(REVIEWS_CHANGED);
    expect(previewEffects([{ item_no: 30, ...request("dismiss_stale_reviews") }])).toEqual([
      400,
      "invalid_request",
      `C30: ${REVIEWS_CHANGED}.`,
    ]);
    expect(isRefused(previewEffects([{ item_no: 30, ...request("keep_reviews") }]))).toBe(false);
  });

  it("RC9 refuses an item asked again in bulk; a group still takes it (mod.rs `decide`)", () => {
    const row = {
      item_no: 21,
      ...request("reject", {
        reason: "The cap is a soft limit by design.",
        item_revision: byNo(21).revision,
      }),
    };
    expect(decideBatch("bulk", [row])).toEqual([
      400,
      "invalid_request",
      "C21: it was asked again, so it is decided on its own, not in bulk.",
    ]);
    expect(byNo(21).state).toBe("reopened");
    expect(isRefused(decideBatch("group", [row]))).toBe(false);
  });

  it("an item the latest read no longer produces leaves the queue, Decide the rest and P1; bulk refuses it", () => {
    const merged = () => previewOf(24, request("merge_with_follow_ups")).flatMap((e) => e.params.item_nos);
    expect(overview().queue.flatMap((e) => e.item_nos)).toContain(22);
    expect(decideRestList().rows.map((r) => r.row.item_no)).toContain(22);
    expect(merged()).toContain(22);
    Object.assign(byNo(22), { gone_at: "2026-11-18T09:00:00Z" });
    expect(overview().queue.flatMap((e) => e.item_nos)).not.toContain(22);
    const rest = decideRestList();
    expect([...rest.rows.map((r) => r.row.item_no), ...rest.left_out.map((l) => l.item_no)]).not.toContain(
      22,
    );
    expect(merged()).not.toContain(22);
    const row = { item_no: 22, ...request("fix_here", { item_revision: byNo(22).revision }) };
    expect(decideBatch("bulk", [row])).toEqual([
      400,
      "invalid_request",
      "C22: it is not in the latest read from GitHub, so it is decided on its own, not in bulk.",
    ]);
    expect(isRefused(decide("C22", row))).toBe(false);
  });

  it("a standing Merge with follow-ups covers its items in every waiting list until it is undone (mod.rs `followed_up`)", () => {
    const queued = () => new Set(overview().queue.flatMap((e) => e.item_nos));
    const rest = () => {
      const r = decideRestList();
      return [...r.rows.map((x) => x.row.item_no), ...r.left_out.map((x) => x.item_no)];
    };
    const p1 = decide("C24", request("merge_with_follow_ups", { item_revision: byNo(24).revision }));
    if (isRefused(p1)) throw new Error(p1[2]);
    const covered = (p1 as Decision).effects.find((e) => e.kind === "follow_up_pr")!.params.item_nos;
    expect(covered).toEqual(expect.arrayContaining([21, 22]));
    // The covered items stay open on their own; the server's queue.rs `waiting` leaves them out.
    expect(byNo(22).state).toBe("undecided");
    expect(queued()).toEqual(new Set([14, 15]));
    expect(overview().counts.undecided).toBe(2);
    expect(rest()).not.toContain(22);
    expect(rest()).not.toContain(21);
    // A later round's merge question acts on what is still waiting: the gate items keep their own decision.
    listItems().push({
      ...byNo(24),
      id: "00000000-0000-4000-8000-000000000031",
      no: 31,
      stable_key: "pr:4:merge",
      state: "undecided",
      decisions: [],
    });
    expect(previewOf(31, request("merge_with_follow_ups"))).toEqual([]);
    // Undone, the follow-up covers nothing: its items wait again.
    expect(isRefused(undo("C24", byNo(24).revision))).toBe(false);
    expect(queued()).toContain(22);
    expect(previewOf(31, request("merge_with_follow_ups")).flatMap((e) => e.params.item_nos)).toContain(22);
  });

  it("RC16 gives P1 and P3 rows a plan digest, other rows none (mod.rs `Loaded::plan_digest`)", () => {
    const digest = (no: number, req: DecideRequest) => {
      const answer = previewEffects([{ item_no: no, ...req }]);
      if (isRefused(answer)) throw new Error(answer[2]);
      return answer[0]!.plan_digest;
    };
    expect(digest(24, request("merge_with_follow_ups"))).toEqual(expect.any(String));
    expect(digest(24, request("another_round"))).toBeUndefined();
    expect(digest(22, request("fix_here"))).toBeUndefined();
    decideAllOfThepastaclaw();
    askDismissal();
    expect(digest(30, request("dismiss_stale_reviews"))).toEqual(expect.any(String));
    expect(digest(30, request("keep_reviews"))).toBeUndefined();
  });

  it("RC8 refuses a P1 whose confirmed digest no longer fits (409 conflict); a fresh digest stores the larger plan", () => {
    const digestOf = () => {
      const answer = previewEffects([{ item_no: 24, ...request("merge_with_follow_ups") }]);
      if (isRefused(answer)) throw new Error(answer[2]);
      return answer[0]!.plan_digest!;
    };
    expect(isRefused(decide("C11", request("fix_here", { item_revision: byNo(11).revision })))).toBe(false);
    const confirmed = digestOf();
    // Asked again by another reader: C11 joins what "Merge now" follows up.
    expect(isRefused(reopen("C11", byNo(11).revision))).toBe(false);
    const merge = (digest: string) =>
      decide(
        "C24",
        request("merge_with_follow_ups", { item_revision: byNo(24).revision, plan_digest: digest }),
      );
    expect(merge(confirmed)).toEqual([
      409,
      "conflict",
      "What the decision would do changed since you confirmed it: C24. Check the plan again.",
    ]);
    const fresh = digestOf();
    expect(fresh).not.toBe(confirmed);
    const stored = merge(fresh);
    if (isRefused(stored)) throw new Error(stored[2]);
    expect(stored.effects.flatMap((e) => e.params.item_nos)).toContain(11);
  });

  it("a digest on a row whose plan has none is refused; a PR-level row without one is stored as planned then", () => {
    const row = request("fix_here", { item_revision: byNo(22).revision, plan_digest: "d" });
    expect(decide("C22", row)).toEqual([
      409,
      "conflict",
      "What the decision would do changed since you confirmed it: C22. Check the plan again.",
    ]);
    expect(
      isRefused(decide("C24", request("merge_with_follow_ups", { item_revision: byNo(24).revision }))),
    ).toBe(false);
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

describe("decisions on linked items", () => {
  it("plans and stores effects for each following thread, keeps gates independent and applies a standing decision to later links", () => {
    setScenario("comments-v14");
    resetReviewLoop();
    try {
      const gate = byNo(31);
      gate.gates = [{ gate: "G-FUNDS", source: "owner" }];
      const body = request("fix_here");
      const preview = previewEffects([{ ...body, item_no: 14 }]);
      expect(preview).toEqual(
        expect.arrayContaining([
          expect.objectContaining({
            item_no: 30,
            effects: expect.arrayContaining([
              expect.objectContaining({
                kind: "resolve",
                params: expect.objectContaining({ thread_id: "PRRT_30" }),
              }),
            ]),
          }),
        ]),
      );
      const recorded = decide("C14", body) as Decision;
      expect(byNo(30).decisions.at(-1)).toMatchObject({
        kind: "fix_here",
        carried_from: recorded.id,
        effects: expect.arrayContaining([
          expect.objectContaining({
            kind: "resolve",
            params: expect.objectContaining({ thread_id: "PRRT_30" }),
          }),
        ]),
      });
      expect(gate.decisions).toHaveLength(0);
      Object.assign(byNo(22), { duplicate_of: 14, decision_follows_primary: true, state: "assessing" });
      expect(byNo(22).decisions.at(-1)).toMatchObject({
        carried_from: recorded.id,
        effects: expect.arrayContaining([
          expect.objectContaining({
            kind: "resolve",
            params: expect.objectContaining({ thread_id: "PRRT_22" }),
          }),
        ]),
      });
      expect(byNo(22).decisions).toHaveLength(1);
      undo("C14", byNo(14).revision);
      for (const no of [22, 30])
        expect(
          byNo(no)
            .decisions.at(-1)
            ?.effects.every((effect) => effect.state === "cancelled"),
        ).toBe(true);
    } finally {
      setScenario(null);
      resetReviewLoop();
    }
  });
});
