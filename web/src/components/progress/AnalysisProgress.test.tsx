import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import type { AnalysisState } from "../../api/generated/types";
import { AnalysisProgress, STALE_MS, type ProgressActivity } from "./AnalysisProgress";

function show(state: AnalysisState, busy = false, activity?: ProgressActivity) {
  const onCancel = vi.fn();
  const onRetry = vi.fn();
  const view = render(
    <AnalysisProgress
      state={state}
      reviewHref="/analyses/a1"
      busy={busy}
      onCancel={onCancel}
      onRetry={onRetry}
      activity={activity}
    />,
  );
  return { onCancel, onRetry, ...view };
}

const NOW = Date.parse("2026-11-17T10:00:00Z");
const MIN = 60_000;
/** Connected, stage seen starting `stageAgo` ms ago, last server update `updateAgo` ms ago. */
const activity = (stageAgo: number, over: Partial<ProgressActivity> = {}): ProgressActivity => ({
  now: NOW,
  stageSince: { at: NOW - stageAgo, exact: true },
  lastUpdateAt: NOW - stageAgo,
  connected: true,
  ...over,
});
const indicator = () => document.querySelector("[data-activity]");

const bar = () => screen.getByRole("progressbar", { name: "Analysis progress" });
const buttons = () => screen.queryAllByRole("button").map((b) => b.textContent);

