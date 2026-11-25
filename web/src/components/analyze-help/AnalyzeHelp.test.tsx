import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import { AnalyzeHelp } from "./AnalyzeHelp";

describe("AnalyzeHelp", () => {
  it("explains what happens after Start analysis and links the settings", () => {
    render(
      <AnalyzeHelp
        llmHref="/settings/llm"
        githubSyncHref="/settings/github-sync"
        repositoriesHref="/settings/repositories"
      />,
    );
    expect(screen.getByRole("complementary", { name: "What happens after Start analysis" })).toBeVisible();
    expect(screen.getAllByRole("listitem")).toHaveLength(4);
    expect(screen.getByText(/nothing starts until you press Start analysis/)).toBeVisible();
    for (const [label, href] of [
      ["LLM & usage", "/settings/llm"],
      ["GitHub sync", "/settings/github-sync"],
      ["Repositories", "/settings/repositories"],
    ])
      expect(screen.getByRole("link", { name: label })).toHaveAttribute("href", href);
    expectTrustWording(document.body.textContent ?? "");
  });
});
