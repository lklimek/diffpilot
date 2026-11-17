import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { Tooltip } from "./Tooltip";

describe("Tooltip", () => {
  it("describes its trigger and shows on focus and hover", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip tip="Explains the button">
        <button type="button">Go</button>
      </Tooltip>,
    );
    const button = screen.getByRole("button", { name: "Go" });
    expect(button).toHaveAccessibleDescription("Explains the button");
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    await user.tab();
    expect(screen.getByRole("tooltip")).toHaveTextContent("Explains the button");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    await user.hover(button);
    expect(screen.getByRole("tooltip")).toBeVisible();
    await user.unhover(button);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("opens above a trigger near the bottom of the viewport so the tip stays visible", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip tip="Near the bottom">
        <button type="button">Low</button>
      </Tooltip>,
    );
    const button = screen.getByRole("button", { name: "Low" });
    const wrapper = button.parentElement!;
    const top = window.innerHeight - 30;
    wrapper.getBoundingClientRect = () => new DOMRect(10, top, 40, 20);
    await user.hover(button);
    const tip = screen.getByRole("tooltip");
    expect(tip.style.bottom).toBe(`${window.innerHeight - top + 4}px`);
    expect(tip.style.top).toBe("");
  });

  it("opens below a trigger with room under it", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip tip="Room below">
        <button type="button">High</button>
      </Tooltip>,
    );
    const button = screen.getByRole("button", { name: "High" });
    button.parentElement!.getBoundingClientRect = () => new DOMRect(10, 100, 40, 20);
    await user.hover(button);
    expect(screen.getByRole("tooltip").style.top).toBe("124px");
  });
});
