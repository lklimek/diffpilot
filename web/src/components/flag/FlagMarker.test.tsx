import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
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

it("writes the strongest level in its colour, names every flag by level, and is focusable for its tooltip", () => {
  render(
    <FlagMarker
      annotations={[remark("n"), remark("y", "yellow"), remark("r", "red"), remark("z", "yellow")]}
      scope="step"
      subject="step 2"
    />,
  );
  const marker = screen.getByRole("img", { name: "LLM flags on step 2: 1 red flag, 2 yellow flags" });
  expect(marker).toHaveTextContent(/^⚑ red$/);
  expect(marker).toHaveClass("text-high");
  expect(marker).toHaveAttribute("tabindex", "0");
  expect(marker).toHaveAccessibleDescription(
    /^LLM flags on step 2: 1 red flag, 2 yellow flags\. .*not checked/,
  );
  expectTrustWording(marker.textContent ?? "");
});

it("a yellow flag reads yellow; a flow says 'in'", () => {
  render(<FlagMarker annotations={[remark("y", "yellow")]} scope="flow" subject="flow 4" />);
  const marker = screen.getByRole("img", { name: "LLM flags in flow 4: 1 yellow flag" });
  expect(marker).toHaveTextContent(/^⚑ yellow$/);
  expect(marker).toHaveClass("text-med");
});

it("a flag stored without a level shows the glyph without a colour level", () => {
  render(<FlagMarker annotations={[remark("u", "none")]} scope="step" subject="step 1" />);
  const marker = screen.getByRole("img", { name: "LLM flags on step 1: 1 flag without a level" });
  expect(marker).toHaveTextContent(/^⚑$/);
});

it("shows nothing without flags (never an all-clear)", () => {
  const { container } = render(<FlagMarker annotations={[remark("n")]} scope="step" subject="step 1" />);
  expect(container).toBeEmptyDOMElement();
});
