// Provenance tags as brief §4 rule 3 wants them on screen: each takes keyboard focus and opens its own tooltip.
import { expect } from "vitest";

const TIPS =
  /^(This text was written by the LLM; it was not checked\.|Taken from GitHub as written there\.)$/;

/** `element` holds `count` provenance tags, each focusable and described by its tooltip. */
export function expectTagsWithTips(element: HTMLElement, count: number): void {
  const tags = element.querySelectorAll<HTMLElement>("[data-provenance]");
  expect(tags).toHaveLength(count);
  for (const tag of tags) {
    expect(tag).toHaveAttribute("tabindex", "0");
    expect(tag).toHaveAccessibleDescription(TIPS);
  }
}
