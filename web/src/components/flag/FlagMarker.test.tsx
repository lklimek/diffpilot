import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import type { Annotation, FlagLevel } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FlagMarker } from "./FlagMarker";

const remark = (id: string, level?: FlagLevel | "none"): Annotation => ({
  id,
  type: level === undefined ? "note" : "flag",
  basis: "inference",
  range: { start_line: 1, end_line: 1 },
  was: "",
  is: "",
  text: "Potential race:",
  evidence: [],
  ...(level && level !== "none" ? { flag: { level, scenario: ["a", "b"], not_checked: "c" } } : {}),
});

it("writes the strongest level in its colour and jumps to the first flag at that level; the tooltip counts them", async () => {
  const onSelect = vi.fn();
  const onRow = vi.fn();
  render(
    // A marker inside a clickable row acts on its own.
    <div onClick={onRow}>
      <FlagMarker
        annotations={[remark("n"), remark("y", "yellow"), remark("r", "red"), remark("z", "yellow")]}
        scope="step"
        subject="step 2"
        onSelect={onSelect}
      />
    </div>,
  );
  const marker = screen.getByRole("button", { name: "First red flag on step 2" });
  expect(marker).toHaveTextContent(/^⚑ red$/);
  expect(marker).toHaveClass("text-high");
  expect(marker).toHaveAccessibleDescription(
    /^LLM flags on step 2: 1 red flag, 2 yellow flags\. Click to go to the first red flag\. .*not checked/,
  );
  expectTrustWording(marker.textContent ?? "");
  await userEvent.click(marker);
  marker.focus();
  await userEvent.keyboard("{Enter}");
  expect(onSelect).toHaveBeenCalledTimes(2);
  expect(onRow).not.toHaveBeenCalled();
});

it("a yellow flag reads yellow; a flow says 'in'", () => {
  render(
    <FlagMarker annotations={[remark("y", "yellow")]} scope="flow" subject="flow 4" onSelect={() => {}} />,
  );
  const marker = screen.getByRole("button", { name: "First yellow flag in flow 4" });
  expect(marker).toHaveTextContent(/^⚑ yellow$/);
  expect(marker).toHaveClass("text-med");
  expect(marker).toHaveAccessibleDescription(/^LLM flags in flow 4: 1 yellow flag\./);
});

it("a flag stored without a level shows the glyph without a colour level", () => {
  render(
    <FlagMarker annotations={[remark("u", "none")]} scope="step" subject="step 1" onSelect={() => {}} />,
  );
  const marker = screen.getByRole("button", { name: "First flag without a level on step 1" });
  expect(marker).toHaveTextContent(/^⚑$/);
});

it("shows nothing without flags (never an all-clear)", () => {
  const { container } = render(
    <FlagMarker annotations={[remark("n")]} scope="step" subject="step 1" onSelect={() => {}} />,
  );
  expect(container).toBeEmptyDOMElement();
});
