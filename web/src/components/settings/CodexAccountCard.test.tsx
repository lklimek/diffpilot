import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { CredentialStatus } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { CodexAccountCard } from "./CodexAccountCard";
const status: CredentialStatus = {
  mode: "chatgpt_plan",
  state: "relink_needed",
  relink_reason: "quarantined",
  label: null,
  plan: null,
  paused_until: null,
  last_call: null,
};
const now = new Date("2026-11-18T09:27:00Z");
it("discloses logout risk and quarantine before relinking; rendering does not link", () => {
  const onLink = vi.fn();
  const { container } = render(
    <CodexAccountCard status={status} now={now} busy={false} onLink={onLink} onUnlink={vi.fn()} />,
  );
  expect(screen.getByText(/Linking may log your Codex out on other machines/)).toBeVisible();
  expect(screen.getByText(/outcome is uncertain/)).toBeVisible();
  expect(onLink).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Relink Codex account" }));
  expect(onLink).toHaveBeenCalledOnce();
  expectTrustWording(container.textContent ?? "");
});
it("shows a code and safe link; unlink requires explicit confirmation", () => {
  const onUnlink = vi.fn();
  const { rerender } = render(
    <CodexAccountCard
      status={{ ...status, state: "linking" }}
      challenge={{
        user_code: "FAKE-PLAN",
        verification_url: "https://example.invalid/device",
        expires_at: "2026-11-18T09:32:00Z",
      }}
      now={now}
      busy={false}
      onLink={vi.fn()}
      onUnlink={onUnlink}
    />,
  );
  expect(screen.getByText("FAKE-PLAN")).toBeVisible();
  expect(screen.getByRole("link", { name: /Open the device linking page/ })).toHaveAttribute(
    "rel",
    "noopener noreferrer",
  );
  rerender(
    <CodexAccountCard
      status={{ ...status, state: "linked", label: "alice@example.invalid", plan: "Plus" }}
      now={now}
      busy={false}
      onLink={vi.fn()}
      onUnlink={onUnlink}
    />,
  );
  expect(screen.getByText(/alice@example.invalid/)).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "Unlink Codex account" }));
  expect(onUnlink).not.toHaveBeenCalled();
  expect(screen.getByText(/Running turns will end/)).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "Confirm unlink" }));
  expect(onUnlink).toHaveBeenCalledOnce();
});
it("never links unsafe URLs or expired codes", () => {
  const { rerender } = render(
    <CodexAccountCard
      status={status}
      challenge={{
        user_code: "OLD",
        verification_url: "javascript:alert(1)",
        expires_at: "2026-11-18T09:32:00Z",
      }}
      now={now}
      busy={false}
      onLink={vi.fn()}
      onUnlink={vi.fn()}
    />,
  );
  expect(screen.queryByRole("link")).toBeNull();
  rerender(
    <CodexAccountCard
      status={status}
      challenge={{
        user_code: "OLD",
        verification_url: "https://example.invalid/device",
        expires_at: now.toISOString(),
      }}
      now={now}
      busy={false}
      onLink={vi.fn()}
      onUnlink={vi.fn()}
    />,
  );
  expect(screen.getByText(/code expired/)).toBeVisible();
  expect(screen.queryByRole("link")).toBeNull();
});

it("keeps busy controls focusable while ignoring activation", () => {
  const onLink = vi.fn();
  const onUnlink = vi.fn();
  render(<CodexAccountCard status={status} now={now} busy onLink={onLink} onUnlink={onUnlink} />);
  const button = screen.getByRole("button", { name: "Relink Codex account" });
  expect(button).toHaveAttribute("aria-disabled", "true");
  expect(button).not.toBeDisabled();
  fireEvent.click(button);
  expect(onLink).not.toHaveBeenCalled();
});
