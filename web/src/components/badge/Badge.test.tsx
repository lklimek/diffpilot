import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import { DifferingLinesBadge, ProvenanceBadge } from "./Badge";
import { ClassificationLabel, GroupLabel, KindLabel, RiskLabel } from "./LlmLabel";

describe("deterministic badges", () => {
  it("name exactly what was checked; only a content check gets the green tick", () => {
    const { container } = render(
      <>
        <ProvenanceBadge badge="text_identical" />
        <ProvenanceBadge badge="classified_by_path" />
        <ProvenanceBadge badge="classified_by_line_rule" />
        <DifferingLinesBadge count={2} />
      </>,
    );
    const badges = [...container.querySelectorAll("[data-badge]")];
    expect(badges.map((b) => b.textContent)).toEqual([
      "✓ text identical",
      "classified by path",
      "classified by line rule",
      "≠ 2 lines differ",
    ]);
    expect(screen.getByText("✓ text identical")).toHaveAccessibleDescription(/whitespace/);
    expectTrustWording(container.textContent ?? "");
  });

  it("names a binary file as git's content check, without the green tick", () => {
    render(<ProvenanceBadge badge="binary_content" />);
    const badge = screen.getByText("binary file");
    expect(badge).toHaveAttribute("data-badge", "binary_content");
    expect(badge).toHaveAccessibleDescription("Git treats this file as binary; it is not shown or analysed.");
  });

  it("says one line differs in the singular", () => {
    render(<DifferingLinesBadge count={1} />);
    expect(screen.getByText("≠ 1 line differs")).toBeInTheDocument();
  });
});

describe("LLM labels", () => {
  it("are not badges and say they are LLM ratings", () => {
    const { container } = render(
      <>
        <RiskLabel risk="high" />
        <RiskLabel risk={null} />
        <ClassificationLabel value="refactor_pure" />
        <KindLabel kind="deleted_site" />
        <GroupLabel group="llm_classified_refactor" />
        <GroupLabel group="behaviour" />
      </>,
    );
    expect(container.querySelectorAll("[data-badge]")).toHaveLength(0);
    expect([...container.querySelectorAll("[data-llm-label]")].map((l) => l.textContent)).toEqual([
      "▲ HIGH",
      "not rated",
      "refactor pure",
      "deleted site",
      "LLM-classified refactor",
    ]);
    expect(screen.getByText("▲ HIGH")).toHaveAccessibleDescription(/LLM rating/);
    for (const label of container.querySelectorAll("[data-llm-label]")) {
      expect(label).toHaveAttribute("aria-describedby");
    }
    expectTrustWording(container.textContent ?? "");
  });
});
