import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { LanguageTag } from "./LanguageTag";

it.each([
  ["en", "EN", "LLM texts of this analysis are in English."],
  ["pl", "PL", "LLM texts of this analysis are in Polish."],
  ["ru", "RU", "LLM texts of this analysis are in Russian."],
] as const)("%s: a small tag with a tooltip naming the language", (language, text, tip) => {
  render(<LanguageTag language={language} />);
  const tag = screen.getByText(text);
  expect(tag).toHaveAccessibleDescription(tip);
  expect(tag).toHaveAttribute("tabindex", "0");
});

it("renders nothing when the language is not known", () => {
  const { container } = render(<LanguageTag language={undefined} />);
  expect(container).toBeEmptyDOMElement();
});
