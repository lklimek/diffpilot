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
  ratings: [{ rating: "high", steps: 1 }],
};

it("a step: the LLM rating, then the remark count in plain weight; one pill, never split by severity", async () => {
  const onSelect = vi.fn();
  render(<RemarkPill summary={step} scope="step" subject="this step" onSelect={onSelect} />);
  const pill = screen.getByRole("button");
  expect(pill).toHaveTextContent("▲ HIGH · 5 remarks");
  expect(pill).toHaveAccessibleName("LLM rated this step HIGH (not checked) · 5 LLM remarks");
  expect(pill).toHaveAccessibleDescription(/go to its first LLM remark/);
  expect(pill.querySelector(".font-normal")).toHaveTextContent("5 remarks");
  expect(pill.className).toContain("bg-llm-bg");
  expect(pill.className).toContain(REMARK_LOOK.high.text);
  expect(pill.className).toMatch(/min-h-6/);
  await userEvent.click(pill);
  expect(onSelect).toHaveBeenCalled();
  expectTrustWording(document.body.textContent ?? "");
});

it("remarks without a rating get a neutral pill; a rating without remarks stands alone", () => {
  const { rerender } = render(
    <RemarkPill
      summary={{ rating: null, rated: 0, remarks: 2, handled: 0, ratings: [{ rating: null, steps: 1 }] }}
      scope="step"
      subject="step 3"
      onSelect={() => {}}
    />,
  );
  expect(screen.getByRole("button")).toHaveTextContent(/^2 remarks$/);
  expect(screen.getByRole("button")).toHaveAccessibleName("LLM did not rate step 3 · 2 LLM remarks");
  expect(screen.getByRole("button").className).toContain(REMARK_LOOK.none.text);
  rerender(
    <RemarkPill
      summary={{ rating: "low", rated: 1, remarks: 0, handled: 0, ratings: [{ rating: "low", steps: 1 }] }}
      scope="step"
      subject="step 3"
      onSelect={() => {}}
    />,
  );
  expect(screen.getByRole("button")).toHaveTextContent(/^● LOW$/);
  expect(screen.getByRole("button")).toHaveAccessibleName("LLM rated step 3 LOW (not checked)");
});

it("a flow: the highest step rating ×steps at it and all remarks; the tooltip lists the steps per rating", () => {
  render(
    <RemarkPill
      summary={{
        rating: "high",
        rated: 2,
        remarks: 11,
        handled: 11,
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
  expect(pill).toHaveTextContent("▲ HIGH ×2 · 11 remarks");
  expect(pill).toHaveAccessibleName(
    "LLM rated 2 steps in flow 2 HIGH (not checked), the highest rating there · 11 LLM remarks",
  );
  expect(pill).toHaveAccessibleDescription(/Steps by LLM rating: HIGH 2, LOW 3, not rated 1\./);
  // All remarks have the user's feedback: dimmed, and said.
  expect(pill.className).toContain("text-muted");
  expect(pill).toHaveAccessibleDescription(/All 11 have your feedback/);
});

it("next to the flow's own rating label it shows the remarks only; the tooltip keeps the steps per rating", () => {
  const flow: RemarkSummary = {
    rating: "high",
    rated: 1,
    remarks: 11,
    handled: 0,
    ratings: [
      { rating: "high", steps: 1 },
      { rating: "low", steps: 2 },
    ],
  };
  const { rerender, container } = render(
    <RemarkPill summary={flow} scope="flow" subject="flow 2" withRating={false} onSelect={() => {}} />,
  );
  const pill = screen.getByRole("button");
  expect(pill).toHaveTextContent(/^11 remarks$/);
  expect(pill).toHaveAccessibleName("11 LLM remarks in flow 2");
  expect(pill).toHaveAccessibleDescription(
    /Steps by LLM rating: HIGH 1, LOW 2 \(the flow's rating is its highest step rating\)\./,
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
