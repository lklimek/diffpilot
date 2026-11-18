import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { PrSummary } from "../../api/generated/types";
import { PrCheckCard, SETTLE_MS } from "./PrCheckCard";
import { LanguageTag } from "../language-tag/LanguageTag";

const pr: PrSummary = {
  repo: { owner: "acme", name: "node-client" },
  number: 482,
  title: "feat(rpc): retry with backoff on timeout",
  author: "dana",
  html_url: "https://github.com/acme/node-client/pull/482",
  base_sha: "92f2274efbb93570cc8f820a49c058cf66b0dbe9",
  head_sha: "52d2da140191a9d47becf6a446310be803c007fc",
};

describe("PrCheckCard (Home: the result of Check)", () => {
  it("names the PR, scrolls into view and gives its primary action the focus", () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll as unknown as Element["scrollIntoView"];
    render(
      <PrCheckCard
        pr={pr}
        actions={(describedBy) => (
          <button type="button" data-primary aria-describedby={describedBy}>
            Start analysis
          </button>
        )}
        onCancel={() => {}}
      >
        <p>Details</p>
      </PrCheckCard>,
    );
    const card = screen.getByRole("region", { name: "Pull request check" });
    expect(card).toHaveTextContent("acme/node-client #482 feat(rpc): retry with backoff on timeout");
    expect(card).toHaveTextContent("by dana");
    expect(screen.getByRole("button", { name: "Start analysis" })).toHaveFocus();
    expect(scroll).toHaveBeenCalled();
  });

  it("Cancel has a tooltip and clears the check", async () => {
    const onCancel = vi.fn();
    render(
      <PrCheckCard pr={null} onCancel={onCancel}>
        Not found.
      </PrCheckCard>,
    );
    const cancel = screen.getByRole("button", { name: "Cancel" });
    expect(cancel).toHaveAccessibleDescription(/nothing was started/i);
    await userEvent.click(cancel);
    expect(onCancel).toHaveBeenCalledOnce();
  });

  function startCard(onStart: () => void) {
    render(
      <PrCheckCard
        pr={pr}
        actions={(describedBy) => (
          <button type="button" data-primary aria-describedby={describedBy} onClick={onStart}>
            Start analysis anyway
          </button>
        )}
        onCancel={() => {}}
      >
        <p role="note">Large PR: 27 081 changed lines in 371 files.</p>
      </PrCheckCard>,
    );
    return screen.getByRole("button", { name: "Start analysis anyway" });
  }
  const settle = () => new Promise((resolve) => setTimeout(resolve, SETTLE_MS + 50));

  it("the primary action is described by what the check found, so the warning is read with it", () => {
    expect(startCard(() => {})).toHaveAccessibleDescription(/Large PR: 27 081 changed lines in 371 files\./);
  });

  it("an Enter right after the card appears does not start anything; a later one does", async () => {
    const onStart = vi.fn();
    const start = startCard(onStart);
    expect(start).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    expect(onStart).not.toHaveBeenCalled();
    await settle();
    await userEvent.keyboard("{Enter}");
    expect(onStart).toHaveBeenCalledOnce();
  });

  it("a held Enter (auto-repeat) never activates the primary action", async () => {
    const onStart = vi.fn();
    startCard(onStart);
    await settle();
    // One press, then two auto-repeats of the held key: only the press counts.
    await userEvent.keyboard("{Enter>3}");
    expect(onStart).toHaveBeenCalledOnce();
  });

  it("shows the PR's size facts when the check has them", () => {
    render(
      <PrCheckCard
        pr={pr}
        size={{ changed_lines: 1240, changed_files: 12 }}
        estimate={{ remaining_seconds: 1500, basis: "recent_runs", rough: false }}
        onCancel={() => {}}
      >
        {null}
      </PrCheckCard>,
    );
    expect(screen.getByRole("region", { name: "Pull request check" })).toHaveTextContent(
      "1 240 changed lines in 12 files. Planning: about 25 min (estimate from recent runs here).",
    );
  });

  it("an existing analysis names its language with a tag", () => {
    render(
      <PrCheckCard pr={pr} onCancel={() => {}}>
        <p>
          <LanguageTag language="pl" /> Analysis found.
        </p>
      </PrCheckCard>,
    );
    expect(screen.getByText("PL")).toHaveAccessibleDescription("LLM texts of this analysis are in Polish.");
  });
});
