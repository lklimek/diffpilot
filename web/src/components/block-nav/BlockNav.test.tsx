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

  it("disables the missing direction at the ends of the tour", () => {
    render(<BlockNav next="Flow 1: x" onPrevious={() => {}} onNext={() => {}} />);
    expect(screen.getByRole("button", { name: "← Previous" })).toBeDisabled();
    expect(screen.getByRole("button", { name: /Next/ })).toBeEnabled();
  });
});
