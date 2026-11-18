import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { PrUrlForm } from "./PrUrlForm";

describe("PrUrlForm", () => {
  it("submits the trimmed URL; shows an example URL bound to the field", async () => {
    const onSubmit = vi.fn();
    render(<PrUrlForm busy={false} onSubmit={onSubmit} />);
    const field = screen.getByLabelText("GitHub pull request URL");
    expect(field).toHaveAccessibleDescription("Example: https://github.com/owner/repo/pull/123");
    await userEvent.type(field, "  https://github.com/acme/node-client/pull/482 ");
    await userEvent.click(screen.getByRole("button", { name: "Check" }));
    expect(onSubmit).toHaveBeenCalledWith("https://github.com/acme/node-client/pull/482");
  });

  it("starts with the initial URL; Check says it starts nothing and shows the check running", () => {
    const { rerender } = render(
      <PrUrlForm
        busy={false}
        initialUrl="https://github.com/acme/node-client/pull/482"
        onSubmit={() => {}}
      />,
    );
    expect(screen.getByLabelText("GitHub pull request URL")).toHaveValue(
      "https://github.com/acme/node-client/pull/482",
    );
    expect(screen.getByRole("button", { name: "Check" })).toHaveAccessibleDescription(/nothing starts/i);
    rerender(
      <PrUrlForm
        busy
        checking
        initialUrl="https://github.com/acme/node-client/pull/482"
        onSubmit={() => {}}
      />,
    );
    const check = screen.getByRole("button", { name: "Checking…" });
    expect(check).toHaveAttribute("aria-disabled", "true");
    expect(check).not.toHaveAttribute("disabled");
    expect(check).toHaveAccessibleDescription(/Not available while the current request runs\./);
    expect(screen.getByLabelText("GitHub pull request URL")).toHaveAttribute("aria-busy", "true");
  });

  it("does not submit while busy", async () => {
    const onSubmit = vi.fn();
    render(<PrUrlForm busy initialUrl="https://github.com/acme/node-client/pull/482" onSubmit={onSubmit} />);
    await userEvent.click(screen.getByRole("button", { name: "Check" }));
    expect(onSubmit).not.toHaveBeenCalled();
  });
});
