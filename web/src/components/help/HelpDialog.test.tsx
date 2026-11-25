import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import { CHANGE_LOOK } from "../code-view/changeLook";
import { HelpDialog } from "./HelpDialog";
import { FILES_SHORTCUTS, SHORTCUTS, shortcutAction } from "./shortcuts";

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
      "jnext block (including Not analysed and pending review); on a step it also ticks it as reviewed",
      "kprevious block",
      "nnext flow; after the last flow, next quiet block",
      "pprevious flow",
      "xtick the current step as reviewed",
      "dshow / hide old code of the current step",
      "ccomment on the current line",
      "aopen the Ask chat about the current step, flow or PR context",
      "rnext step with LLM remarks (its first remark flashes)",
      "sswitch to the Files view at this step's lines",
      "gopen the current line on GitHub (new tab)",
      "?this help",
      "Escclose",
    ]);
  });

  it("shows the Files view's keys when given: no x, since steps are ticked only in Flows", () => {
    render(<HelpDialog open onClose={() => {}} shortcuts={FILES_SHORTCUTS} />);
    const keys = within(screen.getByRole("table", { name: "Keyboard shortcuts" }))
      .getAllByRole("row")
      .map((row) => row.querySelector("kbd")?.textContent);
    expect(keys).toEqual(["j", "k", "n", "p", "v", "r", "d", "c", "a", "s", "g", "?", "Esc"]);
  });

  it("has the legend of how code is drawn (brief §4.3)", () => {
    render(<HelpDialog open onClose={() => {}} />);
    const legend = screen.getByRole("list", { name: "How code is drawn" });
    const text = legend.textContent ?? "";
    for (const part of [
      "+ added line: green, as in the classic diff (deterministic, from the diff)",
      "− removed line: red, as in the classic diff (deterministic, from the diff)",
      "HIGH: solid bar",
      "MEDIUM: dashed bar",
      "LOW or affected: dotted bar",
      "LLM remark without a risk rating: thin neutral line",
      "A changed line without a bar has no LLM remark.",
      "moved, text identical: double bar",
      "moved lines keep the change fill, as in the classic diff",
      "≠ line differs from the moved original",
      "outside the diff, unchanged",
      "◆ LLM remark",
      "your pending comment (accent bubble)",
      "GitHub review thread (slate bubble)",
      "N comment/import lines hidden · Show",
      "fading frame: the line an evidence link jumped to",
      "▲ HIGH · 5 remarks",
      "the LLM's rating of a step (not checked) and the number of its LLM remarks, two separate facts",
      "a flow shows its rating once (its label: the highest step rating) and a pill with all its remarks",
      "remarks on an unrated step: a neutral pill",
      "no pill means neither, not that the code is safe",
    ]) {
      expect(text).toContain(part);
    }
    // Change swatches come from the definition the code view and the classic diff use.
    expect(legend.querySelector(`.${CHANGE_LOOK.added.fill}`)).not.toBeNull();
    expect(legend.querySelector(`.${CHANGE_LOOK.removed.fill}`)).not.toBeNull();
    // The bar is the LLM's rating, never a fill: no remark colour fills a swatch of code.
    expect(
      legend.querySelector('[class*="bg-high-bg"],[class*="bg-low-bg"],[class*="bg-med-bg"]'),
    ).toBeNull();
    expect(text).toContain("not checked");
    expect(text).not.toContain("▣");
    // Both bubbles are drawn as in the code, in their colours, next to their words.
    const bubbles = [...legend.querySelectorAll("svg")].map((svg) => svg.parentElement!.className);
    expect(bubbles.some((c) => /(^|\s)text-accent(\s|$)/.test(c))).toBe(true);
    expect(bubbles.some((c) => /(^|\s)text-thread(\s|$)/.test(c))).toBe(true);
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

  it("returns focus to the element that had it when it closes", () => {
    const { rerender } = render(
      <>
        <button type="button">Help</button>
        <HelpDialog open={false} onClose={() => {}} />
      </>,
    );
    const trigger = screen.getByRole("button", { name: "Help" });
    trigger.focus();
    rerender(
      <>
        <button type="button">Help</button>
        <HelpDialog open onClose={() => {}} />
      </>,
    );
    expect(screen.getByRole("button", { name: "Close" })).toHaveFocus();
    rerender(
      <>
        <button type="button">Help</button>
        <HelpDialog open={false} onClose={() => {}} />
      </>,
    );
    expect(trigger).toHaveFocus();
  });

  it("describes each key with the action the tour's key map runs for it", () => {
    for (const { key, action } of SHORTCUTS) expect(shortcutAction(key)).toBe(action);
    expect(shortcutAction("j")).toBe("next_block");
    expect(shortcutAction("k")).toBe("previous_block");
    expect(shortcutAction("q")).toBeUndefined();
  });

  it("keeps the wording rules", () => {
    const { container } = render(<HelpDialog open onClose={() => {}} />);
    expectTrustWording(container.textContent ?? "");
  });
});
