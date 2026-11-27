import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { RowMenu } from "../work-item/RowMenu";
import { itemById } from "../test-fixtures";
import { snoozeDays } from "./SnoozePanel";

const item = itemById("PLAT-1203");

const open = async () => {
  await userEvent.click(screen.getByRole("button", { name: "More actions for PLAT-1203" }));
  await userEvent.click(screen.getByRole("menuitem", { name: "Snooze…" }));
  return screen.getByRole("group", { name: "Snooze PLAT-1203" });
};

describe("RowMenu snooze", () => {
  it("is closed until its button opens it; the button says so", async () => {
    render(<RowMenu item={item} onSnooze={vi.fn()} />);
    const button = screen.getByRole("button", { name: "More actions for PLAT-1203" });
    expect(button).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("group")).toBeNull();
    const menu = await open();
    expect(button).toHaveAttribute("aria-expanded", "false");
    expect(within(menu).getByRole("button", { name: "1 day" })).toHaveFocus();
    expect(
      within(menu)
        .getAllByRole("button")
        .map((b) => b.textContent),
    ).toEqual(["1 day", "3 days", "7 days", "30 days", "Snooze"]);
    expect(menu).toHaveTextContent(/earlier when you get a new review request or mention/);
  });

  it("a preset snoozes for its days, closes and returns the focus to the button", async () => {
    const onSnooze = vi.fn();
    render(<RowMenu item={item} onSnooze={onSnooze} />);
    const menu = await open();
    await userEvent.click(within(menu).getByRole("button", { name: "7 days" }));
    expect(onSnooze).toHaveBeenCalledWith(7);
    expect(screen.queryByRole("group")).toBeNull();
    expect(screen.getByRole("button", { name: "More actions for PLAT-1203" })).toHaveFocus();
  });

  it("a typed number of days from 1 to 365 snoozes; anything else says why and snoozes nothing", async () => {
    const onSnooze = vi.fn();
    render(<RowMenu item={item} onSnooze={onSnooze} />);
    let menu = await open();
    const days = within(menu).getByLabelText("Days (1–365)");
    await userEvent.type(days, "400{Enter}");
    expect(onSnooze).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("Type a whole number of days from 1 to 365.");
    expect(days).toHaveAttribute("aria-invalid", "true");
    expect(days).toHaveAccessibleDescription("Type a whole number of days from 1 to 365.");
    await userEvent.clear(days);
    expect(screen.queryByRole("alert")).toBeNull();
    await userEvent.type(days, "12");
    await userEvent.click(within(menu).getByRole("button", { name: "Snooze" }));
    expect(onSnooze).toHaveBeenCalledWith(12);
    menu = await open();
    expect(within(menu).getByLabelText("Days (1–365)")).toHaveValue(null);
  });

  it("Escape and a click outside close the Snooze panel", async () => {
    const onSnooze = vi.fn();
    render(
      <>
        <RowMenu item={item} onSnooze={onSnooze} />
        <p>outside</p>
      </>,
    );
    let menu = await open();
    within(menu).getByRole("button", { name: "1 day" }).focus();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("group")).toBeNull();
    expect(screen.getByRole("button", { name: "More actions for PLAT-1203" })).toHaveFocus();
    menu = await open();
    const day = within(menu).getByRole("button", { name: "1 day" });
    expect(day).toHaveFocus();
    expect(onSnooze).not.toHaveBeenCalled();
    await userEvent.click(screen.getByText("outside"));
    expect(screen.queryByRole("group")).toBeNull();
  });

  it("reads typed days as whole numbers from 1 to 365", () => {
    expect(["1", " 30 ", "365"].map(snoozeDays)).toEqual([1, 30, 365]);
    expect(["0", "366", "-1", "2.5", "1e2", "", "x"].map(snoozeDays)).toEqual([
      null,
      null,
      null,
      null,
      null,
      null,
      null,
    ]);
  });
});
