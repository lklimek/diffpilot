import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import type { MyAnalysis } from "../../api/generated/types";
import { RecentReviews } from "./RecentReviews";

const NOW = new Date("2026-11-18T12:00:00Z");
const row = (over: Partial<MyAnalysis>): MyAnalysis => ({
  id: "6d3a2f1e-0482-4c5e-9a7b-000000000482",
  pr: { repo: { owner: "acme", name: "node-client" }, number: 482 },
  title: "Retry RPC calls",
  status: "finished",
  created_at: "2026-11-18T11:48:00Z",
  ...over,
});
const href = (analysis: MyAnalysis) => `/analyses/${analysis.id}`;

describe("RecentReviews", () => {
  it("lists one row per analysis: PR, title as text, status and when, linking to it", async () => {
    const onNavigate = vi.fn();
    const rows = [
      row({}),
      row({
        id: "00000000-0000-4000-8000-000000000096",
        pr: { repo: { owner: "acme", name: "wallet-ui" }, number: 96 },
        title: "<b>split</b> validation",
        status: "running",
        created_at: "2026-11-16T09:00:00Z",
      }),
    ];
    render(<RecentReviews analyses={rows} error={null} hrefOf={href} now={NOW} onNavigate={onNavigate} />);
    const section = screen.getByRole("region", { name: "Recent reviews" });
    const items = within(section).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    const [one, two] = items as [HTMLElement, HTMLElement];
    const first = within(one).getByRole("link", { name: "acme/node-client #482 Retry RPC calls" });
    expect(first).toHaveAttribute("href", `/analyses/${rows[0]!.id}`);
    expect(one).toHaveTextContent("Analysed");
    expect(one).toHaveTextContent("12 min ago");
    expect(within(one).getByText("12 min ago").closest("time")).toHaveAttribute(
      "datetime",
      "2026-11-18T11:48:00Z",
    );
    // An untrusted title is text, never markup.
    expect(two).toHaveTextContent("<b>split</b> validation");
    expect(two.querySelector("b")).toBeNull();
    expect(two).toHaveTextContent("Analysing flows");
    expect(two).toHaveTextContent("2 d ago");
    await userEvent.click(first);
    expect(onNavigate).toHaveBeenCalledWith(`/analyses/${rows[0]!.id}`);
    expectTrustWording(section.textContent ?? "");
  });

  it.each([
    ["planning", "Planning flows"],
    ["running", "Analysing flows"],
    ["paused", "Paused"],
    ["failed", "Analysis failed"],
    ["finished", "Analysed"],
    ["cancelled", "Analysis cancelled"],
  ] as const)("status %s reads %s", (status, text) => {
    render(<RecentReviews analyses={[row({ status })]} error={null} hrefOf={href} now={NOW} />);
    expect(screen.getByRole("listitem")).toHaveTextContent(text);
  });

  it("empty, loading and error states", () => {
    const { rerender } = render(<RecentReviews analyses={[]} error={null} hrefOf={href} now={NOW} />);
    expect(screen.getByRole("status")).toHaveTextContent(
      "No reviews yet. Paste a PR URL above to start. Analyses you start will be listed here.",
    );
    rerender(<RecentReviews analyses={undefined} error={null} hrefOf={href} now={NOW} />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading recent reviews…");
    rerender(<RecentReviews analyses={undefined} error="The server is busy." hrefOf={href} now={NOW} />);
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Recent reviews could not be loaded. The server is busy.",
    );
    expect(screen.getByRole("region", { name: "Recent reviews" })).toBeVisible();
  });
});
