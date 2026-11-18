import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import type { Annotation } from "../../api/generated/types";
import { FlagCard } from "./FlagCard";

const observers: { cb: ResizeObserverCallback; el: Element; last: number }[] = [];
class FakeRO {
  constructor(private cb: ResizeObserverCallback) {}
  observe(el: Element) {
    observers.push({ cb: this.cb, el, last: box(el) });
  }
  disconnect() {}
  unobserve() {}
}
const box = (el: Element) => (el.className.includes("max-h-[5lh]") ? 80 : 120);
function flushObservers() {
  for (const o of observers) {
    const now = box(o.el);
    if (now !== o.last) {
      o.last = now;
      o.cb([], o as unknown as ResizeObserver);
    }
  }
}

afterEach(() => {
  vi.restoreAllMocks();
  observers.length = 0;
});

const annotation: Annotation = {
  id: "f1.5.a3",
  type: "flag",
  basis: "inference",
  range: { start_line: 99, end_line: 99 },
  was: "",
  is: "",
  text: "Potential double send:",
  evidence: [],
  flag: { level: "red", scenario: ["a", "b", "c", "d", "e", "f"], not_checked: "Not run." },
};

it("keeps keyboard focus on the toggle after Show less", async () => {
  vi.stubGlobal("ResizeObserver", FakeRO);
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(120);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockImplementation(function (this: HTMLElement) {
    return box(this);
  });
  render(
    <ul>
      <FlagCard annotation={annotation} />
    </ul>,
  );
  screen.getByRole("button", { name: "Show all" }).focus();
  await userEvent.keyboard("{Enter}");
  act(flushObservers);
  expect(screen.getByRole("button", { name: "Show less" })).toHaveFocus();
  await userEvent.keyboard("{Enter}");
  const between = screen.queryByRole("button", { name: "Show all" });
  act(flushObservers);
  const after = screen.getByRole("button", { name: "Show all" });
  expect({ presentBetween: between !== null, focused: document.activeElement === after }).toEqual({
    presentBetween: true,
    focused: true,
  });
});
