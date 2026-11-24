import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { AssessmentSettingsCard } from "./AssessmentSettingsCard";
import { AssessmentControls } from "../comments/AssessmentControls";
const now = new Date("2026-11-18T09:00:00Z");
const settings = {
  revision: "initial",
  config: { primary_method: "claudius_host" as const, ab_prs: ["acme/node-client#482"], ab_until: null },
  quota: { remaining: 79, resets_at: "2026-11-19T00:00:00Z" },
};
it("shows primary, selected PRs, cost and quota; saving preserves selections", () => {
  const onSave = vi.fn();
  render(<AssessmentSettingsCard now={now} settings={settings} busy={false} onSave={onSave} />);
  expect(screen.getByLabelText("Primary assessment method")).toHaveValue("claudius_host");
  expect(screen.getByText(/79 of 80/)).toBeVisible();
  expect(screen.getByText(/2026-11-19T00:00:00Z/)).toBeVisible();
  expect(screen.getByText(/roughly 2×/)).toBeVisible();
  expect(screen.getByText("acme/node-client#482")).toBeVisible();
  fireEvent.change(screen.getByLabelText("Primary assessment method"), { target: { value: "prompt_v4" } });
  fireEvent.click(screen.getByRole("button", { name: "Save assessment settings" }));
  expect(onSave).toHaveBeenCalledWith({ ...settings.config, primary_method: "prompt_v4" });
});
it("adds only this PR without starting a pass", () => {
  const onSave = vi.fn();
  render(
    <AssessmentControls now={now} settings={settings} prKey="acme/other#10" busy={false} onSave={onSave} />,
  );
  fireEvent.click(screen.getByRole("checkbox", { name: "Compare both methods on this PR" }));
  expect(onSave).toHaveBeenCalledWith({
    ...settings.config,
    ab_prs: [...settings.config.ab_prs, "acme/other#10"],
  });
  expect(screen.getByText(/does not start an assessment/)).toBeVisible();
});
it("removes only this PR and discloses the independent deadline", () => {
  const onSave = vi.fn();
  render(
    <AssessmentControls
      now={now}
      settings={{ ...settings, config: { ...settings.config, ab_until: "2026-11-19T00:00:00Z" } }}
      prKey="acme/node-client#482"
      busy={false}
      onSave={onSave}
    />,
  );
  fireEvent.click(screen.getByRole("checkbox", { name: "Compare both methods on this PR" }));
  expect(onSave).toHaveBeenCalledWith({ ...settings.config, ab_until: "2026-11-19T00:00:00Z", ab_prs: [] });
  expect(screen.getByText(/deadline also enables A\/B/)).toBeVisible();
});

it("round-trips an optional UTC deadline and does not save an invalid date", () => {
  const onSave = vi.fn();
  render(<AssessmentSettingsCard now={now} settings={settings} busy={false} onSave={onSave} />);
  const deadline = screen.getByLabelText("Optional A/B deadline (UTC)");
  fireEvent.change(deadline, { target: { value: "2026-11-30T00:00:00Z" } });
  fireEvent.click(screen.getByRole("button", { name: "Save assessment settings" }));
  expect(onSave).toHaveBeenLastCalledWith({ ...settings.config, ab_until: "2026-11-30T00:00:00.000Z" });
  onSave.mockClear();
  fireEvent.change(deadline, { target: { value: "not-a-date" } });
  const save = screen.getByRole("button", { name: "Save assessment settings" });
  expect(save).toHaveAttribute("aria-disabled", "true");
  fireEvent.click(save);
  expect(onSave).not.toHaveBeenCalled();
});
it("recognizes the same GitHub PR regardless of repository casing", () => {
  const onSave = vi.fn();
  render(
    <AssessmentControls
      now={now}
      settings={settings}
      prKey="Acme/Node-Client#482"
      busy={false}
      onSave={onSave}
    />,
  );
  expect(screen.getByRole("checkbox", { name: "Compare both methods on this PR" })).toBeChecked();
});

it("labels a past A/B deadline accurately", () => {
  render(
    <AssessmentControls
      now={now}
      settings={{ ...settings, config: { ...settings.config, ab_until: "2026-11-17T00:00:00Z" } }}
      prKey="acme/other#10"
      busy={false}
      onSave={vi.fn()}
    />,
  );
  expect(screen.getByText(/deadline .* has passed/)).toBeVisible();
  expect(screen.queryByText(/deadline also enables/)).toBeNull();
});