describe("AnalysisProgress (brief §4.2)", () => {
  it("planning: indeterminate bar, 'Planning flows…', Cancel analysis", () => {
    show({ state: "planning" });
    expect(screen.getByText("Planning flows…")).toBeVisible();
    expect(bar()).not.toHaveAttribute("aria-valuenow");
    expect(bar()).toHaveAttribute("aria-valuetext", "Planning flows");
    expect(buttons()).toEqual(["Cancel analysis"]);
    expect(screen.queryByRole("link", { name: "Open review" })).toBeNull();
  });

  it("running: k of n flows analysed, determinate bar, Cancel analysis", () => {
    show({ state: "running", done: 2, total: 5 });
    expect(screen.getByText("2 of 5 flows analysed")).toBeVisible();
    expect(bar()).toHaveAttribute("aria-valuenow", "2");
    expect(bar()).toHaveAttribute("aria-valuemax", "5");
    const [filled, empty] = Array.from(bar().children) as HTMLElement[];
    expect([filled!.style.flexGrow, empty!.style.flexGrow]).toEqual(["2", "3"]);
    expect(buttons()).toEqual(["Cancel analysis"]);
    expect(screen.queryByRole("link", { name: "Open review" })).toBeNull();
  });

  it("paused: usage limit with the resume time", () => {
    const resumes = new Date();
    resumes.setHours(23, 59, 0, 0);
    show({ state: "paused", resumes_at: resumes.toISOString(), done: 2, total: 5 });
    expect(screen.getByText("Paused until 23:59 — 2 of 5")).toBeVisible();
    expect(screen.getByText("The LLM provider's usage limit was reached.")).toBeVisible();
    expect(bar()).toHaveAttribute("aria-valuenow", "2");
    expect(bar()).toHaveAttribute("aria-valuemax", "5");
    const [filled, empty] = Array.from(bar().children) as HTMLElement[];
    expect([filled!.style.flexGrow, empty!.style.flexGrow]).toEqual(["2", "3"]);
    expect(buttons()).toEqual(["Cancel analysis"]);
  });

  it("paused while planning with a resume time: no count", () => {
    const resumes = new Date();
    resumes.setHours(14, 20, 0, 0);
    show({ state: "paused", resumes_at: resumes.toISOString(), done: 0, total: null });
    expect(screen.getByText("Paused until 14:20")).toBeVisible();
  });

  it("paused while planning: no count, indeterminate bar", () => {
    show({ state: "paused", resumes_at: null, done: 0, total: null });
    expect(screen.getByText("Paused; the resume time is not known")).toBeVisible();
    expect(bar()).not.toHaveAttribute("aria-valuenow");
  });

  it("failed in a flow: names the flow, shows the plain description, Retry and Cancel", async () => {
    const { onRetry, onCancel } = show({
      state: "failed",
      flow: { id: "f3", title: "Classify RPC errors" },
      message: "The LLM answer did not match the expected format.",
    });
    expect(screen.getByText("Analysis of flow “Classify RPC errors” failed")).toBeVisible();
    expect(screen.getByText("The LLM answer did not match the expected format.")).toBeVisible();
    expect(buttons()).toEqual(["Cancel analysis", "Retry"]);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
    await userEvent.click(screen.getByRole("button", { name: "Cancel analysis" }));
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("failed before any flow: plain description and Retry", () => {
    show({
      state: "failed",
      flow: null,
      message: "The repository could not be fetched. Retry the analysis.",
    });
    expect(screen.getByText("The analysis failed before any flow was planned")).toBeVisible();
    expect(screen.getByText("The repository could not be fetched. Retry the analysis.")).toBeVisible();
    expect(screen.getByRole("button", { name: "Retry" })).toBeEnabled();
  });

  it("finished: n of n flows analysed and Open review", () => {
    show({ state: "finished", total: 5 });
    expect(screen.getByText("5 of 5 flows analysed")).toBeVisible();
    expect(bar()).toHaveAttribute("aria-valuenow", "5");
    expect(screen.getByRole("link", { name: "Open review" })).toHaveAttribute("href", "/analyses/a1");
    expect(buttons()).toEqual([]);
  });

  it("disables the actions while a request runs", async () => {
    const { onRetry, onCancel } = show({ state: "failed", flow: null, message: "x" }, true);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await userEvent.click(screen.getByRole("button", { name: "Cancel analysis" }));
    expect(onRetry).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
    for (const name of ["Retry", "Cancel analysis"]) {
      const button = screen.getByRole("button", { name });
      expect(button).toHaveAttribute("aria-disabled", "true");
      expect(button).not.toHaveAttribute("disabled");
      expect(button).toHaveAccessibleDescription(/Not available while the current request runs\./);
    }
  });

  it("every button and link has a tooltip; no banned wording in any state", () => {
    const states: AnalysisState[] = [
      { state: "planning" },
      { state: "running", done: 1, total: 3 },
      { state: "paused", resumes_at: null, done: 1, total: 3 },
      { state: "failed", flow: null, message: "Failed." },
      { state: "finished", total: 3 },
    ];
    for (const state of states) {
      const { unmount } = render(
        <AnalysisProgress
          state={state}
          reviewHref="/r"
          busy={false}
          onCancel={() => {}}
          onRetry={() => {}}
        />,
      );
      const region = screen.getByRole("region", { name: "Analysis progress" });
      for (const control of [
        ...within(region).queryAllByRole("button"),
        ...within(region).queryAllByRole("link"),
      ]) {
        expect(control).toHaveAccessibleDescription(/.+/);
      }
      expectTrustWording(region.textContent ?? "");
      unmount();
    }
  });

  describe("current stage and activity (owner: the screen must not look frozen)", () => {
    it.each([
      [{ state: "planning" } as AnalysisState, "Planning the review: grouping the changes into flows."],
      [
        { state: "running", done: 0, total: 1 } as AnalysisState,
        "Reading the PR context, then analysing flow 1 of 1.",
      ],
      [{ state: "running", done: 2, total: 5 } as AnalysisState, "Analysing flow 3 of 5."],
      [{ state: "running", done: 5, total: 5 } as AnalysisState, "Finishing the review: every flow is done."],
    ])("names the stage in plain words: %j", (state, text) => {
      show(state, false, activity(0));
      const line = screen.getByText(text);
      expect(line).toBeVisible();
      // Announced politely with the count; the count line stays as the brief has it.
      expect(line.closest('[aria-live="polite"]')).not.toBeNull();
      expectTrustWording(document.body.textContent ?? "");
    });

    it.each([
      { state: "paused", resumes_at: null, done: 1, total: 3 },
      { state: "failed", flow: null, message: "Failed." },
      { state: "finished", total: 3 },
    ] as AnalysisState[])("shows no stage or activity when nothing runs: %j", (state) => {
      show(state, false, activity(MIN));
      expect(screen.queryByText(/this stage started/)).toBeNull();
      expect(screen.queryByText(/^(Planning the review|Reading the PR context|Analysing flow)/)).toBeNull();
      expect(indicator()).toBeNull();
    });

    it("shows the real time since the stage started; a lower bound when it began before this page saw it", () => {
      const { rerender } = show({ state: "planning" }, false, activity(20_000));
      expect(screen.getByText("Working · this stage started less than a minute ago")).toBeVisible();
      const props = { state: { state: "planning" } as AnalysisState, reviewHref: "/r", busy: false };
      const later = (a: ProgressActivity) => (
        <AnalysisProgress {...props} onCancel={() => {}} onRetry={() => {}} activity={a} />
      );
      rerender(later(activity(2 * MIN + 30_000, { lastUpdateAt: NOW - MIN })));
      expect(screen.getByText("Working · this stage started 2 min ago")).toBeVisible();
      rerender(later(activity(75 * MIN, { lastUpdateAt: NOW - MIN })));
      expect(screen.getByText("Working · this stage started 1 h 15 min ago")).toBeVisible();
      rerender(
        later({
          ...activity(3 * MIN, { lastUpdateAt: NOW }),
          stageSince: { at: NOW - 3 * MIN, exact: false },
        }),
      );
      expect(screen.getByText("Working · this stage started at least 3 min ago")).toBeVisible();
      // Seen only from a (re)connection a moment ago: no time is claimed.
      rerender(later({ ...activity(10_000), stageSince: { at: NOW - 10_000, exact: false } }));
      expect(indicator()).toHaveTextContent(/^Working$/);
    });

    it("keeps the elapsed time out of the live region (no announcement per tick)", () => {
      show({ state: "running", done: 0, total: 1 }, false, activity(2 * MIN));
      expect(screen.getByText(/this stage started/).closest("[aria-live]")).toBeNull();
    });

    it("shows a running indicator only while the stream is open and recent; never a bare CSS loop", () => {
      const { unmount } = show({ state: "running", done: 0, total: 1 }, false, activity(MIN));
      expect(indicator()).not.toBeNull();
      expect(indicator()!.querySelector(".motion-safe\\:animate-pulse")).not.toBeNull();
      unmount();
      show({ state: "running", done: 0, total: 1 }, false, activity(MIN, { connected: false }));
      expect(indicator()).toBeNull();
      expect(screen.queryByText(/^Working/)).toBeNull();
      expect(screen.getByText("This stage started 1 min ago")).toBeVisible();
    });

    it("under reduced motion the indicator has a static text alternative and nothing animates", () => {
      show({ state: "planning" }, false, activity(MIN));
      // Animation only under motion-safe; the word "Working" is the static alternative.
      for (const animated of document.querySelectorAll("[class*='animate-']")) {
        expect(animated.className).not.toMatch(/(^|\s)animate-/);
      }
      expect(indicator()).toHaveTextContent("Working");
    });

    it("says plainly when the server has been silent unusually long, and stops the indicator", () => {
      show({ state: "planning" }, false, activity(STALE_MS + 2 * MIN));
      const notice = screen.getByText(
        "No update from the server for more than 5 min; the analysis may still be running.",
      );
      expect(notice.closest('[aria-live="polite"]')).not.toBeNull();
      expect(indicator()).toBeNull();
      expect(bar().querySelector("[class*='animate-']")).toBeNull();
      expect(screen.getByText("This stage started 7 min ago")).toBeVisible();
      expectTrustWording(document.body.textContent ?? "");
    });

    it("is not stale just before the threshold", () => {
      show({ state: "running", done: 1, total: 3 }, false, activity(STALE_MS - 1000));
      expect(screen.queryByText(/No update from the server/)).toBeNull();
      expect(indicator()).not.toBeNull();
    });
  });
});
