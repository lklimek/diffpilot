import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import { HelpDialog } from "./HelpDialog";

describe("HelpDialog", () => {
  it("renders nothing while closed", () => {
    const { container } = render(<HelpDialog open={false} onClose={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("is a labelled modal dialog with the keyboard shortcuts and takes focus", () => {
    render(<HelpDialog open onClose={() => {}} />);
    const dialog = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(dialog).toContainElement(document.activeElement as HTMLElement);
    const keys = within(screen.getByRole("table", { name: "Keyboard shortcuts" }))
      .getAllByRole("row")
      .map((row) => row.textContent);
    expect(keys).toEqual([
      "j kprevious / next block (flow introduction or step)",
      "n pnext / previous flow",
      "xtick the current step as reviewed",
      "dshow / hide old code of the current step",
      "ccomment on the current line",
      "aask about the current step",
      "?this help",
      "Escclose",
    ]);
  });

  it("has the legend of how code is drawn (brief §4.3)", () => {
    render(<HelpDialog open onClose={() => {}} />);
    const legend = screen.getByRole("list", { name: "How code is drawn" });
    const text = legend.textContent ?? "";
    for (const part of [
      "changed line: neutral bar in the gutter (deterministic, from the diff)",
      "HIGH: solid bar",
      "MEDIUM: dashed bar",
      "LOW or affected: dotted bar",
      "A changed line without colour has no LLM remark.",
      "moved, text identical: double bar",
      "≠ line differs from the moved original",
      "outside the diff, unchanged",
      "removed code shown at the deletion marker",
      "◆ LLM remark",
      "▣ your comment",
    ]) {
      expect(text).toContain(part);
    }
  });

  it("closes with the button, Escape and a click outside", async () => {
    const onClose = vi.fn();
    render(<HelpDialog open onClose={onClose} />);
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    await userEvent.keyboard("{Escape}");
    await userEvent.click(screen.getByTestId("help-backdrop"));
    expect(onClose).toHaveBeenCalledTimes(3);
  });

  it("keeps focus inside while open", async () => {
    render(<HelpDialog open onClose={() => {}} />);
    await userEvent.tab();
    expect(screen.getByRole("dialog")).toContainElement(document.activeElement as HTMLElement);
    await userEvent.tab({ shift: true });
    expect(screen.getByRole("dialog")).toContainElement(document.activeElement as HTMLElement);
  });

  it("keeps the wording rules", () => {
    const { container } = render(<HelpDialog open onClose={() => {}} />);
    expectTrustWording(container.textContent ?? "");
  });
});
