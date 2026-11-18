import { render, screen } from "@testing-library/react";
import { useState } from "react";
import { expect, it, vi } from "vitest";
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
}: {
  prefill: { text: string } | null;
  drafts: CommentDrafts;
  onPrefilled: () => void;
}) {
  const [unresolved, setUnresolved] = useState<UnresolvedPosts>({});
  return (
    <CommentComposer
      available
      line={line}
      comments={[]}
      onCreate={vi.fn<() => Promise<CreateCommentResponse>>()}
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
