import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { CredentialStatus } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { unlinkedCodex } from "../../mocks/assessment";
import { CodexAccountCard } from "./CodexAccountCard";
const status: CredentialStatus = {
  ...unlinkedCodex,
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

it.each([
  ["none", undefined, false],
  ["linking", { ...status, state: "linking" as const }, false],
  ["linked", { ...status, state: "linked" as const, relink_reason: null }, true],
  ["relink_needed", { ...status, relink_reason: null }, false],
  ["quarantined", status, false],
  ["terminal", { ...status, relink_reason: "terminal" as const }, false],
])("offers unlink only for a linked credential: %s", (_, current, expected) => {
  render(<CodexAccountCard status={current} now={now} busy={false} onLink={vi.fn()} onUnlink={vi.fn()} />);
  expect(!!screen.queryByRole("button", { name: "Unlink Codex account" })).toBe(expected);
});
it("expired challenge replaces stale waiting text and permits a new explicit link", () => {
  render(
    <CodexAccountCard
      status={{ ...status, state: "linking" }}
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
  expect(screen.queryByText(/Waiting for you/)).toBeNull();
  expect(screen.getByRole("button", { name: "Relink Codex account" })).not.toHaveAttribute("aria-disabled");
});
it("moves focus into unlink confirmation and returns it on either close action", () => {
  render(
    <CodexAccountCard
      status={{ ...status, state: "linked" }}
      now={now}
      busy={false}
      onLink={vi.fn()}
      onUnlink={vi.fn()}
    />,
  );
  const trigger = screen.getByRole("button", { name: "Unlink Codex account" });
  fireEvent.click(trigger);
  expect(screen.getByRole("button", { name: "Keep linked" })).toHaveFocus();
  fireEvent.click(screen.getByRole("button", { name: "Keep linked" }));
  expect(trigger).toHaveFocus();
  fireEvent.click(trigger);
  fireEvent.click(screen.getByRole("button", { name: "Confirm unlink" }));
  expect(trigger).toHaveFocus();
});
it("renders measured account telemetry and explicitly labels absent observations", () => {
  render(
    <CodexAccountCard
      status={{
        ...status,
        state: "linked",
        telemetry: {
          linked_at: "2026-11-18T09:00:00Z",
          primary: { used_percent: 12.5, resets_at: "2026-11-18T12:00:00Z" },
          secondary: null,
          tokens_today: {
            input_tokens: 100,
            cached_input_tokens: 80,
            output_tokens: 20,
            reasoning_output_tokens: null,
            jobs: 1,
          },
          tokens_month: null,
          live_containers: [{ shape: "pr", pull_request_id: 42, payer: "alice" }],
        },
      }}
      now={now}
      busy={false}
      onLink={vi.fn()}
      onUnlink={vi.fn()}
    />,
  );
  expect(screen.getByText(/12.5%/)).toBeVisible();
  expect(screen.getByText(/Tokens today .*120/)).toBeVisible();
  expect(screen.getByText(/Tokens this month .*not reported yet/)).toBeVisible();
  expect(screen.getByText(/Live containers .*1/)).toBeVisible();
});
it("explains an unavailable status in the link tooltip", () => {
  render(
    <CodexAccountCard
      now={now}
      busy={false}
      error="Gateway unavailable"
      onLink={vi.fn()}
      onUnlink={vi.fn()}
    />,
  );
  expect(screen.getByRole("button", { name: "Link Codex account" })).toHaveAccessibleDescription(
    "Account status is unavailable; linking is possible once it loads.",
  );
});

it("hides an observed plan percentage when its reset arrives without a newer observation", () => {
  const current = {
    ...status,
    state: "linked" as const,
    telemetry: {
      ...status.telemetry,
      primary: { used_percent: 80, resets_at: "2026-11-18T09:28:00Z" },
      secondary: { used_percent: 90, resets_at: "2026-11-18T09:26:00Z" },
    },
  };
  const props = { status: current, busy: false, onLink: vi.fn(), onUnlink: vi.fn() };
  const { rerender } = render(<CodexAccountCard {...props} now={now} />);
  expect(screen.getByText(/Primary plan usage:/)).toHaveTextContent("80%");
  expect(screen.getByText(/Secondary plan usage:/)).toHaveTextContent(
    "reset at 2026-11-18T09:26:00Z; no newer observation",
  );
  expect(screen.getByText(/Secondary plan usage:/)).not.toHaveTextContent("90%");
  rerender(<CodexAccountCard {...props} now={new Date("2026-11-18T09:28:00Z")} />);
  expect(screen.getByText(/Primary plan usage:/)).toHaveTextContent(
    "reset at 2026-11-18T09:28:00Z; no newer observation",
  );
  expect(screen.getByText(/Primary plan usage:/)).not.toHaveTextContent("80%");
});
