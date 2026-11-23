import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import type { QueueEntry } from "../../api/generated/types";
import { listItems, overview, resetReviewLoop } from "../../mocks/review-loop";
import { inListOrder, itemsAfter, QueueList } from "./QueueList";

beforeEach(() => resetReviewLoop());

const showQueue = () => {
  const items = new Map(listItems().map((i) => [i.no, i]));
  render(
    <QueueList
      queue={inListOrder(overview().queue)}
      items={items}
      current={null}
      onOpen={() => {}}
      restCount={0}
      onDecideRest={() => {}}
      onOpenLog={() => {}}
      onOpenCarryOvers={() => {}}
    />,
  );
  return screen.getByRole("navigation", { name: "Queue" });
};

describe("QueueList order (owner 2026-11-23)", () => {
  it("lists Needs you first and the PR-level questions last", () => {
    const queue = showQueue();
    expect(
      within(queue)
        .getAllByRole("heading")
        .map((h) => h.textContent),
    ).toEqual([
      "Needs you (10)",
      expect.stringMatching(/^Previously decided \(\d+\)$/),
      "Outside diffpilot (1)",
      "PR-level (1)",
    ]);
  });

  it("inListOrder puts PR-level entries after the others and keeps each section's order", () => {
    const e = (no: number, reason: QueueEntry["reason"], skipped = false): QueueEntry => ({
      item_nos: [no],
      reason,
      skipped,
    });
    const queue = [e(24, "pr_question"), e(21, "asked_again"), e(14, "gate"), e(11, "single", true)];
    expect(inListOrder(queue).map((x) => x.item_nos[0])).toEqual([21, 14, 11, 24]);
  });
});

describe("itemsAfter: where Decide and Skip go next (plan §3.2 item 9)", () => {
  const e = (...nos: number[]): QueueEntry => ({
    item_nos: nos,
    reason: nos.length > 1 ? "group" : "single",
  });
  const queue = [e(21), e(14), e(15), e(18, 19), e(24)];

  it("the entries after the given one, then from the top, without its own items", () => {
    expect(itemsAfter(queue, e(15))).toEqual([18, 19, 24, 21, 14]);
    expect(itemsAfter(queue, e(18, 19))).toEqual([24, 21, 14, 15]);
    expect(itemsAfter(queue, e(24))).toEqual([21, 14, 15, 18, 19]);
  });

  it("an item not in the queue (opened by link) is followed by the whole queue", () => {
    expect(itemsAfter(queue, e(9))).toEqual([21, 14, 15, 18, 19, 24]);
  });
});
