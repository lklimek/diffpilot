import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
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

/** Text shown in `element`, without the hidden tooltips of its provenance tags. */
const shown = (element: Element | null) => {
  const copy = element?.cloneNode(true) as Element | undefined;
  copy?.querySelectorAll("[role='tooltip']").forEach((tip) => tip.remove());
  return copy?.textContent ?? null;
};

/** The marks line of a queue row (the row's button is named by its ID first). */
const marksOf = (queue: HTMLElement, id: RegExp) =>
  within(queue).getByRole("button", { name: id }).querySelector<HTMLElement>("[data-marks]");

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

describe("QueueList state marks in Needs you (owner 2026-11-23, plan §3.9)", () => {
  it("each row names its state in words; LLM recommendations carry the LLM tag and are no fact badges", () => {
    const queue = showQueue();
    const c14 = marksOf(queue, /^C14(?!\d)/)!;
    expect(c14).toHaveTextContent("G-FUNDS");
    expect(c14).toHaveTextContent("Blocking");
    expect(c14.querySelector("[data-provenance='github']")).not.toBeNull();
    const fix = c14.querySelector<HTMLElement>("[data-recommendation='fix_here']")!;
    expect(shown(fix)).toBe("LLM recommends Fix here");
    expect(fix.querySelector("[data-provenance='llm']")).not.toBeNull();
    // The gate of C14 comes from the LLM assessment: tagged and dashed like LLM output, in the gate's red.
    expect(c14.querySelector("[data-gate='G-FUNDS'] [data-provenance='llm']")).not.toBeNull();
    expect(c14.querySelector("[data-gate='G-FUNDS']")).toHaveClass("border-dashed", "text-high");

    const c21 = marksOf(queue, /^C21(?!\d)/)!;
    expect(c21).toHaveTextContent("asked again");
    expect(shown(c21.querySelector("[data-recommendation='reject']"))).toBe("LLM recommends Reject");
    expect(marksOf(queue, /^C12(?!\d)/)).toHaveTextContent("Ask answer");

    const group = within(queue).getByRole("button", { name: /threat model \(2\)/ });
    expect(group.querySelector("[data-recommendation='reject']")).not.toBeNull();
    expect(marksOf(queue, /^P24(?!\d)/)).toBeNull();

    expect(queue.querySelector("[data-marks] [data-fact]")).toBeNull();
    for (const mark of queue.querySelectorAll("[data-marks] *"))
      expect(mark).not.toHaveAttribute("tabindex", "0");
    expectTrustWording(queue.textContent ?? "");
  });

  it("an owner's gate mark has no LLM tag; a recommendation to resolve says already fixed in words", () => {
    const items = listItems().map((i) =>
      i.no === 15
        ? { ...i, gates: [{ gate: "G-SECRET" as const, source: "owner" as const }] }
        : i.no === 22
          ? {
              ...i,
              assessment: {
                ...i.assessment!,
                recommendation: { ...i.assessment!.recommendation, kind: "resolve" as const },
              },
            }
          : i,
    );
    render(
      <QueueList
        queue={inListOrder(overview().queue)}
        items={new Map(items.map((i) => [i.no, i]))}
        current={null}
        onOpen={() => {}}
        restCount={0}
        onDecideRest={() => {}}
        onOpenLog={() => {}}
        onOpenCarryOvers={() => {}}
      />,
    );
    const queue = screen.getByRole("navigation", { name: "Queue" });
    expect(marksOf(queue, /^C15(?!\d)/)!.querySelector("[data-gate='G-SECRET']")).toHaveTextContent(
      "G-SECRET",
    );
    expect(
      marksOf(queue, /^C15(?!\d)/)!.querySelector("[data-gate='G-SECRET'] [data-provenance]"),
    ).toBeNull();
    // The owner's own gate stays solid.
    expect(marksOf(queue, /^C15(?!\d)/)!.querySelector("[data-gate='G-SECRET']")).not.toHaveClass(
      "border-dashed",
    );
    expect(shown(marksOf(queue, /^C22(?!\d)/)!.querySelector("[data-recommendation='resolve']"))).toBe(
      "LLM recommends Resolve (already fixed)",
    );
  });
});
