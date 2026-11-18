import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { RefreshStatus, ScheduleStatus } from "../../../api/generated/types";
import { FAILED, RANKING, SCANNING, SUMMARY, NOW } from "../test-fixtures";
import { RefreshControl } from "./RefreshControl";

const snapshot = SUMMARY.snapshot!;
const finished = SUMMARY.latest_refresh!;
const button = () => screen.getByRole("button", { name: /Refresh|Retry/ });

function renderControl(latest: RefreshStatus | null, extra: { schedule?: ScheduleStatus; now?: Date } = {}) {
  const onRefresh = vi.fn();
  render(
    <RefreshControl
      latest={latest}
      snapshot={latest === null ? null : snapshot}
      schedule={extra.schedule ?? SUMMARY.schedule}
      onRefresh={onRefresh}
      now={extra.now ?? NOW}
    />,
  );
  return onRefresh;
}

describe("RefreshControl", () => {
  it("says how and when the snapshot was refreshed, with judged and reused counts", () => {
    const onRefresh = renderControl(finished);
    expect(screen.getByRole("status")).toHaveTextContent(
      /Refreshed manually at \d\d:\d\d · 2 h 13 min ago · judged 6 · reused 31/,
    );
    fireEvent.click(button());
    expect(onRefresh).toHaveBeenCalledOnce();
  });

  it("says automatically for a scheduled refresh", () => {
    renderControl({ ...finished, trigger: "scheduled" });
    expect(screen.getByRole("status")).toHaveTextContent(/Refreshed automatically at/);
  });

  it("blocks Refresh while a scan runs, keeping the button focusable with its reason", () => {
    const onRefresh = renderControl(SCANNING);
    expect(screen.getByRole("status")).toHaveTextContent(/Scanning… .* · showing/);
    const refresh = button();
    expect(refresh).toHaveTextContent("Refreshing…");
    expect(refresh).toHaveAttribute("aria-disabled", "true");
    expect(refresh).not.toBeDisabled();
    fireEvent.click(refresh);
    expect(onRefresh).not.toHaveBeenCalled();
    fireEvent.focus(refresh);
    expect(screen.getByRole("tooltip")).toHaveTextContent("A scan is running");
  });

  it("allows Refresh while the LLM ranks: a new scan supersedes the ranking", () => {
    const onRefresh = renderControl(RANKING);
    expect(screen.getByRole("status")).toHaveTextContent("LLM ranking 10 / 14");
    fireEvent.click(button());
    expect(onRefresh).toHaveBeenCalledOnce();
    fireEvent.focus(button());
    expect(screen.getByRole("tooltip")).toHaveTextContent("The ranking running now stops");
  });

  it("waits for GitHub's rate-limit reset after a failure, then offers Refresh again", () => {
    const onRefresh = renderControl(FAILED);
    expect(button()).toHaveTextContent(/Retry at \d\d:\d\d/);
    expect(button()).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(button());
    expect(onRefresh).not.toHaveBeenCalled();
  });

  it("offers Refresh after the reset time", () => {
    const onRefresh = renderControl(FAILED, { now: new Date("2026-11-18T10:42:00Z") });
    expect(button()).toHaveTextContent("Refresh");
    fireEvent.click(button());
    expect(onRefresh).toHaveBeenCalledOnce();
  });

  it("shows a failed automatic refresh until a later scan succeeds", () => {
    const failed: ScheduleStatus = {
      ...SUMMARY.schedule,
      last_run: { at: "2026-11-18T09:00:00Z", outcome: "needs_sign_in", reason: null },
    };
    renderControl(finished, { schedule: failed });
    expect(screen.getByText(/Automatic refresh at \d\d:\d\d needs you to sign in again/)).toBeInTheDocument();
  });

  it("hides a scheduled failure older than the shown snapshot", () => {
    const old: ScheduleStatus = {
      ...SUMMARY.schedule,
      last_run: { at: "2026-11-18T05:00:00Z", outcome: "failed", reason: "GitHub unavailable" },
    };
    renderControl(finished, { schedule: old });
    expect(document.querySelector("[data-schedule-problem]")).toBeNull();
  });

  it("before the first refresh says so", () => {
    renderControl(null);
    expect(screen.getByRole("status")).toHaveTextContent("Not refreshed yet");
  });
});
