import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ExcludedItem, ExclusionReason, Exclusions } from "../../../api/generated/types";
import { ExcludedItems, exclusionDescription } from "./ExcludedItems";

const item = (number: number, reason: ExclusionReason, extra: Partial<ExcludedItem> = {}): ExcludedItem => ({
  key: `acme~platform~pr~${number}`,
  kind: "pr",
  id: `PLAT-${number}`,
  repo: { owner: "acme", name: "platform" },
  number,
  url: `https://github.com/acme/platform/pull/${number}`,
  title: `change ${number}`,
  author: "olek-d",
  reason,
  merged_into: null,
  ...extra,
});

const EXCLUSIONS: Exclusions = {
  total: 14,
  counts: [
    { reason: "excluded_author", count: 3 },
    { reason: "outside_owned_areas", count: 9 },
    { reason: "draft", count: 2 },
  ],
  items: [
    item(1, "excluded_author", { author: "dependabot", title: "<b>bump</b> serde" }),
    item(2, "excluded_author"),
    item(3, "excluded_author"),
    ...[10, 11, 12, 13, 14, 15, 16, 17, 18].map((n) => item(n, "outside_owned_areas")),
    item(20, "draft"),
    item(21, "draft"),
  ],
};

describe("ExcludedItems", () => {
  it("describes the reasons in descending count order", () => {
    expect(exclusionDescription(EXCLUSIONS)).toBe("9 outside your areas · 3 excluded authors · 2 drafts");
  });

  it("lists items by reason with links to GitHub; titles stay plain text", async () => {
    const onNavigate = vi.fn();
    render(<ExcludedItems exclusions={EXCLUSIONS} onNavigate={onNavigate} />);
    const groups = screen.getAllByRole("group");
    expect(groups.map((g) => g.getAttribute("aria-label"))).toEqual([
      "Outside your areas: 9",
      "Excluded authors: 3",
      "Drafts: 2",
    ]);
    const authors = within(groups[1]!);
    const link = authors.getByRole("link", { name: "Open PLAT-1 on GitHub (new tab)" });
    expect(link.getAttribute("href")).toBe("https://github.com/acme/platform/pull/1");
    expect(link.getAttribute("target")).toBe("_blank");
    expect(authors.getByText("<b>bump</b> serde")).toBeTruthy();
    expect(groups[1]!.querySelector("b")).toBeNull();
    await userEvent.click(authors.getByRole("link", { name: "Excluded authors in the settings guide" }));
    expect(onNavigate).toHaveBeenCalledWith("/settings/help#skip_authors");
    expect(
      within(groups[0]!).getByRole("link", { name: "Ownership policy in the settings guide" }),
    ).toHaveAttribute("href", "/settings/help#ownership_policy");
    expect(within(groups[2]!).queryByRole("link", { name: /settings guide/ })).toBeNull();
  });

  it("names the item a merged one went into, says when the list is capped, and survives an unknown reason", () => {
    const exclusions: Exclusions = {
      total: 250,
      counts: [
        { reason: "merged_into_another", count: 1 },
        { reason: "some_new_rule" as ExclusionReason, count: 249 },
      ],
      items: [
        item(5, "merged_into_another", { merged_into: "PLAT-6" }),
        item(7, "some_new_rule" as ExclusionReason),
      ],
    };
    render(<ExcludedItems exclusions={exclusions} />);
    expect(exclusionDescription(exclusions)).toBe(
      "249 by another rule · 1 merged into another item by the LLM",
    );
    expect(screen.getByText("merged into PLAT-6")).toBeTruthy();
    expect(screen.getByRole("group", { name: "Another rule: 249" })).toBeTruthy();
    expect(screen.getByText("Listed: the first 2 of 250.")).toBeTruthy();
  });

  it("snoozed items show when they wake and Wake now, which is unavailable while it runs", async () => {
    const onWake = vi.fn();
    const until = "2026-11-25T09:00:00Z";
    const exclusions: Exclusions = {
      total: 3,
      counts: [
        { reason: "snoozed", count: 2 },
        { reason: "draft", count: 1 },
      ],
      items: [
        item(8, "snoozed", { snoozed_until: until }),
        item(9, "snoozed", { snoozed_until: until }),
        item(20, "draft"),
      ],
    };
    const { rerender } = render(<ExcludedItems exclusions={exclusions} onWake={onWake} wakingKeys={[]} />);
    const snoozed = within(screen.getByRole("group", { name: "Snoozed: 2" }));
    expect(snoozed.getByText(/^Items you snoozed\. Each comes back at its date/)).toBeTruthy();
    const when = new Intl.DateTimeFormat("en-GB", {
      day: "numeric",
      month: "short",
      year: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }).format(new Date(until));
    expect(snoozed.getAllByText(`Snoozed until ${when}`)).toHaveLength(2);
    await userEvent.click(snoozed.getByRole("button", { name: "Wake now PLAT-8" }));
    expect(onWake).toHaveBeenCalledWith("acme~platform~pr~8");
    expect(
      within(screen.getByRole("group", { name: "Drafts: 1" })).queryByRole("button", { name: /Wake now/ }),
    ).toBeNull();
    rerender(<ExcludedItems exclusions={exclusions} onWake={onWake} wakingKeys={["acme~platform~pr~9"]} />);
    const busy = screen.getByRole("button", { name: "Wake now PLAT-9" });
    expect(busy).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(busy);
    expect(onWake).toHaveBeenCalledTimes(1);
  });

  it("a snapshot without the record says so instead of 0; none excluded is a fact too", () => {
    const { rerender } = render(<ExcludedItems exclusions={undefined} />);
    expect(screen.getByText("Exclusions were not recorded in this snapshot.")).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
    rerender(<ExcludedItems exclusions={{ total: 0, counts: [], items: [] }} />);
    expect(screen.getByText("No items are excluded or snoozed.")).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
  });
});
