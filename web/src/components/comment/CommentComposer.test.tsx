import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef } from "react";
import { describe, expect, it, vi } from "vitest";
import type {
  CommentTarget,
  CreateCommentRequest,
  CreateCommentResponse,
  PendingComment,
  PendingReview,
} from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FIXTURE_STATES, seedFor } from "../../mocks/fixtures";
import { CommentComposer, type CommentComposerProps, type CommentLine } from "./CommentComposer";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const target96: CommentTarget = { path: "src/rpc/client.rs", side: "head", start_line: null, line: 96 };
const PERMALINK = "https://github.com/acme/node-client/blob/52d2da1/src/rpc/client.rs#L96";
const line96: CommentLine = { target: target96, placement: "line", permalink: PERMALINK };
const missedComments = (seedFor("missed", "pending-review") as PendingReview).comments;

const created = (over: Partial<CreateCommentResponse> = {}): CreateCommentResponse => ({
  comment: { ...missedComments[0]!, body: "x" },
  missed_by_llm_recorded: false,
  ...over,
});

function setup(props: Partial<CommentComposerProps> = {}) {
  const all: CommentComposerProps = {
    available: true,
    line: line96,
    comments: [],
    onCreate: vi.fn().mockResolvedValue(created()),
    onEdit: vi.fn().mockResolvedValue(undefined),
    onDelete: vi.fn().mockResolvedValue(undefined),
    onCancel: vi.fn(),
    ...props,
  };
  const view = render(<CommentComposer {...all} />);
  return { ...all, ...view, user: userEvent.setup() };
}

const field = () => screen.getByRole("textbox", { name: /Comment on src\/rpc\/client\.rs:96/ });
const addButton = () => screen.getByRole("button", { name: "Add to GitHub pending review" });

