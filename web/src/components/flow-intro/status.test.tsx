import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { AnalysisState, FlowAnalysisStatus } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FlowWorkLabel, flowWorkStatus } from "./status";

const status = (state: FlowAnalysisStatus["state"]): FlowAnalysisStatus => ({
  state,
  started_at: null,
  message: null,
});
const RUNNING: AnalysisState = { state: "running", done: 1, total: 5 };
const USER_PAUSED: AnalysisState = {
  state: "paused",
  reason: "user",
  paused_by: "alice",
  resumes_at: null,
  done: 1,
  total: 5,
};
const LIMIT_PAUSED: AnalysisState = { state: "paused", resumes_at: null, done: 1, total: 5 };
const FAILED: AnalysisState = { state: "failed", flow: null, message: "The runner stopped." };

describe("flowWorkStatus (pending flow texts per analysis state)", () => {
  it("running: the flow's own work state", () => {
    expect(flowWorkStatus(status("queued"), RUNNING).text).toBe("Queued");
    expect(flowWorkStatus(status("not_analysed"), RUNNING).text).toBe("Not analysed yet");
    expect(flowWorkStatus(status("analysing"), RUNNING).text).toBe("Being analysed");
    expect(flowWorkStatus(status("failed"), RUNNING).text).toBe("Failed");
  });

  it("paused (by a user or the usage limit): waiting flows say the analysis is paused", () => {
    for (const paused of [USER_PAUSED, LIMIT_PAUSED]) {
      expect(flowWorkStatus(status("queued"), paused).text).toBe("Queued — analysis paused");
      expect(flowWorkStatus(status("not_analysed"), paused).text).toBe("Not analysed yet — analysis paused");
      // The running job finishes during a pause.
      expect(flowWorkStatus(status("analysing"), paused).text).toBe("Being analysed");
    }
  });

  it("failed analysis: nothing waits any more, never Queued", () => {
    for (const s of ["queued", "not_analysed", "analysing"] as const)
      expect(flowWorkStatus(status(s), FAILED).text).toBe("Not analysed — the analysis failed");
    expect(flowWorkStatus(status("failed"), FAILED).text).toBe("Failed");
  });

  it("every text and tip is free of safety wording and says the flow is not assessed", () => {
    for (const analysis of [RUNNING, USER_PAUSED, LIMIT_PAUSED, FAILED])
      for (const s of ["queued", "not_analysed", "analysing", "failed"] as const) {
        const { text, tip } = flowWorkStatus(status(s), analysis);
        expectTrustWording(`${text} ${tip}`);
        expect(tip).toMatch(/not assessed/i);
      }
  });

  it("label: the text with its tip, keyboard reachable", () => {
    render(<FlowWorkLabel status={status("queued")} analysis={USER_PAUSED} />);
    const label = screen.getByText("Queued — analysis paused");
    expect(label).toHaveAttribute("tabindex", "0");
    expect(label).toHaveAccessibleDescription(/paused/);
  });
});
