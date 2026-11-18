import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import { AnalyzeHelp } from "./AnalyzeHelp";

describe("AnalyzeHelp", () => {
  it("explains what happens after Start analysis and links the settings", () => {
    render(<AnalyzeHelp settingsHref="/settings" />);
    expect(screen.getByRole("complementary", { name: "What happens after Start analysis" })).toBeVisible();
    expect(screen.getAllByRole("listitem")).toHaveLength(4);
    expect(screen.getByText(/nothing starts until you press Start analysis/)).toBeVisible();
    expect(screen.getByRole("link", { name: "Settings" })).toHaveAttribute("href", "/settings");
    expectTrustWording(document.body.textContent ?? "");
  });
});
