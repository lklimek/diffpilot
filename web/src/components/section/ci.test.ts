import { describe, expect, it } from "vitest";
import { ciText, ciTip } from "./ci";
const zero = { passed: 0, failed: 0, pending: 0, skipped: 0, neutral: 0 };
describe("CI evidence", () => {
  it("only claims all passing with complete successful source counts", () => {
    const ci = {
      state: "passing" as const,
      total: 1,
      failing: [],
      check_runs: { ...zero, passed: 1 },
      statuses: zero,
    };
    expect(ciText(ci)).toContain("all passing");
    expect(ciText({ ...ci, statuses: undefined })).not.toContain("all passing");
    expect(ciText({ ...ci, check_runs: { ...zero, skipped: 1 } })).not.toContain("all passing");
    expect(ciText({ ...ci, check_runs: { ...zero, neutral: 1 } })).not.toContain("all passing");
    expect(ciText({ ...ci, check_runs: zero })).not.toContain("all passing");
    expect(ciTip({ ...ci, statuses: undefined })).toContain("Statuses: not checked");
  });
  it("explains no_failures without implying all checks ran", () => {
    expect(
      ciText({
        state: "no_failures",
        total: 5,
        failing: [],
        check_runs: { ...zero, passed: 1, skipped: 4 },
        statuses: zero,
      }),
    ).toContain("Nothing failed among the checks read, but not everything ran successfully");
  });
});
