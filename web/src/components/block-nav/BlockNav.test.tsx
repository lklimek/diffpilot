import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { BlockNav } from "./BlockNav";

describe("BlockNav", () => {
  it("names the neighbouring blocks and calls back", async () => {
    const onPrevious = vi.fn();
    const onNext = vi.fn();
    render(<BlockNav previous="PR context" next="Step 1: Trigger" onPrevious={onPrevious} onNext={onNext} />);
    await userEvent.click(screen.getByRole("button", { name: "← Previous: PR context" }));
    await userEvent.click(screen.getByRole("button", { name: "Next: Step 1: Trigger →" }));
    expect(onPrevious).toHaveBeenCalledOnce();
    expect(onNext).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: /Next/ })).toHaveAccessibleDescription(/k|j/);
  });

  it("marks the missing direction aria-disabled at the ends: focusable, its tip says why, no action", async () => {
    const user = userEvent.setup();
    const onPrevious = vi.fn();
    render(<BlockNav next="Flow 1: x" onPrevious={onPrevious} onNext={() => {}} />);
    const previous = screen.getByRole("button", { name: "← Previous" });
    expect(previous).toHaveAttribute("aria-disabled", "true");
    expect(previous).not.toHaveAttribute("disabled");
    expect(previous).toHaveAccessibleDescription("This is the first block; there is none before it.");
    await user.tab();
    expect(previous).toHaveFocus();
    expect(screen.getByRole("tooltip")).toHaveTextContent("This is the first block");
    await user.click(previous);
    await user.keyboard("{Enter}");
    expect(onPrevious).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: /Next/ })).not.toHaveAttribute("aria-disabled");
  });

  it("at the last block Next says there is none after it", () => {
    render(<BlockNav previous="Flow 5: y" onPrevious={() => {}} onNext={() => {}} />);
    const next = screen.getByRole("button", { name: "Next →" });
    expect(next).toHaveAttribute("aria-disabled", "true");
    expect(next).toHaveAccessibleDescription("This is the last block; there is none after it.");
  });

  it("says on Next that it marks a step reviewed only when asked to", () => {
    const { rerender } = render(
      <BlockNav previous="a" next="b" marksReviewed onPrevious={() => {}} onNext={() => {}} />,
    );
    expect(screen.getByRole("button", { name: /Next/ })).toHaveAccessibleDescription(
      "Marks this step reviewed and goes to the next block (j).",
    );
    rerender(<BlockNav previous="a" next="b" onPrevious={() => {}} onNext={() => {}} />);
    expect(screen.getByRole("button", { name: /Next/ })).toHaveAccessibleDescription("Next block (j)");
  });
});
