import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import { NotificationsCard } from "./NotificationsCard";

const BOX = "Notify me when work finishes";

describe("NotificationsCard", () => {
  it("offers the opt-in, says what it covers, and reports a change", async () => {
    const onChange = vi.fn();
    render(<NotificationsCard support="default" enabled={false} onChange={onChange} />);
    const region = screen.getByRole("region", { name: "Notifications" });
    const box = screen.getByRole("checkbox", { name: BOX });
    expect(box).not.toBeChecked();
    expect(region).toHaveTextContent(/Ask answer/);
    expect(region).toHaveTextContent(/only in this browser/i);
    await userEvent.click(box);
    expect(onChange).toHaveBeenCalledWith(true);
    expectTrustWording(region.textContent ?? "");
  });

  it("shows a stored choice as on and turns it off", async () => {
    const onChange = vi.fn();
    render(<NotificationsCard support="granted" enabled onChange={onChange} />);
    const box = screen.getByRole("checkbox", { name: BOX });
    expect(box).toBeChecked();
    await userEvent.click(box);
    expect(onChange).toHaveBeenCalledWith(false);
  });

  it.each([
    ["unsupported", /cannot show notifications/],
    ["insecure", /https/],
    ["denied", /blocked/],
  ] as const)("%s: no option, a plain reason why it cannot work", (support, reason) => {
    render(<NotificationsCard support={support} enabled={false} onChange={() => {}} />);
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.getByRole("region", { name: "Notifications" })).toHaveTextContent(reason);
  });
});
