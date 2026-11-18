import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import type { Annotation } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FlagCard } from "./FlagCard";

const flag = (extra: Partial<Annotation> = {}): Annotation => ({
  id: "f1.5.a3",
  type: "flag",
  basis: "inference",
  range: { start_line: 99, end_line: 99 },
  was: "",
  is: "",
  text: "Potential TOCTOU between preflight and submission:",
  evidence: [{ file: "src/rpc/client.rs", side: "head", line: 99 }],
  flag: {
    level: "red",
    scenario: [
      "Wallet A runs the preflight check; the nonce is free.",
      "Wallet B submits with the same nonce.",
      "Wallet A submits; the node rejects it.",
      "The user sees a failed payment after a passed check.",
      "The retry repeats the race.",
      "The fee is spent twice.",
    ],
    not_checked: "Not reproduced; no test was run.",
  },
  ...extra,
});

const card = (annotation: Annotation, props: Partial<Parameters<typeof FlagCard>[0]> = {}) =>
  render(
    <ul>
      <FlagCard annotation={annotation} {...props} />
    </ul>,
  );

afterEach(() => vi.restoreAllMocks());

/** jsdom has no layout: the scenario list reports `height` px of content in a `box` px box. */
function layout(height: number, box: number) {
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(height);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(box);
}

it("shows the level in words, the LLM tag, the title, the numbered scenario, what was not checked", () => {
  card(flag());
  expect(screen.getByText("⚑ Red flag")).toHaveAccessibleDescription(/likely defect.*not checked/);
  expect(screen.getByText("LLM")).toHaveAttribute("data-provenance", "llm");
  expect(screen.getByText("Potential TOCTOU between preflight and submission:")).toBeVisible();
  const scenario = screen.getByRole("list", { name: "Scenario" });
  expect(scenario.tagName).toBe("OL");
  expect(within(scenario).getAllByRole("listitem")).toHaveLength(6);
  expect(screen.getByText(/^Not checked:/)).toHaveTextContent(
    "Not checked: Not reproduced; no test was run.",
  );
  expect(screen.getByText(/Evidence ·/)).toBeInTheDocument();
  expectTrustWording(document.body.textContent ?? "");
});

it("clips a long scenario to 5 lines with Show all / Show less (keyboard, aria-expanded)", async () => {
  layout(120, 80);
  card(flag());
  const scenario = screen.getByRole("list", { name: "Scenario" });
  expect(scenario.className).toContain("max-h-[5lh]");
  const toggle = screen.getByRole("button", { name: "Show all" });
  expect(toggle).toHaveAttribute("aria-expanded", "false");
  expect(toggle).toHaveAttribute("aria-controls", scenario.id);
  toggle.focus();
  await userEvent.keyboard("{Enter}");
  expect(toggle).toHaveTextContent("Show less");
  expect(toggle).toHaveAttribute("aria-expanded", "true");
  expect(scenario.className).not.toContain("max-h-[5lh]");
  await userEvent.click(toggle);
  expect(screen.getByRole("button", { name: "Show all" })).toHaveAttribute("aria-expanded", "false");
});

it("offers no Show all when the scenario fits in 5 lines", () => {
  layout(60, 80);
  card(flag());
  expect(screen.queryByRole("button", { name: "Show all" })).toBeNull();
});

it("a yellow flag says so in words; the title opens the flagged lines", async () => {
  const onOpen = vi.fn();
  card(flag({ flag: { level: "yellow", scenario: ["A reads.", "B writes."], not_checked: "Not run." } }), {
    onOpen,
  });
  expect(screen.getByText("⚑ Yellow flag")).toHaveAccessibleDescription(/worth a look/);
  await userEvent.click(
    screen.getByRole("button", { name: "Potential TOCTOU between preflight and submission:" }),
  );
  expect(onOpen).toHaveBeenCalledOnce();
});

it("a flag stored before levels: Flag, its question as the title, no scenario and no not-checked line", () => {
  card(flag({ text: "Can it be sent twice?", flag: { level: "unknown", scenario: [], not_checked: "" } }));
  expect(screen.getByText("⚑ Flag")).toHaveAccessibleDescription(/stored before flags had levels/);
  expect(screen.getByText("Can it be sent twice?")).toBeVisible();
  expect(screen.queryByRole("list", { name: "Scenario" })).toBeNull();
  expect(screen.queryByText(/^Not checked:/)).toBeNull();
  expect(screen.getByText(/Stored before flags had levels/)).toBeVisible();
});
