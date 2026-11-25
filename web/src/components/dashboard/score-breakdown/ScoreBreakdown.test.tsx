import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { expectTagsWithTips } from "../../../__tests__/provenance";
import { ITEMS, NOW } from "../test-fixtures";
import { ScoreBreakdown } from "./ScoreBreakdown";

describe("ScoreBreakdown", () => {
  it("the LLM tag of each LLM-judged part takes keyboard focus and shows its tip (brief §4 rule 3)", () => {
    const item = ITEMS.find((i) =>
      i.score.parts.some((p) => p.source === "llm" && p.status !== "estimated"),
    )!;
    render(<ScoreBreakdown item={item} now={NOW} />);
    const tagged = [...document.querySelectorAll<HTMLElement>("tr[data-part] [data-source]")].filter((cell) =>
      cell.querySelector("[data-provenance]"),
    );
    expect(tagged.length).toBeGreaterThan(0);
    for (const cell of tagged) expectTagsWithTips(cell, 1);
  });
});
