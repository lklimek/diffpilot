import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { CommentTarget, CreateCommentResponse } from "../../api/generated/types";
import {
  CommentComposer,
  type CommentDrafts,
  type CommentLine,
  type UnresolvedPosts,
} from "./CommentComposer";

const target: CommentTarget = { path: "src/rpc/client.rs", side: "head", start_line: 96, line: 99 };
const line: CommentLine = {
  target,
  placement: "line",
  permalink: "https://github.com/a/b/blob/c/src/rpc/client.rs#L96",
};

function store(initial: Record<string, string> = {}): CommentDrafts {
  const all = new Map(Object.entries(initial));
  return { get: (k) => all.get(k) ?? "", set: (k, t) => (t ? all.set(k, t) : all.delete(k)) };
}

function Harness({
  prefill,
  drafts,
  onPrefilled,
  onCreate = vi.fn<() => Promise<CreateCommentResponse>>(),
}: {
  prefill: { text: string } | null;
  drafts: CommentDrafts;
  onPrefilled: () => void;
  onCreate?: () => Promise<CreateCommentResponse>;
}) {
  const [unresolved, setUnresolved] = useState<UnresolvedPosts>({});
  return (
    <CommentComposer
      available
      line={line}
      comments={[]}
      onCreate={onCreate}
      onEdit={() => {}}
      onDelete={() => {}}
      onCancel={() => {}}
      unresolved={unresolved}
      onUnresolvedChange={setUnresolved}
      drafts={drafts}
      prefill={prefill}
      onPrefilled={onPrefilled}
    />
  );
}

const field = () => screen.getByRole("textbox", { name: /^Comment on src\/rpc\/client\.rs/ });

it("a prefill opens as an editable draft on the line, focused, and is never sent by itself", () => {
  const onPrefilled = vi.fn();
  render(
    <Harness
      prefill={{ text: "LLM-suggested — review before posting\n\nX" }}
      drafts={store()}
      onPrefilled={onPrefilled}
    />,
  );
  expect(field()).toHaveValue("LLM-suggested — review before posting\n\nX");
  expect(field()).toHaveFocus();
  expect(onPrefilled).toHaveBeenCalledOnce();
});

it("appends to a draft already open on the line after a blank line", async () => {
  const drafts = store();
  const { rerender } = render(<Harness prefill={null} drafts={drafts} onPrefilled={() => {}} />);
  const { default: userEvent } = await import("@testing-library/user-event");
  await userEvent.type(field(), "My own note");
  rerender(<Harness prefill={{ text: "Suggested" }} drafts={drafts} onPrefilled={() => {}} />);
  expect(field()).toHaveValue("My own note\n\nSuggested");
});

describe("a prefill while a post on the line is unsettled is kept (shown once the post settles)", () => {
  const unknown = () => Object.assign(new Error("GitHub did not answer."), { code: "github_unavailable" });
  const createdComment = {
    comment: { id: 1, kind: "line" },
    missed_by_llm_recorded: false,
  } as unknown as CreateCommentResponse;
  const add = () => screen.getByRole("button", { name: "Add to GitHub pending review" });

  async function failedPost(onCreate: () => Promise<CreateCommentResponse>) {
    const drafts = store();
    const view = render(
      <Harness prefill={null} drafts={drafts} onPrefilled={() => {}} onCreate={onCreate} />,
    );
    const user = userEvent.setup();
    await user.type(field(), "Posted text");
    await user.click(add());
    return { ...view, drafts, user, onCreate };
  }

  it("Discard restores the posted text with the suggestion after it", async () => {
    const { rerender, drafts, user } = await failedPost(vi.fn().mockRejectedValue(unknown()));
    await screen.findByRole("button", { name: "Retry" });
    rerender(<Harness prefill={{ text: "Suggested" }} drafts={drafts} onPrefilled={() => {}} />);
    expect(field()).toHaveValue("Posted text");
    await user.click(screen.getByRole("button", { name: "Discard" }));
    expect(field()).toHaveValue("Posted text\n\nSuggested");
  });

  it("a successful Retry leaves the suggestion as the new draft and confirms the post", async () => {
    const onCreate = vi.fn().mockRejectedValueOnce(unknown()).mockResolvedValue(createdComment);
    const { rerender, drafts, user } = await failedPost(onCreate);
    await screen.findByRole("button", { name: "Retry" });
    rerender(
      <Harness prefill={{ text: "Suggested" }} drafts={drafts} onPrefilled={() => {}} onCreate={onCreate} />,
    );
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Added to your GitHub pending review");
    expect(field()).toHaveValue("Suggested");
  });

  it("a post in flight: after success the suggestion is the draft; after a refusal it follows the text", async () => {
    let answer!: (r: CreateCommentResponse) => void;
    const ok = vi.fn(() => new Promise<CreateCommentResponse>((resolve) => (answer = resolve)));
    const first = await failedPost(ok);
    first.rerender(
      <Harness prefill={{ text: "Suggested" }} drafts={first.drafts} onPrefilled={() => {}} onCreate={ok} />,
    );
    await act(async () => answer(createdComment));
    expect(await screen.findByRole("status")).toHaveTextContent("Added to your GitHub pending review");
    expect(field()).toHaveValue("Suggested");
    first.unmount();

    let refuse!: (e: Error) => void;
    const refused = vi.fn(() => new Promise<CreateCommentResponse>((_, reject) => (refuse = reject)));
    const second = await failedPost(refused);
    second.rerender(
      <Harness
        prefill={{ text: "Suggested" }}
        drafts={second.drafts}
        onPrefilled={() => {}}
        onCreate={refused}
      />,
    );
    await act(async () =>
      refuse(Object.assign(new Error("The comment is too long."), { code: "invalid_request" })),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("The comment is too long.");
    expect(field()).toHaveValue("Posted text\n\nSuggested");
  });
});
