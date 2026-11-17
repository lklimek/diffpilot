import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import type { AnalysisState } from "../../api/generated/types";
import { AnalysisProgress } from "./AnalysisProgress";

function show(state: AnalysisState, busy = false) {
  const onCancel = vi.fn();
  const onRetry = vi.fn();
  render(
    <AnalysisProgress
      state={state}
      reviewHref="/analyses/a1"
      busy={busy}
      onCancel={onCancel}
      onRetry={onRetry}
    />,
  );
  return { onCancel, onRetry };
}

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

  it("disables the actions while a request runs", () => {
    show({ state: "failed", flow: null, message: "x" }, true);
    expect(screen.getByRole("button", { name: "Retry" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Cancel analysis" })).toBeDisabled();
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
});
