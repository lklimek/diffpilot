import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef, useState } from "react";
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
import {
  CommentComposer,
  type CommentComposerProps,
  type CommentDrafts,
  type CommentLine,
  type UnresolvedPosts,
} from "./CommentComposer";

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

type HarnessProps = Omit<CommentComposerProps, "unresolved" | "onUnresolvedChange" | "drafts"> & {
  mounted?: boolean;
  drafts?: CommentDrafts & { all: Map<string, string> };
};

/** Drafts kept outside React state, as the screen keeps them; `all` shows what is stored. */
function draftStore(): CommentDrafts & { all: Map<string, string> } {
  const all = new Map<string, string>();
  return {
    all,
    get: (key) => all.get(key) ?? "",
    set: (key, text) => {
      if (text === "") all.delete(key);
      else all.set(key, text);
    },
  };
}

/** The screen's role: keeps unresolved posts and drafts for the session while the composer mounts and unmounts. */
function Harness({ mounted = true, drafts: given, ...props }: HarnessProps) {
  const [unresolved, setUnresolved] = useState<UnresolvedPosts>({});
  const [drafts] = useState(() => given ?? draftStore());
  return mounted ? (
    <CommentComposer {...props} unresolved={unresolved} onUnresolvedChange={setUnresolved} drafts={drafts} />
  ) : null;
}

function setup(props: Partial<HarnessProps> = {}) {
  const all: HarnessProps & { drafts: ReturnType<typeof draftStore> } = {
    drafts: draftStore(),
    available: true,
    line: line96,
    comments: [],
    onCreate: vi.fn().mockResolvedValue(created()),
    onEdit: vi.fn().mockResolvedValue(undefined),
    onDelete: vi.fn().mockResolvedValue(undefined),
    onCancel: vi.fn(),
    ...props,
  };
  const view = render(<Harness {...all} />);
  return { ...all, ...view, user: userEvent.setup() };
}

const field = () => screen.getByRole("textbox", { name: /Comment on src\/rpc\/client\.rs:96/ });
const addButton = () => screen.getByRole("button", { name: "Add to GitHub pending review" });

