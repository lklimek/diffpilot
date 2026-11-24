import { render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { assessmentComparison } from "../../mocks/assessment";
import { expectTrustWording } from "../../__tests__/wording";
import { AssessmentComparisonPanel, AssessmentComparisonReport } from "./AssessmentComparisonReport";
import { ProposalText } from "./ProposalText";
it("keeps missing, failed and unmatched rows with denominators, owner revisions and metering", () => {
  const { container } = render(<AssessmentComparisonReport report={assessmentComparison} />);
  const table = screen.getByRole("table", { name: "Assessment items" });
  expect(within(table).getAllByRole("row")).toHaveLength(3);
  expect(within(table).getByText("Method failed")).toBeVisible();
  expect(within(table).getByText("Unmatched point")).toBeVisible();
  expect(within(table).getByText(/revision 2/)).toBeVisible();
  expect(screen.getByText(/0 \/ 1 owner-decided/)).toBeVisible();
  expect(screen.getByText(/Known usage only/)).toBeVisible();
  expect(screen.getAllByText(/input 1200, cached 200, output 300/)).toHaveLength(2);
  expect(screen.queryByRole("button")).toBeNull();
  expectTrustWording(container.textContent ?? "");
});
it("retains mismatched head and unavailable proposals as distinct states", () => {
  const report = structuredClone(assessmentComparison);
  report.items[0]!.status = "head_mismatch";
  report.items[0]!.proposal = null;
  render(<AssessmentComparisonReport report={report} />);
  expect(screen.getByText("Different report head; excluded from scoring")).toBeVisible();
  expect(screen.getAllByText("Proposal unavailable").length).toBeGreaterThan(0);
});
it("preserves external PR URLs and existing follow-up proposals as text, without action conversion", () => {
  render(
    <ProposalText
      proposal={{
        kind: "follow_up_pr",
        target_url: "https://github.com/acme/other/pull/99",
        target_pr: null,
        title: "existing follow-up",
        fixed_in: "abc1234",
      }}
    />,
  );
  expect(screen.getByText(/https:\/\/github.com\/acme\/other\/pull\/99/)).toBeVisible();
  expect(screen.getByText(/existing follow-up/)).toBeVisible();
  expect(screen.queryByRole("link")).toBeNull();
});

it("labels per-point differences without scoring incomplete rows", () => {
  const report = structuredClone(assessmentComparison);
  report.items[1] = {
    ...report.items[1]!,
    status: "matched",
    verdict: "invalid",
    native_verdict: "wrong",
    proposal: { kind: "fix_here", target_pr: null, target_url: null, title: null, fixed_in: null },
  };
  render(<AssessmentComparisonReport report={report} />);
  expect(screen.getByText("Different verdicts; different proposals.")).toBeVisible();
  expect(screen.getByText("Incomplete comparison.")).toBeVisible();
});

it("keeps a closed, unrequested report free of loading content", () => {
  render(<AssessmentComparisonPanel expanded={false} loading onOpenChange={vi.fn()} />);
  expect(screen.queryByText("Loading comparison…")).toBeNull();
});
