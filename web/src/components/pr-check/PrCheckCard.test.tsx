import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { PrSummary } from "../../api/generated/types";
import { PrCheckCard } from "./PrCheckCard";

const pr: PrSummary = {
  repo: { owner: "acme", name: "node-client" },
  number: 482,
  title: "feat(rpc): retry with backoff on timeout",
  author: "dana",
  html_url: "https://github.com/acme/node-client/pull/482",
  base_sha: "92f2274efbb93570cc8f820a49c058cf66b0dbe9",
  head_sha: "52d2da140191a9d47becf6a446310be803c007fc",
};

describe("PrCheckCard (Home: the result of Check)", () => {
  it("names the PR, scrolls into view and gives its primary action the focus", () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll as unknown as Element["scrollIntoView"];
    render(
      <PrCheckCard
        pr={pr}
        actions={
          <button type="button" data-primary>
            Start analysis
          </button>
        }
        onCancel={() => {}}
      >
        <p>Details</p>
      </PrCheckCard>,
    );
    const card = screen.getByRole("region", { name: "Pull request check" });
    expect(card).toHaveTextContent("acme/node-client #482 feat(rpc): retry with backoff on timeout");
    expect(card).toHaveTextContent("by dana");
    expect(screen.getByRole("button", { name: "Start analysis" })).toHaveFocus();
    expect(scroll).toHaveBeenCalled();
  });

  it("Cancel has a tooltip and clears the check", async () => {
    const onCancel = vi.fn();
    render(
      <PrCheckCard pr={null} onCancel={onCancel}>
        Not found.
      </PrCheckCard>,
    );
    const cancel = screen.getByRole("button", { name: "Cancel" });
    expect(cancel).toHaveAccessibleDescription(/nothing was started/i);
    await userEvent.click(cancel);
    expect(onCancel).toHaveBeenCalledOnce();
  });
});
