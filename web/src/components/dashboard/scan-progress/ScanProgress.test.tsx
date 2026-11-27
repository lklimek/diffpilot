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
    const progress = screen.getByRole("progressbar", { name: "Scan progress" });
    expect(progress).toHaveAttribute("aria-valuenow", "23");
    expect(progress).toHaveAttribute("aria-valuemax", "41");
    expect(progress).toHaveAttribute("aria-valuetext", "PR details: 23 of 41");
    expect(progress).not.toHaveTextContent("%");
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
  it("uses indeterminate progress when the step has no counts", () => {
    const steps = SCANNING.steps.map((step) => ({ ...step, total: null, done: null }));
    render(<ScanProgress status={{ ...SCANNING, steps }} snapshotAt={null} onCancel={vi.fn()} now={NOW} />);
    const progress = screen.getByRole("progressbar", { name: "Scan progress" });
    expect(progress).not.toHaveAttribute("aria-valuenow");
    expect(progress).not.toHaveAttribute("aria-valuemax");
  });
});
