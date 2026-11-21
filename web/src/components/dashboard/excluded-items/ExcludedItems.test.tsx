import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ExcludedItem, ExclusionReason, Exclusions } from "../../../api/generated/types";
import { ExcludedItems } from "./ExcludedItems";

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
  it("one quiet line counts the excluded items per reason, largest first", () => {
    render(<ExcludedItems exclusions={EXCLUSIONS} />);
    expect(screen.getByText(/^Excluded: 14/).textContent).toBe(
      "Excluded: 14 — 9 outside your areas · 3 excluded authors · 2 drafts",
    );
    expect(screen.queryByRole("list")).toBeNull();
  });

  it("the open list has Hide in its top-right corner; it closes the list and returns the focus", async () => {
    render(<ExcludedItems exclusions={EXCLUSIONS} />);
    const toggle = screen.getByRole("button", { name: /Excluded: 14/ });
    expect(screen.queryByRole("button", { name: "Hide excluded items" })).toBeNull();
    await userEvent.click(toggle);
    const hide = screen.getByRole("button", { name: "Hide excluded items" });
    expect(hide).toHaveTextContent(/^Hide$/);
    expect(hide).toHaveAccessibleDescription(/Hides the list/);
    hide.focus();
    await userEvent.keyboard("{Enter}");
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryAllByRole("group")).toHaveLength(0);
    expect(toggle).toHaveFocus();
  });

  it("the disclosure lists the items by reason with links to GitHub; titles stay plain text", async () => {
    const onNavigate = vi.fn();
    render(<ExcludedItems exclusions={EXCLUSIONS} onNavigate={onNavigate} />);
    const toggle = screen.getByRole("button", { name: /Excluded: 14/ });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    await userEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
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

  it("names the item a merged one went into, says when the list is capped, and survives an unknown reason", async () => {
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
    expect(screen.getByText(/^Excluded: 250/).textContent).toBe(
      "Excluded: 250 — 249 by another rule · 1 merged into another item by the LLM",
    );
    await userEvent.click(screen.getByRole("button", { name: /Excluded: 250/ }));
    expect(screen.getByText("merged into PLAT-6")).toBeTruthy();
    expect(screen.getByRole("group", { name: "Another rule: 249" })).toBeTruthy();
    expect(screen.getByText("Listed: the first 2 of 250.")).toBeTruthy();
  });

  it("a snapshot without the record says so instead of 0; none excluded is a fact too", () => {
    const { rerender } = render(<ExcludedItems exclusions={undefined} />);
    expect(screen.getByText("Excluded: not recorded for this snapshot — refresh to see it.")).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
    rerender(<ExcludedItems exclusions={{ total: 0, counts: [], items: [] }} />);
    expect(screen.getByText("Excluded: none")).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
  });
});
