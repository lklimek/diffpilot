import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { PrUrlForm } from "./PrUrlForm";

describe("PrUrlForm (UX-005)", () => {
  it("submits the trimmed URL; shows an example URL bound to the field", async () => {
    const onSubmit = vi.fn();
    render(<PrUrlForm busy={false} onSubmit={onSubmit} />);
    const field = screen.getByLabelText("GitHub pull request URL");
    expect(field).toHaveAccessibleDescription("Example: https://github.com/owner/repo/pull/123");
    await userEvent.type(field, "  https://github.com/acme/node-client/pull/482 ");
    await userEvent.click(screen.getByRole("button", { name: "Analyze" }));
    expect(onSubmit).toHaveBeenCalledWith("https://github.com/acme/node-client/pull/482");
  });

  it("starts with the initial URL; Analyze has a tooltip and is disabled while busy", () => {
    render(<PrUrlForm busy initialUrl="https://github.com/acme/node-client/pull/482" onSubmit={() => {}} />);
    expect(screen.getByLabelText("GitHub pull request URL")).toHaveValue(
      "https://github.com/acme/node-client/pull/482",
    );
    const analyze = screen.getByRole("button", { name: "Analyze" });
    expect(analyze).toBeDisabled();
    expect(analyze).toHaveAccessibleDescription(/existing analysis/);
  });
});
