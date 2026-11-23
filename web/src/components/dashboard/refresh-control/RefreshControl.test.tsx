import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { RefreshStatus, ScheduleStatus, SnapshotMeta } from "../../../api/generated/types";
import { FAILED, RANKING, SCANNING, SUMMARY, NOW } from "../test-fixtures";
import { RefreshControl } from "./RefreshControl";

const snapshot = SUMMARY.snapshot!;
const finished = SUMMARY.latest_refresh!;
const button = () => screen.getByRole("button", { name: /Refresh|Retry/ });
/** The visible scan line of the top bar. */
const info = () => document.querySelector<HTMLElement>("[data-scan-info]")!;

function renderControl(
  latest: RefreshStatus | null,
  extra: { schedule?: ScheduleStatus; now?: Date; snapshot?: SnapshotMeta } = {},
) {
  const onRefresh = vi.fn();
  render(
    <RefreshControl
      latest={latest}
      snapshot={latest === null ? null : (extra.snapshot ?? snapshot)}
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
    expect(info()).toHaveTextContent(
      /Refreshed manually at \d\d:\d\d · 2 h 13 min ago · judged 6 · reused 31/,
    );
    fireEvent.click(button());
    expect(onRefresh).toHaveBeenCalledOnce();
  });

  it("says automatically for a scheduled refresh", () => {
    renderControl({ ...finished, trigger: "scheduled" });
    expect(info()).toHaveTextContent(/Refreshed automatically at/);
    fireEvent.focus(info());
    expect(screen.getByRole("tooltip")).toHaveTextContent("The daily schedule started it.");
  });

  it("says automatically for a refresh started while My work was open (plan §M)", () => {
    renderControl({ ...finished, trigger: "automatic" });
    expect(info()).toHaveTextContent(/Refreshed automatically at/);
    fireEvent.focus(info());
    expect(screen.getByRole("tooltip")).toHaveTextContent(
      "It started by itself while My work was open: every 30 minutes, and on opening after 15.",
    );
  });

  it("says when the listed pull requests were checked after the scan", () => {
    const checked = new Date(NOW.getTime() - 2 * 60_000).toISOString();
    renderControl(finished, { snapshot: { ...snapshot, facts_checked_at: checked } });
    expect(info()).toHaveTextContent(/2 h 13 min ago · PRs checked 2 min ago · judged 6/);
    fireEvent.focus(info());
    expect(screen.getByRole("tooltip")).toHaveTextContent(
      "CI, reviews and merge state of the listed pull requests were read from GitHub at",
    );
    expect(screen.getByRole("tooltip")).toHaveTextContent("every 5 minutes while My work is open.");
  });

  it("says nothing of a check when none followed the scan", () => {
    renderControl(finished);
    expect(info()).not.toHaveTextContent("PRs checked");
  });

  it("blocks Refresh while a scan runs, keeping the button focusable with its reason", () => {
    const onRefresh = renderControl(SCANNING);
    expect(info()).toHaveTextContent(/Scanning… .* · showing/);
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
    expect(info()).toHaveTextContent("LLM ranking 10 / 13");
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
    expect(info()).toHaveTextContent("Not refreshed yet");
  });

  it("announces state changes only: the ticking scan line is not a live region", () => {
    const at = (now: Date, latest: RefreshStatus) => (
      <RefreshControl
        latest={latest}
        snapshot={snapshot}
        schedule={SUMMARY.schedule}
        onRefresh={() => {}}
        now={now}
      />
    );
    const later = new Date(NOW.getTime() + 5 * 60_000);
    const { rerender } = render(at(NOW, finished));
    expect(info()).not.toHaveAttribute("role");
    expect(info()).not.toHaveAttribute("aria-live");
    expect(info().closest("[aria-live]")).toBeNull();
    const live = screen.getByRole("status");
    expect(live).toHaveTextContent(/^Refreshed at \d\d:\d\d\.$/);
    const idle = live.textContent;
    rerender(at(later, finished));
    expect(live.textContent).toBe(idle);

    rerender(at(NOW, SCANNING));
    expect(live).toHaveTextContent("Scanning GitHub and MemCan.");
    rerender(at(later, SCANNING));
    expect(live).toHaveTextContent("Scanning GitHub and MemCan.");
    rerender(at(NOW, RANKING));
    expect(live).toHaveTextContent("The scan finished; the LLM is ranking.");
    rerender(at(NOW, { ...RANKING, ranking: { ...RANKING.ranking!, judged: 12 } }));
    expect(live).toHaveTextContent("The scan finished; the LLM is ranking.");
    rerender(at(NOW, FAILED));
    expect(live).toHaveTextContent("The refresh failed.");
  });

  it("no spinner while the ranking waits for a review analysis or is paused", () => {
    const { unmount } = renderControlRanking("waiting_for_analysis");
    expect(info()).toHaveTextContent(/Refreshed \d\d:\d\d · LLM ranking waits for a review analysis/);
    expect(info().querySelector("[data-spinner]")).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent(
      "The scan finished; the LLM ranking waits for a review analysis.",
    );
    unmount();

    renderControlRanking("paused", "2026-11-18T11:00:00Z");
    expect(info()).toHaveTextContent(/Refreshed \d\d:\d\d · LLM ranking paused until \d\d:\d\d/);
    expect(info().querySelector("[data-spinner]")).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent(
      /^The scan finished; the LLM ranking is paused until \d\d:\d\d\.$/,
    );
  });

  it("a running ranking shows the spinner", () => {
    renderControl(RANKING);
    expect(info().querySelector("[data-spinner]")).not.toBeNull();
  });
});

function renderControlRanking(state: "waiting_for_analysis" | "paused", resumesAt: string | null = null) {
  return render(
    <RefreshControl
      latest={{ ...RANKING, ranking: { ...RANKING.ranking!, state, resumes_at: resumesAt } }}
      snapshot={snapshot}
      schedule={SUMMARY.schedule}
      onRefresh={() => {}}
      now={NOW}
    />,
  );
}