describe("CommentComposer", () => {
  it("replaces the composer by a plain explanation when commenting is not available", () => {
    setup({ available: false });
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
    expect(document.body).toHaveTextContent(
      "Commenting is not available for this pull request: the smart-review GitHub App is not installed on this repository",
    );
  });

  it("tells how to start a comment when no line is chosen", () => {
    setup({ line: null });
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(document.body).toHaveTextContent("Click a line number in the code to comment on that line");
  });

  it("writes a line comment inside the diff and focuses the field", () => {
    setup();
    expect(field()).toHaveFocus();
    expect(addButton()).toBeEnabled();
    expect(document.body).not.toHaveTextContent("posted as a file comment");
  });

  it("notes that a line outside the diff is posted as a file comment", () => {
    setup({ line: { ...line96, placement: "file" } });
    expect(document.body).toHaveTextContent(
      "This line is outside the PR diff, so the comment is posted as a file comment whose text links to the line.",
    );
    expect(addButton()).toBeInTheDocument();
  });

  it("offers the GitHub link instead of a field for a file outside the PR diff", () => {
    setup({ line: { ...line96, placement: "not_in_diff" } });
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(document.body).toHaveTextContent("This file is not part of the PR diff");
    expect(screen.getByRole("link", { name: /Open the line on GitHub/ })).toHaveAttribute("href", PERMALINK);
  });

  it("asks for text instead of posting an empty comment", async () => {
    const { user, onCreate } = setup();
    await user.click(addButton());
    expect(onCreate).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("Write a comment first.");
    expect(field()).toHaveAttribute("aria-invalid", "true");
  });

  it("posts the trimmed comment with a new idempotency key and reports a missed-by-the-LLM record", async () => {
    const onCreate = vi.fn().mockResolvedValue(created({ missed_by_llm_recorded: true }));
    const { user } = setup({ onCreate });
    await user.type(field(), "  Every attempt gets the full timeout.  ");
    await user.click(addButton());
    const request = onCreate.mock.calls[0]![0] as CreateCommentRequest;
    expect(request.target).toEqual(target96);
    expect(request.body).toBe("Every attempt gets the full timeout.");
    expect(request.idempotency_key).toMatch(UUID);
    expect(field()).toHaveValue("");
    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent("Added to your GitHub pending review as a line comment.");
    expect(status).toHaveTextContent("Also recorded as missed by the LLM");
  });

  it("names a file comment in the confirmation", async () => {
    const onCreate = vi.fn().mockResolvedValue(created({ comment: { ...missedComments[0]!, kind: "file" } }));
    const { user } = setup({ line: { ...line96, placement: "file" }, onCreate });
    await user.type(field(), "text");
    await user.click(addButton());
    expect(await screen.findByRole("status")).toHaveTextContent("as a file comment that links to the line");
  });

  it("posts once while a post is pending (double submit)", async () => {
    let resolve!: (r: CreateCommentResponse) => void;
    const onCreate = vi.fn().mockReturnValue(new Promise<CreateCommentResponse>((r) => (resolve = r)));
    const { user } = setup({ onCreate });
    await user.type(field(), "text");
    await user.click(addButton());
    await user.click(screen.getByRole("button", { name: /Adding/ }));
    expect(onCreate).toHaveBeenCalledOnce();
    resolve(created());
    expect(await screen.findByRole("status")).toBeInTheDocument();
  });

  it("keeps an unresolved post after a failure: whitespace cannot change it, Retry reuses key and payload (QA-002)", async () => {
    const onCreate = vi
      .fn()
      .mockRejectedValueOnce(new Error("GitHub is not reachable. Try again."))
      .mockResolvedValueOnce(created());
    const { user } = setup({ onCreate });
    await user.type(field(), "text");
    await user.click(addButton());
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Not confirmed: GitHub is not reachable. Try again.",
    );
    expect(document.body).toHaveTextContent("The comment may already be in your GitHub pending review");
    expect(field()).toHaveAttribute("readonly");
    await user.type(field(), " ");
    expect(field()).toHaveValue("text");
    expect(screen.queryByRole("button", { name: "Add to GitHub pending review" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    const [first, second] = onCreate.mock.calls.map((c) => c[0] as CreateCommentRequest);
    expect(second).toEqual(first);
    expect(await screen.findByRole("status")).toHaveTextContent("Added to your GitHub pending review");
    expect(field()).not.toHaveAttribute("readonly");
    expect(field()).toHaveValue("");
  });

  it("keeps the unresolved post when leaving and re-entering the line (QA-002)", async () => {
    const onCreate = vi.fn().mockRejectedValueOnce(new Error("timeout")).mockResolvedValueOnce(created());
    const { user, rerender, ...props } = setup({ onCreate });
    await user.type(field(), "text");
    await user.click(addButton());
    await screen.findByRole("alert");
    rerender(
      <CommentComposer
        {...props}
        onCreate={onCreate}
        line={{ ...line96, target: { ...target96, line: 97 } }}
      />,
    );
    expect(screen.getByRole("textbox", { name: /client\.rs:97/ })).toHaveValue("");
    rerender(<CommentComposer {...props} onCreate={onCreate} line={null} />);
    rerender(<CommentComposer {...props} onCreate={onCreate} line={line96} />);
    expect(field()).toHaveValue("text");
    await user.click(screen.getByRole("button", { name: "Retry" }));
    const keys = onCreate.mock.calls.map((c) => (c[0] as CreateCommentRequest).idempotency_key);
    expect(keys[1]).toBe(keys[0]);
  });

  it("discards an unresolved post into an editable draft that posts with a new key (QA-002)", async () => {
    const onCreate = vi.fn().mockRejectedValueOnce(new Error("timeout")).mockResolvedValueOnce(created());
    const { user } = setup({ onCreate });
    await user.type(field(), "text");
    await user.click(addButton());
    await screen.findByRole("alert");
    await user.click(screen.getByRole("button", { name: "Discard" }));
    expect(field()).not.toHaveAttribute("readonly");
    expect(field()).toHaveValue("text");
    await user.click(addButton());
    const keys = onCreate.mock.calls.map((c) => (c[0] as CreateCommentRequest).idempotency_key);
    expect(keys[1]).not.toBe(keys[0]);
  });

  it("uses a new key for the next comment after a success", async () => {
    const onCreate = vi.fn().mockResolvedValue(created());
    const { user } = setup({ onCreate });
    await user.type(field(), "one");
    await user.click(addButton());
    await screen.findByRole("status");
    await user.type(field(), "two");
    await user.click(addButton());
    const keys = onCreate.mock.calls.map((c) => (c[0] as CreateCommentRequest).idempotency_key);
    expect(keys[1]).not.toBe(keys[0]);
  });

  it("keeps the field read-only while posting and clears only what was sent (QA-001)", async () => {
    let resolve!: (r: CreateCommentResponse) => void;
    const onCreate = vi.fn().mockReturnValue(new Promise<CreateCommentResponse>((r) => (resolve = r)));
    const { user } = setup({ onCreate });
    await user.type(field(), "A");
    await user.click(addButton());
    expect(field()).toHaveAttribute("readonly");
    await user.type(field(), "B");
    expect(field()).toHaveValue("A");
    resolve(created());
    expect(await screen.findByRole("status")).toBeInTheDocument();
    expect(field()).toHaveValue("");
  });

  it("ignores a completion for a line that is no longer current (QA-001)", async () => {
    let resolve!: (r: CreateCommentResponse) => void;
    const onCreate = vi.fn().mockReturnValue(new Promise<CreateCommentResponse>((r) => (resolve = r)));
    const { user, rerender, ...props } = setup({ onCreate });
    await user.type(field(), "A");
    await user.click(addButton());
    rerender(
      <CommentComposer
        {...props}
        onCreate={onCreate}
        line={{ ...line96, target: { ...target96, line: 97 } }}
      />,
    );
    const other = screen.getByRole("textbox", { name: /client\.rs:97/ });
    await user.type(other, "B");
    resolve(created());
    await Promise.resolve();
    expect(screen.queryByRole("status")).toBeNull();
    expect(other).toHaveValue("B");
  });

  it("drops the draft when the line changes, also through null (QA-003)", async () => {
    const { user, rerender, ...props } = setup();
    await user.type(field(), "for 96");
    rerender(<CommentComposer {...props} line={{ ...line96, target: { ...target96, line: 97 } }} />);
    expect(screen.getByRole("textbox", { name: /client\.rs:97/ })).toHaveValue("");
    await user.type(screen.getByRole("textbox"), "for 97");
    rerender(<CommentComposer {...props} line={null} />);
    rerender(<CommentComposer {...props} line={line96} />);
    expect(field()).toHaveValue("");
  });

  it("treats a change of only start_line as another target with its own key (QA-005)", async () => {
    const range = (start: number): CommentLine => ({ ...line96, target: { ...target96, start_line: start } });
    const onCreate = vi.fn().mockRejectedValueOnce(new Error("timeout")).mockResolvedValueOnce(created());
    const { user, rerender, ...props } = setup({ onCreate, line: range(90) });
    await user.type(screen.getByRole("textbox"), "range");
    await user.click(addButton());
    await screen.findByRole("alert");
    rerender(<CommentComposer {...props} onCreate={onCreate} line={range(93)} />);
    const fresh = screen.getByRole("textbox", { name: /client\.rs:93–96/ });
    expect(fresh).not.toHaveAttribute("readonly");
    expect(fresh).toHaveValue("");
    await user.type(fresh, "range");
    await user.click(addButton());
    const [a, b] = onCreate.mock.calls.map((c) => c[0] as CreateCommentRequest);
    expect(b!.idempotency_key).not.toBe(a!.idempotency_key);
    expect(b!.target.start_line).toBe(93);
  });

  it("says the placement is decided on posting when the preview cannot know it (QA-006)", () => {
    setup({ line: { ...line96, placement: "on_post" } });
    expect(document.body).toHaveTextContent(
      "Whether this becomes a line comment or a file comment is decided when it is posted.",
    );
  });

  it("shows the server's placement when it differs from the preview (QA-006)", async () => {
    const onCreate = vi.fn().mockResolvedValue(created({ comment: { ...missedComments[0]!, kind: "file" } }));
    const { user } = setup({ onCreate });
    await user.type(field(), "text");
    await user.click(addButton());
    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent("as a file comment that links to the line");
    expect(status).toHaveTextContent("not as the line comment previewed");
  });

  it("keeps an edited pending comment read-only while it is saved", async () => {
    const onEdit = vi.fn().mockReturnValue(new Promise(() => {}));
    const { user } = setup({ comments: missedComments, onEdit });
    const list = screen.getByRole("list", { name: "Your pending comments on this line" });
    await user.click(within(list).getByRole("button", { name: "Edit" }));
    await user.click(within(list).getByRole("button", { name: "Save" }));
    expect(within(list).getByRole("textbox", { name: "Edit comment" })).toHaveAttribute("readonly");
  });

  it("cancels on Escape and with the Cancel button", async () => {
    const { user, onCancel } = setup();
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalledTimes(2);
  });

  it("exposes the field for the `c` shortcut", () => {
    const ref = createRef<HTMLTextAreaElement>();
    setup({ textareaRef: ref });
    expect(ref.current).toBe(field());
  });

  it("lists the user's pending comments on the line (missed state) and edits one on GitHub", async () => {
    const { user, onEdit } = setup({ comments: missedComments });
    const list = screen.getByRole("list", { name: "Your pending comments on this line" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(1);
    expect(list).toHaveTextContent("Every attempt gets the full configured timeout");
    await user.click(within(list).getByRole("button", { name: "Edit" }));
    const edit = within(list).getByRole("textbox", { name: "Edit comment" });
    expect(edit).toHaveFocus();
    await user.clear(edit);
    await user.type(edit, "Shorter.");
    await user.click(within(list).getByRole("button", { name: "Save" }));
    expect(onEdit).toHaveBeenCalledWith("PRRC_fixture482_c1", "Shorter.");
  });

  it("deletes a pending comment on GitHub only after confirmation", async () => {
    const { user, onDelete } = setup({ comments: missedComments });
    const list = screen.getByRole("list", { name: "Your pending comments on this line" });
    await user.click(within(list).getByRole("button", { name: "Delete" }));
    expect(onDelete).not.toHaveBeenCalled();
    await user.click(within(list).getByRole("button", { name: "Keep" }));
    await user.click(within(list).getByRole("button", { name: "Delete" }));
    await user.click(within(list).getByRole("button", { name: "Delete from GitHub" }));
    expect(onDelete).toHaveBeenCalledWith("PRRC_fixture482_c1");
  });

  it("shows why an edit failed and keeps the edited text", async () => {
    const onEdit = vi.fn().mockRejectedValue(new Error("The PR changed; analyse it again."));
    const { user } = setup({ comments: missedComments, onEdit });
    const list = screen.getByRole("list", { name: "Your pending comments on this line" });
    await user.click(within(list).getByRole("button", { name: "Edit" }));
    await user.type(within(list).getByRole("textbox", { name: "Edit comment" }), "!");
    await user.click(within(list).getByRole("button", { name: "Save" }));
    expect(await within(list).findByRole("alert")).toHaveTextContent(
      "Not saved: The PR changed; analyse it again.",
    );
    expect(within(list).getByRole("textbox", { name: "Edit comment" })).toHaveValue(
      `${missedComments[0]!.body}!`,
    );
  });

  it("ignores pending comments on other lines", () => {
    const other: PendingComment = { ...missedComments[0]!, id: "c2", line: 97 };
    setup({ comments: [other] });
    expect(screen.queryByRole("list", { name: "Your pending comments on this line" })).toBeNull();
  });

  it("renders comment text as text, never as HTML or links (T12)", () => {
    const evil = '<img src=x onerror="alert(1)"> javascript:alert(1)';
    const { container } = setup({ comments: [{ ...missedComments[0]!, body: evil }] });
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("a")).toBeNull();
    expect(container).toHaveTextContent('<img src=x onerror="alert(1)">');
  });

  it.each(FIXTURE_STATES)("keeps the wording rules in state %s", (state) => {
    const review = seedFor(state, "pending-review") as PendingReview;
    for (const placement of ["line", "file", "not_in_diff"] as const) {
      const { container, unmount } = setup({ comments: review.comments, line: { ...line96, placement } });
      expectTrustWording(container.textContent ?? "");
      unmount();
    }
  });
});
