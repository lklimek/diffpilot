import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Tour } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { ASK_MIN_PX, DOCK_KEY_STEP_PX, Inspector } from "./Inspector";

const tour = seedFor("default", "tour") as Tour;
const sections = (askOpen: boolean) => ({
  isOpen: (key: string) => (key === "ask" ? askOpen : true),
  toggle: () => {},
  reveal: () => {},
});

function renderDock({
  height = null,
  askOpen = true,
  onDockHeight = vi.fn(),
}: { height?: number | null; askOpen?: boolean; onDockHeight?: ((px: number | null) => void) | null } = {}) {
  render(
    <Inspector
      tour={tour}
      block={{ block: "step", step_id: "f1.5" }}
      sections={sections(askOpen)}
      dock={<p>dock</p>}
      dockHeight={height}
      onDockHeight={onDockHeight ?? undefined}
    />,
  );
  return { onDockHeight, dock: screen.getByRole("group", { name: "Ask, feedback and comment" }) };
}

const handle = () => screen.getByRole("separator", { name: "Resize the Ask area" });

beforeEach(() => {
  window.localStorage.clear();
  // jsdom has no layout: the Inspector is 800 px tall, the dock 300 px.
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    const height = this.matches("[data-inspector]") ? 800 : this.matches("[data-dock]") ? 300 : 0;
    return {
      height,
      width: 300,
      top: 0,
      left: 0,
      bottom: height,
      right: 300,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    };
  });
});
afterEach(() => vi.restoreAllMocks());

describe("Inspector: the Ask area resizes from its top edge", () => {
  it("is a focusable horizontal separator with a tooltip; a stored height sizes the dock", () => {
    const { dock } = renderDock({ height: 420 });
    expect(handle()).toHaveAttribute("aria-orientation", "horizontal");
    expect(handle()).toHaveAttribute("tabindex", "0");
    expect(handle()).toHaveAttribute("aria-valuenow", "420");
    expect(handle()).toHaveAttribute("aria-valuemin", String(ASK_MIN_PX));
    expect(handle()).toHaveAccessibleDescription(/Drag to make Ask taller or shorter/);
    expect(handle().className).toMatch(/(^|\s)h-6(\s|$)/);
    expect(dock.style.height).toBe("420px");
    // The separator sits above the dock: Ask grows upwards, the sections above shrink.
    expect(handle().compareDocumentPosition(dock) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("arrow keys resize in steps, Home and End go to the limits, Enter and double-click reset", async () => {
    const { onDockHeight } = renderDock({ height: 420 });
    handle().focus();
    await userEvent.keyboard("{ArrowUp}");
    expect(onDockHeight).toHaveBeenLastCalledWith(420 + DOCK_KEY_STEP_PX);
    await userEvent.keyboard("{ArrowDown}");
    expect(onDockHeight).toHaveBeenLastCalledWith(420 - DOCK_KEY_STEP_PX);
    await userEvent.keyboard("{Home}");
    expect(onDockHeight).toHaveBeenLastCalledWith(ASK_MIN_PX);
    await userEvent.keyboard("{End}");
    // The Inspector keeps room for its heading above.
    expect(onDockHeight).toHaveBeenLastCalledWith(800 - 120);
    await userEvent.keyboard("{Enter}");
    expect(onDockHeight).toHaveBeenLastCalledWith(null);
    await userEvent.dblClick(handle());
    expect(onDockHeight).toHaveBeenLastCalledWith(null);
  });

  it("starts from the drawn height when none is stored; dragging up makes Ask taller", () => {
    const { onDockHeight } = renderDock();
    expect(handle()).toHaveAttribute("aria-valuenow", "300");
    fireEvent.pointerDown(handle(), { clientY: 500, pointerId: 1 });
    fireEvent.pointerMove(handle(), { clientY: 400, pointerId: 1 });
    expect(onDockHeight).toHaveBeenLastCalledWith(400);
    fireEvent.pointerUp(handle(), { pointerId: 1 });
    fireEvent.pointerMove(handle(), { clientY: 100, pointerId: 1 });
    expect(onDockHeight).toHaveBeenCalledTimes(1);
  });

  it("with Ask collapsed the dock keeps its natural height and has no handle", () => {
    const { dock } = renderDock({ height: 420, askOpen: false });
    expect(screen.queryByRole("separator", { name: "Resize the Ask area" })).toBeNull();
    expect(dock.style.height).toBe("");
  });

  it("offers no handle without a height callback", () => {
    renderDock({ onDockHeight: null });
    expect(screen.queryByRole("separator", { name: "Resize the Ask area" })).toBeNull();
  });
});
