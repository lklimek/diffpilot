import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FAILED, NOW, RANKING, SCANNING, SUMMARY } from "../test-fixtures";
import { RankingBanner } from "../ranking-banner/RankingBanner";
import { RefreshError } from "../refresh-error/RefreshError";
import { ScanProgress } from "./ScanProgress";

const snapshotAt = SUMMARY.snapshot!.scanned_at;

describe("ScanProgress", () => {
  it("shows the progress bar, the step states as text and the running step's count", () => {
    render(<ScanProgress status={SCANNING} snapshotAt={snapshotAt} onCancel={() => {}} now={NOW} />);
    expect(screen.getByRole("progressbar", { name: "Scan progress" })).toHaveAttribute("aria-valuenow", "45");
    const running = document.querySelector('[data-step="pr_details"]')!;
    expect(running).toHaveTextContent("(running)");
    expect(running).toHaveTextContent("23 / 41 · acme/platform#1199");
    expect(document.querySelector('[data-step="search_prs"]')).toHaveTextContent("(done)");
  });

  it("cancels on request and blocks a second cancel while it runs", () => {
    const onCancel = vi.fn();
    const { rerender } = render(
      <ScanProgress status={SCANNING} snapshotAt={snapshotAt} onCancel={onCancel} now={NOW} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Cancel scan" }));
    expect(onCancel).toHaveBeenCalledOnce();
    rerender(
      <ScanProgress status={SCANNING} snapshotAt={snapshotAt} onCancel={onCancel} cancelling now={NOW} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Cancel scan" }));
    expect(onCancel).toHaveBeenCalledOnce();
  });
});

describe("RankingBanner", () => {
  it("counts judged items while the LLM ranks", () => {
    render(<RankingBanner ranking={RANKING.ranking!} scannedAt={snapshotAt} onRetry={() => {}} now={NOW} />);
    expect(screen.getByRole("status")).toHaveTextContent("Ranking with the LLM: 10 of 14 items judged.");
  });

  it("waits for a review analysis that holds the LLM", () => {
    render(
      <RankingBanner
        ranking={{ ...RANKING.ranking!, state: "waiting_for_analysis" }}
        scannedAt={snapshotAt}
        onRetry={() => {}}
        now={NOW}
      />,
    );
    expect(screen.getByRole("status")).toHaveTextContent("Waiting for the review analysis to finish.");
  });

  it("offers Retry ranking after a failure, with the plain reason", () => {
    const onRetry = vi.fn();
    render(
      <RankingBanner
        ranking={{ ...RANKING.ranking!, state: "failed", message: "the LLM answered outside the schema" }}
        scannedAt={snapshotAt}
        onRetry={onRetry}
        now={NOW}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "LLM ranking failed: the LLM answered outside the schema. Provisional scores stay approximate.",
    );
    fireEvent.click(screen.getByRole("button", { name: "Retry ranking" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it("a failure message ending with a period reads as one sentence", () => {
    render(
      <RankingBanner
        ranking={{ ...RANKING.ranking!, state: "failed", message: "The LLM stopped answering. " }}
        scannedAt={snapshotAt}
        onRetry={() => {}}
        now={NOW}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "LLM ranking failed: The LLM stopped answering. Provisional scores stay approximate.",
    );
    expect(screen.getByRole("alert").textContent).not.toMatch(/\.\./);
  });

  it("shows nothing once the ranking finished", () => {
    const { container } = render(
      <RankingBanner
        ranking={{ ...RANKING.ranking!, state: "finished" }}
        scannedAt={snapshotAt}
        onRetry={() => {}}
        now={NOW}
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });
});

describe("RefreshError", () => {
  it("says nothing was replaced, when GitHub allows requests again, and keeps Retry blocked until then", () => {
    const onRetry = vi.fn();
    render(<RefreshError status={FAILED} snapshotAt={snapshotAt} onRetry={onRetry} now={NOW} />);
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("GitHub rate limit reached");
    expect(alert).toHaveTextContent(/you are looking at the \d\d:\d\d snapshot/);
    expect(alert).toHaveTextContent("(in 14 min)");
    const retry = screen.getByRole("button", { name: /Retry at/ });
    fireEvent.click(retry);
    expect(onRetry).not.toHaveBeenCalled();
    expect(document.querySelector('[data-step="pr_details"]')).toHaveTextContent("(failed)");
    expect(screen.getByText("Scan log")).toBeInTheDocument();
  });

  it("retries once the reset passed", () => {
    const onRetry = vi.fn();
    render(
      <RefreshError
        status={FAILED}
        snapshotAt={snapshotAt}
        onRetry={onRetry}
        now={new Date("2026-11-18T10:41:00Z")}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });
});
