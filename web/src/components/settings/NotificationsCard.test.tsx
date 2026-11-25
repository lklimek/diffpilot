import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { NotificationsCard } from "./NotificationsCard";

describe("NotificationsCard", () => {
  it.each([
    ["default", false, "Enable browser notifications", /only in this browser/i, true],
    ["granted", false, "Turn on", /Allowed by the browser, off for diffpilot/, true],
    ["granted", true, "Turn off", /Allowed in this browser/, false],
  ] as const)("%s, enabled %s: %s", async (support, enabled, name, state, next) => {
    const onChange = vi.fn();
    render(<NotificationsCard support={support} enabled={enabled} onChange={onChange} />);
    expect(screen.getByRole("region", { name: "Browser" })).toHaveTextContent(state);
    await userEvent.click(screen.getByRole("button", { name }));
    expect(onChange).toHaveBeenCalledWith(next);
  });

  it("offers to recheck browser permission without asking for it", async () => {
    const onCheck = vi.fn();
    const onChange = vi.fn();
    render(<NotificationsCard support="denied" enabled={false} onChange={onChange} onCheck={onCheck} />);
    expect(screen.getByText(/Blocked by the browser for this site/)).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Check again" }));
    expect(onCheck).toHaveBeenCalledOnce();
    expect(onChange).not.toHaveBeenCalled();
  });

  it.each([
    ["unsupported", /cannot show notifications/],
    ["insecure", /https/],
  ] as const)("%s: shows the reason", (support, reason) => {
    render(<NotificationsCard support={support} enabled={false} onChange={() => {}} />);
    expect(screen.queryByRole("button", { name: /Enable|Turn on|Check again/ })).toBeNull();
    expect(screen.getByRole("region", { name: "Browser" })).toHaveTextContent(reason);
  });
});
