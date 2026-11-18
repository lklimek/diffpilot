import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { AnalysisState } from "../../api/generated/types";
import { AnalysisStateBanner } from "./AnalysisStateBanner";

const renderBanner = (state: AnalysisState, busy = false) => {
  const onResume = vi.fn();
  const onRetry = vi.fn();
  const view = render(
    <AnalysisStateBanner
      state={state}
      busy={busy}
      onResume={onResume}
      onRetry={onRetry}
      progressHref="/analyses/a1/progress"
    />,
  );
  return { ...view, onResume, onRetry };
};
const banner = () => screen.getByRole("region", { name: "Analysis state" });

describe("AnalysisStateBanner (Tour: paused or failed analysis)", () => {
  it("running and finished: nothing", () => {
    const { container, rerender } = renderBanner({ state: "running", done: 1, total: 5 });
    expect(container).toBeEmptyDOMElement();
    rerender(
      <AnalysisStateBanner
        state={{ state: "finished", total: 5 }}
        busy={false}
        onResume={() => {}}
        onRetry={() => {}}
        progressHref="/p"
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("user pause: who paused it and Resume", async () => {
    const { onResume } = renderBanner({
      state: "paused",
      reason: "user",
      paused_by: "alice",
      paused_at: "2026-11-17T18:04:00Z",
      resumes_at: null,
      done: 1,
      total: 5,
    });
    expect(banner()).toHaveTextContent("Analysis paused by alice");
    expect(banner()).toHaveTextContent("Flows not analysed yet wait until it resumes.");
    await userEvent.click(screen.getByRole("button", { name: "Resume" }));
    expect(onResume).toHaveBeenCalledOnce();
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("usage limit: the reason and when it resumes; no Resume (it resumes by itself)", () => {
    renderBanner({
      state: "paused",
      reason: "usage_limit",
      resumes_at: "2026-11-17T14:20:00Z",
      done: 1,
      total: 5,
    });
    expect(banner()).toHaveTextContent("The LLM provider's usage limit was reached.");
    expect(banner()).toHaveTextContent(/It resumes at [^.]*\d\d:\d\d\./);
    expect(screen.queryByRole("button", { name: "Resume" })).toBeNull();
  });

  it("usage limit without a known time says so", () => {
    renderBanner({ state: "paused", resumes_at: null, done: 1, total: 5 });
    expect(banner()).toHaveTextContent("The resume time is not known.");
  });

  it("failed: the failure sentence, Retry and the details link", async () => {
    const { onRetry } = renderBanner({
      state: "failed",
      flow: { id: "f3", title: "Classify RPC errors" },
      message: "The agent's answer did not have the expected structure.",
    });
    expect(banner()).toHaveTextContent("The analysis of flow “Classify RPC errors” failed.");
    expect(banner()).toHaveTextContent("The agent's answer did not have the expected structure.");
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
    expect(screen.getByRole("link", { name: "Details" })).toHaveAttribute("href", "/analyses/a1/progress");
  });

  it("failed on the configuration: no Retry (an administrator has to act)", () => {
    renderBanner({
      state: "failed",
      reason: "configuration",
      flow: null,
      message: "The runner is not configured.",
    });
    expect(banner()).toHaveTextContent("The analysis failed.");
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("busy: actions stay focusable but do nothing", async () => {
    const { onRetry } = renderBanner({ state: "failed", flow: null, message: "Boom." }, true);
    const retry = screen.getByRole("button", { name: "Retry" });
    expect(retry).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(retry);
    expect(onRetry).not.toHaveBeenCalled();
  });
});
