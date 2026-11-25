import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Tooltip } from "./Tooltip";

describe("Tooltip", () => {
  it.each(["below", "right"] as const)("keeps a 16 px gutter on narrow screens (%s)", async (placement) => {
    vi.stubGlobal("innerWidth", 320);
    try {
      const user = userEvent.setup();
      render(
        <Tooltip tip="Long statement" placement={placement} scrollable>
          <button type="button">Narrow</button>
        </Tooltip>,
      );
      const button = screen.getByRole("button", { name: "Narrow" });
      button.parentElement!.getBoundingClientRect = () => new DOMRect(280, 100, 20, 20);
      await user.hover(button);
      expect(screen.getByRole("tooltip").style.left).toBe("16px");
    } finally {
      vi.unstubAllGlobals();
    }
  });

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

  it("does not open on the focus a click gives, and closes on the click (WCAG 1.4.13)", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip tip="Explains the row">
        <button type="button">Row</button>
      </Tooltip>,
    );
    const button = screen.getByRole("button", { name: "Row" });
    await user.hover(button);
    expect(screen.getByRole("tooltip")).toBeVisible();
    await user.click(button);
    // Clicked and still focused under the pointer: the tip stays closed and cannot cover the next row.
    expect(button).toHaveFocus();
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    // It opens again only after the pointer left and came back.
    await user.unhover(button);
    await user.hover(button);
    expect(screen.getByRole("tooltip")).toBeVisible();
  });

  it("opens on keyboard focus and stays while the pointer moves onto it (hoverable)", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip tip="Explains the key">
        <button type="button">Key</button>
      </Tooltip>,
    );
    await user.tab();
    const tip = screen.getByRole("tooltip");
    expect(tip).toBeVisible();
    await user.hover(tip);
    expect(screen.getByRole("tooltip")).toBeVisible();
  });

  it("gives way to a nested trigger's own tip while the pointer is on it, then shows again", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip tip="Was → is of the row">
        <div tabIndex={0} data-testid="row">
          code
          <Tooltip tip="Icon tip">
            <button type="button">Icon</button>
          </Tooltip>
        </div>
      </Tooltip>,
    );
    await user.hover(screen.getByTestId("row"));
    expect(screen.getByText("Was → is of the row")).toBeVisible();
    await user.hover(screen.getByRole("button", { name: "Icon" }));
    expect(screen.getByText("Icon tip")).toBeVisible();
    expect(screen.getByText("Was → is of the row")).not.toBeVisible();
    await user.hover(screen.getByTestId("row"));
    expect(screen.getByText("Icon tip")).not.toBeVisible();
    expect(screen.getByText("Was → is of the row")).toBeVisible();
  });

  it("opens beside its trigger with placement right, off the rows below", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip tip="Beside" placement="right">
        <button type="button">Side</button>
      </Tooltip>,
    );
    const button = screen.getByRole("button", { name: "Side" });
    button.parentElement!.getBoundingClientRect = () => new DOMRect(10, 100, 200, 20);
    await user.hover(button);
    const tip = screen.getByRole("tooltip");
    expect(tip.style.left).toBe("218px");
    expect(tip.style.top).toBe("100px");
  });
});

it("keeps long statements in the viewport and scrolls them from the focused trigger", async () => {
  const user = userEvent.setup();
  render(
    <Tooltip scrollable tip="Long statement">
      <span tabIndex={0}>Source statement</span>
    </Tooltip>,
  );
  const trigger = screen.getByText("Source statement");
  trigger.parentElement!.getBoundingClientRect = () => new DOMRect(10, window.innerHeight - 100, 200, 20);
  await user.tab();
  const tip = screen.getByRole("tooltip");
  expect(tip.style.bottom).toBe("104px");
  expect(Number.parseFloat(tip.style.maxHeight)).toBeLessThan(window.innerHeight);
  const scrollBy = vi.fn();
  tip.scrollBy = scrollBy;
  Object.defineProperty(tip, "scrollHeight", { value: 1000 });
  Object.defineProperty(tip, "clientHeight", { value: 300 });
  await user.keyboard("{PageDown}{ArrowUp}");
  expect(scrollBy).toHaveBeenNthCalledWith(1, { top: 300 });
  expect(scrollBy).toHaveBeenNthCalledWith(2, { top: -40 });
  expect(trigger).toHaveFocus();
});
