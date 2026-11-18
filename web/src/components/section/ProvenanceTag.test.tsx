import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ProvenanceTag } from "./ProvenanceTag";

describe("ProvenanceTag", () => {
  it("tags LLM text, saying in its tooltip that the LLM wrote it and nothing checked it", () => {
    render(<ProvenanceTag source="llm" />);
    const tag = screen.getByText("LLM");
    expect(tag).toHaveAttribute("tabindex", "0");
    expect(tag).toHaveAccessibleDescription(/written by the LLM.*not checked/);
  });

  it("tags GitHub content, saying in its tooltip that it comes from GitHub", () => {
    render(<ProvenanceTag source="github" />);
    const tag = screen.getByText("GitHub");
    expect(tag).toHaveAttribute("tabindex", "0");
    expect(tag).toHaveAccessibleDescription(/from GitHub/);
  });
});
