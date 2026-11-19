import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { LinesResponse, PendingComment } from "../../api/generated/types";
import { CodeView } from "./CodeView";

function plain(from: number, texts: string[], extra: Partial<LinesResponse> = {}): LinesResponse {
  return {
    path: "src/a.rs",
    side: "head",
    from,
    to: from + texts.length - 1,
    total_lines: from + texts.length - 1,
    lines: texts,
    changed: [],
    deleted: [],
    moved: [],
    comments: { scope: "in_diff", line_thread_ranges: [] },
    ...extra,
  };
}
const mine: PendingComment = {
  id: "c1",
  thread_id: "t1",
  path: "src/a.rs",
  kind: "line",
  side: "head",
  start_line: null,
  line: 2,
  body: "Is the retry bounded?",
};

// Owner: a click pins the popup until the next click; without a pin it disappears when the pointer leaves.
describe("second click on the icon, then pointer away", () => {
  it("ask '?'", async () => {
    const user = userEvent.setup();
    render(<CodeView lines={plain(1, ["a", "b", "c"])} onAskLine={vi.fn()} />);
    const icon = screen.getByRole("button", { name: "Ask the LLM about line 2" });
    await user.click(icon);
    await user.click(icon);
    await user.unhover(icon);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("comment marker", async () => {
    const user = userEvent.setup();
    render(<CodeView lines={plain(1, ["a", "b", "c"])} comments={[mine]} />);
    const icon = screen.getByRole("button", { name: "Your pending comment at line 2" });
    await user.click(icon);
    expect(screen.getByRole("dialog")).toBeVisible();
    await user.click(icon);
    await user.unhover(icon);
    expect(icon).toHaveFocus();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("show the rest", async () => {
    const user = userEvent.setup();
    const block = { after_line: 1, base_start: 10, lines: ["x", "y"], total_lines: 7 };
    render(<CodeView lines={plain(1, ["a", "b"], { deleted: [block] })} onLoadDeleted={vi.fn()} />);
    const icon = screen.getByRole("button", { name: "Show 5 more deleted lines" });
    await user.click(icon);
    await user.click(icon);
    await user.unhover(icon);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
