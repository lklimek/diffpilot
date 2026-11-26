import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { NOW, SCANNING, SUMMARY } from "../test-fixtures";
import { RefreshControl } from "./RefreshControl";

const defaults = {
  latest: SUMMARY.latest_refresh,
  snapshot: SUMMARY.snapshot,
  schedule: SUMMARY.schedule,
  now: NOW,
  onRefresh: vi.fn(),
};

describe("RefreshControl", () => {
  it("summarizes the scan and live coverage without automatic-refresh copy", () => {
    render(
      <RefreshControl
        {...defaults}
        live={{ live_repositories: 2, not_live_repositories: 1, last_event_at: NOW.toISOString() }}
      />,
    );
    const status = screen.getByText(/Live · full scan/);
    expect(status).toHaveAccessibleDescription(
      /2 live repositories · 1 not live.*Last GitHub event.*judged.*reused/i,
    );
    expect(status).not.toHaveTextContent("PRs checked");
  });

  it("keeps nightly scan failures and shows an explicit no-App status", () => {
    render(
      <RefreshControl
        {...defaults}
        live={{ live_repositories: 0, not_live_repositories: 3 }}
        schedule={{
          ...SUMMARY.schedule,
          last_run: { at: NOW.toISOString(), outcome: "failed", reason: "GitHub unavailable" },
        }}
      />,
    );
    expect(screen.getByText(/Not live · scan/)).toBeVisible();
    expect(screen.getByText(/Nightly scan at .* failed/)).toHaveAccessibleDescription("GitHub unavailable");
  });

  it("shows that a cancelled scan kept the last full scan", () => {
    render(<RefreshControl {...defaults} latest={{ ...SCANNING, state: "cancelled" }} />);
    expect(screen.getByText(/^Scan cancelled · Full scan \d\d:\d\d$/)).toHaveAccessibleDescription(
      /Scan started at .* was cancelled.*Last full scan:/,
    );
  });

  it("shows the cancelled scan and the ranking failure next to Retry ranking", async () => {
    const onRetryRanking = vi.fn();
    render(
      <RefreshControl
        {...defaults}
        latest={{ ...SCANNING, state: "cancelled" }}
        rankingNotices={[{ label: "Ranking failed", detail: "The LLM stopped answering." }]}
        onRetryRanking={onRetryRanking}
      />,
    );
    expect(screen.getByText(/^Ranking failed · Scan cancelled · Full scan \d\d:\d\d$/)).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Retry ranking" }));
    expect(onRetryRanking).toHaveBeenCalledOnce();
    expect(defaults.onRefresh).not.toHaveBeenCalled();
  });

  it("refreshes normally and re-judges outdated items using the menu", async () => {
    const onRefresh = vi.fn();
    const onRejudgeOutdated = vi.fn();
    const { rerender } = render(
      <RefreshControl
        {...defaults}
        onRefresh={onRefresh}
        outdated={2}
        onRejudgeOutdated={onRejudgeOutdated}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(onRefresh).toHaveBeenCalledOnce();
    const more = screen.getByRole("button", { name: "More refresh options" });
    await userEvent.click(more);
    const rejudge = screen.getByRole("menuitem", { name: "Refresh and re-judge outdated (2)" });
    expect(rejudge).toHaveAccessibleDescription(
      /uses up to 2 judgements from today’s limit.*caps may leave.*outdated/i,
    );
    await userEvent.click(rejudge);
    expect(onRejudgeOutdated).toHaveBeenCalledOnce();
    expect(more).toHaveFocus();
    rerender(<RefreshControl {...defaults} outdated={0} onRejudgeOutdated={onRejudgeOutdated} />);
    await userEvent.click(more);
    expect(screen.getByRole("menuitem", { name: "Refresh and re-judge outdated (0)" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  it("shows elapsed scan time and offers keyboard cancellation", async () => {
    const onCancel = vi.fn();
    render(<RefreshControl {...defaults} latest={SCANNING} onCancel={onCancel} />);
    expect(screen.getByRole("button", { name: /Refreshing… \d+:\d\d/ })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    screen.getByRole("button", { name: "More refresh options" }).focus();
    await userEvent.keyboard("{ArrowDown}{End}{Enter}");
    expect(onCancel).toHaveBeenCalledOnce();
  });
  it("shows ranking failure in the status and keeps Retry ranking reachable in the Refresh menu", async () => {
    const onRetryRanking = vi.fn();
    render(
      <RefreshControl
        {...defaults}
        rankingNotices={[{ label: "Ranking failed", detail: "The LLM stopped answering." }]}
        onRetryRanking={onRetryRanking}
      />,
    );
    expect(screen.getByText(/Ranking failed · Full scan/)).toHaveAccessibleDescription(
      /The LLM stopped answering/,
    );
    await userEvent.click(screen.getByRole("button", { name: "More refresh options" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Retry ranking" }));
    expect(onRetryRanking).toHaveBeenCalledOnce();
  });
});