describe("CommentComposer", () => {
  it("replaces the composer by a plain explanation when commenting is not available", () => {
    setup({ available: false });
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
    expect(document.body).toHaveTextContent("Commenting is not available for this repository right now.");
    // The server cannot tell a missing App from a failed check (commenting_available is one boolean).
    expect(document.body).not.toHaveTextContent(/App is not installed/);
    expect(document.body).toHaveTextContent(
      "The smart-review GitHub App may not be installed on it, or its installation could not be checked.",
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
    expect(addButton()).not.toHaveAttribute("aria-disabled");
    expect(addButton()).toHaveAccessibleDescription(
      "Adds this comment to your pending review on GitHub; nothing is published. You submit the review on GitHub.",
    );
    expect(document.body).not.toHaveTextContent("posted as a file comment");
  });

  it("notes that a line outside the diff is posted as a file comment", () => {
    setup({ line: { ...line96, placement: "file" } });
    expect(document.body).toHaveTextContent(
      "This line is outside the PR diff, so the comment is posted as a file comment whose text links to the line.",
    );
    expect(addButton()).toBeInTheDocument();
  });

  it("offers the GitHub link instead of a field for a file outside the PR diff, without the raw URL", () => {
    const permalink = "https://github.com/acme/node-client/blob/52d2da1/src/sync/peers.rs#L20";
    setup({
      line: {
        target: { path: "src/sync/peers.rs", side: "head", start_line: null, line: 20 },
        placement: "not_in_diff",
        permalink,
      },
    });
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(document.body).toHaveTextContent("On src/sync/peers.rs:20. This file is not part of the PR diff");
    expect(screen.getByRole("link", { name: "line 20 on GitHub ↗" })).toHaveAttribute("href", permalink);
    expect(document.body).not.toHaveTextContent("https://");
  });

  it("a comment_published answer shows the server's sentence, keeps the text and offers no Retry", async () => {
    const sentence =
      "The pull request changed while the comment was being saved, and your review was published on GitHub in the meantime. The comment had been published on other code and was removed again. Analyse the pull request again to comment.";
    const onCreate = vi
      .fn()
      .mockRejectedValue(Object.assign(new Error(sentence), { code: "comment_published" }));
    const { user } = setup({ onCreate });
    await user.type(
      screen.getByRole("textbox", { name: /^Comment on/ }),
      "Each attempt gets the full timeout.",
    );
    await user.click(addButton());
    expect(await screen.findByRole("alert")).toHaveTextContent(sentence);
    expect(screen.getByRole("textbox", { name: /^Comment on/ })).toHaveValue(
      "Each attempt gets the full timeout.",
    );
    expect(screen.getByRole("textbox", { name: /^Comment on/ })).not.toHaveAttribute("readonly");
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    expect(document.body).not.toHaveTextContent(/may already be in your GitHub pending review/);
  });

  it.each([
    ["pr_head_changed", "The pull request changed since this analysis. Analyse it again to comment."],
    ["position_rejected", "GitHub did not accept a comment on this line."],
    ["app_not_installed", "The smart-review GitHub App is not installed on this repository."],
    ["invalid_request", "The comment is too long."],
  ])(
    "a definite refusal (%s) states the reason, keeps the text editable and offers no Retry",
    async (code, sentence) => {
      const onCreate = vi.fn().mockRejectedValue(Object.assign(new Error(sentence), { code }));
      const { user } = setup({ onCreate });
      await user.type(field(), "kept text");
      await user.click(addButton());
      expect(await screen.findByRole("alert")).toHaveTextContent(sentence);
      expect(field()).toHaveValue("kept text");
      expect(field()).not.toHaveAttribute("readonly");
      expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
      expect(document.body).not.toHaveTextContent(/may already be in your GitHub pending review/);
    },
  );

  it("an unknown outcome (github_unavailable) keeps Retry with the same key", async () => {
    const onCreate = vi
      .fn()
      .mockRejectedValue(Object.assign(new Error("GitHub did not answer."), { code: "github_unavailable" }));
    const { user } = setup({ onCreate });
    await user.type(field(), "text");
    await user.click(addButton());
    expect(await screen.findByRole("button", { name: "Retry" })).toBeVisible();
    expect(document.body).toHaveTextContent("The comment may already be in your GitHub pending review");
  });

  it("keeps no stored draft while the outcome is unknown: a reload cannot offer the text for a second Add", async () => {
    const onCreate = vi
      .fn()
      .mockRejectedValue(Object.assign(new Error("GitHub did not answer."), { code: "github_unavailable" }));
    const { user, drafts } = setup({ onCreate });
    await user.type(field(), "maybe added");
    expect([...drafts.all.values()]).toEqual(["maybe added"]);
    await user.click(addButton());
    await screen.findByRole("button", { name: "Retry" });
    expect(drafts.all.size).toBe(0);
    expect(field()).toHaveValue("maybe added");
    // Discard is the explicit way back to an editable (and kept) text.
    await user.click(screen.getByRole("button", { name: "Discard" }));
    expect([...drafts.all.values()]).toEqual(["maybe added"]);
  });

  it.each([
    ["position_rejected", "GitHub did not accept a comment on this line."],
    ["unauthenticated", "Sign in again to continue."],
  ])("keeps the stored draft after a definite refusal (%s)", async (code, sentence) => {
    const onCreate = vi.fn().mockRejectedValue(Object.assign(new Error(sentence), { code }));
    const { user, drafts } = setup({ onCreate });
    await user.type(field(), "kept text");
    await user.click(addButton());
    await screen.findByRole("alert");
    expect([...drafts.all.values()]).toEqual(["kept text"]);
  });

  it("after a head-moved refusal links to analysing the PR again, when the screen offers it", async () => {
    const sentence = "The pull request changed since this analysis. Analyse it again to comment.";
    const onCreate = vi
      .fn()
      .mockRejectedValue(Object.assign(new Error(sentence), { code: "pr_head_changed" }));
    const { user } = setup({
      onCreate,
      reanalyseHref: "/?pr=https%3A%2F%2Fgithub.com%2Facme%2Fnode-client%2Fpull%2F482",
    });
    await user.type(field(), "text");
    await user.click(addButton());
    await screen.findByRole("alert");
    const link = screen.getByRole("link", { name: "Analyse the new head" });
    expect(link).toHaveAttribute("href", "/?pr=https%3A%2F%2Fgithub.com%2Facme%2Fnode-client%2Fpull%2F482");
    expect(link).toHaveAccessibleDescription(/Home/);
  });

  it("offers no re-analyse link for other refusals", async () => {
    const onCreate = vi
      .fn()
      .mockRejectedValue(Object.assign(new Error("Too long."), { code: "invalid_request" }));
    const { user } = setup({ onCreate, reanalyseHref: "/?pr=x" });
    await user.type(field(), "text");
    await user.click(addButton());
    await screen.findByRole("alert");
    expect(screen.queryByRole("link", { name: "Analyse the new head" })).toBeNull();
  });

  it("an edit answered comment_published shows the server's sentence, not 'Not saved'", async () => {
    const sentence =
      "This comment was published on GitHub in the meantime; the change was applied to the published comment.";
    const onEdit = vi
      .fn()
      .mockRejectedValue(Object.assign(new Error(sentence), { code: "comment_published" }));
    const { user } = setup({ comments: missedComments, onEdit });
    await user.click(screen.getByRole("button", { name: "Edit" }));
    await user.type(screen.getByRole("textbox", { name: "Edit comment" }), " more");
    await user.click(screen.getByRole("button", { name: "Save" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(sentence);
    expect(alert).not.toHaveTextContent("Not saved");
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

  it("keeps an unresolved post after a failure: whitespace cannot change it, Retry reuses key and payload", async () => {
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

  it("keeps the unresolved post when leaving and re-entering the line", async () => {
    const onCreate = vi.fn().mockRejectedValueOnce(new Error("timeout")).mockResolvedValueOnce(created());
    const { user, rerender, ...props } = setup({ onCreate });
    await user.type(field(), "text");
    await user.click(addButton());
    await screen.findByRole("alert");
    rerender(
      <Harness {...props} onCreate={onCreate} line={{ ...line96, target: { ...target96, line: 97 } }} />,
    );
    expect(screen.getByRole("textbox", { name: /client\.rs:97/ })).toHaveValue("");
    rerender(<Harness {...props} onCreate={onCreate} line={null} />);
    rerender(<Harness {...props} onCreate={onCreate} line={line96} />);
    expect(field()).toHaveValue("text");
    await user.click(screen.getByRole("button", { name: "Retry" }));
    const keys = onCreate.mock.calls.map((c) => (c[0] as CreateCommentRequest).idempotency_key);
    expect(keys[1]).toBe(keys[0]);
  });

  it("offers an unresolved post again after the composer was unmounted and remounted", async () => {
    const onCreate = vi.fn().mockRejectedValueOnce(new Error("timeout")).mockResolvedValueOnce(created());
    const { user, rerender, ...props } = setup({ onCreate });
    await user.type(field(), "text");
    await user.click(addButton());
    await screen.findByRole("alert");
    rerender(<Harness {...props} onCreate={onCreate} mounted={false} />);
    rerender(<Harness {...props} onCreate={onCreate} mounted />);
    expect(field()).toHaveAttribute("readonly");
    expect(field()).toHaveValue("text");
    await user.click(screen.getByRole("button", { name: "Retry" }));
    const [first, second] = onCreate.mock.calls.map((c) => c[0] as CreateCommentRequest);
    expect(second).toEqual(first);
  });

  it("keeps a post that was pending at unmount locked after remount, never posting it with a new key", async () => {
    let reject!: (e: unknown) => void;
    const onCreate = vi
      .fn()
      .mockReturnValueOnce(new Promise<CreateCommentResponse>((_, r) => (reject = r)))
      .mockResolvedValueOnce(created());
    const { user, rerender, ...props } = setup({ onCreate });
    await user.type(field(), "text");
    await user.click(addButton());
    rerender(<Harness {...props} onCreate={onCreate} mounted={false} />);
    rerender(<Harness {...props} onCreate={onCreate} mounted />);
    expect(field()).toHaveAttribute("readonly");
    expect(field()).toHaveValue("text");
    const adding = screen.getByRole("button", { name: /Adding/ });
    expect(adding).toHaveAttribute("aria-disabled", "true");
    expect(adding).toHaveAccessibleDescription(/Not available while the current request runs\./);
    await user.click(adding);
    expect(onCreate).toHaveBeenCalledOnce();
    reject(new Error("timeout"));
    await user.click(await screen.findByRole("button", { name: "Retry" }));
    const [first, second] = onCreate.mock.calls.map((c) => c[0] as CreateCommentRequest);
    expect(second).toEqual(first);
  });

  it("discards an unresolved post into an editable draft that posts with a new key", async () => {
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

  it("keeps the field read-only while posting and clears only what was sent", async () => {
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

  it("ignores a completion for a line that is no longer current", async () => {
    let resolve!: (r: CreateCommentResponse) => void;
    const onCreate = vi.fn().mockReturnValue(new Promise<CreateCommentResponse>((r) => (resolve = r)));
    const { user, rerender, ...props } = setup({ onCreate });
    await user.type(field(), "A");
    await user.click(addButton());
    rerender(
      <Harness {...props} onCreate={onCreate} line={{ ...line96, target: { ...target96, line: 97 } }} />,
    );
    const other = screen.getByRole("textbox", { name: /client\.rs:97/ });
    await user.type(other, "B");
    resolve(created());
    await Promise.resolve();
    expect(screen.queryByRole("status")).toBeNull();
    expect(other).toHaveValue("B");
  });

  it("keeps a draft per line: another line starts empty, returning restores it, marked as a draft", async () => {
    const { user, rerender, ...props } = setup();
    expect(screen.queryByText("Draft")).toBeNull();
    await user.type(field(), "for 96");
    expect(screen.getByText("Draft")).toHaveAccessibleDescription(/Not added to GitHub yet/);
    rerender(<Harness {...props} line={{ ...line96, target: { ...target96, line: 97 } }} />);
    expect(screen.getByRole("textbox", { name: /client\.rs:97/ })).toHaveValue("");
    await user.type(screen.getByRole("textbox"), "for 97");
    rerender(<Harness {...props} line={null} />);
    rerender(<Harness {...props} line={line96} />);
    expect(field()).toHaveValue("for 96");
    expect(screen.getByText("Draft")).toBeVisible();
  });

  it("keeps the draft after Close; an added comment clears it", async () => {
    const { user, rerender, ...props } = setup();
    await user.type(field(), "kept");
    const close = screen.getByRole("button", { name: "Close" });
    expect(close).toHaveAccessibleDescription(/text not added yet is kept/);
    await user.click(close);
    rerender(<Harness {...props} line={null} />);
    rerender(<Harness {...props} line={line96} />);
    expect(field()).toHaveValue("kept");
    await user.click(addButton());
    await screen.findByRole("status");
    rerender(<Harness {...props} line={null} />);
    rerender(<Harness {...props} line={line96} />);
    expect(field()).toHaveValue("");
    expect(screen.queryByText("Draft")).toBeNull();
  });

  it("treats a change of only start_line as another target with its own key", async () => {
    const range = (start: number): CommentLine => ({ ...line96, target: { ...target96, start_line: start } });
    const onCreate = vi.fn().mockRejectedValueOnce(new Error("timeout")).mockResolvedValueOnce(created());
    const { user, rerender, ...props } = setup({ onCreate, line: range(90) });
    await user.type(screen.getByRole("textbox"), "range");
    await user.click(addButton());
    await screen.findByRole("alert");
    rerender(<Harness {...props} onCreate={onCreate} line={range(93)} />);
    const fresh = screen.getByRole("textbox", { name: /client\.rs:93–96/ });
    expect(fresh).not.toHaveAttribute("readonly");
    expect(fresh).toHaveValue("");
    await user.type(fresh, "range");
    await user.click(addButton());
    const [a, b] = onCreate.mock.calls.map((c) => c[0] as CreateCommentRequest);
    expect(b!.idempotency_key).not.toBe(a!.idempotency_key);
    expect(b!.target.start_line).toBe(93);
  });

  it("says the placement is decided on posting when the preview cannot know it", () => {
    setup({ line: { ...line96, placement: "on_post" } });
    expect(document.body).toHaveTextContent(
      "Whether this becomes a line comment or a file comment is decided when it is posted.",
    );
  });

  it("shows the server's placement when it differs from the preview", async () => {
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

  it("closes on Escape and with the Close button", async () => {
    const { user, onCancel } = setup();
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(onCancel).toHaveBeenCalledTimes(2);
  });

  it("explains every button in a tooltip (brief §4 rule 3)", async () => {
    const onCreate = vi.fn().mockRejectedValue(new Error("GitHub is not reachable."));
    const { user, unmount } = setup({ comments: missedComments, onCreate });
    const tips = new Map<string, string>();
    const collect = () => {
      for (const button of screen.getAllByRole("button")) {
        expect(button, button.textContent ?? "").toHaveAccessibleDescription(/\S/);
        tips.set(button.textContent ?? "", button.getAttribute("aria-describedby") ?? "");
      }
    };
    collect();
    const list = screen.getByRole("list", { name: "Your pending comments on this line" });
    await user.click(within(list).getByRole("button", { name: "Delete" }));
    collect();
    await user.click(within(list).getByRole("button", { name: "Keep" }));
    await user.click(within(list).getByRole("button", { name: "Edit" }));
    collect();
    await user.type(field(), "text");
    await user.click(addButton());
    await screen.findByRole("button", { name: "Retry" });
    collect();
    expect([...tips.keys()].sort()).toEqual(
      [
        "Add to GitHub pending review",
        "Cancel",
        "Close",
        "Delete",
        "Delete from GitHub",
        "Discard",
        "Edit",
        "Keep",
        "Retry",
        "Save",
      ].sort(),
    );
    expect(within(list).getByRole("button", { name: "Save" })).toHaveAccessibleDescription(
      /pending review on GitHub; nothing is published/,
    );
    unmount();
    setup({ line: { ...line96, placement: "not_in_diff" } });
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveAccessibleDescription(
      /nothing is sent to GitHub/,
    );
  });

  it("keeps Save and Delete from GitHub focusable while their request runs, with the reason in the tip", async () => {
    const onEdit = vi.fn().mockReturnValue(new Promise(() => {}));
    const onDelete = vi.fn().mockReturnValue(new Promise(() => {}));
    const { user } = setup({ comments: missedComments, onEdit, onDelete });
    const list = screen.getByRole("list", { name: "Your pending comments on this line" });
    await user.click(within(list).getByRole("button", { name: "Edit" }));
    const save = within(list).getByRole("button", { name: "Save" });
    await user.click(save);
    await user.click(save);
    expect(onEdit).toHaveBeenCalledOnce();
    expect(save).not.toBeDisabled();
    expect(save).toHaveAttribute("aria-disabled", "true");
    expect(save).toHaveAccessibleDescription(/Not available while the current request runs\./);
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
