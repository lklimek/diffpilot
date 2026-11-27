import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { PrCheckCard } from "./PrCheckCard";
import { LanguageTag } from "../language-tag/LanguageTag";

it("describes the start action with the size, warning and estimate and ignores key repeats", async () => {
  const onStart = vi.fn();
  render(
    <PrCheckCard
      size={{ changed_lines: 1240, changed_files: 12 }}
      estimate={{ remaining_seconds: 1500, basis: "recent_runs", rough: false }}
      actions={(id) => (
        <button type="button" aria-describedby={id} onClick={onStart}>
          Start analysis anyway
        </button>
      )}
    >
      <p role="note">Large PR.</p>
    </PrCheckCard>,
  );
  const start = screen.getByRole("button", { name: "Start analysis anyway" });
  expect(start).toHaveAccessibleDescription(
    "1 240 changed lines in 12 files. Planning: about 25 min (estimate from recent runs here). Large PR.",
  );
  start.focus();
  await userEvent.keyboard("{Enter>3}");
  expect(onStart).toHaveBeenCalledOnce();
});

it("keeps the language tag on an existing analysis", () => {
  render(
    <PrCheckCard>
      <LanguageTag language="pl" />
    </PrCheckCard>,
  );
  expect(screen.getByText("PL")).toHaveAccessibleDescription("LLM texts of this analysis are in Polish.");
});
