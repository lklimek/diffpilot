import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { itemById } from "../test-fixtures";
import { matchesStatus } from "../work-list/StatusFilter";
import { WorkJudgement } from "./WorkJudgement";

// C1: `WorkItem.judgement` is absent when the scorer never asks the LLM; no "not judged yet" promise applies.
it("shows no judgement mark for an item the scorer never asks the LLM about", () => {
  const item = { ...itemById("NC-482"), llm: null, judgement: undefined };
  const { rerender } = render(<WorkJudgement item={item} />);
  expect(screen.queryByText(/not judged/)).toBeNull();
  rerender(<WorkJudgement item={{ ...item, llm: { ...itemById("NC-482").llm!, inherited: true } }} />);
  expect(screen.queryByRole("button", { name: /outdated/ })).toBeNull();
});

it("leaves items without judgement out of the ranking status filters", () => {
  const item = { ...itemById("NC-482"), llm: null, judgement: undefined };
  expect(matchesStatus(item, "outdated")).toBe(false);
  expect(matchesStatus(item, "not_judged")).toBe(false);
});

it("keeps judging items in the outdated filter per J7", () => {
  const item = {
    ...itemById("NC-482"),
    judgement: { state: "judging", judged_at: null, outdated_because: null } as const,
  };
  expect(matchesStatus(item, "outdated")).toBe(true);
});
