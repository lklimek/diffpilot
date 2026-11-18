import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import { REMARK_LOOK } from "../code-view/changeLook";
import { RemarkPills } from "./RemarkPills";

const counts = [
  { band: "high", count: 1, handled: 0 },
  { band: "medium", count: 2, handled: 0 },
  { band: "low", count: 3, handled: 3 },
] as const;

it("shows one pill per band, highest first, labelled in words and LLM-styled, with the summary as tooltip", async () => {
  const onSelect = vi.fn();
  render(<RemarkPills counts={[...counts]} subject="on this step" onSelect={onSelect} />);
  const pills = screen.getAllByRole("button");
  expect(pills.map((p) => p.textContent)).toEqual(["▲ HIGH 1", "◆ MED 2", "● LOW 3"]);
  expect(pills[0]).toHaveAccessibleDescription(
    "LLM remarks on this step: 1 high, 2 medium, 3 low — click to go to the first high one.",
  );
  expect(pills[0]!.className).toContain("bg-llm-bg");
  expect(pills[0]!.className).toContain(REMARK_LOOK.high.text);
  expect(pills[0]!.className).toMatch(/min-h-6/);
  // Remarks that all have the user's feedback are dimmed and say so.
  expect(pills[2]!.className).toContain("text-muted");
  expect(pills[2]).toHaveAccessibleDescription(/all 3 have your feedback/);
  await userEvent.click(pills[1]!);
  expect(onSelect).toHaveBeenCalledWith("medium");
  expectTrustWording(document.body.textContent ?? "");
});

it("shows nothing at all without remarks: no zero, no all-clear", () => {
  const { container } = render(<RemarkPills counts={[]} subject="on this step" onSelect={() => {}} />);
  expect(container).toBeEmptyDOMElement();
});

it("can carry a leading label", () => {
  render(
    <RemarkPills
      counts={[counts[0]]}
      subject="in this flow"
      label="Remarks in this flow:"
      onSelect={() => {}}
    />,
  );
  expect(screen.getByText("Remarks in this flow:")).toBeVisible();
});
