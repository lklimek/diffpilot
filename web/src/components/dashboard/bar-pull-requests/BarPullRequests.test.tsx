import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { BarPullRequests, type BarPullRequest } from "./BarPullRequests";

const pr = (id: string, extra: Partial<BarPullRequest> = {}): BarPullRequest => ({
  id,
  title: null,
  url: `https://github.com/${id.replace("#", "/pull/")}`,
  ...extra,
});

const OPENED = [pr("acme/platform#1210", { title: "feat: <b>bold</b> retry" }), pr("acme/docs#7")];
const CLOSED = [
  pr("acme/platform#1187", { title: "fix: race", outcome: "merged" }),
  pr("acme/consensus#99", { outcome: "closed_unmerged" }),
];

/** A row's text without its tooltips. */
const rowText = (li: HTMLElement) => {
  const copy = li.cloneNode(true) as HTMLElement;
  copy.querySelectorAll('[role="tooltip"]').forEach((tip) => tip.remove());
  return copy.textContent;
};

const renderList = (extra: Partial<Parameters<typeof BarPullRequests>[0]> = {}) => {
  const onHide = vi.fn();
  render(<BarPullRequests period="Tue 17 Nov" opened={OPENED} closed={CLOSED} onHide={onHide} {...extra} />);
  return { onHide };
};

describe("BarPullRequests", () => {
  it("names the bar and lists opened and closed pull requests with their counts", () => {
    renderList();
    const region = screen.getByRole("region", { name: "Pull requests of Tue 17 Nov" });
    expect(within(region).getByRole("heading", { name: "Pull requests of Tue 17 Nov" })).toBeVisible();
    const opened = within(region).getByRole("group", { name: "Opened (2)" });
    const closed = within(region).getByRole("group", { name: "Closed (2)" });
    expect(within(opened).getByText("Opened (2)")).toHaveAccessibleDescription("Pull requests you opened");
    expect(within(closed).getByText("Closed (2)")).toHaveAccessibleDescription(
      "Your pull requests merged or closed without merge",
    );
    expect(within(opened).getAllByRole("listitem").map(rowText)).toEqual([
      "acme/platform#1210 (opens on GitHub in a new tab)feat: <b>bold</b> retry",
      "acme/docs#7 (opens on GitHub in a new tab)",
    ]);
    expect(within(closed).getAllByRole("listitem").map(rowText)).toEqual([
      "acme/platform#1187 (opens on GitHub in a new tab)fix: racemerged",
      "acme/consensus#99 (opens on GitHub in a new tab)closed without merge",
    ]);
  });

  it("links each pull request to GitHub in a new tab with the GitHub mark", () => {
    renderList();
    const link = screen.getByRole("link", { name: /^acme\/platform#1187/ });
    expect(link).toHaveAttribute("href", "https://github.com/acme/platform/pull/1187");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    expect(link.querySelector('[data-icon="github"]')).not.toBeNull();
    expect(link).toHaveAccessibleDescription("Open acme/platform#1187 on GitHub (new tab)");
  });

  it("without a link the id stays text", () => {
    render(
      <BarPullRequests
        period="Tue 17 Nov"
        opened={[pr("odd", { url: null })]}
        closed={[]}
        onHide={() => {}}
      />,
    );
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.getByText("odd")).toBeVisible();
  });

  it("says so when a list is empty", () => {
    render(<BarPullRequests period="Week of 16 Nov" opened={[]} closed={CLOSED} onHide={() => {}} />);
    expect(within(screen.getByRole("group", { name: "Opened (0)" })).getByText("None.")).toBeVisible();
  });

  it("Hide in its top-right corner closes it and returns the focus to the chart by keyboard", async () => {
    const chart = document.createElement("button");
    document.body.append(chart);
    const { onHide } = renderList({ focusAfter: () => chart });
    const hide = screen.getByRole("button", { name: "Hide pull requests of Tue 17 Nov" });
    expect(hide).toHaveTextContent(/^Hide$/);
    expect(hide).toHaveAccessibleDescription(/Hides this list/);
    hide.focus();
    await userEvent.keyboard("{Enter}");
    expect(onHide).toHaveBeenCalledOnce();
    expect(chart).toHaveFocus();
    chart.remove();
  });
});
