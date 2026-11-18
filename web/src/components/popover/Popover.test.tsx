import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Tooltip } from "../tooltip/Tooltip";
import { Popover } from "./Popover";

const marker = (name: string, text: string) => (
  <Popover
    label={`${name} details`}
    content={(pinned) => (
      <>
        <p>{text}</p>
        {pinned && <a href="#x">Only when pinned</a>}
      </>
    )}
  >
    <button type="button" aria-label={name}>
      ◆
    </button>
  </Popover>
);

describe("Popover", () => {
  it("shows its content on hover and on keyboard focus, and hides it after", async () => {
    const user = userEvent.setup();
    render(marker("Remark", "Full remark text"));
    const trigger = screen.getByRole("button", { name: "Remark" });
    expect(screen.queryByRole("dialog")).toBeNull();
    await user.hover(trigger);
    expect(screen.getByRole("dialog", { name: "Remark details" })).toHaveTextContent("Full remark text");
    expect(screen.queryByText("Only when pinned")).toBeNull();
    await user.unhover(trigger);
    expect(screen.queryByRole("dialog")).toBeNull();
    await user.tab();
    expect(trigger).toHaveFocus();
    expect(screen.getByRole("dialog")).toBeVisible();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("describes its trigger by its content and announces the pinned state", async () => {
    const user = userEvent.setup();
    render(marker("Remark", "Full remark text"));
    const trigger = screen.getByRole("button", { name: "Remark" });
    expect(trigger).toHaveAccessibleDescription("Full remark text");
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(trigger).toHaveAttribute("aria-haspopup", "dialog");
    await user.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(trigger).toHaveAttribute("aria-controls", screen.getByRole("dialog").id);
  });

  it("pins on click: stays after the pointer leaves, selectable, with the pinned-only content", async () => {
    const user = userEvent.setup();
    render(marker("Remark", "Full remark text"));
    const trigger = screen.getByRole("button", { name: "Remark" });
    await user.click(trigger);
    await user.unhover(trigger);
    const dialog = screen.getByRole("dialog");
    expect(dialog).toBeVisible();
    expect(dialog.className).toMatch(/(^|\s)select-text(\s|$)/);
    expect(screen.getByRole("link", { name: "Only when pinned" })).toBeVisible();
  });

  it("pins by Enter and Space; the close button returns the focus to the trigger", async () => {
    const user = userEvent.setup();
    render(marker("Remark", "Full remark text"));
    await user.tab();
    await user.keyboard("{Enter}");
    const close = screen.getByRole("button", { name: "Close" });
    await user.click(close);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("button", { name: "Remark" })).toHaveFocus();
    await user.keyboard(" ");
    expect(screen.getByRole("button", { name: "Close" })).toBeVisible();
  });

  it("closes a pinned popover by Escape before any page shortcut sees it, returning the focus", async () => {
    const user = userEvent.setup();
    const shortcut = vi.fn();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) shortcut(event.key);
    };
    document.addEventListener("keydown", onKey);
    render(marker("Remark", "Full remark text"));
    await user.click(screen.getByRole("button", { name: "Remark" }));
    await user.tab();
    expect(screen.getByRole("button", { name: "Close" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("button", { name: "Remark" })).toHaveFocus();
    expect(shortcut).not.toHaveBeenCalled();
    document.removeEventListener("keydown", onKey);
  });

  it("closes a pinned popover on a click outside, and keeps it on a click inside", async () => {
    const user = userEvent.setup();
    render(
      <>
        {marker("Remark", "Full remark text")}
        <p>Elsewhere</p>
      </>,
    );
    await user.click(screen.getByRole("button", { name: "Remark" }));
    await user.click(screen.getByText("Full remark text"));
    expect(screen.getByRole("dialog")).toBeVisible();
    await user.click(screen.getByText("Elsewhere"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("keeps one pinned popover: pinning another closes the first", async () => {
    const user = userEvent.setup();
    render(
      <>
        {marker("First", "First text")}
        {marker("Second", "Second text")}
      </>,
    );
    await user.click(screen.getByRole("button", { name: "First" }));
    await user.click(screen.getByRole("button", { name: "Second" }));
    await user.unhover(screen.getByRole("button", { name: "Second" }));
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(screen.getByRole("dialog")).toHaveTextContent("Second text");
    expect(screen.getByRole("button", { name: "First" })).toHaveAttribute("aria-expanded", "false");
  });

  it("does not let a click inside reach handlers around it (a highlight selecting its remark)", async () => {
    const user = userEvent.setup();
    const outer = vi.fn();
    render(<div onClick={outer}>{marker("Remark", "Full remark text")}</div>);
    await user.click(screen.getByRole("button", { name: "Remark" }));
    await user.click(screen.getByText("Full remark text"));
    expect(outer).not.toHaveBeenCalled();
  });

  it("takes precedence over the tooltip of the row around it while the pointer is on it", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip tip="Was → is">
        <div tabIndex={0} data-testid="row">
          {marker("Remark", "Full remark text")}
        </div>
      </Tooltip>,
    );
    await user.hover(screen.getByTestId("row"));
    expect(screen.getByText("Was → is")).toBeVisible();
    await user.hover(screen.getByRole("button", { name: "Remark" }));
    expect(screen.getByText("Was → is")).not.toBeVisible();
    await user.click(screen.getByRole("button", { name: "Remark" }));
    await user.hover(screen.getByTestId("row"));
    expect(screen.getByRole("dialog")).toBeVisible();
  });
});
