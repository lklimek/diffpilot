import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { LlmSelectionCard } from "./LlmSelectionCard";

const selection = {
  models: [
    { id: "gpt-6.1-sol", label: "Sol 6.1" },
    { id: "gpt-6-astra", label: "Astra 6" },
  ],
  efforts: ["low", "medium", "high"] as const,
  current: { model: "gpt-6.1-sol", reasoning_effort: "high" as const },
  changed_by: "alice",
  changed_at: "2026-11-17T18:04:00Z",
};
it("saves the selected installation model and effort and names the editor", async () => {
  const save = vi.fn();
  render(
    <LlmSelectionCard
      selection={{ ...selection, efforts: [...selection.efforts] }}
      busy={false}
      onSave={save}
    />,
  );
  await userEvent.selectOptions(screen.getByLabelText("Model"), "gpt-6-astra");
  await userEvent.selectOptions(screen.getByLabelText("Reasoning effort"), "low");
  await userEvent.click(screen.getByRole("button", { name: "Save" }));
  expect(save).toHaveBeenCalledWith({ model: "gpt-6-astra", reasoning_effort: "low" });
  expect(screen.getByText(/changed by alice/)).toBeVisible();
  expect(screen.getByText(/including the remaining jobs of running analyses/)).toBeVisible();
});
it("keeps a pending save read-only with an explained reachable button", async () => {
  const save = vi.fn();
  render(
    <LlmSelectionCard selection={{ ...selection, efforts: [...selection.efforts] }} busy onSave={save} />,
  );
  expect(screen.getByLabelText("Model")).toHaveAttribute("aria-disabled", "true");
  const button = screen.getByRole("button", { name: "Save" });
  expect(button).toHaveAttribute("aria-disabled", "true");
  expect(button).toHaveAccessibleDescription(/Saving/);
  await userEvent.selectOptions(screen.getByLabelText("Model"), "gpt-6-astra");
  expect(screen.getByLabelText("Model")).toHaveValue("gpt-6.1-sol");
  await userEvent.click(button);
  expect(save).not.toHaveBeenCalled();
});
