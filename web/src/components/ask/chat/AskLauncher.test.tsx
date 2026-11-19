import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { AskLauncher } from "./AskLauncher";

describe("AskLauncher (proposal AC1)", () => {
  it("opens the chat; its name carries the new answers and an answer being written", async () => {
    const onOpen = vi.fn();
    render(<AskLauncher unread={2} busy onOpen={onOpen} />);
    const button = screen.getByRole("button", { name: /^Ask 2 new answers: open your private chat/ });
    expect(button).toHaveAccessibleName(/an answer is being written/);
    await userEvent.setup().click(button);
    expect(onOpen).toHaveBeenCalled();
  });

  it("without news it is just Ask", () => {
    render(<AskLauncher unread={0} busy={false} onOpen={() => {}} />);
    expect(screen.getByRole("button", { name: /^Ask: open your private chat about this PR$/ })).toBeVisible();
  });
});
