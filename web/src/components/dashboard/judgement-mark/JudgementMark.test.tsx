import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { JudgementMark } from "./JudgementMark";

const judged_at = "2026-11-17T12:00:00Z";

describe("JudgementMark", () => {
  it("offers one direct action with the prompt-change reason and cost", async () => {
    const onRejudge = vi.fn();
    render(
      <JudgementMark
        judgement={{ state: "outdated", judged_at, outdated_because: "prompt_changed" }}
        onRejudge={onRejudge}
      />,
    );
    const button = screen.getByRole("button", { name: /outdated · 17 Nov/ });
    expect(button).toHaveAccessibleDescription(/ranking prompt changed.*1 of today’s 120/i);
    await userEvent.click(button);
    expect(onRejudge).toHaveBeenCalledOnce();
  });

  it("shows running, queued, unjudged and capped states without another action", () => {
    const { rerender } = render(
      <JudgementMark judgement={{ state: "judging", judged_at, outdated_because: null }} />,
    );
    expect(screen.getByText(/judging…/)).toBeVisible();
    expect(screen.queryByRole("button")).toBeNull();
    rerender(<JudgementMark judgement={{ state: "queued", judged_at: null, outdated_because: null }} />);
    expect(screen.getByText(/not judged yet · queued/)).toBeVisible();
    rerender(<JudgementMark judgement={{ state: "not_judged", judged_at: null, outdated_because: null }} />);
    expect(screen.getByText(/not judged yet/)).toBeVisible();
    rerender(
      <JudgementMark
        judgement={{ state: "capped", judged_at: null, outdated_because: null }}
        usageHref="/settings#llm-usage"
      />,
    );
    expect(screen.getByRole("link", { name: "Usage and limits" })).toHaveAttribute(
      "href",
      "/settings#llm-usage",
    );
    rerender(<JudgementMark judgement={{ state: "current", judged_at, outdated_because: null }} />);
    expect(screen.queryByText(/judg/)).toBeNull();
  });
  it.each(["not_judged", "queued", "capped"] as const)("renders the %s mark in neutral colours", (state) => {
    render(<JudgementMark judgement={{ state, judged_at: null, outdated_because: null }} />);
    expect(screen.getByText(/^◌ not judged/)).toHaveClass("border-mv-bd", "text-mv");
  });
});
