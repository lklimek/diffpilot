import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { TopBar } from "./TopBar";

const PR = {
  repo: { owner: "acme", name: "node-client" },
  number: 482,
  title: "feat(rpc): retry",
  html_url: "https://github.com/acme/node-client/pull/482",
  head_sha: "52d2da140191a9d47becf6a446310be803c007fc",
};

describe("TopBar view menu: Comments (review-workflow-plan §3)", () => {
  it("offers Comments beside Flows, Files and Dependencies", async () => {
    const onNavigate = vi.fn();
    render(
      <TopBar
        pr={PR}

        view={{
          current: "flows",
          flowsHref: "/analyses/a",
          filesHref: "/analyses/a/files",
          depsHref: "/analyses/a/deps",
          commentsHref: "/pr/acme/node-client/482/comments",
          prContextHref: "/analyses/a#pr_context",
          onNavigate,
        }}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "View: Flows" }));
    const comments = screen.getByRole("link", { name: "Comments" });
    expect(comments).toHaveAttribute("href", "/pr/acme/node-client/482/comments");
    await userEvent.click(comments);
    expect(onNavigate).toHaveBeenCalledWith("/pr/acme/node-client/482/comments");
  });

  it("without an analysis the other views say they need one and lead to the PR page", async () => {
    render(
      <TopBar
        pr={PR}

        view={{
          current: "comments",
          flowsHref: null,
          filesHref: null,
          depsHref: null,
          commentsHref: "/pr/acme/node-client/482/comments",
          prPageHref: "/pr/acme/node-client/482",
          prContextHref: "/pr/acme/node-client/482",
          onNavigate: vi.fn(),
        }}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "View: Comments" }));
    const list = screen.getByRole("list");
    const flows = within(list).getByRole("link", { name: /Flows/ });
    expect(flows).toHaveTextContent("Needs an analysis");
    expect(flows).toHaveAttribute("href", "/pr/acme/node-client/482");
    expect(within(list).getByRole("link", { name: "Comments" })).toHaveAttribute("aria-current", "page");
  });
});
