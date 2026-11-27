import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { ChangeSizeMark, SizeMark } from "../../../api/generated/types";
import { ALL_SIZES, SIZE_RULE } from "./size";
import { SizePill } from "./Tags";

const mark = (size: SizeMark, additions: number, deletions: number): ChangeSizeMark => ({
  size,
  additions,
  deletions,
});

describe("size marks", () => {
  it("one shared list gives the classes and the rule text", () => {
    expect(ALL_SIZES).toEqual(["tiny", "small", "medium", "large", "huge"]);
    expect(SIZE_RULE).toBe(
      "tiny < 150, small 150–499, medium 500–2999, large 3000–9999, huge ≥ 10 000 changed lines",
    );
  });

  it("a pill names the size and its lines; the tooltip gives the evidence and the thresholds", () => {
    render(<SizePill mark={mark("large", 3200, 400)} />);
    const pill = screen.getByText("large").closest("[data-size-pill]")!;
    expect(pill).toHaveAccessibleName("Change size: large, 3600 changed lines");
    expect(pill).toHaveAccessibleDescription(
      "3600 changed lines (+3200 −400); tiny < 150, small 150–499, medium 500–2999, large 3000–9999, huge ≥ 10 000 changed lines",
    );
    expect(pill.className).toContain("rounded-full");
  });

  it("no mark, no pill; an unknown class from a newer server still shows its name", () => {
    const { container, rerender } = render(<SizePill mark={undefined} />);
    expect(container.querySelector("[data-size-pill]")).toBeNull();
    rerender(<SizePill mark={mark("micro" as SizeMark, 3, 1)} />);
    expect(screen.getByText("micro")).toBeInTheDocument();
  });
});
