import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { NOW, SUMMARY } from "../test-fixtures";
import { EmptyWork } from "../work-list/EmptyWork";
import { KeyboardHelp } from "./KeyboardHelp";

describe("KeyboardHelp", () => {
  it("lists the dashboard keys, takes the focus and closes on Escape", () => {
    const onClose = vi.fn();
    render(<KeyboardHelp open onClose={onClose} />);
    const dialog = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
    expect(document.activeElement).toBe(dialog);
    for (const key of ["j", "k", "Enter", "v", "g", "?", "Esc"])
      expect(screen.getByText(key, { selector: "kbd" })).toBeInTheDocument();
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("renders nothing when closed", () => {
    const { container } = render(<KeyboardHelp open={false} onClose={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe("EmptyWork", () => {
  it("says nothing needs the user, what was checked, and links the review start page", () => {
    const onNavigate = vi.fn();
    const snapshot = SUMMARY.snapshot!;
    render(
      <EmptyWork
        checkedAt={snapshot.scanned_at}
        checked={snapshot.checked}
        awaiting={snapshot.awaiting.length}
        reviewStartHref="/"
        onNavigate={onNavigate}
        now={NOW}
      />,
    );
    expect(screen.getByRole("heading", { name: "Nothing needs you right now." })).toBeInTheDocument();
    expect(screen.getByText(/Checked at/)).toHaveTextContent("14 PRs, 3 issues and 2 open TODOs");
    fireEvent.click(screen.getByRole("link", { name: "Open a pull request by URL" }));
    expect(onNavigate).toHaveBeenCalledWith("/");
  });
});

it("does not say nothing needs you when quick replies are waiting", () => {
  const snapshot = SUMMARY.snapshot!;
  render(
    <EmptyWork
      checkedAt={snapshot.scanned_at}
      checked={snapshot.checked}
      awaiting={0}
      hasMentions
      reviewStartHref="/review"
      now={NOW}
    />,
  );
  expect(screen.getByRole("heading", { name: "No ranked work right now." })).toBeInTheDocument();
});
