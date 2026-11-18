import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import type { Annotation } from "../../api/generated/types";
import { FlagCard } from "./FlagCard";

const flag: Annotation = {
  id: "f1.5.a3",
  type: "flag",
  basis: "inference",
  range: { start_line: 99, end_line: 99 },
  was: "",
  is: "",
  text: "Potential double send:",
  evidence: [],
  flag: { level: "yellow", scenario: ["One.", "Two."], not_checked: "Not reproduced." },
};

it("offers Use as review comment, with a tooltip saying nothing is posted by it", async () => {
  const onUseAsComment = vi.fn();
  render(
    <ul>
      <FlagCard annotation={flag} onUseAsComment={onUseAsComment} />
    </ul>,
  );
  const button = screen.getByRole("button", { name: "Use as review comment" });
  expect(button).toHaveAccessibleDescription(/draft.*nothing is posted/i);
  await userEvent.click(button);
  expect(onUseAsComment).toHaveBeenCalledOnce();
});

it("offers nothing without the callback", () => {
  render(
    <ul>
      <FlagCard annotation={flag} />
    </ul>,
  );
  expect(screen.queryByRole("button", { name: "Use as review comment" })).toBeNull();
});
