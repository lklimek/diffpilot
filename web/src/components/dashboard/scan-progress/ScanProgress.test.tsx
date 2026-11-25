import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { NOW, SCANNING, SUMMARY } from "../test-fixtures";
import { ScanProgress } from "./ScanProgress";

describe("ScanProgress", () => {
  it("shows the running step and observed counts, with details closed by default", async () => {
    const onCancel = vi.fn();
    render(
      <ScanProgress
        status={SCANNING}
        snapshotAt={SUMMARY.snapshot!.scanned_at}
        onCancel={onCancel}
        now={NOW}
      />,
    );
    expect(screen.getByText(/PR details 23 \/ 41/)).toBeVisible();
    expect(screen.getByText(/started \d\d:\d\d/)).toBeVisible();
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.queryByRole("list")).toBeNull();
    const details = screen.getByRole("button", { name: "Details" });
    await userEvent.click(details);
    expect(details).toHaveAttribute("aria-expanded", "true");
    expect(screen.getAllByRole("listitem")).toHaveLength(7);
    await userEvent.keyboard("{Escape}");
    expect(details).toHaveFocus();
    expect(screen.queryByRole("list")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Cancel scan" }));
    expect(onCancel).toHaveBeenCalledOnce();
  });
});
