import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { GateMenu } from "./GateMenu";

describe("GateMenu (RC15)", () => {
  it("opens the four gates; marks one, clears the owner's mark, keeps the assessment's; Esc closes", async () => {
    const onMark = vi.fn();
    render(
      <GateMenu
        gates={[
          { gate: "G-FUNDS", source: "assessment" },
          { gate: "G-DATA", source: "owner" },
        ]}
        itemLabel="C14"
        busy={false}
        error={null}
        onMark={onMark}
      />,
    );
    const user = userEvent.setup();
    const button = screen.getByRole("button", { name: /Mark gate \(1\)/ });
    expect(button).toHaveAttribute("aria-expanded", "false");
    await user.click(button);
    expect(button).toHaveAttribute("aria-expanded", "true");
    const group = screen.getByRole("group", { name: "Gates of C14" });
    expect(group).toBeVisible();
    const funds = screen.getByRole("checkbox", { name: /G-FUNDS/ });
    expect(funds).toBeChecked();
    expect(funds).toHaveAttribute("aria-disabled", "true");
    await user.click(funds);
    expect(onMark).not.toHaveBeenCalled();
    await user.click(screen.getByRole("checkbox", { name: /G-SECRET/ }));
    expect(onMark).toHaveBeenLastCalledWith("G-SECRET", true);
    await user.click(screen.getByRole("checkbox", { name: /G-DATA/ }));
    expect(onMark).toHaveBeenLastCalledWith("G-DATA", false);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("group", { name: "Gates of C14" })).toBeNull();
    expect(button).toHaveFocus();
  });

  it("shows a failed mark in plain words", async () => {
    render(
      <GateMenu gates={[]} itemLabel="C2" busy={false} error="C2 changed. Read it again." onMark={vi.fn()} />,
    );
    await userEvent.setup().click(screen.getByRole("button", { name: /Mark gate/ }));
    expect(screen.getByRole("alert")).toHaveTextContent("C2 changed. Read it again.");
  });
});
