import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
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
  it("mounts its content only while open, with no dangling aria references when closed", async () => {
    const user = userEvent.setup();
    const { container } = render(marker("Remark", "Full remark text"));
    const trigger = screen.getByRole("button", { name: "Remark" });
    expect(container).not.toHaveTextContent("Full remark text");
    expect(container.querySelector("[role=dialog]")).toBeNull();
    expect(trigger).not.toHaveAttribute("aria-controls");
    expect(trigger).not.toHaveAttribute("aria-describedby");
    await user.hover(trigger);
    expect(container).toHaveTextContent("Full remark text");
    expect(trigger).toHaveAccessibleDescription("Full remark text");
    expect(document.getElementById(trigger.getAttribute("aria-controls")!)).not.toBeNull();
    await user.unhover(trigger);
    expect(container).not.toHaveTextContent("Full remark text");
    await user.click(trigger);
    await user.unhover(trigger);
    expect(container).toHaveTextContent("Full remark text");
    await user.click(document.body);
    expect(container).not.toHaveTextContent("Full remark text");
  });

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
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(trigger).toHaveAttribute("aria-haspopup", "dialog");
    await user.click(trigger);
    expect(trigger).toHaveAccessibleDescription(/^Full remark text/);
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

  describe("placement", () => {
    const rect = (top: number, height: number, left = 100, width = 24) =>
      ({ top, bottom: top + height, left, right: left + width, width, height, x: left, y: top }) as DOMRect;
    const contentHeight = (px: number) =>
      vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(function (this: HTMLElement) {
        return this.hasAttribute("data-popover-box") ? px : 0;
      });
    afterEach(() => vi.restoreAllMocks());

    it("opens above its trigger when the measured content does not fit below", async () => {
      const user = userEvent.setup();
      vi.spyOn(window, "innerHeight", "get").mockReturnValue(800);
      contentHeight(300);
      render(marker("Remark", "Full remark text"));
      const trigger = screen.getByRole("button", { name: "Remark" });
      vi.spyOn(trigger, "getBoundingClientRect").mockReturnValue(rect(600, 24));
      await user.click(trigger);
      const dialog = screen.getByRole("dialog");
      expect(dialog.style.bottom).toBe("200px");
      expect(dialog.style.top).toBe("");
    });

    it("keeps content taller than either side within the viewport, on the larger side, with its own scroll", async () => {
      const user = userEvent.setup();
      vi.spyOn(window, "innerHeight", "get").mockReturnValue(800);
      contentHeight(900);
      render(marker("Remark", "Full remark text"));
      const trigger = screen.getByRole("button", { name: "Remark" });
      vi.spyOn(trigger, "getBoundingClientRect").mockReturnValue(rect(300, 24));
      await user.click(trigger);
      const dialog = screen.getByRole("dialog");
      expect(dialog.style.top).toBe("324px");
      expect(parseInt(dialog.style.maxHeight)).toBeLessThanOrEqual(800 - 324);
      const box = dialog.querySelector("[data-popover-box]")!;
      expect(box.className).toMatch(/(^|\s)overflow-y-auto(\s|$)/);
      expect(box.className).toMatch(/\[overflow-wrap:anywhere\]/);
    });

    it("hides a pinned popover while its trigger is scrolled out of its scroll container, and shows it again", async () => {
      const user = userEvent.setup();
      render(
        <div data-testid="column" style={{ overflow: "auto" }}>
          {marker("Remark", "Full remark text")}
        </div>,
      );
      const column = screen.getByTestId("column");
      vi.spyOn(column, "getBoundingClientRect").mockReturnValue(rect(100, 400, 0, 800));
      const trigger = screen.getByRole("button", { name: "Remark" });
      const at = vi.spyOn(trigger, "getBoundingClientRect").mockReturnValue(rect(200, 24));
      await user.click(trigger);
      await user.unhover(trigger);
      expect(screen.getByRole("dialog")).toBeVisible();
      at.mockReturnValue(rect(40, 24));
      fireEvent.scroll(column);
      expect(screen.queryByRole("dialog")).toBeNull();
      at.mockReturnValue(rect(300, 24));
      fireEvent.scroll(column);
      expect(screen.getByRole("dialog")).toBeVisible();
    });
  });

  describe("keyboard", () => {
    it("Escape outside the popover is left to the focused element; the popover stays", async () => {
      const user = userEvent.setup();
      const onComposerKey = vi.fn();
      render(
        <>
          {marker("Remark", "Full remark text")}
          <textarea aria-label="Comment" onKeyDown={(event) => onComposerKey(event.key)} />
        </>,
      );
      await user.tab();
      await user.keyboard("{Enter}");
      act(() => screen.getByRole("textbox", { name: "Comment" }).focus());
      await user.keyboard("{Escape}");
      expect(onComposerKey).toHaveBeenCalledWith("Escape");
      expect(screen.getByRole("dialog")).toBeVisible();
    });

    it("Escape in the popover closes only the popover: handlers around it do not see the key", async () => {
      const user = userEvent.setup();
      const around = vi.fn();
      render(<div onKeyDown={(event) => around(event.key)}>{marker("Remark", "Full remark text")}</div>);
      await user.click(screen.getByRole("button", { name: "Remark" }));
      await user.keyboard("{Escape}");
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(around).not.toHaveBeenCalled();
      await user.click(screen.getByRole("button", { name: "Remark" }));
      await user.click(screen.getByText("Full remark text"));
      await user.keyboard("{Escape}");
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(around).not.toHaveBeenCalled();
    });

    it("costs one tab stop: its tag is focusable only when pinned; aria-expanded follows the shown popover", async () => {
      const user = userEvent.setup();
      const tagged = (name: string) => (
        <Popover label={`${name} details`} tag="llm" content={() => <p>{name} text</p>}>
          <button type="button" aria-label={name}>
            ◆
          </button>
        </Popover>
      );
      render(
        <>
          {tagged("First")}
          {tagged("Second")}
        </>,
      );
      await user.tab();
      const first = screen.getByRole("button", { name: "First" });
      expect(first).toHaveFocus();
      expect(first).toHaveAttribute("aria-expanded", "true");
      await user.tab();
      expect(screen.getByRole("button", { name: "Second" })).toHaveFocus();
      expect(first).toHaveAttribute("aria-expanded", "false");
      await user.keyboard("{Enter}");
      await user.tab();
      expect(within(screen.getByRole("dialog")).getByText("LLM")).toHaveFocus();
    });
  });
});
