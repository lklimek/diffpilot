import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import { REMARK_LOOK } from "../code-view/changeLook";
import { RemarkPill } from "./RemarkPills";
import type { RemarkSummary } from "./remarks";

const step: RemarkSummary = {
  rating: "high",
  rated: 1,
  remarks: 5,
  handled: 0,
  flags: { red: 0, yellow: 0, unknown: 0 },
  ratings: [{ rating: "high", steps: 1 }],
};

it("a step: the pill shows the LLM rating only; the remark count is in its tooltip", async () => {
  const onSelect = vi.fn();
  render(<RemarkPill summary={step} scope="step" subject="this step" onSelect={onSelect} />);
  const pill = screen.getByRole("button");
  expect(pill).toHaveTextContent(/^▲ HIGH$/);
  expect(pill).toHaveAccessibleName("LLM rated this step HIGH (not checked)");
  expect(pill).toHaveAccessibleDescription(/^5 LLM remarks on this step\. .*go to its first LLM remark/);
  expect(pill.className).toContain("bg-llm-bg");
  expect(pill.className).toContain(REMARK_LOOK.high.text);
  expect(pill.className).toMatch(/min-h-6/);
  await userEvent.click(pill);
  expect(onSelect).toHaveBeenCalled();
  expectTrustWording(document.body.textContent ?? "");
});

it("remarks without a rating get a small neutral marker, never an all-clear; a rating without remarks stands alone", () => {
  const { rerender } = render(
    <RemarkPill
      summary={{
        rating: null,
        rated: 0,
        remarks: 2,
        handled: 0,
        flags: { red: 0, yellow: 0, unknown: 0 },
        ratings: [{ rating: null, steps: 1 }],
      }}
      scope="step"
      subject="step 3"
      onSelect={() => {}}
    />,
  );
  const marker = screen.getByRole("button");
  expect(marker).toHaveTextContent(/^$/);
  expect(marker).toHaveAccessibleName("2 LLM remarks on step 3");
  expect(marker).toHaveAccessibleDescription(/^2 LLM remarks on step 3; the LLM did not rate it\. /);
  expect(marker.className).toContain(REMARK_LOOK.none.text);
  expect(marker.className).toMatch(/min-h-6/);
  expect(marker.className).toMatch(/min-w-6/);
  rerender(
    <RemarkPill
      summary={{
        rating: "low",
        rated: 1,
        remarks: 0,
        handled: 0,
        flags: { red: 0, yellow: 0, unknown: 0 },
        ratings: [{ rating: "low", steps: 1 }],
      }}
      scope="step"
      subject="step 3"
      onSelect={() => {}}
    />,
  );
  expect(screen.getByRole("button")).toHaveTextContent(/^● LOW$/);
  expect(screen.getByRole("button")).toHaveAccessibleName("LLM rated step 3 LOW (not checked)");
  expect(screen.getByRole("button")).toHaveAccessibleDescription("Click to open the step.");
});

it("a flow: the highest step rating ×steps at it; the tooltip has the remarks and the steps per rating", () => {
  render(
    <RemarkPill
      summary={{
        rating: "high",
        rated: 2,
        remarks: 11,
        handled: 11,
        flags: { red: 0, yellow: 0, unknown: 0 },
        ratings: [
          { rating: "high", steps: 2 },
          { rating: "low", steps: 3 },
          { rating: null, steps: 1 },
        ],
      }}
      scope="flow"
      subject="flow 2"
      onSelect={() => {}}
    />,
  );
  const pill = screen.getByRole("button");
  expect(pill).toHaveTextContent(/^▲ HIGH ×2$/);
  expect(pill).toHaveAccessibleName(
    "LLM rated 2 steps in flow 2 HIGH (not checked), the highest rating there",
  );
  expect(pill).toHaveAccessibleDescription(
    /^11 LLM remarks in flow 2\. Steps by LLM rating: HIGH 2, LOW 3, not rated 1\./,
  );
  // All remarks have the user's feedback: dimmed, and said.
  expect(pill.className).toContain("text-muted");
  expect(pill).toHaveAccessibleDescription(/All 11 have your feedback/);
});

it("next to the flow's own rating label: a neutral marker for its remarks; the tooltip keeps the steps per rating", () => {
  const flow: RemarkSummary = {
    rating: "high",
    rated: 1,
    remarks: 11,
    handled: 0,
    flags: { red: 0, yellow: 0, unknown: 0 },
    ratings: [
      { rating: "high", steps: 1 },
      { rating: "low", steps: 2 },
    ],
  };
  const { rerender, container } = render(
    <RemarkPill summary={flow} scope="flow" subject="flow 2" withRating={false} onSelect={() => {}} />,
  );
  const pill = screen.getByRole("button");
  expect(pill).toHaveTextContent(/^$/);
  expect(pill).toHaveAccessibleName("11 LLM remarks in flow 2");
  expect(pill).toHaveAccessibleDescription(
    /^11 LLM remarks in flow 2\. Steps by LLM rating: HIGH 1, LOW 2 \(the flow's rating is its highest step rating\)\./,
  );
  rerender(
    <RemarkPill
      summary={{ ...flow, remarks: 0 }}
      scope="flow"
      subject="flow 2"
      withRating={false}
      onSelect={() => {}}
    />,
  );
  expect(container).toBeEmptyDOMElement();
});

it("shows nothing without a rating and without remarks: never an all-clear", () => {
  const { container } = render(
    <RemarkPill summary={null} scope="step" subject="this step" onSelect={() => {}} />,
  );
  expect(container).toBeEmptyDOMElement();
});

it("can carry a leading label", () => {
  render(<RemarkPill summary={step} scope="flow" subject="this flow" label="LLM:" onSelect={() => {}} />);
  expect(screen.getByText("LLM:")).toBeVisible();
});

it("counts the LLM's flags by level in the tooltip; the pill still shows only the rating", () => {
  const flags = { red: 1, yellow: 2, unknown: 0 };
  const { rerender } = render(
    <RemarkPill summary={{ ...step, flags }} scope="step" subject="this step" onSelect={() => {}} />,
  );
  const pill = screen.getByRole("button");
  expect(pill).toHaveTextContent(/^▲ HIGH$/);
  expect(pill).toHaveAccessibleName("LLM rated this step HIGH (not checked)");
  expect(pill).toHaveAccessibleDescription(/^LLM flags on this step: 1 red flag, 2 yellow flags\. /);
  // The neutral marker of an unrated step is named by its flags.
  rerender(
    <RemarkPill
      summary={{ ...step, rating: null, rated: 0, flags }}
      scope="step"
      subject="step 3"
      onSelect={() => {}}
    />,
  );
  expect(screen.getByRole("button")).toHaveAccessibleName("LLM flags on step 3: 1 red flag, 2 yellow flags");
});
