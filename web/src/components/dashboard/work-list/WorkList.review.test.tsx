import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { WorkItem } from "../../../api/generated/types";
import { ITEMS, NOW } from "../test-fixtures";
import { ALL_KINDS, WorkList } from "./WorkList";

const reviewHref = (item: WorkItem) => `/review?pr=${encodeURIComponent(item.url ?? "")}`;
const commentsHref = (item: WorkItem) => `/pr/${item.repo!.owner}/${item.repo!.name}/${item.number}/comments`;

// Owner 2026-11-23 (U4): "every pull request row shows N comments · M need you", or Watch without counts.
describe("WorkList: comments of a P0 alarm pull request (owner 2026-11-23)", () => {
  const nc482 = ITEMS.find((item) => item.id === "NC-482")!;
  const alarm = (comments: WorkItem["comments"]): WorkItem => ({
    ...nc482,
    score: { ...nc482.score, tier: "p0" },
    llm: nc482.llm && { ...nc482.llm, alarm: "Changes requested by a maintainer." },
    comments,
  });
  const list = (item: WorkItem) => (
    <WorkList
      items={[item]}
      kinds={ALL_KINDS}
      onKindsChange={vi.fn()}
      reviewHref={reviewHref}
      commentsHref={commentsHref}
      onWatch={vi.fn()}
      now={NOW}
    />
  );

  it("a P0 alarm pull request with counts links to its Comments like a ranked row", () => {
    render(list(alarm({ total: 12, needs_you: 5 })));
    expect(screen.getByRole("group", { name: "P0 alarm NC-482" })).toBeVisible();
    expect(
      screen.getByRole("link", { name: /^12 comments · 5 need you: comments of #482 / }),
    ).toHaveAttribute("href", "/pr/acme/node-client/482/comments");
  });

  it("a P0 alarm pull request without counts offers Watch like a ranked row", () => {
    render(list(alarm(undefined)));
    expect(screen.getByRole("button", { name: /^Watch #482 / })).toBeVisible();
  });
});

// RW1 refuses private repositories (`private_repository`: "diffpilot supports public repositories"), so a Watch on a
// private pull request row can never succeed.
describe("WorkList: Watch on a private pull request (RW1 reads public repositories only)", () => {
  it("a private pull request row without counts offers no Watch", () => {
    const nc482 = ITEMS.find((item) => item.id === "NC-482")!;
    const hidden: WorkItem = { ...nc482, private: true, comments: undefined };
    render(
      <WorkList
        items={[hidden]}
        kinds={ALL_KINDS}
        onKindsChange={vi.fn()}
        reviewHref={reviewHref}
        commentsHref={commentsHref}
        onWatch={vi.fn()}
        now={NOW}
      />,
    );
    expect(screen.queryByRole("button", { name: /^Watch #482 / })).toBeNull();
  });
});
