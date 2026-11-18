import { act, render, screen } from "@testing-library/react";
import { useRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FLASH_MS, useFlashTarget, type FlashRequest } from "./useFlashTarget";

function Harness({ request, hidden = false }: { request: FlashRequest | null; hidden?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  useFlashTarget(ref, request);
  return (
    <div ref={ref}>
      <p data-item="a">First</p>
      <div hidden={hidden}>
        <p data-item="b">Second</p>
      </div>
    </div>
  );
}

const frame = () => act(() => vi.advanceTimersByTime(20));

describe("useFlashTarget", () => {
  let scroll: ReturnType<typeof vi.fn>;
  beforeEach(() => {
    vi.useFakeTimers();
    scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll as unknown as Element["scrollIntoView"];
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("scrolls the target into view and frames it for about three seconds", () => {
    render(<Harness request={{ selector: '[data-item="a"]', nonce: 1 }} />);
    frame();
    const target = screen.getByText("First");
    expect(scroll.mock.contexts[0]).toBe(target);
    expect(scroll).toHaveBeenCalledWith(expect.objectContaining({ block: "center", behavior: "smooth" }));
    expect(target).toHaveAttribute("data-flash");
    act(() => vi.advanceTimersByTime(FLASH_MS));
    expect(target).not.toHaveAttribute("data-flash");
    expect(FLASH_MS).toBe(3000);
  });

  it("flashes the same target again for a new request", () => {
    const { rerender } = render(<Harness request={{ selector: '[data-item="a"]', nonce: 1 }} />);
    frame();
    act(() => vi.advanceTimersByTime(FLASH_MS));
    rerender(<Harness request={{ selector: '[data-item="a"]', nonce: 2 }} />);
    frame();
    expect(screen.getByText("First")).toHaveAttribute("data-flash");
    expect(scroll).toHaveBeenCalledTimes(2);
  });

  it("moves the frame when the target changes", () => {
    const { rerender } = render(<Harness request={{ selector: '[data-item="a"]', nonce: 1 }} />);
    frame();
    rerender(<Harness request={{ selector: '[data-item="b"]', nonce: 2 }} />);
    frame();
    expect(screen.getByText("First")).not.toHaveAttribute("data-flash");
    expect(screen.getByText("Second")).toHaveAttribute("data-flash");
  });

  it("jumps without smooth scrolling when the user prefers reduced motion", () => {
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: query.includes("reduce") }));
    render(<Harness request={{ selector: '[data-item="a"]', nonce: 1 }} />);
    frame();
    expect(scroll).toHaveBeenCalledWith(expect.objectContaining({ behavior: "auto" }));
    expect(screen.getByText("First")).toHaveAttribute("data-flash");
  });

  it("waits for a target that is still collapsed, then flashes it once it shows", () => {
    const { rerender } = render(<Harness hidden request={{ selector: '[data-item="b"]', nonce: 1 }} />);
    frame();
    expect(scroll).not.toHaveBeenCalled();
    rerender(<Harness request={{ selector: '[data-item="b"]', nonce: 1 }} />);
    frame();
    expect(screen.getByText("Second")).toHaveAttribute("data-flash");
  });

  it("does nothing without a request or a matching target", () => {
    const { rerender } = render(<Harness request={null} />);
    frame();
    rerender(<Harness request={{ selector: '[data-item="none"]', nonce: 1 }} />);
    act(() => vi.advanceTimersByTime(2000));
    expect(scroll).not.toHaveBeenCalled();
    expect(document.querySelector("[data-flash]")).toBeNull();
  });
});
