import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Tour } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { seedFor } from "../../mocks/fixtures";
import { TopBar } from "./TopBar";

const tour = seedFor("default", "tour") as Tour;

describe("TopBar", () => {
  it("shows repo, PR number (new tab), title, head SHA and CI with failing checks in the tooltip", () => {
    render(<TopBar pr={tour.analysis.pr} ci={tour.pr_context.facts.ci} login="alice" />);
    const banner = screen.getByRole("banner");
    const prLink = within(banner).getByRole("link", { name: "#482 (opens on GitHub in a new tab)" });
    expect(
      within(banner).getByRole("link", { name: "acme/node-client (opens on GitHub in a new tab)" }),
    ).toHaveAttribute("href", "https://github.com/acme/node-client");
    expect(prLink).toHaveAttribute("href", "https://github.com/acme/node-client/pull/482");
    expect(prLink).toHaveAttribute("target", "_blank");
    expect(prLink).toHaveAttribute("rel", expect.stringContaining("noopener"));
    expect(banner).toHaveTextContent("feat(rpc): retry with backoff on timeout");
    expect(banner).toHaveTextContent(tour.analysis.pr.head_sha.slice(0, 7));
    const ci = within(banner).getByText(
      "Check runs: 3 passed, 1 failed, 1 skipped, 1 neutral. Statuses: 1 pending.",
    );
    expect(ci).toHaveAccessibleDescription(/test \(integration\)/);
    expect(banner).toHaveTextContent("alice");
    expectTrustWording(banner.textContent ?? "");
  });

  it("tags the analysis with the language of its LLM texts", () => {
    render(<TopBar pr={tour.analysis.pr} language="pl" login="alice" />);
    expect(within(screen.getByRole("banner")).getByText("PL")).toHaveAccessibleDescription(
      "LLM texts of this analysis are in Polish.",
    );
  });

  it("marks an incomplete pending-review count and says why in the tooltip", () => {
    render(<TopBar login="alice" pendingCount="1+" onOpenPending={() => {}} />);
    const button = screen.getByRole("button", { name: /Pending review/ });
    expect(button).toHaveTextContent("Pending review 1+");
    expect(button).toHaveAccessibleDescription(
      /More pending comments exist on GitHub than are listed here\./,
    );
  });

  it("works without a PR (home)", () => {
    render(<TopBar login={null} />);
    expect(screen.getByRole("link", { name: "diffpilot" })).toHaveAttribute("href", "/");
  });

  it("links the CI summary to the checks page it is given, in a new tab, with its tooltip", () => {
    const href = "https://github.com/acme/node-client/pull/482/checks";
    render(<TopBar pr={tour.analysis.pr} ci={tour.pr_context.facts.ci} checksHref={href} login="alice" />);
    const link = screen.getByRole("link", { name: /^Check runs: .* \(opens on GitHub in a new tab\)$/ });
    expect(link.querySelector('svg[data-icon="github"]')).not.toBeNull();
    expect(link).toHaveAttribute("href", href);
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    expect(link).toHaveAccessibleDescription(/^Open the checks on GitHub \(new tab\)/);
  });

  it("review views: breadcrumb diffpilot › repo #n (PR context) with its own GitHub link › view menu", async () => {
    const onNavigate = vi.fn();
    const view = {
      current: "files" as const,
      flowsHref: "/analyses/a",
      filesHref: "/analyses/a/files",
      prContextHref: "/analyses/a#pr_context",
      onNavigate,
    };
    render(<TopBar pr={tour.analysis.pr} login="alice" view={view} progress={{ reviewed: 1, total: 5 }} />);
    const crumbs = screen.getByRole("navigation", { name: "Breadcrumb" });
    expect(within(crumbs).getByRole("link", { name: "diffpilot" })).toHaveAttribute("href", "/");
    const repo = within(crumbs).getByRole("link", { name: "acme/node-client #482" });
    expect(repo).toHaveAttribute("href", "/analyses/a#pr_context");
    expect(repo).toHaveAccessibleDescription(/PR context/);
    const github = within(crumbs).getByRole("link", {
      name: "Pull request #482 on GitHub (opens on GitHub in a new tab)",
    });
    expect(github).toHaveAttribute("href", "https://github.com/acme/node-client/pull/482");
    expect(github).toHaveAttribute("target", "_blank");
    await userEvent.click(repo);
    expect(onNavigate).toHaveBeenLastCalledWith("/analyses/a#pr_context");
    const menu = within(crumbs).getByRole("button", { name: "View: Files" });
    expect(menu).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(menu);
    expect(menu).toHaveAttribute("aria-expanded", "true");
    expect(within(crumbs).getByRole("link", { name: "Files" })).toHaveAttribute("aria-current", "page");
    const flows = within(crumbs).getByRole("link", { name: "Flows" });
    expect(flows).not.toHaveAttribute("aria-current");
    await userEvent.click(flows);
    expect(onNavigate).toHaveBeenLastCalledWith("/analyses/a");
    expect(menu).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(menu);
    await userEvent.keyboard("{Escape}");
    expect(menu).toHaveAttribute("aria-expanded", "false");
    expect(menu).toHaveFocus();
    expect(screen.getByRole("banner")).toHaveTextContent("Your progress 1 / 5");
  });
});
