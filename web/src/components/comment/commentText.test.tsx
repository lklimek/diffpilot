import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { PendingReview } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { CommentMarker } from "./LineComments";
import { PendingCommentItem } from "./PendingCommentItem";

const body =
  "**Finding**: use `nonce`. [Docs](https://github.com/acme/repo)\n<details><summary>Prompt for AI agents</summary>Hidden.</details>";
const comment = { ...(seedFor("missed", "pending-review") as PendingReview).comments[0]!, body };

describe("GitHub pending comment display", () => {
  it("formats the display while preserving the original text for editing", async () => {
    const { container } = render(
      <ul>
        <PendingCommentItem comment={comment} onEdit={vi.fn()} />
      </ul>,
    );
    expect(container).toHaveTextContent("Finding: use nonce. Docs (https://github.com/acme/repo)");
    expect(container.querySelector("code:not([class])")).toHaveTextContent("nonce");
    expect(container.querySelector("a,details,img")).toBeNull();
    expect(container).not.toHaveTextContent(/Prompt|Hidden/);
    await userEvent.click(screen.getByRole("button", { name: /^Edit comment on/ }));
    expect(screen.getByRole("textbox")).toHaveValue(body);
  });

  it("formats the comment marker's preview through the same display function", async () => {
    const { container } = render(
      <CommentMarker
        comments={[comment]}
        subject="line 96"
        open={false}
        panelId="comments"
        onToggle={vi.fn()}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "Your pending comment at line 96" }));
    expect(container).toHaveTextContent("Finding: use nonce. Docs (https://github.com/acme/repo)");
    expect(container).not.toHaveTextContent(/Prompt|Hidden|<details>/);
    expect(container.querySelector("a,details,img")).toBeNull();
  });
});
