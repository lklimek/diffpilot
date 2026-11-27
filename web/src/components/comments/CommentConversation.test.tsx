import { render, screen, within } from "@testing-library/react";
import { expect, it } from "vitest";
import { CommentConversation } from "./CommentConversation";

it("keeps an agent annotation beside the exact original comment, including a reply", () => {
  const root = "https://github.com/acme/node-client/pull/482#discussion_r1";
  const reply = "https://github.com/acme/node-client/pull/482#discussion_r2";
  const created_at = "2026-11-27T12:00:00Z";
  render(
    <CommentConversation
      heading="Review thread"
      source="GitHub + agent"
      comments={[
        { author: "bob", body: "Original finding", url: root, created_at },
        { author: "alice", body: "Original reply", url: reply, created_at },
      ]}
      annotations={[
        { id: "agent-annotation", target_url: reply, agent: "Review helper", body: "Agent note", created_at },
      ]}
    />,
  );
  const source = screen.getByText("Original reply").closest("li")!;
  expect(within(source).getByText("Review helper · agent")).toBeVisible();
  expect(within(source).getByText("Agent note")).toBeVisible();
  expect(within(screen.getByText("Original finding").closest("li")!).queryByText("Agent note")).toBeNull();
});
