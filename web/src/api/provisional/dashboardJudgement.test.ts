import { describe, expect, it } from "vitest";
import { itemById } from "../../components/dashboard/test-fixtures";
import { itemJudgement } from "./dashboardJudgement";

describe("legacy judgement", () => {
  const item = itemById("NC-482");
  const llm = item.llm!;
  const provisional = { ...item.score, state: "provisional" as const };

  it("keeps a current judgement when the score has open parts", () => {
    expect(itemJudgement({ ...item, llm: { ...llm, inherited: false }, score: provisional })).toMatchObject({
      state: "current",
      judged_at: llm.judged_at,
    });
  });

  it("identifies an inherited judgement by its provenance", () => {
    expect(itemJudgement({ ...item, llm: { ...llm, inherited: true }, score: provisional })).toMatchObject({
      state: "outdated",
      judged_at: llm.judged_at,
      outdated_because: null,
    });
  });

  it("treats an absent LLM judgement as unjudged", () => {
    expect(itemJudgement({ ...item, llm: null })).toMatchObject({ state: "not_judged", judged_at: null });
  });
});
