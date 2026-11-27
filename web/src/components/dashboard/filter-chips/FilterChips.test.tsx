import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { FilterChips } from "./FilterChips";
import { decodeFilters, encodeFilters, matchesFilters } from "./filters";
import { itemById } from "../test-fixtures";

describe("dashboard filters", () => {
  it("round-trips punctuation, ignores unknown fields and deduplicates selections", () => {
    const input =
      "label:ci_failing,-author:dependabot%5Bbot%5D,repo:acme/platform,unknown:a,label:ci_failing";
    expect(encodeFilters(decodeFilters(input))).toBe(
      "label:ci_failing,-author:dependabot%5Bbot%5D,repo:acme/platform",
    );
    const filters = decodeFilters("label:release%2Cbranch%3A2");
    expect(filters).toEqual([{ field: "label", key: "release,branch:2", excluded: false }]);
    expect(decodeFilters(encodeFilters(filters))).toEqual(filters);
  });

  it("ORs includes within each field, ANDs fields and vetoes exclusions", () => {
    const item = itemById("NC-482");
    const tags = [{ field: "label", key: "ci_failing", label: "CI failing" }] as const;
    const repo = `${item.repo!.owner}/${item.repo!.name}`;
    expect(matchesFilters(item, decodeFilters(`label:ci_passing,label:ci_failing,repo:${repo}`), tags)).toBe(
      true,
    );
    expect(matchesFilters(item, decodeFilters("label:ci_failing,repo:elsewhere/repo"), tags)).toBe(false);
    expect(matchesFilters(item, decodeFilters(`repo:${repo},-author:${item.author}`), tags)).toBe(false);
  });

  it("flips, removes and clears chips with Escape", async () => {
    function Example() {
      const [filters, setFilters] = useState(decodeFilters("label:ci_failing,repo:acme/platform"));
      return <FilterChips filters={filters} labels={[]} onChange={setFilters} />;
    }
    render(<Example />);
    const body = screen.getByRole("button", { name: "label: CI failing" });
    await userEvent.click(body);
    expect(body).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(screen.getByRole("button", { name: "Remove filter label CI failing" }));
    expect(screen.getByRole("button", { name: "repo: acme/platform" })).toHaveFocus();
    await userEvent.keyboard("{Escape}");
    expect(screen.getByText(/Click a label, size, author or repository/)).toBeVisible();
  });
});
